use std::sync::Arc;

use codex_code_mode_protocol::CellId;
use codex_code_mode_protocol::CodeModeNestedToolCall;
use codex_code_mode_protocol::CodeModeSession;
use codex_code_mode_protocol::CodeModeSessionDelegate;
use codex_code_mode_protocol::CodeModeSessionProvider;
use codex_code_mode_protocol::CodeModeSessionProviderFuture;
use codex_code_mode_protocol::CodeModeSessionResultFuture;
use codex_code_mode_protocol::ExecuteRequest;
use codex_code_mode_protocol::NotificationFuture;
use codex_code_mode_protocol::StartedCell;
use codex_code_mode_protocol::ToolInvocationFuture;
use codex_code_mode_protocol::WaitOutcome;
use codex_code_mode_protocol::WaitRequest;
use tokio_util::sync::CancellationToken;

const CODE_MODE_UNAVAILABLE: &str = "code mode is unavailable on iOS";

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
        _delegate: Arc<dyn CodeModeSessionDelegate>,
    ) -> CodeModeSessionProviderFuture<'a> {
        Box::pin(async { Ok(Arc::new(InProcessCodeModeSession) as Arc<dyn CodeModeSession>) })
    }
}

pub struct InProcessCodeModeSession;

impl InProcessCodeModeSession {
    pub fn new() -> Self {
        Self
    }

    pub fn with_delegate(_delegate: Arc<dyn CodeModeSessionDelegate>) -> Self {
        Self
    }

    pub fn with_delegate_and_task_failure_handler(
        _delegate: Arc<dyn CodeModeSessionDelegate>,
        _task_failure_handler: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Self {
        Self
    }

    pub async fn execute(&self, _request: ExecuteRequest) -> Result<StartedCell, String> {
        Err(CODE_MODE_UNAVAILABLE.to_string())
    }

    pub async fn wait(&self, _request: WaitRequest) -> Result<WaitOutcome, String> {
        Err(CODE_MODE_UNAVAILABLE.to_string())
    }

    pub async fn terminate(&self, _cell_id: CellId) -> Result<WaitOutcome, String> {
        Err(CODE_MODE_UNAVAILABLE.to_string())
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

/// Controls whether V8 may generate executable code at runtime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum V8JitMode {
    #[default]
    Enabled,
    Disabled,
}

/// Reports that the V8-backed code-mode runtime is unavailable on iOS.
pub fn initialize_v8(_jit_mode: V8JitMode) -> Result<(), String> {
    Err(CODE_MODE_UNAVAILABLE.to_string())
}
