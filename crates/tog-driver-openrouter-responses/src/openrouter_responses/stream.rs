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
    AssistantResponse, ConversationEventId, ConversationEventPersistence, FailureCategory,
    InvalidAssistantResponse, InvalidModelResponse, InvalidModelSpecificEvent, InvalidToolRequest,
    ModelContinuation, ModelData, ModelOutcome, ModelResponse, ModelSpecificEvent, ToolName,
    ToolRequest, Usage,
};
use tog::model_driver::{ModelDriverOutput, ModelDriverOutputBatch, ModelOutputStream};

type ResponseByteStream = BoxStream<'static, Result<Vec<u8>, OpenRouterResponsesError>>;
type ProviderOutputStream = BoxStream<'static, Result<ModelDriverEvent, OpenRouterResponsesError>>;

const CONTENT_DELTA_EVENT_TYPE: &str = "content_delta";
const REASONING_DELTA_EVENT_TYPE: &str = "reasoning_delta";
const REASONING_SUMMARY_DELTA_EVENT_TYPE: &str = "reasoning_summary_delta";
const TOOL_CALL_ARGUMENTS_DELTA_EVENT_TYPE: &str = "tool_call_arguments_delta";

pub(crate) fn model_output_stream(
    response_bytes: ResponseByteStream,
    model_request_id: ConversationEventId,
) -> ModelOutputStream {
    conversation_event_stream(model_event_stream(response_bytes), model_request_id)
}

pub(crate) fn context_limit_stream(model_request_id: ConversationEventId) -> ModelOutputStream {
    terminal_model_output(
        stream::once(async move {
            Ok(ModelDriverEvent::Terminal {
                outcome: failed_outcome(
                    FailureCategory::ContextLimitExceeded,
                    "The model context limit was exceeded.",
                ),
                continuation: ModelContinuation::Complete,
                usage: None,
            })
        })
        .boxed(),
        model_request_id,
    )
}

pub(crate) fn failed_terminal_stream(
    model_request_id: ConversationEventId,
    failure_stage: FailureStage,
    error: OpenRouterResponsesError,
) -> ModelOutputStream {
    let (category, message) = provider_failure(&error, failure_stage);
    terminal_model_output(
        stream::once(async move {
            Ok(ModelDriverEvent::Terminal {
                outcome: failed_outcome(category, &message),
                continuation: ModelContinuation::Complete,
                usage: None,
            })
        })
        .boxed(),
        model_request_id,
    )
}

#[derive(Clone)]
enum ModelDriverEvent {
    AssistantResponse {
        content: String,
    },
    ModelSpecificEvent {
        event_type: String,
        payload: Value,
        message: Option<String>,
        persistence: ConversationEventPersistence,
    },
    ToolRequest {
        tool_name: ToolName,
        arguments: Value,
        provider_call_id: Option<String>,
    },
    Terminal {
        outcome: ModelOutcome,
        continuation: ModelContinuation,
        usage: Option<Usage>,
    },
}

fn terminal_model_output(
    provider_events: ProviderOutputStream,
    model_request_id: ConversationEventId,
) -> ModelOutputStream {
    conversation_event_stream(provider_events, model_request_id)
}

struct ConversationEventStreamState {
    provider_events: ProviderOutputStream,
    model_request_id: ConversationEventId,
    pending_batches: VecDeque<ModelDriverOutputBatch>,
    terminated: bool,
}

fn conversation_event_stream(
    provider_events: ProviderOutputStream,
    model_request_id: ConversationEventId,
) -> ModelOutputStream {
    stream::unfold(
        ConversationEventStreamState {
            provider_events,
            model_request_id,
            pending_batches: VecDeque::new(),
            terminated: false,
        },
        |mut state| async move {
            if let Some(batch) = state.pending_batches.pop_front() {
                return Some((Ok(batch), state));
            }
            if state.terminated {
                return None;
            }
            match state.provider_events.next().await {
                Some(Ok(driver_event)) => {
                    if let Err(error) = translate_model_driver_event(
                        driver_event,
                        state.model_request_id,
                        &mut state,
                    ) {
                        fail_stream(&mut state, error);
                    }
                    state
                        .pending_batches
                        .pop_front()
                        .map(|batch| (Ok(batch), state))
                }
                Some(Err(error)) => {
                    fail_stream(&mut state, error);
                    state
                        .pending_batches
                        .pop_front()
                        .map(|batch| (Ok(batch), state))
                }
                None => None,
            }
        },
    )
    .boxed()
}

fn fail_stream(state: &mut ConversationEventStreamState, error: OpenRouterResponsesError) {
    let (category, message) = provider_failure(&error, FailureStage::DuringStream);
    let response = terminal_model_response(
        state.model_request_id,
        failed_outcome(category, &message),
        ModelContinuation::Complete,
        None,
    )
    .expect("the failed terminal model response should be valid");
    state.pending_batches = VecDeque::from([ModelDriverOutputBatch::from(
        ModelDriverOutput::ModelResponse(response),
    )]);
    state.terminated = true;
}

fn terminal_model_response(
    model_request_id: ConversationEventId,
    outcome: ModelOutcome,
    continuation: ModelContinuation,
    usage: Option<Usage>,
) -> Result<ModelResponse, InvalidModelResponse> {
    ModelResponse::new(model_request_id, Vec::new(), outcome, continuation, usage)
}

fn translate_model_driver_event(
    driver_event: ModelDriverEvent,
    model_request_id: ConversationEventId,
    state: &mut ConversationEventStreamState,
) -> Result<(), OpenRouterResponsesError> {
    let output = match driver_event {
        ModelDriverEvent::AssistantResponse { content } => ModelDriverOutput::AssistantResponse(
            AssistantResponse::new(model_request_id, content)
                .map_err(invalid_assistant_response)?,
        ),
        ModelDriverEvent::ModelSpecificEvent {
            event_type,
            payload,
            message,
            persistence,
        } => {
            let specific_event = match persistence {
                ConversationEventPersistence::Durable => ModelSpecificEvent::new(
                    model_request_id,
                    event_type,
                    PROVIDER_PAYLOAD_VERSION,
                    payload,
                    message,
                ),
                ConversationEventPersistence::Unpersisted => ModelSpecificEvent::new_unpersisted(
                    model_request_id,
                    event_type,
                    PROVIDER_PAYLOAD_VERSION,
                    payload,
                    message,
                ),
            }
            .map_err(invalid_model_specific_event)?;
            ModelDriverOutput::ModelSpecificEvent(specific_event)
        }
        ModelDriverEvent::ToolRequest {
            tool_name,
            arguments,
            provider_call_id,
        } => {
            let data = provider_call_id
                .map(|call_id| {
                    ModelData::new(Map::from_iter([(
                        "call_id".to_owned(),
                        Value::String(call_id),
                    )]))
                })
                .transpose()
                .map_err(|error| OpenRouterResponsesError::InvalidResponse(error.to_string()))?;
            ModelDriverOutput::ToolRequest(
                ToolRequest::try_new(model_request_id, tool_name, arguments, data)
                    .map_err(invalid_tool_request)?,
            )
        }
        ModelDriverEvent::Terminal {
            outcome,
            continuation,
            usage,
        } => {
            let response = terminal_model_response(model_request_id, outcome, continuation, usage)
                .map_err(invalid_model_response)?;
            state.pending_batches = VecDeque::from([ModelDriverOutputBatch::from(
                ModelDriverOutput::ModelResponse(response),
            )]);
            state.terminated = true;
            return Ok(());
        }
    };
    state
        .pending_batches
        .push_back(ModelDriverOutputBatch::from(output));
    Ok(())
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
    model_outputs: VecDeque<ModelDriverEvent>,
    response_end: Option<ResponseEnd>,
    terminated: bool,
}

#[derive(Clone, Copy)]
enum ResponseEnd {
    BodyEnded,
    DoneSentinel,
}

fn model_event_stream(response_bytes: ResponseByteStream) -> ProviderOutputStream {
    let state = OpenRouterResponsesStreamState {
        response_bytes,
        decoder: ServerSentEventDecoder::default(),
        interpreter: StreamInterpreterState::default(),
        model_outputs: VecDeque::new(),
        response_end: None,
        terminated: false,
    };

    stream::unfold(state, |mut state| async move {
        loop {
            if let Some(model_output) = state.model_outputs.pop_front() {
                return Some((Ok(model_output), state));
            }
            if state.terminated {
                return None;
            }
            if let Some(server_sent_event) = state.decoder.events.pop_front() {
                match process_event(server_sent_event, &mut state.interpreter) {
                    Ok(ProcessEventResult::Outputs(model_outputs)) => {
                        if state.interpreter.completed {
                            state.terminated = true;
                        }
                        state.model_outputs.extend(model_outputs);
                    }
                    Ok(ProcessEventResult::Done) => {
                        state.decoder.events.clear();
                        state.response_end = Some(ResponseEnd::DoneSentinel);
                    }
                    Err(error) => {
                        state.terminated = true;
                        return Some((Err(error), state));
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
                return Some((Err(error), state));
            }

            match state.response_bytes.next().await {
                Some(Ok(bytes)) => {
                    if let Err(error) = state.decoder.push(&bytes) {
                        state.terminated = true;
                        return Some((Err(error), state));
                    }
                }
                Some(Err(error)) => {
                    state.terminated = true;
                    return Some((Err(error), state));
                }
                None => {
                    if let Err(error) = state.decoder.finish() {
                        state.terminated = true;
                        return Some((Err(error), state));
                    }
                    state.response_end = Some(ResponseEnd::BodyEnded);
                }
            }
        }
    })
    .boxed()
}

enum ProcessEventResult {
    Outputs(Vec<ModelDriverEvent>),
    Done,
}

#[derive(Default)]
struct StreamInterpreterState {
    function_call_call_ids: HashMap<String, String>,
    completed: bool,
}

fn process_event(
    server_sent_event: ServerSentEvent,
    state: &mut StreamInterpreterState,
) -> Result<ProcessEventResult, OpenRouterResponsesError> {
    if server_sent_event.data == "[DONE]" {
        return Ok(ProcessEventResult::Done);
    }

    let payload: Value = serde_json::from_str(&server_sent_event.data)
        .map_err(|error| OpenRouterResponsesError::InvalidResponse(error.to_string()))?;
    let event_type = resolve_event_type(&payload, server_sent_event.name)?;

    let model_events = match event_type.as_str() {
        "response.output_text.delta" | "response.content_part.delta" => {
            text_delta_progress(&payload)
        }
        "response.reasoning_text.delta" => delta_progress(
            REASONING_DELTA_EVENT_TYPE,
            payload
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        "response.reasoning_summary_text.delta" => delta_progress(
            REASONING_SUMMARY_DELTA_EVENT_TYPE,
            payload
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        "response.function_call_arguments.delta" => {
            tool_argument_delta_progress(&payload, &state.function_call_call_ids)?
        }
        "response.output_item.added" => {
            track_function_call(&payload, &mut state.function_call_call_ids)?;
            Vec::new()
        }
        "response.completed" | "response.done" => {
            let model_events = final_outputs(&payload)?;
            state.completed = true;
            model_events
        }
        "response.incomplete" => {
            let model_events = incomplete_terminal(&payload)?;
            state.completed = true;
            model_events
        }
        "error" | "response.failed" => {
            let model_events = failed_terminal(&payload)?;
            state.completed = true;
            model_events
        }
        _ => Vec::new(),
    };
    Ok(ProcessEventResult::Outputs(model_events))
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

fn delta_progress(event_type: &str, delta: &str) -> Vec<ModelDriverEvent> {
    if delta.is_empty() {
        Vec::new()
    } else {
        vec![progress_event(
            event_type,
            json!({}),
            Some(delta.to_owned()),
        )]
    }
}

fn text_delta_progress(payload: &Value) -> Vec<ModelDriverEvent> {
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
    delta_progress(CONTENT_DELTA_EVENT_TYPE, delta)
}

fn progress_event(event_type: &str, payload: Value, message: Option<String>) -> ModelDriverEvent {
    ModelDriverEvent::ModelSpecificEvent {
        event_type: event_type.to_owned(),
        payload,
        message,
        persistence: ConversationEventPersistence::Unpersisted,
    }
}

fn tool_argument_delta_progress(
    payload: &Value,
    function_call_call_ids: &HashMap<String, String>,
) -> Result<Vec<ModelDriverEvent>, OpenRouterResponsesError> {
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
    Ok(vec![progress_event(
        TOOL_CALL_ARGUMENTS_DELTA_EVENT_TYPE,
        correlation,
        Some(delta.to_owned()),
    )])
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

fn final_outputs(payload: &Value) -> Result<Vec<ModelDriverEvent>, OpenRouterResponsesError> {
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

    let mut model_events = Vec::new();
    let mut has_assistant = false;
    let mut has_tool_request = false;
    let mut has_reasoning = false;
    let mut refusal = None;
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => {
                if let Some(event) = reasoning_replay_event(item) {
                    model_events.push(event);
                }
                has_reasoning = true;
            }
            Some("message") => {
                let (assistant_text, message_refusal) = message_outputs(item)?;
                if let Some(text) = message_refusal {
                    refusal = Some(text);
                } else if let Some(text) = assistant_text {
                    model_events.push(ModelDriverEvent::AssistantResponse { content: text });
                    has_assistant = true;
                }
            }
            Some("function_call") => {
                model_events.push(function_call_event(item)?);
                has_tool_request = true;
            }
            _ => {}
        }
    }

    if let Some(text) = refusal {
        return Ok(vec![ModelDriverEvent::Terminal {
            outcome: failed_outcome(FailureCategory::Refusal, &text),
            continuation: ModelContinuation::Complete,
            usage,
        }]);
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
    model_events.push(ModelDriverEvent::Terminal {
        outcome: ModelOutcome::Succeeded,
        continuation,
        usage,
    });
    Ok(model_events)
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

fn function_call_event(item: &Value) -> Result<ModelDriverEvent, OpenRouterResponsesError> {
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
    Ok(ModelDriverEvent::ToolRequest {
        tool_name: ToolName::try_new(name.to_owned())
            .map_err(|error| OpenRouterResponsesError::InvalidResponse(error.to_string()))?,
        arguments: parse_function_call_arguments(arguments)?,
        provider_call_id: item
            .get("call_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
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

fn reasoning_replay_event(item: &Value) -> Option<ModelDriverEvent> {
    let item_id = item.get("id").and_then(Value::as_str)?;
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
        return None;
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
    Some(ModelDriverEvent::ModelSpecificEvent {
        event_type: REASONING_ITEM_EVENT_TYPE.to_owned(),
        payload: Value::Object(payload),
        message,
        persistence: ConversationEventPersistence::Durable,
    })
}

fn joined_summary_texts(summary: &[Value]) -> Option<String> {
    let message = summary
        .iter()
        .filter_map(|value| value.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!message.is_empty()).then_some(message)
}

fn incomplete_terminal(payload: &Value) -> Result<Vec<ModelDriverEvent>, OpenRouterResponsesError> {
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
    Ok(vec![ModelDriverEvent::Terminal {
        outcome: failed_outcome(
            category,
            "The model response was cut off before completing.",
        ),
        continuation,
        usage: parsed_usage(payload),
    }])
}

fn failed_terminal(payload: &Value) -> Result<Vec<ModelDriverEvent>, OpenRouterResponsesError> {
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
    Ok(vec![ModelDriverEvent::Terminal {
        outcome: failed_outcome(category, message),
        continuation: ModelContinuation::Complete,
        usage: parsed_usage(payload),
    }])
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use serde_json::json;

    use super::{
        ConversationEventPersistence, ModelDriverEvent, OpenRouterResponsesError,
        ProcessEventResult, ResponseByteStream, ServerSentEvent, StreamInterpreterState,
        model_event_stream, model_output_stream, process_event, resolve_event_type,
    };
    use tog::conversation::{ModelContinuation, ModelOutcome};
    use tog::model_driver::{ModelDriverOutput, ModelOutputStream};

    fn response_byte_stream(chunks: Vec<Vec<u8>>) -> ResponseByteStream {
        futures_util::stream::iter(
            chunks
                .into_iter()
                .map(Ok::<Vec<u8>, OpenRouterResponsesError>),
        )
        .boxed()
    }

    async fn collect_event_stream(
        input: &str,
    ) -> Vec<Result<ModelDriverEvent, OpenRouterResponsesError>> {
        model_event_stream(response_byte_stream(vec![input.as_bytes().to_vec()]))
            .collect()
            .await
    }

    fn event(input: &str) -> ServerSentEvent {
        ServerSentEvent {
            name: None,
            data: input.to_owned(),
        }
    }

    #[test]
    fn delta_events_emit_unpersisted_progress_only() {
        let mut state = StreamInterpreterState::default();
        let results = process_event(event(
            "{\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}",
        ), &mut state)
        .expect("the delta should process");
        let ProcessEventResult::Outputs(outputs) = results else {
            panic!("the delta should not end the stream");
        };
        assert_eq!(outputs.len(), 1);
        assert!(matches!(
            &outputs[0],
            ModelDriverEvent::ModelSpecificEvent {
                event_type,
                persistence,
                message: Some(message),
                ..
            } if event_type == "content_delta"
                && *persistence == ConversationEventPersistence::Unpersisted
                && message == "Hello"
        ));
    }

    #[test]
    fn output_text_done_events_are_ignored_for_durability() {
        let mut state = StreamInterpreterState::default();
        let results = process_event(
            event(
                "{\"type\":\"response.output_text.done\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"text\":\"Hello\"}",
            ),
            &mut state,
        )
        .expect("the done event should process");
        let ProcessEventResult::Outputs(outputs) = results else {
            panic!("the done event should not end the stream");
        };
        assert!(
            outputs.is_empty(),
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

        let events = collect_event_stream(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the response stream should parse");

        assert_eq!(events.len(), 4, "two deltas, answer, then terminal");
        for (position, expected_text) in [(0, "Hello "), (1, "world")] {
            assert!(matches!(
                &events[position],
                ModelDriverEvent::ModelSpecificEvent {
                    event_type,
                    persistence,
                    message: Some(message),
                    ..
                } if event_type == "content_delta"
                    && *persistence == ConversationEventPersistence::Unpersisted
                    && message == expected_text
            ));
        }
        assert!(matches!(
            &events[2],
            ModelDriverEvent::AssistantResponse { content } if content == "Hello world"
        ));
        let ModelDriverEvent::Terminal {
            outcome,
            continuation,
            usage,
        } = &events[3]
        else {
            panic!("the final event should be a terminal");
        };
        assert!(matches!(outcome, ModelOutcome::Succeeded));
        assert_eq!(*continuation, ModelContinuation::Complete);
        let usage = usage.as_ref().expect("usage should be recorded");
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

        let events = collect_event_stream(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the response stream should parse");

        assert_eq!(
            events.len(),
            4,
            "reasoning replay, answer, tool request, then terminal"
        );
        assert!(matches!(
            &events[0],
            ModelDriverEvent::ModelSpecificEvent {
                event_type,
                payload,
                persistence,
                message,
            } if event_type == "response_reasoning_item"
                && *persistence == ConversationEventPersistence::Durable
                && payload["item_id"] == "rs_1"
                && payload["encrypted_content"] == "gAAAA-secret"
                && message.as_deref() == Some("Thinking.")
        ));
        assert!(matches!(
            &events[1],
            ModelDriverEvent::AssistantResponse { content } if content == "Answer"
        ));
        assert!(matches!(
            &events[2],
            ModelDriverEvent::ToolRequest {
                tool_name,
                arguments,
                provider_call_id,
            } if tool_name.as_str() == "shell"
                && arguments == &json!({ "command": "pwd" })
                && provider_call_id.as_deref() == Some("call_1")
        ));
        assert!(matches!(
            &events[3],
            ModelDriverEvent::Terminal {
                outcome: ModelOutcome::Succeeded,
                continuation: ModelContinuation::Continue,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_reasoning_only_completed_response_requests_continuation() {
        let input = concat!(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"output\":[",
            "{\"type\":\"reasoning\",\"id\":\"rs_1\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Thinking.\"}],\"encrypted_content\":\"gAAAA-secret\"}",
            "]}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_event_stream(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the reasoning stream should parse");

        assert_eq!(events.len(), 2, "reasoning replay then terminal");
        assert!(matches!(
            &events[0],
            ModelDriverEvent::ModelSpecificEvent {
                event_type,
                persistence,
                ..
            } if event_type == "response_reasoning_item"
                && *persistence == ConversationEventPersistence::Durable
        ));
        assert!(matches!(
            &events[1],
            ModelDriverEvent::Terminal {
                outcome: ModelOutcome::Succeeded,
                continuation: ModelContinuation::Continue,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_reasoning_item_without_replay_state_still_requests_continuation() {
        let input = concat!(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"output\":[",
            "{\"type\":\"reasoning\",\"id\":\"rs_1\",\"summary\":[],\"content\":[{\"type\":\"reasoning_text\",\"text\":\"Thinking.\"}],\"encrypted_content\":null}",
            "]}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_event_stream(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the reasoning stream should parse");

        assert_eq!(
            events.len(),
            1,
            "only the terminal, no replay state to persist"
        );
        assert!(matches!(
            &events[0],
            ModelDriverEvent::Terminal {
                outcome: ModelOutcome::Succeeded,
                continuation: ModelContinuation::Continue,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn tool_argument_deltas_are_unpersisted_progress_correlated_by_call_id() {
        let input = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":0,\"delta\":\"{\\\"command\\\":\\\"pwd\\\"}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"output\":[{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}]}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_event_stream(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the tool call stream should parse");

        assert_eq!(events.len(), 3, "argument delta, request, then terminal");
        assert!(matches!(
            &events[0],
            ModelDriverEvent::ModelSpecificEvent {
                event_type,
                payload,
                persistence,
                message: Some(message),
            } if event_type == "tool_call_arguments_delta"
                && payload["call_id"] == "call_1"
                && *persistence == ConversationEventPersistence::Unpersisted
                && message == "{\"command\":\"pwd\"}"
        ));
        assert!(matches!(
            &events[1],
            ModelDriverEvent::ToolRequest {
                arguments,
                provider_call_id,
                ..
            } if arguments == &json!({ "command": "pwd" })
                && provider_call_id.as_deref() == Some("call_1")
        ));
        assert!(matches!(
            &events[2],
            ModelDriverEvent::Terminal {
                outcome: ModelOutcome::Succeeded,
                continuation: ModelContinuation::Continue,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn premature_body_end_yields_only_progress_and_an_interruption() {
        let input = "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}\n\n";
        let mut events = model_event_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        assert!(matches!(
            events.next().await,
            Some(Ok(ModelDriverEvent::ModelSpecificEvent { message: Some(message), .. }))
                if message == "Hello"
        ));
        assert!(matches!(
            events.next().await,
            Some(Err(OpenRouterResponsesError::StreamInterrupted(_)))
        ));
        assert!(
            events.next().await.is_none(),
            "no durable output may follow an interrupted stream"
        );
    }

    #[tokio::test]
    async fn a_done_sentinel_before_a_terminal_is_an_invalid_response() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}\n\n",
            "data: [DONE]\n\n"
        );
        let mut events = model_event_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        assert!(matches!(
            events.next().await,
            Some(Ok(ModelDriverEvent::ModelSpecificEvent { .. }))
        ));
        assert!(matches!(
            events.next().await,
            Some(Err(OpenRouterResponsesError::InvalidResponse(_)))
        ));
        assert!(events.next().await.is_none());
    }

    #[tokio::test]
    async fn malformed_json_is_a_stream_error() {
        let mut events =
            model_event_stream(response_byte_stream(vec![b"data: not-json\n\n".to_vec()]));

        assert!(matches!(
            events.next().await,
            Some(Err(OpenRouterResponsesError::InvalidResponse(_)))
        ));
    }

    #[tokio::test]
    async fn a_mid_stream_provider_error_yields_only_a_failed_terminal() {
        let input = concat!(
            "data: {\"type\":\"error\",\"message\":\"late failure\"}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_event_stream(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the provider failure should produce a terminal");

        assert_eq!(
            events.len(),
            1,
            "a failed response produces no durable outputs"
        );
        assert!(matches!(
            &events[0],
            ModelDriverEvent::Terminal {
                outcome: ModelOutcome::Failed { failure },
                continuation: ModelContinuation::Complete,
                ..
            } if failure.category() == tog::conversation::FailureCategory::ProviderFailure
        ));
    }

    #[tokio::test]
    async fn a_context_limit_stream_error_is_a_context_limit_terminal() {
        let input = concat!(
            "data: {\"type\":\"error\",\"code\":\"context_length_exceeded\"}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_event_stream(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the context limit error should produce a terminal");

        assert!(matches!(
            &events[0],
            ModelDriverEvent::Terminal {
                outcome: ModelOutcome::Failed { failure },
                ..
            } if failure.category() == tog::conversation::FailureCategory::ContextLimitExceeded
        ));
    }

    #[tokio::test]
    async fn an_incomplete_response_is_a_failed_terminal() {
        let input = concat!(
            "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_1\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_event_stream(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the incomplete response should parse");

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ModelDriverEvent::Terminal {
                outcome: ModelOutcome::Failed { failure },
                continuation: ModelContinuation::Complete,
                ..
            } if failure.category() == tog::conversation::FailureCategory::ProviderFailure
        ));
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

    #[tokio::test]
    async fn interrupted_stream_translates_to_a_failed_terminal_without_durable_outputs() {
        let mut stream = model_output_stream(
            response_byte_stream(vec![concat!(
                "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"\"}}\n\n",
                "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":0,\"delta\":\"{\\\"command\\\":\\\"pwd\\\"}\"}\n\n"
            )
            .as_bytes()
            .to_vec()]),
            tog::conversation::ConversationEventId::new(),
        );

        let outputs = collect_outputs(&mut stream).await;
        assert_eq!(outputs.len(), 2, "unpersisted progress then the failure");
        assert!(matches!(
            &outputs[0],
            ModelDriverOutput::ModelSpecificEvent(event)
                if event.persistence() == ConversationEventPersistence::Unpersisted
        ));
        assert!(matches!(
            &outputs[1],
            ModelDriverOutput::ModelResponse(response)
                if matches!(response.outcome(), ModelOutcome::Failed { .. })
        ));
        assert!(
            !outputs.iter().any(|output| matches!(
                output,
                ModelDriverOutput::ToolRequest(_) | ModelDriverOutput::AssistantResponse(_)
            )),
            "no durable output may be reconstructed from an interrupted stream"
        );
    }

    async fn collect_outputs(stream: &mut ModelOutputStream) -> Vec<ModelDriverOutput> {
        let mut outputs = Vec::new();
        while let Some(batch) = stream.next().await {
            let batch = batch.expect("the batch should be valid");
            outputs.extend(batch.into_outputs());
        }
        outputs
    }
}
