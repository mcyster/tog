use std::sync::Arc;

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use schemars::{Schema, schema_for};
use serde_json::Value;

use tog::conversation::{OperationFailure, ToolDefinition, ToolName};
use tog_context::toolset::ExecutableTool;

use super::execution::{execution_failed, invalid_arguments};
use super::service::ShellService;
use super::{
    ExecutionSnapshot, ShellCancelParameters, ShellCancelResult, ShellExecuteParameters,
    ShellExecuteResult, ShellWaitParameters, StreamReadParameters, StreamReadResult,
};

pub(super) struct ShellExecuteTool {
    definition: ToolDefinition,
    service: Arc<ShellService>,
}

impl ShellExecuteTool {
    pub(super) fn new(service: Arc<ShellService>) -> Self {
        Self {
            definition: tool_definition(
                "shell_execute",
                "Run a command with /bin/sh -c. Waits up to wait_time_seconds for the command \
                 to finish and returns a shell execution state plus execution and stream \
                 identifiers; a longer command keeps running and can be followed with \
                 shell_wait or cancelled with shell_cancel.",
                schema_for!(ShellExecuteParameters),
                schema_for!(ShellExecuteResult),
            ),
            service,
        }
    }
}

impl ExecutableTool for ShellExecuteTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute<'execute>(
        &'execute self,
        arguments: Value,
    ) -> BoxFuture<'execute, Result<Value, OperationFailure>> {
        async move {
            let parameters: ShellExecuteParameters =
                serde_json::from_value(arguments).map_err(|error| {
                    invalid_arguments(format!("invalid shell_execute arguments: {error}"))
                })?;
            let result = self.service.execute(parameters).await?;
            serde_json::to_value(result).map_err(|error| {
                execution_failed(format!(
                    "the shell_execute result could not be serialized: {error}"
                ))
            })
        }
        .boxed()
    }
}

pub(super) struct ShellWaitTool {
    definition: ToolDefinition,
    service: Arc<ShellService>,
}

impl ShellWaitTool {
    pub(super) fn new(service: Arc<ShellService>) -> Self {
        Self {
            definition: tool_definition(
                "shell_wait",
                "Wait up to wait_time_seconds for a running shell execution to finish or produce \
                 output beyond the given combined-stream byte offset, and return its updated \
                 state. Returns the output that has arrived since the offset, bounded by \
                 output_limit_bytes.",
                schema_for!(ShellWaitParameters),
                schema_for!(ExecutionSnapshot),
            ),
            service,
        }
    }
}

impl ExecutableTool for ShellWaitTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute<'execute>(
        &'execute self,
        arguments: Value,
    ) -> BoxFuture<'execute, Result<Value, OperationFailure>> {
        async move {
            let parameters: ShellWaitParameters =
                serde_json::from_value(arguments).map_err(|error| {
                    invalid_arguments(format!("invalid shell_wait arguments: {error}"))
                })?;
            let state = self.service.wait(parameters).await?;
            serde_json::to_value(state).map_err(|error| {
                execution_failed(format!(
                    "the shell_wait result could not be serialized: {error}"
                ))
            })
        }
        .boxed()
    }
}

pub(super) struct ShellCancelTool {
    definition: ToolDefinition,
    service: Arc<ShellService>,
}

impl ShellCancelTool {
    pub(super) fn new(service: Arc<ShellService>) -> Self {
        Self {
            definition: tool_definition(
                "shell_cancel",
                "Terminate a running shell execution and its child processes. A later shell_wait \
                 reports the cancelled execution's final state. Cancelling an execution that has \
                 already completed is a no-op.",
                schema_for!(ShellCancelParameters),
                schema_for!(ShellCancelResult),
            ),
            service,
        }
    }
}

impl ExecutableTool for ShellCancelTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute<'execute>(
        &'execute self,
        arguments: Value,
    ) -> BoxFuture<'execute, Result<Value, OperationFailure>> {
        async move {
            let parameters: ShellCancelParameters =
                serde_json::from_value(arguments).map_err(|error| {
                    invalid_arguments(format!("invalid shell_cancel arguments: {error}"))
                })?;
            let result = self.service.cancel(parameters)?;
            serde_json::to_value(result).map_err(|error| {
                execution_failed(format!(
                    "the shell_cancel result could not be serialized: {error}"
                ))
            })
        }
        .boxed()
    }
}

pub(super) struct StreamReadTool {
    definition: ToolDefinition,
    service: Arc<ShellService>,
}

impl StreamReadTool {
    pub(super) fn new(service: Arc<ShellService>) -> Self {
        Self {
            definition: tool_definition(
                "stream_read",
                "Read up to limit_bytes of captured bytes from a shell output stream at a byte \
                 offset, waiting up to timeout_seconds at the live end for more data, and report \
                 the content, the next byte offset, and whether the stream has ended.",
                schema_for!(StreamReadParameters),
                schema_for!(StreamReadResult),
            ),
            service,
        }
    }
}

impl ExecutableTool for StreamReadTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute<'execute>(
        &'execute self,
        arguments: Value,
    ) -> BoxFuture<'execute, Result<Value, OperationFailure>> {
        async move {
            let parameters: StreamReadParameters =
                serde_json::from_value(arguments).map_err(|error| {
                    invalid_arguments(format!("invalid stream_read arguments: {error}"))
                })?;
            let result = self.service.read_stream(parameters).await?;
            serde_json::to_value(result).map_err(|error| {
                execution_failed(format!(
                    "the stream_read result could not be serialized: {error}"
                ))
            })
        }
        .boxed()
    }
}

fn tool_definition(
    name: &str,
    description: &str,
    parameters: Schema,
    result: Schema,
) -> ToolDefinition {
    ToolDefinition::try_new(
        ToolName::try_new(name.to_owned()).expect("the tool name should be valid"),
        description.to_owned(),
        parameters,
        result,
    )
    .expect("the tool definition should be valid")
}
