#[cfg(not(target_os = "ios"))]
mod cell_actor;
mod remote_session;
#[cfg(not(target_os = "ios"))]
mod runtime;
#[cfg(not(target_os = "ios"))]
mod service;
#[cfg(not(target_os = "ios"))]
mod session_runtime;
#[cfg(target_os = "ios")]
mod unsupported_ios;
#[cfg(not(target_os = "ios"))]
mod v8_init;

#[cfg(not(target_os = "ios"))]
pub(crate) type TaskFailureHandler = std::sync::Arc<dyn Fn(String) + Send + Sync>;

pub use codex_code_mode_protocol::*;
pub use remote_session::ProcessOwnedCodeModeSession;
pub use remote_session::ProcessOwnedCodeModeSessionProvider;
#[cfg(not(target_os = "ios"))]
pub use service::InProcessCodeModeSession;
#[cfg(not(target_os = "ios"))]
pub use service::InProcessCodeModeSessionProvider;
#[cfg(not(target_os = "ios"))]
pub use service::NoopCodeModeSessionDelegate;
#[cfg(target_os = "ios")]
pub use unsupported_ios::InProcessCodeModeSession;
#[cfg(target_os = "ios")]
pub use unsupported_ios::InProcessCodeModeSessionProvider;
#[cfg(target_os = "ios")]
pub use unsupported_ios::NoopCodeModeSessionDelegate;
#[cfg(target_os = "ios")]
pub use unsupported_ios::V8JitMode;
#[cfg(target_os = "ios")]
pub use unsupported_ios::initialize_v8;
#[cfg(not(target_os = "ios"))]
pub use v8_init::V8JitMode;
#[cfg(not(target_os = "ios"))]
pub use v8_init::initialize_v8;
