#[cfg(test)]
mod tests;

mod execution;
mod service;
mod stream;
mod tools;

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use tog_context::toolset::ToolRegistry;

pub const DEFAULT_WAIT_SECONDS: u64 = 30;
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
pub const DEFAULT_INLINE_OUTPUT_BYTES: u64 = 64 * 1024;

pub struct ShellTools;

impl ShellTools {
    pub fn register(registry: &mut ToolRegistry) {
        let service = Arc::new(service::ShellService::new());
        registry.register(tools::ShellExecuteTool::new(Arc::clone(&service)));
        registry.register(tools::ShellWaitTool::new(Arc::clone(&service)));
        registry.register(tools::ShellCancelTool::new(Arc::clone(&service)));
        registry.register(tools::StreamReadTool::new(Arc::clone(&service)));
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ShellExecuteParameters {
    commands: String,
    #[serde(default)]
    shell_context_id: Option<String>,
    #[serde(default)]
    wait_time_seconds: Option<u64>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
    #[serde(default)]
    output_limit_bytes: Option<u64>,
}

#[derive(Debug, JsonSchema, Serialize)]
pub(crate) struct ShellExecuteResult {
    execution_id: String,
    stdout_stream_id: String,
    stderr_stream_id: String,
    output_stream_id: String,
    state: ExecutionState,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ShellWaitParameters {
    execution_id: String,
    offset: u64,
    #[serde(default)]
    wait_time_seconds: Option<u64>,
    #[serde(default)]
    output_limit_bytes: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ShellCancelParameters {
    execution_id: String,
}

#[derive(Debug, JsonSchema, Serialize)]
pub(crate) struct ShellCancelResult {
    cancelled: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct StreamReadParameters {
    stream_id: String,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    limit_bytes: Option<u64>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[derive(Debug, JsonSchema, Serialize)]
pub(crate) struct StreamReadResult {
    content: String,
    next_offset: u64,
    eof: bool,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub(crate) struct ExecutionState {
    output: String,
    next_offset: u64,
    output_eof: bool,
    status: ExecutionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_status: Option<ShellExitStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ShellExecutionError>,
}

#[derive(Clone, Copy, Debug, Default, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExecutionStatus {
    #[default]
    Running,
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ShellExitStatus {
    Exited { code: i32 },
    Signaled { signal: i32 },
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct ShellExecutionError {
    kind: ShellExecutionErrorKind,
    message: String,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ShellExecutionErrorKind {
    TimedOut,
    Cancelled,
    CaptureFailed,
}
