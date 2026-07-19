use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use codex_code_mode_protocol::CellId;
use codex_code_mode_protocol::CodeModeNestedToolCall;
use codex_code_mode_protocol::CodeModeSession;
use codex_code_mode_protocol::CodeModeSessionDelegate;
use codex_code_mode_protocol::CodeModeSessionProvider;
use codex_code_mode_protocol::CodeModeSessionProviderFuture;
use codex_code_mode_protocol::CodeModeSessionResultFuture;
use codex_code_mode_protocol::ExecuteRequest;
use codex_code_mode_protocol::FunctionCallOutputContentItem;
use codex_code_mode_protocol::NotificationFuture;
use codex_code_mode_protocol::RuntimeResponse;
use codex_code_mode_protocol::StartedCell;
use codex_code_mode_protocol::ToolInvocationFuture;
use codex_code_mode_protocol::WaitOutcome;
use codex_code_mode_protocol::WaitRequest;
use serde_json::Value as JsonValue;
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const NODE_BRIDGE: &str = r###"
const readline = require("readline");
const vm = require("vm");

(async () => {
const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
const lines = rl[Symbol.asyncIterator]();
const first = await lines.next();
if (first.done) process.exit(2);
const request = JSON.parse(first.value);
const output = [];
const stored = new Map(Object.entries(request.stored_values || {}));
const writes = {};
let nextToolCallId = 1;

function send(value) {
  process.stdout.write(JSON.stringify(value) + "\n");
}

async function receive() {
  const line = await lines.next();
  if (line.done) throw new Error("Codex closed the Node bridge input");
  return JSON.parse(line.value);
}

function stringify(value) {
  if (typeof value === "string") return value;
  if (value === undefined) return "undefined";
  try { return JSON.stringify(value); } catch (_) { return String(value); }
}

async function callTool(name, input) {
  const id = String(nextToolCallId++);
  send({ type: "tool", id, name, input });
  const response = await receive();
  if (response.id !== id) throw new Error("out-of-order tool response");
  if (!response.ok) throw new Error(response.error || "nested tool failed");
  return response.result;
}

const tools = new Proxy({}, {
  get(_target, property) {
    if (typeof property !== "string") return undefined;
    return (input) => callTool(property, input);
  }
});

function text(value) {
  output.push({ type: "input_text", text: stringify(value) });
}

function image(value, detail) {
  const imageUrl = typeof value === "string" ? value : value && value.image_url;
  const imageDetail = detail ?? (value && value.detail) ?? undefined;
  if (!imageUrl) throw new Error("image() requires an image URL");
  const item = { type: "input_image", image_url: imageUrl };
  if (imageDetail != null) item.detail = imageDetail;
  output.push(item);
}

function generatedImage(value) {
  image(value && value.image_url);
  if (value && value.output_hint) text(value.output_hint);
}

function store(key, value) {
  JSON.stringify(value);
  stored.set(String(key), value);
  writes[String(key)] = value;
}

function load(key) {
  return stored.get(String(key));
}

async function notify(value) {
  const id = String(nextToolCallId++);
  send({ type: "notify", id, text: stringify(value) });
  const response = await receive();
  if (response.id !== id || !response.ok) throw new Error(response.error || "notify failed");
}

const EXIT = Symbol("codex-exit");
function exit() { throw EXIT; }
async function yield_control() {}
function codexSetTimeout(callback, delay) {
  const timer = setTimeout(callback, delay);
  timer.unref();
  return timer;
}

const sandbox = {
  tools,
  text,
  image,
  generatedImage,
  store,
  load,
  notify,
  exit,
  yield_control,
  setTimeout: codexSetTimeout,
  clearTimeout,
  ALL_TOOLS: request.tools.map(({ name, description }) => ({ name, description })),
};
const context = vm.createContext(sandbox, { name: "codex-ios-code-mode" });

let errorText = null;
try {
  const wrapped = `(async () => {\n${request.source}\n})()`;
  await vm.runInContext(wrapped, context, { filename: "codex-cell.js" });
} catch (error) {
  if (error !== EXIT) errorText = error && error.stack ? error.stack : String(error);
}

send({
  type: "result",
  content_items: output,
  stored_value_writes: writes,
  error_text: errorText,
});
rl.close();
})().catch((error) => {
  process.stderr.write((error && error.stack ? error.stack : String(error)) + "\n");
  process.exitCode = 1;
});
"###;

pub struct NoopCodeModeSessionDelegate;

impl CodeModeSessionDelegate for NoopCodeModeSessionDelegate {
    fn invoke_tool<'a>(
        &'a self,
        _invocation: CodeModeNestedToolCall,
        cancellation_token: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async move {
            cancellation_token.cancelled().await;
            Err("code mode nested tools are unavailable".to_string())
        })
    }

    fn notify<'a>(
        &'a self,
        _call_id: String,
        _cell_id: CellId,
        _text: String,
        _cancellation_token: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn cell_closed(&self, _cell_id: &CellId) {}
}

#[derive(Default)]
pub struct InProcessCodeModeSessionProvider;

impl CodeModeSessionProvider for InProcessCodeModeSessionProvider {
    fn create_session<'a>(
        &'a self,
        delegate: Arc<dyn CodeModeSessionDelegate>,
    ) -> CodeModeSessionProviderFuture<'a> {
        Box::pin(async move {
            Ok(Arc::new(InProcessCodeModeSession::with_delegate(delegate))
                as Arc<dyn CodeModeSession>)
        })
    }
}

#[derive(Clone)]
pub struct InProcessCodeModeSession {
    delegate: Arc<dyn CodeModeSessionDelegate>,
    stored_values: Arc<Mutex<HashMap<String, JsonValue>>>,
}

impl InProcessCodeModeSession {
    pub fn new() -> Self {
        Self::with_delegate(Arc::new(NoopCodeModeSessionDelegate))
    }

    pub fn with_delegate(delegate: Arc<dyn CodeModeSessionDelegate>) -> Self {
        Self {
            delegate,
            stored_values: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_delegate_and_task_failure_handler(
        delegate: Arc<dyn CodeModeSessionDelegate>,
        _task_failure_handler: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Self {
        Self::with_delegate(delegate)
    }

    pub async fn execute(&self, request: ExecuteRequest) -> Result<StartedCell, String> {
        let cell_id = CellId::new(format!("ios-node-{}", request.tool_call_id));
        let (response_tx, response_rx) = oneshot::channel();
        let session = self.clone();
        let task_cell_id = cell_id.clone();
        tokio::spawn(async move {
            let response = session.execute_with_node(&task_cell_id, request).await;
            let _ = response_tx.send(response);
        });
        Ok(StartedCell::from_result_receiver(cell_id, response_rx))
    }

    async fn execute_with_node(
        &self,
        cell_id: &CellId,
        request: ExecuteRequest,
    ) -> Result<RuntimeResponse, String> {
        let node_path = physical_jbroot_path("usr/local/lib/nodejs/node")?;
        let mut child = Command::new(&node_path)
            .arg("-e")
            .arg(NODE_BRIDGE)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to start iOS Node code-mode bridge: {error}"))?;

        let mut child_stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Node code-mode bridge stdin is unavailable".to_string())?;
        let child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Node code-mode bridge stdout is unavailable".to_string())?;
        let mut child_stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Node code-mode bridge stderr is unavailable".to_string())?;
        let stderr_task = tokio::spawn(async move {
            let mut text = String::new();
            let _ = child_stderr.read_to_string(&mut text).await;
            text
        });

        let stored_values = self.stored_values.lock().await.clone();
        let tools = request
            .enabled_tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                })
            })
            .collect::<Vec<_>>();
        let initial = json!({
            "source": request.source,
            "tools": tools,
            "stored_values": stored_values,
        });
        write_json_line(&mut child_stdin, &initial).await?;

        let mut lines = BufReader::new(child_stdout).lines();
        let mut response = None;
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|error| format!("failed reading Node code-mode bridge: {error}"))?
        {
            let message: JsonValue = serde_json::from_str(&line)
                .map_err(|error| format!("invalid Node code-mode response: {error}"))?;
            match message.get("type").and_then(JsonValue::as_str) {
                Some("tool") => {
                    self.handle_tool_message(cell_id, &request, &message, &mut child_stdin)
                        .await?;
                }
                Some("notify") => {
                    self.handle_notify_message(cell_id, &request, &message, &mut child_stdin)
                        .await?;
                }
                Some("result") => {
                    response = Some(self.result_message(cell_id, message).await?);
                    break;
                }
                other => return Err(format!("unknown Node code-mode message: {other:?}")),
            }
        }

        drop(child_stdin);
        let status = child
            .wait()
            .await
            .map_err(|error| format!("failed waiting for Node code-mode bridge: {error}"))?;
        let stderr = stderr_task.await.unwrap_or_default();
        if let Some(response) = response {
            return Ok(response);
        }
        Err(format!(
            "Node code-mode bridge exited with {status}: {}",
            stderr.trim()
        ))
    }

    async fn handle_tool_message(
        &self,
        cell_id: &CellId,
        request: &ExecuteRequest,
        message: &JsonValue,
        child_stdin: &mut tokio::process::ChildStdin,
    ) -> Result<(), String> {
        let id = required_string(message, "id")?;
        let name = required_string(message, "name")?;
        let Some(tool) = request.enabled_tools.iter().find(|tool| tool.name == name) else {
            return write_json_line(
                child_stdin,
                &json!({"id": id, "ok": false, "error": format!("unknown tool {name}")}),
            )
            .await;
        };
        let result = self
            .delegate
            .invoke_tool(
                CodeModeNestedToolCall {
                    cell_id: cell_id.clone(),
                    runtime_tool_call_id: id.clone(),
                    tool_name: tool.tool_name.clone(),
                    tool_kind: tool.kind,
                    input: message.get("input").cloned(),
                },
                CancellationToken::new(),
            )
            .await;
        let response = match result {
            Ok(result) => json!({"id": id, "ok": true, "result": result}),
            Err(error) => json!({"id": id, "ok": false, "error": error}),
        };
        write_json_line(child_stdin, &response).await
    }

    async fn handle_notify_message(
        &self,
        cell_id: &CellId,
        request: &ExecuteRequest,
        message: &JsonValue,
        child_stdin: &mut tokio::process::ChildStdin,
    ) -> Result<(), String> {
        let id = required_string(message, "id")?;
        let text = required_string(message, "text")?;
        let result = self
            .delegate
            .notify(
                request.tool_call_id.clone(),
                cell_id.clone(),
                text,
                CancellationToken::new(),
            )
            .await;
        let response = match result {
            Ok(()) => json!({"id": id, "ok": true}),
            Err(error) => json!({"id": id, "ok": false, "error": error}),
        };
        write_json_line(child_stdin, &response).await
    }

    async fn result_message(
        &self,
        cell_id: &CellId,
        mut message: JsonValue,
    ) -> Result<RuntimeResponse, String> {
        let content_items = serde_json::from_value::<Vec<FunctionCallOutputContentItem>>(
            message
                .get_mut("content_items")
                .map(JsonValue::take)
                .unwrap_or_default(),
        )
        .map_err(|error| format!("invalid Node code-mode content: {error}"))?;
        let writes = serde_json::from_value::<HashMap<String, JsonValue>>(
            message
                .get_mut("stored_value_writes")
                .map(JsonValue::take)
                .unwrap_or_default(),
        )
        .map_err(|error| format!("invalid Node code-mode stored values: {error}"))?;
        self.stored_values.lock().await.extend(writes);
        let error_text = message
            .get("error_text")
            .and_then(JsonValue::as_str)
            .map(ToOwned::to_owned);
        Ok(RuntimeResponse::Result {
            cell_id: cell_id.clone(),
            content_items,
            error_text,
        })
    }

    pub async fn wait(&self, request: WaitRequest) -> Result<WaitOutcome, String> {
        Ok(WaitOutcome::MissingCell(missing_cell_response(
            request.cell_id,
        )))
    }

    pub async fn terminate(&self, cell_id: CellId) -> Result<WaitOutcome, String> {
        Ok(WaitOutcome::MissingCell(missing_cell_response(cell_id)))
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        Ok(())
    }
}

impl Default for InProcessCodeModeSession {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeModeSession for InProcessCodeModeSession {
    fn execute<'a>(
        &'a self,
        request: ExecuteRequest,
    ) -> CodeModeSessionResultFuture<'a, StartedCell> {
        Box::pin(InProcessCodeModeSession::execute(self, request))
    }

    fn wait<'a>(&'a self, request: WaitRequest) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(InProcessCodeModeSession::wait(self, request))
    }

    fn terminate<'a>(&'a self, cell_id: CellId) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(InProcessCodeModeSession::terminate(self, cell_id))
    }

    fn shutdown<'a>(&'a self) -> CodeModeSessionResultFuture<'a, ()> {
        Box::pin(InProcessCodeModeSession::shutdown(self))
    }
}

async fn write_json_line(
    stdin: &mut tokio::process::ChildStdin,
    value: &JsonValue,
) -> Result<(), String> {
    let mut line = serde_json::to_vec(value)
        .map_err(|error| format!("failed encoding Node code-mode message: {error}"))?;
    line.push(b'\n');
    stdin
        .write_all(&line)
        .await
        .map_err(|error| format!("failed writing Node code-mode message: {error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("failed flushing Node code-mode message: {error}"))
}

fn required_string(message: &JsonValue, key: &str) -> Result<String, String> {
    message
        .get(key)
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("Node code-mode message is missing string field {key}"))
}

fn physical_jbroot_path(relative_path: &str) -> Result<PathBuf, String> {
    let fixed_home = std::env::var_os("CFFIXED_USER_HOME")
        .ok_or_else(|| "CFFIXED_USER_HOME is unavailable under RootHide".to_string())?;
    let jbroot = Path::new(&fixed_home)
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "CFFIXED_USER_HOME does not contain a RootHide jbroot".to_string())?;
    let path = jbroot.join(relative_path);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("RootHide runtime not found at {}", path.display()))
    }
}

fn missing_cell_response(cell_id: CellId) -> RuntimeResponse {
    RuntimeResponse::Result {
        error_text: Some(format!("exec cell {cell_id} not found")),
        cell_id,
        content_items: Vec::new(),
    }
}

/// Controls whether embedded V8 may generate executable code at runtime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum V8JitMode {
    #[default]
    Enabled,
    Disabled,
}

/// iOS uses the external Node bridge instead of embedded V8.
pub fn initialize_v8(_jit_mode: V8JitMode) -> Result<(), String> {
    Ok(())
}
