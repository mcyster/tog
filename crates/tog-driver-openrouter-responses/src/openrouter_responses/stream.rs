use std::collections::{HashMap, VecDeque};

use futures_util::StreamExt;
use futures_util::stream::{self, BoxStream};
use serde_json::{Map, Value, json};

use super::input::is_summary_text;
use super::usage::parsed_usage;
use super::{
    FailureStage, OpenRouterResponsesError, PROVIDER_PAYLOAD_VERSION, REASONING_ITEM_EVENT_TYPE,
    failed_outcome, is_context_limit_payload, provider_failure,
};
use tog::conversation::{
    AssistantResponse, ConversationEventId, FailureCategory, InvalidAssistantResponse,
    InvalidModelResponse, InvalidModelSpecificEvent, InvalidToolRequest, ModelContinuation,
    ModelData, ModelOutcome, ModelResponse, ModelSpecificEvent, ToolName, ToolRequest, Usage,
};
use tog::model_driver::{ModelDriverOutput, ModelDriverOutputBatch, ModelOutputStream};

type ResponseByteStream = BoxStream<'static, Result<Vec<u8>, OpenRouterResponsesError>>;

const CONTENT_DELTA_EVENT_TYPE: &str = "content_delta";
const REASONING_DELTA_EVENT_TYPE: &str = "reasoning_delta";
const REASONING_SUMMARY_DELTA_EVENT_TYPE: &str = "reasoning_summary_delta";
const TOOL_CALL_ARGUMENTS_DELTA_EVENT_TYPE: &str = "tool_call_arguments_delta";

pub(crate) fn model_output_stream(
    response_bytes: ResponseByteStream,
    model_request_id: ConversationEventId,
) -> ModelOutputStream {
    let state = OpenRouterResponsesStreamState {
        response_bytes,
        decoder: ServerSentEventDecoder::default(),
        interpreter: StreamInterpreterState::default(),
        pending_batches: VecDeque::new(),
        response_end: None,
        terminated: false,
    };

    stream::unfold(state, move |mut state| async move {
        loop {
            if let Some(batch) = state.pending_batches.pop_front() {
                return Some((Ok(batch), state));
            }
            if state.terminated {
                return None;
            }
            if let Some(server_sent_event) = state.decoder.events.pop_front() {
                match process_event(server_sent_event, model_request_id, &mut state.interpreter) {
                    Ok(ProcessEventResult::Batches(batches)) => {
                        if state.interpreter.completed {
                            state.terminated = true;
                        }
                        state.pending_batches.extend(batches);
                    }
                    Ok(ProcessEventResult::Done) => {
                        state.decoder.events.clear();
                        state.response_end = Some(ResponseEnd::DoneSentinel);
                    }
                    Err(error) => {
                        state.terminated = true;
                        state
                            .pending_batches
                            .push_back(failed_terminal_batch(model_request_id, &error));
                    }
                }
                continue;
            }
            if let Some(response_end) = state.response_end {
                state.terminated = true;
                if state.interpreter.completed {
                    return None;
                }
                let error = match response_end {
                    ResponseEnd::BodyEnded => OpenRouterResponsesError::StreamInterrupted(
                        "the response body ended before a terminal response event".to_owned(),
                    ),
                    ResponseEnd::DoneSentinel => OpenRouterResponsesError::InvalidResponse(
                        "the stream ended before a terminal response event".to_owned(),
                    ),
                };
                state
                    .pending_batches
                    .push_back(failed_terminal_batch(model_request_id, &error));
                continue;
            }

            match state.response_bytes.next().await {
                Some(Ok(bytes)) => {
                    if let Err(error) = state.decoder.push(&bytes) {
                        state.terminated = true;
                        state
                            .pending_batches
                            .push_back(failed_terminal_batch(model_request_id, &error));
                    }
                }
                Some(Err(error)) => {
                    state.terminated = true;
                    state
                        .pending_batches
                        .push_back(failed_terminal_batch(model_request_id, &error));
                }
                None => {
                    if let Err(error) = state.decoder.finish() {
                        state.terminated = true;
                        state
                            .pending_batches
                            .push_back(failed_terminal_batch(model_request_id, &error));
                    }
                    state.response_end = Some(ResponseEnd::BodyEnded);
                }
            }
        }
    })
    .boxed()
}

pub(crate) fn context_limit_stream(model_request_id: ConversationEventId) -> ModelOutputStream {
    terminal_only_stream(
        model_request_id,
        failed_outcome(
            FailureCategory::ContextLimitExceeded,
            "The model context limit was exceeded.",
        ),
        ModelContinuation::Complete,
        None,
    )
}

pub(crate) fn failed_terminal_stream(
    model_request_id: ConversationEventId,
    failure_stage: FailureStage,
    error: OpenRouterResponsesError,
) -> ModelOutputStream {
    let (category, message) = provider_failure(&error, failure_stage);
    terminal_only_stream(
        model_request_id,
        failed_outcome(category, &message),
        ModelContinuation::Complete,
        None,
    )
}

fn terminal_only_stream(
    model_request_id: ConversationEventId,
    outcome: ModelOutcome,
    continuation: ModelContinuation,
    usage: Option<Usage>,
) -> ModelOutputStream {
    let response = terminal_model_response(model_request_id, outcome, continuation, usage)
        .expect("the terminal model response should be valid");
    stream::once(async move {
        Ok(ModelDriverOutputBatch::from(
            ModelDriverOutput::ModelResponse(response),
        ))
    })
    .boxed()
}

fn failed_terminal_batch(
    model_request_id: ConversationEventId,
    error: &OpenRouterResponsesError,
) -> ModelDriverOutputBatch {
    let (category, message) = provider_failure(error, FailureStage::DuringStream);
    let response = terminal_model_response(
        model_request_id,
        failed_outcome(category, &message),
        ModelContinuation::Complete,
        None,
    )
    .expect("the failed terminal model response should be valid");
    ModelDriverOutputBatch::from(ModelDriverOutput::ModelResponse(response))
}

fn terminal_model_response(
    model_request_id: ConversationEventId,
    outcome: ModelOutcome,
    continuation: ModelContinuation,
    usage: Option<Usage>,
) -> Result<ModelResponse, InvalidModelResponse> {
    ModelResponse::new(model_request_id, Vec::new(), outcome, continuation, usage)
}

fn invalid_assistant_response(error: InvalidAssistantResponse) -> OpenRouterResponsesError {
    OpenRouterResponsesError::InvalidResponse(error.to_string())
}

fn invalid_model_specific_event(error: InvalidModelSpecificEvent) -> OpenRouterResponsesError {
    OpenRouterResponsesError::InvalidResponse(error.to_string())
}

fn invalid_tool_request(error: InvalidToolRequest) -> OpenRouterResponsesError {
    OpenRouterResponsesError::InvalidResponse(error.to_string())
}

fn invalid_model_response(error: InvalidModelResponse) -> OpenRouterResponsesError {
    OpenRouterResponsesError::InvalidResponse(error.to_string())
}

struct ServerSentEvent {
    name: Option<String>,
    data: String,
}

#[derive(Default)]
struct ServerSentEventDecoder {
    bytes: Vec<u8>,
    event_name: Option<String>,
    data_lines: Vec<String>,
    events: VecDeque<ServerSentEvent>,
}

impl ServerSentEventDecoder {
    fn push(&mut self, bytes: &[u8]) -> Result<(), OpenRouterResponsesError> {
        self.bytes.extend_from_slice(bytes);
        while let Some(newline_position) = self.bytes.iter().position(|byte| *byte == b'\n') {
            let mut line = self.bytes.drain(..=newline_position).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(line)?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), OpenRouterResponsesError> {
        if !self.bytes.is_empty() {
            let mut line = std::mem::take(&mut self.bytes);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(line)?;
        }
        self.dispatch_event();
        Ok(())
    }

    fn process_line(&mut self, line: Vec<u8>) -> Result<(), OpenRouterResponsesError> {
        let line = String::from_utf8(line)
            .map_err(|error| OpenRouterResponsesError::InvalidResponse(error.to_string()))?;
        if line.is_empty() {
            self.dispatch_event();
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }

        let (field, value) = line
            .split_once(':')
            .map_or((line.as_str(), ""), |(field, value)| {
                (field, value.strip_prefix(' ').unwrap_or(value))
            });
        match field {
            "event" => self.event_name = Some(value.to_owned()),
            "data" => self.data_lines.push(value.to_owned()),
            _ => {}
        }
        Ok(())
    }

    fn dispatch_event(&mut self) {
        if !self.data_lines.is_empty() {
            self.events.push_back(ServerSentEvent {
                name: self.event_name.take(),
                data: self.data_lines.join("\n"),
            });
            self.data_lines.clear();
        } else {
            self.event_name = None;
        }
    }
}

struct OpenRouterResponsesStreamState {
    response_bytes: ResponseByteStream,
    decoder: ServerSentEventDecoder,
    interpreter: StreamInterpreterState,
    pending_batches: VecDeque<ModelDriverOutputBatch>,
    response_end: Option<ResponseEnd>,
    terminated: bool,
}

#[derive(Clone, Copy)]
enum ResponseEnd {
    BodyEnded,
    DoneSentinel,
}

enum ProcessEventResult {
    Batches(Vec<ModelDriverOutputBatch>),
    Done,
}

#[derive(Default)]
struct StreamInterpreterState {
    function_call_call_ids: HashMap<String, String>,
    completed: bool,
}

fn process_event(
    server_sent_event: ServerSentEvent,
    model_request_id: ConversationEventId,
    state: &mut StreamInterpreterState,
) -> Result<ProcessEventResult, OpenRouterResponsesError> {
    if server_sent_event.data == "[DONE]" {
        return Ok(ProcessEventResult::Done);
    }

    let payload: Value = serde_json::from_str(&server_sent_event.data)
        .map_err(|error| OpenRouterResponsesError::InvalidResponse(error.to_string()))?;
    let event_type = resolve_event_type(&payload, server_sent_event.name)?;

    let batches = match event_type.as_str() {
        "response.output_text.delta" | "response.content_part.delta" => {
            one_batch_per_output(text_delta_progress(model_request_id, &payload)?)
        }
        "response.reasoning_text.delta" => one_batch_per_output(delta_progress(
            model_request_id,
            REASONING_DELTA_EVENT_TYPE,
            payload
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )?),
        "response.reasoning_summary_text.delta" => one_batch_per_output(delta_progress(
            model_request_id,
            REASONING_SUMMARY_DELTA_EVENT_TYPE,
            payload
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )?),
        "response.function_call_arguments.delta" => {
            one_batch_per_output(tool_argument_delta_progress(
                model_request_id,
                &payload,
                &state.function_call_call_ids,
            )?)
        }
        "response.output_item.added" => {
            track_function_call(&payload, &mut state.function_call_call_ids)?;
            Vec::new()
        }
        "response.completed" | "response.done" => {
            let completed = completed_response(model_request_id, &payload)?;
            state.completed = true;
            response_batches(model_request_id, completed)?
        }
        "response.incomplete" => {
            let completed = incomplete_response(&payload)?;
            state.completed = true;
            response_batches(model_request_id, completed)?
        }
        "error" | "response.failed" => {
            let completed = failed_response(&payload)?;
            state.completed = true;
            response_batches(model_request_id, completed)?
        }
        _ => Vec::new(),
    };
    Ok(ProcessEventResult::Batches(batches))
}

fn resolve_event_type(
    payload: &Value,
    server_sent_event_name: Option<String>,
) -> Result<String, OpenRouterResponsesError> {
    let payload_event_type = match payload.get("type") {
        Some(Value::String(event_type)) => Some(event_type.clone()),
        Some(_) => {
            return Err(OpenRouterResponsesError::InvalidResponse(
                "an OpenRouter stream event contained a non-string type".to_owned(),
            ));
        }
        None => None,
    };
    if let (Some(payload_event_type), Some(server_sent_event_name)) =
        (&payload_event_type, &server_sent_event_name)
        && payload_event_type != server_sent_event_name
    {
        return Err(OpenRouterResponsesError::InvalidResponse(format!(
            "OpenRouter stream event type {server_sent_event_name} did not match payload type {payload_event_type}"
        )));
    }
    Ok(payload_event_type
        .or(server_sent_event_name)
        .unwrap_or_else(|| "unknown".to_owned()))
}

fn one_batch_per_output(outputs: Vec<ModelDriverOutput>) -> Vec<ModelDriverOutputBatch> {
    outputs
        .into_iter()
        .map(ModelDriverOutputBatch::from)
        .collect()
}

fn delta_progress(
    model_request_id: ConversationEventId,
    event_type: &str,
    delta: &str,
) -> Result<Vec<ModelDriverOutput>, OpenRouterResponsesError> {
    if delta.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![progress_output(
        model_request_id,
        event_type,
        json!({}),
        Some(delta.to_owned()),
    )?])
}

fn text_delta_progress(
    model_request_id: ConversationEventId,
    payload: &Value,
) -> Result<Vec<ModelDriverOutput>, OpenRouterResponsesError> {
    let delta = payload
        .get("delta")
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .get("part")
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("output_text"))
                .and_then(|part| part.get("text"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default();
    delta_progress(model_request_id, CONTENT_DELTA_EVENT_TYPE, delta)
}

fn progress_output(
    model_request_id: ConversationEventId,
    event_type: &str,
    payload: Value,
    message: Option<String>,
) -> Result<ModelDriverOutput, OpenRouterResponsesError> {
    Ok(ModelDriverOutput::ModelSpecificEvent(
        ModelSpecificEvent::new_unpersisted(
            model_request_id,
            event_type.to_owned(),
            PROVIDER_PAYLOAD_VERSION,
            payload,
            message,
        )
        .map_err(invalid_model_specific_event)?,
    ))
}

fn tool_argument_delta_progress(
    model_request_id: ConversationEventId,
    payload: &Value,
    function_call_call_ids: &HashMap<String, String>,
) -> Result<Vec<ModelDriverOutput>, OpenRouterResponsesError> {
    let delta = payload
        .get("delta")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if delta.is_empty() {
        return Ok(Vec::new());
    }
    let correlation = function_call_call_ids
        .get(&correlation_key(payload)?)
        .map(|call_id| json!({ "call_id": call_id }))
        .unwrap_or_else(|| json!({}));
    Ok(vec![progress_output(
        model_request_id,
        TOOL_CALL_ARGUMENTS_DELTA_EVENT_TYPE,
        correlation,
        Some(delta.to_owned()),
    )?])
}

fn track_function_call(
    payload: &Value,
    function_call_call_ids: &mut HashMap<String, String>,
) -> Result<(), OpenRouterResponsesError> {
    let Some(item) = payload.get("item") else {
        return Ok(());
    };
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return Ok(());
    }
    let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
        return Ok(());
    };
    function_call_call_ids.insert(correlation_key(payload)?, call_id.to_owned());
    Ok(())
}

fn correlation_key(payload: &Value) -> Result<String, OpenRouterResponsesError> {
    if let Some(index) = payload.get("output_index") {
        let index = index.as_u64().ok_or_else(|| {
            OpenRouterResponsesError::InvalidResponse(
                "an OpenRouter event contained an invalid output_index".to_owned(),
            )
        })?;
        return Ok(format!("output_index={index}"));
    }
    match payload.get("item_id") {
        Some(Value::String(item_id)) => Ok(format!("item_id={item_id}")),
        Some(_) => Err(OpenRouterResponsesError::InvalidResponse(
            "an OpenRouter event contained a non-string item_id".to_owned(),
        )),
        None => Ok("default".to_owned()),
    }
}

struct CompletedResponse {
    outputs: Vec<ModelDriverOutput>,
    outcome: ModelOutcome,
    continuation: ModelContinuation,
    usage: Option<Usage>,
}

fn response_batches(
    model_request_id: ConversationEventId,
    completed: CompletedResponse,
) -> Result<Vec<ModelDriverOutputBatch>, OpenRouterResponsesError> {
    let mut batches = Vec::new();
    if !completed.outputs.is_empty() {
        batches.push(
            ModelDriverOutputBatch::try_new(completed.outputs).map_err(|_| {
                OpenRouterResponsesError::InvalidResponse(
                    "the completed response contained no model message".to_owned(),
                )
            })?,
        );
    }
    let terminal = ModelDriverOutput::ModelResponse(
        terminal_model_response(
            model_request_id,
            completed.outcome,
            completed.continuation,
            completed.usage,
        )
        .map_err(invalid_model_response)?,
    );
    batches.push(ModelDriverOutputBatch::from(terminal));
    Ok(batches)
}

fn completed_response(
    model_request_id: ConversationEventId,
    payload: &Value,
) -> Result<CompletedResponse, OpenRouterResponsesError> {
    let response = payload.get("response").ok_or_else(|| {
        OpenRouterResponsesError::InvalidResponse(
            "the terminal response event did not contain a response object".to_owned(),
        )
    })?;
    if !response.is_object() {
        return Err(OpenRouterResponsesError::InvalidResponse(
            "the terminal response event did not contain a response object".to_owned(),
        ));
    }
    let output = response.get("output").ok_or_else(|| {
        OpenRouterResponsesError::InvalidResponse(
            "the terminal response event did not contain an ordered output array".to_owned(),
        )
    })?;
    let output = output.as_array().ok_or_else(|| {
        OpenRouterResponsesError::InvalidResponse(
            "the terminal response event contained non-array output".to_owned(),
        )
    })?;
    let usage = parsed_usage(payload);

    let mut outputs = Vec::new();
    let mut has_assistant = false;
    let mut has_tool_request = false;
    let mut has_reasoning = false;
    let mut refusal = None;
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => {
                if let Some(output) = reasoning_replay_output(model_request_id, item)? {
                    outputs.push(output);
                }
                has_reasoning = true;
            }
            Some("message") => {
                let (assistant_text, message_refusal) = message_outputs(item)?;
                if let Some(text) = message_refusal {
                    refusal = Some(text);
                } else if let Some(text) = assistant_text {
                    outputs.push(assistant_response_output(model_request_id, text)?);
                    has_assistant = true;
                }
            }
            Some("function_call") => {
                outputs.push(tool_request_output(model_request_id, item)?);
                has_tool_request = true;
            }
            _ => {}
        }
    }

    if let Some(text) = refusal {
        return Ok(CompletedResponse {
            outputs: Vec::new(),
            outcome: failed_outcome(FailureCategory::Refusal, &text),
            continuation: ModelContinuation::Complete,
            usage,
        });
    }
    if !has_assistant && !has_tool_request && !has_reasoning {
        return Err(OpenRouterResponsesError::InvalidResponse(
            "the completed response contained no model message".to_owned(),
        ));
    }
    let continuation = if has_tool_request || (has_reasoning && !has_assistant) {
        ModelContinuation::Continue
    } else {
        ModelContinuation::Complete
    };
    Ok(CompletedResponse {
        outputs,
        outcome: ModelOutcome::Succeeded,
        continuation,
        usage,
    })
}

fn incomplete_response(payload: &Value) -> Result<CompletedResponse, OpenRouterResponsesError> {
    let failed_reason = payload
        .pointer("/response/incomplete_details/reason")
        .or_else(|| payload.pointer("/incomplete_details/reason"))
        .or_else(|| payload.pointer("/response/incomplete_details/failed_reason"))
        .or_else(|| payload.pointer("/incomplete_details/failed_reason"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (category, continuation) = if failed_reason == "content_filter" {
        (FailureCategory::Refusal, ModelContinuation::Complete)
    } else {
        (
            FailureCategory::ProviderFailure,
            ModelContinuation::Complete,
        )
    };
    Ok(CompletedResponse {
        outputs: Vec::new(),
        outcome: failed_outcome(
            category,
            "The model response was cut off before completing.",
        ),
        continuation,
        usage: parsed_usage(payload),
    })
}

fn failed_response(payload: &Value) -> Result<CompletedResponse, OpenRouterResponsesError> {
    let (category, message) = if is_context_limit_payload(payload) {
        (
            FailureCategory::ContextLimitExceeded,
            "The model context limit was exceeded.",
        )
    } else {
        (
            FailureCategory::ProviderFailure,
            "The model provider failed the invocation.",
        )
    };
    Ok(CompletedResponse {
        outputs: Vec::new(),
        outcome: failed_outcome(category, message),
        continuation: ModelContinuation::Complete,
        usage: parsed_usage(payload),
    })
}

fn assistant_response_output(
    model_request_id: ConversationEventId,
    content: String,
) -> Result<ModelDriverOutput, OpenRouterResponsesError> {
    Ok(ModelDriverOutput::AssistantResponse(
        AssistantResponse::new(model_request_id, content).map_err(invalid_assistant_response)?,
    ))
}

fn tool_request_output(
    model_request_id: ConversationEventId,
    item: &Value,
) -> Result<ModelDriverOutput, OpenRouterResponsesError> {
    let name = item.get("name").and_then(Value::as_str).ok_or_else(|| {
        OpenRouterResponsesError::InvalidResponse(
            "a completed function call did not contain a name".to_owned(),
        )
    })?;
    let arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OpenRouterResponsesError::InvalidResponse(
                "a completed function call did not contain string arguments".to_owned(),
            )
        })?;
    let arguments = parse_function_call_arguments(arguments)?;
    let data = item
        .get("call_id")
        .and_then(Value::as_str)
        .map(|call_id| {
            ModelData::new(Map::from_iter([(
                "call_id".to_owned(),
                Value::String(call_id.to_owned()),
            )]))
        })
        .transpose()
        .map_err(|error| OpenRouterResponsesError::InvalidResponse(error.to_string()))?;
    Ok(ModelDriverOutput::ToolRequest(
        ToolRequest::try_new(
            model_request_id,
            ToolName::try_new(name.to_owned())
                .map_err(|error| OpenRouterResponsesError::InvalidResponse(error.to_string()))?,
            arguments,
            data,
        )
        .map_err(invalid_tool_request)?,
    ))
}

fn parse_function_call_arguments(arguments: &str) -> Result<Value, OpenRouterResponsesError> {
    if arguments.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(arguments).map_err(|error| {
        OpenRouterResponsesError::InvalidResponse(format!(
            "a function call contained invalid JSON arguments: {error}"
        ))
    })
}

fn message_outputs(
    item: &Value,
) -> Result<(Option<String>, Option<String>), OpenRouterResponsesError> {
    let mut text = String::new();
    let mut refusal = None;
    if let Some(content) = item.get("content") {
        let content = content.as_array().ok_or_else(|| {
            OpenRouterResponsesError::InvalidResponse(
                "a completed message output item contained non-array content".to_owned(),
            )
        })?;
        for part in content {
            match part.get("type").and_then(Value::as_str) {
                Some("output_text") => {
                    if let Some(part_text) = part.get("text").and_then(Value::as_str) {
                        text.push_str(part_text);
                    }
                }
                Some("refusal") => {
                    if let Some(part_text) = part.get("text").and_then(Value::as_str) {
                        refusal = Some(part_text.to_owned());
                    }
                }
                _ => {}
            }
        }
    }
    let assistant_text = if text.trim().is_empty() {
        None
    } else {
        Some(text)
    };
    Ok((assistant_text, refusal))
}

fn reasoning_replay_output(
    model_request_id: ConversationEventId,
    item: &Value,
) -> Result<Option<ModelDriverOutput>, OpenRouterResponsesError> {
    let Some(item_id) = item.get("id").and_then(Value::as_str) else {
        return Ok(None);
    };
    let summary = match item.get("summary") {
        None => Vec::new(),
        Some(Value::Array(summary)) if summary.iter().all(is_summary_text) => summary.clone(),
        Some(_) => Vec::new(),
    };
    let encrypted_content = item
        .get("encrypted_content")
        .and_then(Value::as_str)
        .filter(|content| !content.is_empty());
    if summary.is_empty() && encrypted_content.is_none() {
        return Ok(None);
    }
    let mut payload = Map::new();
    payload.insert("item_id".to_owned(), Value::String(item_id.to_owned()));
    if let Some(encrypted_content) = encrypted_content {
        payload.insert(
            "encrypted_content".to_owned(),
            Value::String(encrypted_content.to_owned()),
        );
    }
    let message = joined_summary_texts(&summary);
    payload.insert("summary".to_owned(), Value::Array(summary));
    Ok(Some(ModelDriverOutput::ModelSpecificEvent(
        ModelSpecificEvent::new(
            model_request_id,
            REASONING_ITEM_EVENT_TYPE.to_owned(),
            PROVIDER_PAYLOAD_VERSION,
            Value::Object(payload),
            message,
        )
        .map_err(invalid_model_specific_event)?,
    )))
}

fn joined_summary_texts(summary: &[Value]) -> Option<String> {
    let message = summary
        .iter()
        .filter_map(|value| value.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!message.is_empty()).then_some(message)
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use serde_json::json;

    use super::{
        ConversationEventId, ModelDriverOutput, ModelDriverOutputBatch, OpenRouterResponsesError,
        ProcessEventResult, ResponseByteStream, ServerSentEvent, StreamInterpreterState,
        model_output_stream, process_event, resolve_event_type,
    };
    use tog::conversation::{
        AssistantResponse, ConversationEventPersistence, FailureCategory, ModelContinuation,
        ModelOutcome, ModelResponse, ToolRequest,
    };

    fn response_byte_stream(chunks: Vec<Vec<u8>>) -> ResponseByteStream {
        futures_util::stream::iter(
            chunks
                .into_iter()
                .map(Ok::<Vec<u8>, OpenRouterResponsesError>),
        )
        .boxed()
    }

    fn event(input: &str) -> ServerSentEvent {
        ServerSentEvent {
            name: None,
            data: input.to_owned(),
        }
    }

    fn batched_outputs(batches: Vec<ModelDriverOutputBatch>) -> Vec<ModelDriverOutput> {
        batches
            .into_iter()
            .flat_map(|batch| batch.into_outputs())
            .collect()
    }

    async fn collect_stream(input: &str) -> Vec<ModelDriverOutput> {
        let mut stream = model_output_stream(
            response_byte_stream(vec![input.as_bytes().to_vec()]),
            ConversationEventId::new(),
        );
        let mut outputs = Vec::new();
        while let Some(batch) = stream.next().await {
            outputs.extend(batch.expect("the batch should be valid").into_outputs());
        }
        outputs
    }

    fn assistant(output: &ModelDriverOutput) -> Option<&AssistantResponse> {
        match output {
            ModelDriverOutput::AssistantResponse(response) => Some(response),
            _ => None,
        }
    }

    fn specific(output: &ModelDriverOutput) -> Option<&tog::conversation::ModelSpecificEvent> {
        match output {
            ModelDriverOutput::ModelSpecificEvent(event) => Some(event),
            _ => None,
        }
    }

    fn tool_request(output: &ModelDriverOutput) -> Option<&ToolRequest> {
        match output {
            ModelDriverOutput::ToolRequest(request) => Some(request),
            _ => None,
        }
    }

    fn model_response(output: &ModelDriverOutput) -> Option<&ModelResponse> {
        match output {
            ModelDriverOutput::ModelResponse(response) => Some(response),
            _ => None,
        }
    }

    #[test]
    fn delta_events_emit_single_unpersisted_progress_batches() {
        let mut state = StreamInterpreterState::default();
        let results = process_event(
            event(
                "{\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}",
            ),
            ConversationEventId::new(),
            &mut state,
        )
        .expect("the delta should process");
        let ProcessEventResult::Batches(batches) = results else {
            panic!("the delta should not end the stream");
        };
        let outputs = batched_outputs(batches);
        assert_eq!(outputs.len(), 1);
        assert!(matches!(
            specific(&outputs[0]),
            Some(event) if event.provider_event_type() == "content_delta"
                && event.persistence() == ConversationEventPersistence::Unpersisted
                && event.message() == Some("Hello")
        ));
    }

    #[test]
    fn intermediate_done_events_are_ignored() {
        let mut state = StreamInterpreterState::default();
        let results = process_event(
            event(
                "{\"type\":\"response.output_text.done\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"text\":\"Hello\"}",
            ),
            ConversationEventId::new(),
            &mut state,
        )
        .expect("the done event should process");
        let ProcessEventResult::Batches(batches) = results else {
            panic!("the done event should not end the stream");
        };
        assert!(
            batches.is_empty(),
            "intermediate done events must not produce durable output"
        );
    }

    #[tokio::test]
    async fn content_deltas_stream_progress_then_the_completed_output_array_supplies_durable_outputs()
     {
        let input = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello \"}\n\n",
            "data: {\"type\":\"response.content_part.delta\",\"response_id\":\"resp_1\",\"output_index\":0,\"content_index\":0,\"item_id\":\"msg_1\",\"part\":{\"type\":\"output_text\",\"text\":\"world\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello world\",\"annotations\":[]}]}],\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"output_tokens_details\":{\"reasoning_tokens\":2},\"input_tokens_details\":{\"cached_tokens\":3},\"total_tokens\":15,\"cost\":0.0002}}}\n\n",
            "data: [DONE]\n\n"
        );

        let outputs = collect_stream(input).await;
        assert_eq!(outputs.len(), 4, "two deltas, answer, then terminal");
        for (position, expected_text) in [(0, "Hello "), (1, "world")] {
            assert!(matches!(
                specific(&outputs[position]),
                Some(event) if event.provider_event_type() == "content_delta"
                    && event.persistence() == ConversationEventPersistence::Unpersisted
                    && event.message() == Some(expected_text)
            ));
        }
        assert!(matches!(
            assistant(&outputs[2]),
            Some(response) if response.content() == "Hello world"
        ));
        let terminal = model_response(&outputs[3]).expect("the final output should be a terminal");
        assert!(matches!(terminal.outcome(), ModelOutcome::Succeeded));
        assert_eq!(terminal.continuation(), ModelContinuation::Complete);
        let usage = terminal.usage().expect("usage should be recorded");
        assert_eq!(usage.input_tokens(), Some(10));
        assert_eq!(usage.output_tokens(), Some(5));
        assert_eq!(usage.total_tokens(), Some(15));
        assert_eq!(usage.cache_read_input_tokens(), Some(3));
        assert_eq!(usage.reasoning_output_tokens(), Some(2));
        assert_eq!(usage.cost_usd(), Some(0.0002));
    }

    #[tokio::test]
    async fn the_completed_output_array_reconstructs_outputs_in_order() {
        let input = concat!(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"output\":[",
            "{\"type\":\"reasoning\",\"id\":\"rs_1\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Thinking.\"}],\"encrypted_content\":\"gAAAA-secret\"},",
            "{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Answer\",\"annotations\":[]}]},",
            "{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}",
            "]}}\n\n",
            "data: [DONE]\n\n"
        );

        let outputs = collect_stream(input).await;
        assert_eq!(
            outputs.len(),
            4,
            "reasoning replay, answer, tool request, then terminal"
        );
        assert!(matches!(
            specific(&outputs[0]),
            Some(event) if event.provider_event_type() == "response_reasoning_item"
                && event.persistence() == ConversationEventPersistence::Durable
                && event.payload()["item_id"] == "rs_1"
                && event.message() == Some("Thinking.")
        ));
        assert!(matches!(
            assistant(&outputs[1]),
            Some(response) if response.content() == "Answer"
        ));
        assert!(matches!(
            tool_request(&outputs[2]),
            Some(request) if request.tool_name().as_str() == "shell"
                && request.arguments() == &json!({ "command": "pwd" })
                && request.data().and_then(|data| data.content().get("call_id")).and_then(serde_json::Value::as_str)
                    == Some("call_1")
        ));
        let terminal = model_response(&outputs[3]).expect("the final output should be a terminal");
        assert!(matches!(terminal.outcome(), ModelOutcome::Succeeded));
        assert_eq!(terminal.continuation(), ModelContinuation::Continue);
    }

    #[tokio::test]
    async fn a_reasoning_only_completed_response_requests_continuation() {
        let input = concat!(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"output\":[",
            "{\"type\":\"reasoning\",\"id\":\"rs_1\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Thinking.\"}],\"encrypted_content\":\"gAAAA-secret\"}",
            "]}}\n\n",
            "data: [DONE]\n\n"
        );

        let outputs = collect_stream(input).await;
        assert_eq!(outputs.len(), 2, "reasoning replay then terminal");
        assert!(matches!(
            specific(&outputs[0]),
            Some(event) if event.persistence() == ConversationEventPersistence::Durable
        ));
        let terminal = model_response(&outputs[1]).expect("the terminal should be present");
        assert!(matches!(terminal.outcome(), ModelOutcome::Succeeded));
        assert_eq!(terminal.continuation(), ModelContinuation::Continue);
    }

    #[tokio::test]
    async fn a_reasoning_item_without_replay_state_still_requests_continuation() {
        let input = concat!(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"output\":[",
            "{\"type\":\"reasoning\",\"id\":\"rs_1\",\"summary\":[],\"content\":[{\"type\":\"reasoning_text\",\"text\":\"Thinking.\"}],\"encrypted_content\":null}",
            "]}}\n\n",
            "data: [DONE]\n\n"
        );

        let outputs = collect_stream(input).await;
        assert_eq!(
            outputs.len(),
            1,
            "only the terminal, no replay state to persist"
        );
        let terminal = model_response(&outputs[0]).expect("the terminal should be present");
        assert!(matches!(terminal.outcome(), ModelOutcome::Succeeded));
        assert_eq!(terminal.continuation(), ModelContinuation::Continue);
    }

    #[tokio::test]
    async fn tool_argument_deltas_are_unpersisted_progress_correlated_by_call_id() {
        let input = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":0,\"delta\":\"{\\\"command\\\":\\\"pwd\\\"}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"output\":[{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}]}}\n\n",
            "data: [DONE]\n\n"
        );

        let outputs = collect_stream(input).await;
        assert_eq!(outputs.len(), 3, "argument delta, request, then terminal");
        assert!(matches!(
            specific(&outputs[0]),
            Some(event) if event.provider_event_type() == "tool_call_arguments_delta"
                && event.payload()["call_id"] == "call_1"
                && event.persistence() == ConversationEventPersistence::Unpersisted
                && event.message() == Some("{\"command\":\"pwd\"}")
        ));
        assert!(matches!(
            tool_request(&outputs[1]),
            Some(request) if request.data().and_then(|data| data.content().get("call_id")).and_then(serde_json::Value::as_str)
                == Some("call_1")
        ));
        let terminal = model_response(&outputs[2]).expect("the terminal should be present");
        assert_eq!(terminal.continuation(), ModelContinuation::Continue);
    }

    #[tokio::test]
    async fn premature_body_end_records_a_failure_without_durable_outputs() {
        let input = "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}\n\n";

        let outputs = collect_stream(input).await;
        assert_eq!(outputs.len(), 2, "the progress event then the failure");
        assert!(matches!(
            specific(&outputs[0]),
            Some(event) if event.persistence() == ConversationEventPersistence::Unpersisted
        ));
        let terminal = model_response(&outputs[1]).expect("the failure should be a terminal");
        assert!(matches!(
            terminal.outcome(),
            ModelOutcome::Failed { failure } if failure.category() == FailureCategory::StreamInterrupted
        ));
        assert!(
            !outputs
                .iter()
                .any(|output| { assistant(output).is_some() || tool_request(output).is_some() }),
            "no durable output may be reconstructed from an interrupted stream"
        );
    }

    #[tokio::test]
    async fn a_done_sentinel_before_a_terminal_is_recorded_as_a_failure() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}\n\n",
            "data: [DONE]\n\n"
        );

        let outputs = collect_stream(input).await;
        assert_eq!(outputs.len(), 2);
        let terminal = model_response(&outputs[1]).expect("the failure should be a terminal");
        assert!(matches!(
            terminal.outcome(),
            ModelOutcome::Failed { failure } if failure.category() == FailureCategory::InvalidProviderResponse
        ));
    }

    #[tokio::test]
    async fn malformed_json_is_recorded_as_a_failure() {
        let outputs = collect_stream("data: not-json\n\n").await;
        let terminal = model_response(&outputs[0]).expect("the failure should be a terminal");
        assert!(matches!(
            terminal.outcome(),
            ModelOutcome::Failed { failure } if failure.category() == FailureCategory::InvalidProviderResponse
        ));
    }

    #[tokio::test]
    async fn a_mid_stream_provider_error_yields_only_a_failed_terminal() {
        let input = concat!(
            "data: {\"type\":\"error\",\"message\":\"late failure\"}\n\n",
            "data: [DONE]\n\n"
        );

        let outputs = collect_stream(input).await;
        assert_eq!(
            outputs.len(),
            1,
            "a failed response produces no durable outputs"
        );
        let terminal = model_response(&outputs[0]).expect("the failure should be a terminal");
        assert!(matches!(
            terminal.outcome(),
            ModelOutcome::Failed { failure } if failure.category() == FailureCategory::ProviderFailure
        ));
    }

    #[tokio::test]
    async fn a_context_limit_stream_error_is_a_context_limit_terminal() {
        let input = concat!(
            "data: {\"type\":\"error\",\"code\":\"context_length_exceeded\"}\n\n",
            "data: [DONE]\n\n"
        );

        let outputs = collect_stream(input).await;
        let terminal = model_response(&outputs[0]).expect("the failure should be a terminal");
        assert!(matches!(
            terminal.outcome(),
            ModelOutcome::Failed { failure } if failure.category() == FailureCategory::ContextLimitExceeded
        ));
    }

    #[tokio::test]
    async fn an_incomplete_response_is_a_failed_terminal() {
        let input = concat!(
            "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_1\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
            "data: [DONE]\n\n"
        );

        let outputs = collect_stream(input).await;
        assert_eq!(outputs.len(), 1);
        let terminal = model_response(&outputs[0]).expect("the failure should be a terminal");
        assert!(matches!(
            terminal.outcome(),
            ModelOutcome::Failed { failure } if failure.category() == FailureCategory::ProviderFailure
        ));
        assert_eq!(terminal.continuation(), ModelContinuation::Complete);
    }

    #[test]
    fn event_types_must_agree_with_payload_types() {
        assert_eq!(
            resolve_event_type(&json!({ "type": "response.completed" }), None).expect("valid"),
            "response.completed"
        );
        assert!(
            resolve_event_type(
                &json!({ "type": "response.completed" }),
                Some("response.failed".to_owned())
            )
            .is_err()
        );
    }
}
