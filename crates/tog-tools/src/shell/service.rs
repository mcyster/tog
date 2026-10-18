use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tog::conversation::OperationFailure;

use super::execution::{Execution, ExecutionId, ExecutionState, invalid_arguments};
use super::stream::StreamId;
use super::{
    DEFAULT_INLINE_OUTPUT_BYTES, DEFAULT_TIMEOUT_SECONDS, DEFAULT_WAIT_SECONDS, ExecutionSnapshot,
    ShellCancelParameters, ShellCancelResult, ShellExecuteParameters, ShellExecuteResult,
    ShellWaitParameters, StreamReadParameters, StreamReadResult,
};

#[derive(Clone, Copy)]
enum OutputChannel {
    StandardOutput,
    StandardError,
    Combined,
}

#[derive(Clone, Copy)]
struct StreamLocation {
    execution_id: ExecutionId,
    channel: OutputChannel,
}

pub(super) struct ShellService {
    executions: Mutex<HashMap<ExecutionId, Arc<Execution>>>,
    streams: Mutex<HashMap<StreamId, StreamLocation>>,
}

impl ShellService {
    pub(super) fn new() -> Self {
        Self {
            executions: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
        }
    }

    pub(super) async fn execute(
        &self,
        parameters: ShellExecuteParameters,
    ) -> Result<ShellExecuteResult, OperationFailure> {
        if parameters.shell_context_id.is_some() {
            return Err(invalid_arguments(
                "shell contexts are not supported yet; shell_context_id must be omitted".to_owned(),
            ));
        }
        if parameters.commands.trim().is_empty() {
            return Err(invalid_arguments(
                "the shell command must not be blank".to_owned(),
            ));
        }
        let timeout = Duration::from_secs(
            parameters
                .timeout_seconds
                .unwrap_or(DEFAULT_TIMEOUT_SECONDS),
        );
        let wait =
            Duration::from_secs(parameters.wait_time_seconds.unwrap_or(DEFAULT_WAIT_SECONDS));
        let output_limit = parameters
            .output_limit_bytes
            .unwrap_or(DEFAULT_INLINE_OUTPUT_BYTES);

        let execution = Execution::spawn(parameters.commands, timeout)?;
        let execution_id = ExecutionId::new();
        let stdout_stream_id = StreamId::new();
        let stderr_stream_id = StreamId::new();
        let output_stream_id = StreamId::new();
        {
            let mut executions = self
                .executions
                .lock()
                .expect("the execution registry should lock");
            let mut streams = self
                .streams
                .lock()
                .expect("the stream registry should lock");
            executions.insert(execution_id, Arc::clone(&execution));
            streams.insert(
                stdout_stream_id,
                StreamLocation {
                    execution_id,
                    channel: OutputChannel::StandardOutput,
                },
            );
            streams.insert(
                stderr_stream_id,
                StreamLocation {
                    execution_id,
                    channel: OutputChannel::StandardError,
                },
            );
            streams.insert(
                output_stream_id,
                StreamLocation {
                    execution_id,
                    channel: OutputChannel::Combined,
                },
            );
        }

        let _ = execution.wait_for_completion(wait).await;
        let read = execution.combined_output().read(0, output_limit).await;
        let state = composed_snapshot(execution.state(), 0, read);
        Ok(ShellExecuteResult {
            execution_id: execution_id.to_string(),
            stdout_stream_id: stdout_stream_id.to_string(),
            stderr_stream_id: stderr_stream_id.to_string(),
            output_stream_id: output_stream_id.to_string(),
            state,
        })
    }

    pub(super) async fn wait(
        &self,
        parameters: ShellWaitParameters,
    ) -> Result<ExecutionSnapshot, OperationFailure> {
        let execution = self.execution(&parameters.execution_id)?;
        let wait =
            Duration::from_secs(parameters.wait_time_seconds.unwrap_or(DEFAULT_WAIT_SECONDS));
        let output_limit = parameters
            .output_limit_bytes
            .unwrap_or(DEFAULT_INLINE_OUTPUT_BYTES);
        let read = execution
            .combined_output()
            .read_following(parameters.offset, output_limit, wait)
            .await;
        Ok(composed_snapshot(
            execution.state(),
            parameters.offset,
            read,
        ))
    }

    pub(super) fn cancel(
        &self,
        parameters: ShellCancelParameters,
    ) -> Result<ShellCancelResult, OperationFailure> {
        let execution = self.execution(&parameters.execution_id)?;
        execution.cancel();
        Ok(ShellCancelResult { cancelled: true })
    }

    pub(super) async fn read_stream(
        &self,
        parameters: StreamReadParameters,
    ) -> Result<StreamReadResult, OperationFailure> {
        let location = self.stream(&parameters.stream_id)?;
        let execution = self.execution_by_id(location.execution_id)?;
        let stream = match location.channel {
            OutputChannel::StandardOutput => execution.standard_output(),
            OutputChannel::StandardError => execution.standard_error(),
            OutputChannel::Combined => execution.combined_output(),
        };
        let offset = parameters.offset.unwrap_or(0);
        let limit = parameters
            .limit_bytes
            .unwrap_or(DEFAULT_INLINE_OUTPUT_BYTES);
        let timeout =
            Duration::from_secs(parameters.timeout_seconds.unwrap_or(DEFAULT_WAIT_SECONDS));
        let read = stream.read_following(offset, limit, timeout).await;
        let length = read.bytes.len() as u64;
        Ok(StreamReadResult {
            content: String::from_utf8_lossy(&read.bytes).into_owned(),
            next_offset: offset + length,
            eof: read.eof,
        })
    }

    fn execution(&self, identifier: &str) -> Result<Arc<Execution>, OperationFailure> {
        let execution_id = ExecutionId::from_str(identifier).map_err(|_| {
            invalid_arguments(format!("invalid execution identifier: {identifier}"))
        })?;
        self.execution_by_id(execution_id)
    }

    fn execution_by_id(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Arc<Execution>, OperationFailure> {
        self.executions
            .lock()
            .expect("the execution registry should lock")
            .get(&execution_id)
            .cloned()
            .ok_or_else(|| {
                invalid_arguments(format!("unknown execution identifier: {execution_id}"))
            })
    }

    fn stream(&self, identifier: &str) -> Result<StreamLocation, OperationFailure> {
        let stream_id = StreamId::from_str(identifier)
            .map_err(|_| invalid_arguments(format!("invalid stream identifier: {identifier}")))?;
        self.streams
            .lock()
            .expect("the stream registry should lock")
            .get(&stream_id)
            .copied()
            .ok_or_else(|| invalid_arguments(format!("unknown stream identifier: {identifier}")))
    }
}

fn composed_snapshot(
    process: ExecutionState,
    offset: u64,
    read: super::stream::OutputRead,
) -> ExecutionSnapshot {
    ExecutionSnapshot {
        output: String::from_utf8_lossy(&read.bytes).into_owned(),
        next_offset: offset + read.bytes.len() as u64,
        output_eof: read.eof,
        status: process.status,
        exit_status: process.exit_status,
        error: process.error,
    }
}

impl Drop for ShellService {
    fn drop(&mut self) {
        let executions = self
            .executions
            .lock()
            .expect("the execution registry should lock")
            .drain()
            .map(|(_, execution)| execution)
            .collect::<Vec<_>>();
        for execution in executions {
            execution.cancel();
        }
    }
}
