use std::collections::{HashMap, VecDeque};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use futures_util::future::BoxFuture;
use futures_util::stream::{self, BoxStream};
use futures_util::{FutureExt, StreamExt};
use reqwest::Client;
use reqwest::StatusCode;
use schemars::Schema;
use serde_json::{Map, Value, json};

use tog::conversation::{
    AssistantResponse, ConversationEventId, ConversationEventPayload, FailureCategory,
    InvalidAssistantResponse, InvalidModelResponse, InvalidModelSpecificEvent, InvalidToolRequest,
    ModelData, ModelId, ModelOutcome, ModelResponse, ModelSource, ModelSpecificEvent,
    OperationFailure, ProviderId, ToolRequest, Usage, UserContent,
};
use tog::conversation::{ToolAvailability, ToolName};
use tog::conversation_event_store::ConversationEventRecord;
use tog::model_driver::{
    ModelDriver, ModelDriverError, ModelDriverOutput, ModelDriverOutputBatch, ModelOutputStream,
    TurnInput,
};

type ResponseByteStream = BoxStream<'static, Result<Vec<u8>, OpenRouterCompletionsError>>;
type ProviderOutputStream =
    BoxStream<'static, Result<ModelDriverEvent, OpenRouterCompletionsError>>;

const PROVIDER_PAYLOAD_VERSION: u32 = 1;
const CONTENT_DELTA_EVENT_TYPE: &str = "content_delta";

#[derive(Debug, Eq, PartialEq)]
pub enum OpenRouterCompletionsError {
    Authentication(String),
    RateLimited(String),
    Transport(String),
    InvalidRequest(String),
    InvalidResponse(String),
    StreamInterrupted(String),
    Provider(String),
}

impl Display for OpenRouterCompletionsError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Authentication(message) => write!(formatter, "authentication failed: {message}"),
            Self::RateLimited(message) => write!(formatter, "rate limited: {message}"),
            Self::Transport(message) => write!(formatter, "model transport failed: {message}"),
            Self::InvalidRequest(message) => write!(formatter, "invalid model request: {message}"),
            Self::InvalidResponse(message) => {
                write!(formatter, "invalid model response: {message}")
            }
            Self::StreamInterrupted(message) => write!(
                formatter,
                "model response stream was interrupted: {message}"
            ),
            Self::Provider(message) => write!(formatter, "model provider failed: {message}"),
        }
    }
}

impl std::error::Error for OpenRouterCompletionsError {}

#[derive(Clone)]
enum ModelDriverEvent {
    AssistantResponse {
        content: String,
    },
    ModelSpecificEvent {
        event_type: String,
        message: Option<String>,
    },
    ToolRequest {
        tool_name: ToolName,
        arguments: Value,
        provider_call_id: Option<String>,
    },
    Terminal {
        outcome: TerminalModelOutcome,
        usage: Option<Usage>,
    },
}

#[derive(Clone)]
enum TerminalModelOutcome {
    Succeeded,
    Failed {
        category: FailureCategory,
        message: String,
    },
}

pub struct OpenRouterCompletionsModelDriver {
    http_client: Client,
    api_key: String,
    chat_completions_url: String,
    source: ModelSource,
}

impl OpenRouterCompletionsModelDriver {
    pub fn from_environment(model: ModelId) -> Result<Self, OpenRouterCompletionsError> {
        let api_key = std::env::var("OPENROUTER_API_KEY").map_err(|_| {
            OpenRouterCompletionsError::Authentication("OPENROUTER_API_KEY must be set".to_owned())
        })?;
        let base_url = std::env::var("TOG_OPENROUTER_BASE_URL")
            .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_owned());
        Ok(Self {
            http_client: Client::new(),
            api_key,
            chat_completions_url: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            source: ModelSource::new(
                ProviderId::from_str("openrouter")
                    .expect("the OpenRouter provider identifier should be valid"),
                model,
            ),
        })
    }
}

impl ModelDriver for OpenRouterCompletionsModelDriver {
    fn source(&self) -> &ModelSource {
        &self.source
    }

    fn invoke<'invoke>(
        &'invoke self,
        input: TurnInput<'invoke>,
    ) -> BoxFuture<'invoke, Result<ModelOutputStream, ModelDriverError>> {
        let model_request_id = input.model_request_id();
        let events = input.events();
        let mut request_body = Map::new();
        request_body.insert(
            "model".to_owned(),
            Value::String(self.source.model().as_str().to_owned()),
        );
        request_body.insert("messages".to_owned(), chat_messages(events));
        let immediate_tools = input
            .tools()
            .map(|tools| {
                tools
                    .tools()
                    .iter()
                    .filter(|tool| tool.availability() == ToolAvailability::Immediate)
                    .map(|tool| provider_tool(tool.definition()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !immediate_tools.is_empty() {
            request_body.insert("tools".to_owned(), Value::Array(immediate_tools));
        }
        request_body.insert("stream".to_owned(), Value::Bool(true));

        let request = self
            .http_client
            .post(&self.chat_completions_url)
            .bearer_auth(&self.api_key)
            .json(&request_body)
            .build();
        let http_client = self.http_client.clone();
        async move {
            let request = match request {
                Ok(request) => request,
                Err(error) => {
                    let error = OpenRouterCompletionsError::InvalidRequest(error.to_string());
                    return Ok(failed_terminal_stream(
                        model_request_id,
                        FailureStage::BeforeStream,
                        error,
                    ));
                }
            };
            let response = match http_client.execute(request).await {
                Ok(response) => response,
                Err(error) => {
                    return Ok(failed_terminal_stream(
                        model_request_id,
                        FailureStage::BeforeStream,
                        OpenRouterCompletionsError::Transport(error.to_string()),
                    ));
                }
            };
            let response_status = response.status();
            if !response_status.is_success() {
                let response_body = match response.text().await {
                    Ok(response_body) => response_body,
                    Err(error) => {
                        return Ok(failed_terminal_stream(
                            model_request_id,
                            FailureStage::BeforeStream,
                            OpenRouterCompletionsError::Transport(error.to_string()),
                        ));
                    }
                };
                return match classify_response_failure(response_status, response_body) {
                    Ok(()) => Ok(context_limit_stream(model_request_id)),
                    Err(error) => Ok(failed_terminal_stream(
                        model_request_id,
                        FailureStage::BeforeStream,
                        error,
                    )),
                };
            }

            let response_bytes = response
                .bytes_stream()
                .map(|result| {
                    result
                        .map(|bytes| bytes.to_vec())
                        .map_err(|error| OpenRouterCompletionsError::Transport(error.to_string()))
                })
                .boxed();
            Ok(conversation_event_stream(
                model_output_stream(response_bytes),
                model_request_id,
            ))
        }
        .boxed()
    }
}

fn chat_messages(events: &[ConversationEventRecord]) -> Value {
    let provider_call_ids = provider_tool_call_ids(events);
    let mut messages = Vec::<Value>::new();
    let mut open_assistant: Option<(usize, ConversationEventId)> = None;
    for record in events {
        let event = record.event();
        let content = match event.payload() {
            ConversationEventPayload::User(user) => {
                let text = user
                    .content()
                    .iter()
                    .map(|part| match part {
                        UserContent::Text(text) => text.as_str(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(json!({ "role": "user", "content": text }))
            }
            ConversationEventPayload::AssistantResponse(response) => {
                let message = json!({ "role": "assistant", "content": response.content() });
                open_assistant = Some((messages.len(), response.model_request_id()));
                Some(message)
            }
            ConversationEventPayload::ToolRequest(request) => {
                let call_id = provider_call_ids
                    .get(&event.id())
                    .cloned()
                    .unwrap_or_else(|| event.id().to_string());
                let tool_call = json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": request.tool_name().as_str(),
                        "arguments": serde_json::to_string(request.arguments())
                            .expect("the tool request arguments should serialize"),
                    }
                });
                match open_assistant {
                    Some((index, request_id)) if request_id == request.model_request_id() => {
                        let message = messages[index]
                            .as_object_mut()
                            .expect("the open assistant message should be an object");
                        if let Some(tool_calls) = message.get_mut("tool_calls") {
                            tool_calls
                                .as_array_mut()
                                .expect("the open tool calls should be an array")
                                .push(tool_call);
                        } else {
                            message.insert("tool_calls".to_owned(), Value::Array(vec![tool_call]));
                        }
                        None
                    }
                    _ => {
                        open_assistant = None;
                        Some(json!({
                            "role": "assistant",
                            "content": Value::Null,
                            "tool_calls": [tool_call],
                        }))
                    }
                }
            }
            ConversationEventPayload::ToolResponse(response) => {
                let call_id = provider_call_ids
                    .get(&response.tool_request_id())
                    .cloned()
                    .unwrap_or_else(|| response.tool_request_id().to_string());
                open_assistant = None;
                Some(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": serde_json::to_string(response.outcome())
                        .expect("the tool response outcome should serialize"),
                }))
            }
            ConversationEventPayload::TurnStart(_)
            | ConversationEventPayload::TurnEnd(_)
            | ConversationEventPayload::ModelRequest(_)
            | ConversationEventPayload::ModelResponse(_)
            | ConversationEventPayload::ModelSpecificEvent(_)
            | ConversationEventPayload::Automation(_)
            | ConversationEventPayload::Context(_)
            | ConversationEventPayload::Tools(_) => None,
        };
        if let Some(content) = content {
            messages.push(content);
        }
    }
    Value::Array(messages)
}

fn provider_tool(definition: &tog::conversation::ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": definition.name().as_str(),
            "description": definition.description(),
            "parameters": provider_schema(definition.parameters()),
        }
    })
}

fn provider_schema(schema: &Schema) -> Value {
    let mut schema = schema.as_value().clone();
    if let Value::Object(schema_object) = &mut schema {
        schema_object.remove("$schema");
    }
    schema
}

fn provider_tool_call_ids(
    events: &[ConversationEventRecord],
) -> HashMap<ConversationEventId, String> {
    events
        .iter()
        .filter_map(|record| match record.event().payload() {
            ConversationEventPayload::ToolRequest(request) => {
                Some((record.event().id(), provider_tool_call_id(request)))
            }
            _ => None,
        })
        .collect()
}

fn provider_tool_call_id(request: &ToolRequest) -> String {
    request
        .data()
        .and_then(|data| data.content().get("call_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default()
}

fn classify_response_failure(
    status: StatusCode,
    body: String,
) -> Result<(), OpenRouterCompletionsError> {
    if status == StatusCode::BAD_REQUEST && is_context_limit_error(&body) {
        return Ok(());
    }

    Err(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            OpenRouterCompletionsError::Authentication(body)
        }
        StatusCode::TOO_MANY_REQUESTS => OpenRouterCompletionsError::RateLimited(body),
        StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => {
            OpenRouterCompletionsError::InvalidRequest(body)
        }
        _ => OpenRouterCompletionsError::Provider(format!(
            "OpenRouter chat completions returned {status}: {body}"
        )),
    })
}

fn is_context_limit_error(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .is_some_and(|payload| is_context_limit_payload(&payload))
}

fn is_context_limit_payload(payload: &Value) -> bool {
    let error_text = payload
        .pointer("/error/code")
        .or_else(|| payload.pointer("/error/message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    [
        "context_length",
        "context_window",
        "maximum context",
        "too many tokens",
    ]
    .into_iter()
    .any(|needle| error_text.contains(needle))
}

fn context_limit_stream(model_request_id: ConversationEventId) -> ModelOutputStream {
    terminal_model_output(
        stream::once(async move {
            Ok(ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Failed {
                    category: FailureCategory::ContextLimitExceeded,
                    message: "The model context limit was exceeded.".to_owned(),
                },
                usage: None,
            })
        })
        .boxed(),
        model_request_id,
    )
}

#[derive(Clone, Copy)]
enum FailureStage {
    BeforeStream,
    DuringStream,
}

fn failed_terminal_stream(
    model_request_id: ConversationEventId,
    failure_stage: FailureStage,
    error: OpenRouterCompletionsError,
) -> ModelOutputStream {
    let (category, message) = provider_failure(&error, failure_stage);
    terminal_model_output(
        stream::once(async move {
            Ok(ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Failed { category, message },
                usage: None,
            })
        })
        .boxed(),
        model_request_id,
    )
}

fn provider_failure(
    error: &OpenRouterCompletionsError,
    failure_stage: FailureStage,
) -> (FailureCategory, String) {
    match error {
        OpenRouterCompletionsError::Authentication(_) => (
            FailureCategory::Authentication,
            "The model provider could not authenticate the invocation.".to_owned(),
        ),
        OpenRouterCompletionsError::RateLimited(_) => (
            FailureCategory::RateLimited,
            "The model provider rate-limited the invocation.".to_owned(),
        ),
        OpenRouterCompletionsError::Transport(_)
            if matches!(failure_stage, FailureStage::DuringStream) =>
        {
            (
                FailureCategory::StreamInterrupted,
                "The model response stream was interrupted.".to_owned(),
            )
        }
        OpenRouterCompletionsError::Transport(_) => (
            FailureCategory::Transport,
            "The model provider could not be reached.".to_owned(),
        ),
        OpenRouterCompletionsError::InvalidRequest(_) => (
            FailureCategory::InvalidRequest,
            "The model invocation request was invalid.".to_owned(),
        ),
        OpenRouterCompletionsError::InvalidResponse(_) => (
            FailureCategory::InvalidProviderResponse,
            "The model provider returned an invalid response.".to_owned(),
        ),
        OpenRouterCompletionsError::StreamInterrupted(_) => (
            FailureCategory::StreamInterrupted,
            "The model response stream was interrupted.".to_owned(),
        ),
        OpenRouterCompletionsError::Provider(_) => (
            FailureCategory::ProviderFailure,
            "The model provider failed the invocation.".to_owned(),
        ),
    }
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

fn terminal_model_output(
    provider_events: ProviderOutputStream,
    model_request_id: ConversationEventId,
) -> ModelOutputStream {
    conversation_event_stream(provider_events, model_request_id)
}

fn fail_stream(state: &mut ConversationEventStreamState, error: OpenRouterCompletionsError) {
    let (category, message) = provider_failure(&error, FailureStage::DuringStream);
    let outcome = TerminalModelOutcome::Failed { category, message };
    let response = ModelResponse::new(
        state.model_request_id,
        Vec::new(),
        terminal_model_outcome(outcome),
        None,
    )
    .expect("the failed terminal model response should be valid");
    state.pending_batches = VecDeque::from([ModelDriverOutputBatch::from(
        ModelDriverOutput::ModelResponse(response),
    )]);
    state.terminated = true;
}

fn terminal_model_outcome(outcome: TerminalModelOutcome) -> ModelOutcome {
    match outcome {
        TerminalModelOutcome::Succeeded => ModelOutcome::Succeeded,
        TerminalModelOutcome::Failed { category, message } => ModelOutcome::Failed {
            failure: OperationFailure::try_new(category, message, None)
                .expect("the terminal failure should be valid"),
        },
    }
}

fn translate_model_driver_event(
    driver_event: ModelDriverEvent,
    model_request_id: ConversationEventId,
    state: &mut ConversationEventStreamState,
) -> Result<(), OpenRouterCompletionsError> {
    let output = match driver_event {
        ModelDriverEvent::AssistantResponse { content } => ModelDriverOutput::AssistantResponse(
            AssistantResponse::new(model_request_id, content)
                .map_err(invalid_assistant_response)?,
        ),
        ModelDriverEvent::ModelSpecificEvent {
            event_type,
            message,
        } => ModelDriverOutput::ModelSpecificEvent(
            ModelSpecificEvent::new(
                model_request_id,
                event_type,
                PROVIDER_PAYLOAD_VERSION,
                json!({}),
                message,
            )
            .map_err(invalid_model_specific_event)?,
        ),
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
                .map_err(|error| OpenRouterCompletionsError::InvalidResponse(error.to_string()))?;
            ModelDriverOutput::ToolRequest(
                ToolRequest::try_new(model_request_id, tool_name, arguments, data)
                    .map_err(invalid_tool_request)?,
            )
        }
        ModelDriverEvent::Terminal { outcome, usage } => {
            let response = ModelResponse::new(
                model_request_id,
                Vec::new(),
                terminal_model_outcome(outcome),
                usage,
            )
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

fn invalid_assistant_response(error: InvalidAssistantResponse) -> OpenRouterCompletionsError {
    OpenRouterCompletionsError::InvalidResponse(error.to_string())
}

fn invalid_model_specific_event(error: InvalidModelSpecificEvent) -> OpenRouterCompletionsError {
    OpenRouterCompletionsError::InvalidResponse(error.to_string())
}

fn invalid_tool_request(error: InvalidToolRequest) -> OpenRouterCompletionsError {
    OpenRouterCompletionsError::InvalidResponse(error.to_string())
}

fn invalid_model_response(error: InvalidModelResponse) -> OpenRouterCompletionsError {
    OpenRouterCompletionsError::InvalidResponse(error.to_string())
}

struct ServerSentEvent {
    data: String,
}

#[derive(Default)]
struct ServerSentEventDecoder {
    bytes: Vec<u8>,
    data_lines: Vec<String>,
    events: VecDeque<ServerSentEvent>,
}

impl ServerSentEventDecoder {
    fn push(&mut self, bytes: &[u8]) -> Result<(), OpenRouterCompletionsError> {
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

    fn finish(&mut self) -> Result<(), OpenRouterCompletionsError> {
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

    fn process_line(&mut self, line: Vec<u8>) -> Result<(), OpenRouterCompletionsError> {
        let line = String::from_utf8(line)
            .map_err(|error| OpenRouterCompletionsError::InvalidResponse(error.to_string()))?;
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
        if field == "data" {
            self.data_lines.push(value.to_owned());
        }
        Ok(())
    }

    fn dispatch_event(&mut self) {
        if !self.data_lines.is_empty() {
            self.events.push_back(ServerSentEvent {
                data: self.data_lines.join("\n"),
            });
            self.data_lines.clear();
        }
    }
}

struct AccumulatedToolCall {
    index: usize,
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
}

struct OpenRouterStreamState {
    response_bytes: ResponseByteStream,
    decoder: ServerSentEventDecoder,
    assistant_outputs: String,
    tool_calls: Vec<AccumulatedToolCall>,
    usage: Option<Usage>,
    finish_reason: Option<String>,
    model_outputs: VecDeque<ModelDriverEvent>,
    stream_end: Option<ResponseEnd>,
    terminated: bool,
}

#[derive(Clone, Copy)]
enum ResponseEnd {
    BodyEnded,
    DoneSentinel,
}

fn model_output_stream(response_bytes: ResponseByteStream) -> ProviderOutputStream {
    let state = OpenRouterStreamState {
        response_bytes,
        decoder: ServerSentEventDecoder::default(),
        assistant_outputs: String::new(),
        tool_calls: Vec::new(),
        usage: None,
        finish_reason: None,
        model_outputs: VecDeque::new(),
        stream_end: None,
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
                match process_server_sent_event(server_sent_event, &mut state) {
                    Ok(ProcessEventResult::Outputs(model_outputs)) => {
                        state.model_outputs.extend(model_outputs);
                    }
                    Ok(ProcessEventResult::Done) => {
                        state.decoder.events.clear();
                        state.stream_end = Some(ResponseEnd::DoneSentinel);
                    }
                    Err(error) => {
                        state.terminated = true;
                        return Some((Err(error), state));
                    }
                }
                continue;
            }
            if let Some(stream_end) = state.stream_end {
                state.terminated = true;
                if state.finish_reason.is_some() {
                    return None;
                }
                let error = match stream_end {
                    ResponseEnd::BodyEnded => OpenRouterCompletionsError::StreamInterrupted(
                        "the response body ended before a finish_reason".to_owned(),
                    ),
                    ResponseEnd::DoneSentinel => OpenRouterCompletionsError::InvalidResponse(
                        "the stream ended before a finish_reason".to_owned(),
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
                    state.stream_end = Some(ResponseEnd::BodyEnded);
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

fn process_server_sent_event(
    server_sent_event: ServerSentEvent,
    state: &mut OpenRouterStreamState,
) -> Result<ProcessEventResult, OpenRouterCompletionsError> {
    if server_sent_event.data == "[DONE]" {
        return Ok(ProcessEventResult::Done);
    }

    let payload: Value = serde_json::from_str(&server_sent_event.data)
        .map_err(|error| OpenRouterCompletionsError::InvalidResponse(error.to_string()))?;
    if let Some(error) = payload.get("error") {
        return Err(OpenRouterCompletionsError::Provider(format!(
            "OpenRouter stream emitted an error: {error}"
        )));
    }

    state.usage = parsed_usage(&payload).or(state.usage.take());
    if state.finish_reason.is_some() {
        if payload_has_model_deltas(&payload) {
            return Err(OpenRouterCompletionsError::InvalidResponse(
                "OpenRouter emitted a model delta after the response completed".to_owned(),
            ));
        }
        return Ok(ProcessEventResult::Outputs(Vec::new()));
    }
    let finish_reason = parse_choice_deltas(&payload, state)?;
    let Some(finish_reason) = finish_reason else {
        return Ok(ProcessEventResult::Outputs(Vec::new()));
    };
    let model_events = finalize_response(state, &finish_reason)?;
    state.finish_reason = Some(finish_reason);
    Ok(ProcessEventResult::Outputs(model_events))
}

fn payload_has_model_deltas(payload: &Value) -> bool {
    let Some(choices) = payload.get("choices").and_then(Value::as_array) else {
        return false;
    };
    choices.iter().any(|choice| {
        choice
            .get("delta")
            .and_then(|delta| delta.get("content"))
            .and_then(Value::as_str)
            .is_some_and(|content| !content.is_empty())
            || choice
                .get("delta")
                .and_then(|delta| delta.get("tool_calls"))
                .is_some()
    })
}

fn parse_choice_deltas(
    payload: &Value,
    state: &mut OpenRouterStreamState,
) -> Result<Option<String>, OpenRouterCompletionsError> {
    let Some(choices) = payload.get("choices").and_then(Value::as_array) else {
        return Ok(None);
    };
    let Some(choice) = choices.first() else {
        return Ok(None);
    };
    if let Some(delta) = choice.get("delta") {
        if let Some(content) = delta.get("content") {
            let content = content.as_str().ok_or_else(|| {
                OpenRouterCompletionsError::InvalidResponse(
                    "an OpenRouter content delta was not a string".to_owned(),
                )
            })?;
            if !content.trim().is_empty() {
                state
                    .model_outputs
                    .push_back(ModelDriverEvent::ModelSpecificEvent {
                        event_type: CONTENT_DELTA_EVENT_TYPE.to_owned(),
                        message: Some(content.to_owned()),
                    });
            }
            state.assistant_outputs.push_str(content);
        }
        if let Some(tool_calls) = delta.get("tool_calls") {
            for tool_call in tool_calls
                .as_array()
                .ok_or_else(|| {
                    OpenRouterCompletionsError::InvalidResponse(
                        "an OpenRouter tool_calls delta was not an array".to_owned(),
                    )
                })?
                .iter()
            {
                accumulate_tool_call(tool_call, &mut state.tool_calls)?;
            }
        }
    }
    Ok(choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
        .map(str::to_owned))
}

fn accumulate_tool_call(
    tool_call: &Value,
    tool_calls: &mut Vec<AccumulatedToolCall>,
) -> Result<(), OpenRouterCompletionsError> {
    let index = tool_call
        .get("index")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            OpenRouterCompletionsError::InvalidResponse(
                "an OpenRouter tool_calls delta did not contain an integer index".to_owned(),
            )
        })? as usize;
    let call_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let name = tool_call
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let arguments = tool_call
        .get("function")
        .and_then(|function| function.get("arguments"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default();

    if let Some(existing) = tool_calls.iter_mut().find(|call| call.index == index) {
        if existing.call_id.is_none() {
            existing.call_id = call_id;
        }
        if existing.name.is_none() {
            existing.name = name;
        }
        existing.arguments.push_str(&arguments);
        return Ok(());
    }
    tool_calls.push(AccumulatedToolCall {
        index,
        call_id,
        name,
        arguments,
    });
    Ok(())
}

fn parsed_usage(payload: &Value) -> Option<Usage> {
    let usage = payload.get("usage")?;
    let input_tokens = usage.get("prompt_tokens")?.as_u64()?;
    let output_tokens = usage.get("completion_tokens")?.as_u64()?;
    Some(Usage::new(input_tokens, output_tokens))
}

fn finalize_response(
    state: &mut OpenRouterStreamState,
    finish_reason: &str,
) -> Result<Vec<ModelDriverEvent>, OpenRouterCompletionsError> {
    let mut model_events = Vec::new();
    match finish_reason {
        "stop" => {
            let content = std::mem::take(&mut state.assistant_outputs);
            if !content.trim().is_empty() {
                model_events.push(ModelDriverEvent::AssistantResponse { content });
            }
        }
        "tool_calls" => {
            let content = std::mem::take(&mut state.assistant_outputs);
            if !content.trim().is_empty() {
                model_events.push(ModelDriverEvent::AssistantResponse { content });
            }
            let tool_calls = std::mem::take(&mut state.tool_calls);
            if tool_calls.is_empty() {
                return Err(OpenRouterCompletionsError::InvalidResponse(
                    "the response finished with tool_calls but contained no tool calls".to_owned(),
                ));
            }
            for mut tool_call in tool_calls {
                let name = tool_call.name.take().ok_or_else(|| {
                    OpenRouterCompletionsError::InvalidResponse(
                        "a completed tool call did not contain a name".to_owned(),
                    )
                })?;
                let arguments = parse_function_call_arguments(&tool_call.arguments)?;
                model_events.push(ModelDriverEvent::ToolRequest {
                    tool_name: ToolName::try_new(name).map_err(|error| {
                        OpenRouterCompletionsError::InvalidResponse(error.to_string())
                    })?,
                    arguments,
                    provider_call_id: tool_call.call_id,
                });
            }
        }
        "length" => {
            return Ok(vec![ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Failed {
                    category: FailureCategory::ProviderFailure,
                    message: "The model response was cut off before completing.".to_owned(),
                },
                usage: None,
            }]);
        }
        "content_filter" => {
            return Ok(vec![ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Failed {
                    category: FailureCategory::Refusal,
                    message: "The model response was filtered by the provider.".to_owned(),
                },
                usage: None,
            }]);
        }
        other => {
            return Err(OpenRouterCompletionsError::InvalidResponse(format!(
                "OpenRouter stream finished with the unsupported finish_reason {other}"
            )));
        }
    }

    let has_model_output = model_events.iter().any(|event| {
        matches!(
            event,
            ModelDriverEvent::AssistantResponse { .. } | ModelDriverEvent::ToolRequest { .. }
        )
    });
    if !has_model_output {
        return Err(OpenRouterCompletionsError::InvalidResponse(
            "the completed OpenRouter response contained no model message".to_owned(),
        ));
    }
    model_events.push(ModelDriverEvent::Terminal {
        outcome: TerminalModelOutcome::Succeeded,
        usage: state.usage.take(),
    });
    Ok(model_events)
}

fn parse_function_call_arguments(arguments: &str) -> Result<Value, OpenRouterCompletionsError> {
    if arguments.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(arguments).map_err(|error| {
        OpenRouterCompletionsError::InvalidResponse(format!(
            "a tool call contained invalid JSON arguments: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::str::FromStr;
    use std::thread;

    use futures_util::{StreamExt, stream};
    use reqwest::StatusCode;
    use serde_json::{Map, Value, json};

    use tog::conversation::{
        AssistantResponse, Conversation, ConversationEvent, ConversationEventId,
        ConversationEventPayload, ConversationHistory, ConversationId, FailureCategory, ModelData,
        ModelId, ModelOutcome, ModelRequest, ModelSource, ProviderId, ToolOutcome, ToolRequest,
        ToolResponse, TurnStart, User, UserContent,
    };
    use tog::conversation::{ToolDefinition, ToolName};
    use tog::conversation_event_store::ConversationEventRecord;
    use tog::model_driver::{ModelDriver, ModelDriverOutput, ModelOutputStream, TurnInput};

    use super::{
        ModelDriverEvent, OpenRouterCompletionsError, OpenRouterCompletionsModelDriver,
        ResponseByteStream, TerminalModelOutcome, chat_messages, classify_response_failure,
        model_output_stream, provider_tool,
    };

    fn conversation_event(
        conversation_id: ConversationId,
        position: u64,
        content: ConversationEventPayload,
    ) -> ConversationEventRecord {
        ConversationEventRecord::new(position, ConversationEvent::new(conversation_id, content))
    }

    fn user_event(
        conversation_id: ConversationId,
        position: u64,
        text: &str,
    ) -> ConversationEventRecord {
        conversation_event(
            conversation_id,
            position,
            ConversationEventPayload::User(
                User::new(vec![UserContent::Text(text.to_owned())])
                    .expect("the user event should be valid"),
            ),
        )
    }

    fn turn_and_request(
        conversation_id: ConversationId,
    ) -> (ConversationEventRecord, ConversationEventRecord) {
        let turn_start = conversation_event(
            conversation_id,
            1,
            ConversationEventPayload::TurnStart(TurnStart::new(None, 0)),
        );
        let model_request = conversation_event(
            conversation_id,
            2,
            ConversationEventPayload::ModelRequest(
                ModelRequest::new(turn_start.event().id(), source(), 1, Vec::new(), None, None)
                    .expect("the model request should be valid"),
            ),
        );
        (turn_start, model_request)
    }

    fn source() -> ModelSource {
        ModelSource::new(
            ProviderId::from_str("openrouter").expect("the provider should be valid"),
            ModelId::from_str("openrouter/auto").expect("the model should be valid"),
        )
    }

    #[test]
    fn chat_messages_projects_canonical_events_and_ignores_auxiliary_records() {
        let conversation_id = ConversationId::new();
        let (turn_start, model_request) = turn_and_request(conversation_id);
        let request_id = model_request.event().id();
        let assistant = AssistantResponse::new(request_id, "Hello.".to_owned())
            .expect("the assistant response should be valid");
        let conversation = ConversationHistory::from_events(vec![
            user_event(conversation_id, 0, "Hello"),
            turn_start,
            model_request,
            conversation_event(
                conversation_id,
                3,
                ConversationEventPayload::AssistantResponse(assistant),
            ),
        ])
        .expect("the conversation should be valid");

        assert_eq!(
            chat_messages(conversation.events()),
            json!([
                { "role": "user", "content": "Hello" },
                { "role": "assistant", "content": "Hello." }
            ])
        );
    }

    fn tool_definition(name: &str) -> ToolDefinition {
        ToolDefinition::try_new(
            ToolName::try_new(name.to_owned()).expect("the tool name should be valid"),
            "Run a command.".to_owned(),
            schemars::json_schema!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": { "command": { "type": "string" } }
            }),
            schemars::json_schema!({ "type": "object" }),
        )
        .expect("the tool definition should be valid")
    }

    #[test]
    fn provider_tools_translate_definitions_into_chat_function_format() {
        let definition = tool_definition("shell");

        let provider_tool = provider_tool(&definition);

        assert_eq!(provider_tool["type"], "function");
        assert_eq!(provider_tool["function"]["name"], "shell");
        assert_eq!(provider_tool["function"]["description"], "Run a command.");
        assert_eq!(
            provider_tool["function"]["parameters"],
            json!({
                "type": "object",
                "properties": { "command": { "type": "string" } }
            })
        );
        assert!(
            provider_tool["function"]["parameters"]
                .get("$schema")
                .is_none(),
            "the provider parameter schema must drop record schema metadata"
        );
        assert!(
            definition.parameters().as_value().get("$schema").is_some(),
            "the recorded definition must keep its schema metadata"
        );
    }

    #[test]
    fn chat_messages_reconstructs_tool_requests_and_responses() {
        let conversation_id = ConversationId::new();
        let (turn_start, model_request) = turn_and_request(conversation_id);
        let request_id = model_request.event().id();
        let model_data = ModelData::new(Map::from_iter([(
            "call_id".to_owned(),
            Value::String("call_native".to_owned()),
        )]))
        .expect("the model data should be valid");
        let request = ToolRequest::try_new(
            request_id,
            ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
            json!({ "command": "pwd" }),
            Some(model_data),
        )
        .expect("the tool request should be valid");
        let tool_request_record = conversation_event(
            conversation_id,
            3,
            ConversationEventPayload::ToolRequest(request),
        );
        let tool_request_id = tool_request_record.event().id();
        let response_record = conversation_event(
            conversation_id,
            4,
            ConversationEventPayload::ToolResponse(ToolResponse::new(
                tool_request_id,
                ToolOutcome::succeeded(json!({ "stdout": "/tmp\n" })),
            )),
        );
        let conversation = ConversationHistory::from_events(vec![
            user_event(conversation_id, 0, "Run pwd"),
            turn_start,
            model_request,
            tool_request_record,
            response_record,
        ])
        .expect("the conversation should be valid");

        assert_eq!(
            chat_messages(conversation.events()),
            json!([
                { "role": "user", "content": "Run pwd" },
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_native",
                        "type": "function",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"command\":\"pwd\"}"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_native",
                    "content": "{\"type\":\"succeeded\",\"value\":{\"stdout\":\"/tmp\\n\"}}"
                }
            ])
        );
    }

    fn response_byte_stream(chunks: Vec<Vec<u8>>) -> ResponseByteStream {
        stream::iter(
            chunks
                .into_iter()
                .map(Ok::<Vec<u8>, OpenRouterCompletionsError>),
        )
        .boxed()
    }

    async fn collect_driver_events(
        input: &str,
    ) -> Vec<Result<ModelDriverEvent, OpenRouterCompletionsError>> {
        model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]))
            .collect()
            .await
    }

    #[tokio::test]
    async fn content_deltas_emit_partial_events_then_one_assistant_response_and_a_terminal() {
        let input = concat!(
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"Hello \"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"world\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":4}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_driver_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the response stream should parse");

        assert_eq!(events.len(), 4, "partial events, answer, then terminal");
        assert!(matches!(
            &events[0],
            ModelDriverEvent::ModelSpecificEvent {
                event_type,
                message: Some(message),
            } if event_type == "content_delta" && message == "Hello "
        ));
        assert!(matches!(
            &events[1],
            ModelDriverEvent::ModelSpecificEvent {
                event_type,
                message: Some(message),
            } if event_type == "content_delta" && message == "world"
        ));
        assert!(matches!(
            &events[2],
            ModelDriverEvent::AssistantResponse { content } if content == "Hello world"
        ));
        assert!(matches!(
            &events[3],
            ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Succeeded,
                usage: Some(_),
            }
        ));
    }

    #[tokio::test]
    async fn multiple_stream_chunks_in_one_byte_batch_parse() {
        let input = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_driver_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the response stream should parse");

        assert_eq!(events.len(), 3, "partial event, answer, then terminal");
        assert!(matches!(
            &events[0],
            ModelDriverEvent::ModelSpecificEvent {
                message: Some(message),
                ..
            } if message == "Hello"
        ));
        assert!(matches!(
            &events[1],
            ModelDriverEvent::AssistantResponse { content } if content == "Hello"
        ));
        assert!(matches!(
            &events[2],
            ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Succeeded,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn streamed_tool_calls_become_a_portable_tool_request() {
        let input = concat!(
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"shell\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_driver_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the tool call stream should parse");

        assert_eq!(events.len(), 2);
        let ModelDriverEvent::ToolRequest {
            tool_name,
            arguments,
            provider_call_id,
        } = &events[0]
        else {
            panic!("the output should be a tool request");
        };
        assert_eq!(tool_name.as_str(), "shell");
        assert_eq!(arguments, &json!({ "command": "pwd" }));
        assert_eq!(provider_call_id.as_deref(), Some("call_1"));
        assert!(matches!(
            &events[1],
            ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Succeeded,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn parallel_tool_calls_are_accumulated_by_index() {
        let input = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}},{\"index\":1,\"id\":\"call_2\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"ls\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_driver_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the tool call stream should parse");

        assert_eq!(events.len(), 3);
        let ModelDriverEvent::ToolRequest {
            arguments: first,
            provider_call_id: first_call_id,
            ..
        } = &events[0]
        else {
            panic!("the first output should be a tool request");
        };
        let ModelDriverEvent::ToolRequest {
            arguments: second,
            provider_call_id: second_call_id,
            ..
        } = &events[1]
        else {
            panic!("the second output should be a tool request");
        };
        assert_eq!(first, &json!({ "command": "pwd" }));
        assert_eq!(first_call_id.as_deref(), Some("call_1"));
        assert_eq!(second, &json!({ "command": "ls" }));
        assert_eq!(second_call_id.as_deref(), Some("call_2"));
    }

    #[tokio::test]
    async fn a_tool_calls_finish_without_tool_calls_is_a_stream_error() {
        let input = concat!(
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n"
        );

        let results = collect_driver_events(input).await;

        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0],
            Err(OpenRouterCompletionsError::InvalidResponse(_))
        ));
    }

    #[tokio::test]
    async fn malformed_json_is_a_stream_error() {
        let mut events =
            model_output_stream(response_byte_stream(vec![b"data: not-json\n\n".to_vec()]));

        assert!(matches!(
            events.next().await,
            Some(Err(OpenRouterCompletionsError::InvalidResponse(_)))
        ));
    }

    #[tokio::test]
    async fn premature_body_end_before_finish_is_a_stream_interruption() {
        let input =
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n";
        let mut events = model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        assert!(matches!(
            events.next().await,
            Some(Ok(ModelDriverEvent::ModelSpecificEvent { message: Some(message), .. }))
                if message == "Hello"
        ));
        assert!(matches!(
            events.next().await,
            Some(Err(OpenRouterCompletionsError::StreamInterrupted(_)))
        ));
    }

    #[tokio::test]
    async fn a_done_sentinel_before_finish_is_an_invalid_response() {
        let input = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n"
        );
        let mut events = model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        assert!(matches!(
            events.next().await,
            Some(Ok(ModelDriverEvent::ModelSpecificEvent { message: Some(message), .. }))
                if message == "Hello"
        ));
        assert!(matches!(
            events.next().await,
            Some(Err(OpenRouterCompletionsError::InvalidResponse(_)))
        ));
    }

    #[tokio::test]
    async fn content_after_completion_is_a_stream_error() {
        let input = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"again\"},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n"
        );

        let results = collect_driver_events(input).await;

        assert_eq!(
            results.len(),
            4,
            "partial event, answer, terminal, then the late error"
        );
        assert!(results[0].is_ok());
        assert!(matches!(
            results[1],
            Ok(ModelDriverEvent::AssistantResponse { .. })
        ));
        assert!(matches!(results[2], Ok(ModelDriverEvent::Terminal { .. })));
        assert!(matches!(
            results[3],
            Err(OpenRouterCompletionsError::InvalidResponse(_))
        ));
    }

    #[tokio::test]
    async fn a_length_finish_is_a_failed_terminal() {
        let input = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":\"length\"}]}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_driver_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the length finish should parse");

        assert_eq!(events.len(), 2, "partial event then the failed terminal");
        assert!(matches!(
            &events[0],
            ModelDriverEvent::ModelSpecificEvent {
                event_type,
                message: Some(message),
            } if event_type == "content_delta" && message == "Hello"
        ));
        assert!(matches!(
            &events[1],
            ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Failed {
                    category: FailureCategory::ProviderFailure,
                    ..
                },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_content_filter_finish_is_a_refusal_terminal() {
        let input = concat!(
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"content_filter\"}]}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_driver_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the content filter should parse");

        assert!(matches!(
            &events[0],
            ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Failed {
                    category: FailureCategory::Refusal,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn response_statuses_map_to_typed_driver_errors() {
        assert!(matches!(
            classify_response_failure(StatusCode::UNAUTHORIZED, "unauthorized".to_owned()),
            Err(OpenRouterCompletionsError::Authentication(_))
        ));
        assert!(matches!(
            classify_response_failure(StatusCode::TOO_MANY_REQUESTS, "slow down".to_owned()),
            Err(OpenRouterCompletionsError::RateLimited(_))
        ));
        assert!(matches!(
            classify_response_failure(StatusCode::BAD_REQUEST, "bad request".to_owned()),
            Err(OpenRouterCompletionsError::InvalidRequest(_))
        ));
        assert!(matches!(
            classify_response_failure(StatusCode::INTERNAL_SERVER_ERROR, "failed".to_owned()),
            Err(OpenRouterCompletionsError::Provider(_))
        ));
    }

    #[test]
    fn a_context_limit_http_response_is_a_semantic_context_limit() {
        let result = classify_response_failure(
            StatusCode::BAD_REQUEST,
            json!({ "error": { "message": "maximum context length exceeded" } }).to_string(),
        );
        assert!(result.is_ok());
    }

    fn test_conversation(text: &str) -> ConversationHistory {
        let conversation_id = ConversationId::new();
        ConversationHistory::from_events(vec![user_event(conversation_id, 0, text)])
            .expect("the conversation should be valid")
    }

    fn driver_request(conversation: &dyn Conversation) -> TurnInput<'_> {
        let model_request_id = ConversationEventId::new();
        TurnInput::new(conversation, model_request_id, 0)
    }

    fn read_request(connection: &TcpStream) -> Value {
        let mut reader = BufReader::new(
            connection
                .try_clone()
                .expect("the request connection should clone"),
        );
        let mut content_length = None;
        loop {
            let mut header = String::new();
            reader
                .read_line(&mut header)
                .expect("the request header should read");
            if header == "\r\n" {
                break;
            }
            if let Some(length) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                content_length = Some(
                    length
                        .trim()
                        .parse::<usize>()
                        .expect("the content length should be numeric"),
                );
            }
        }
        let mut body = vec![0; content_length.expect("the request should have a body")];
        reader
            .read_exact(&mut body)
            .expect("the request body should read");
        serde_json::from_slice(&body).expect("the request body should be JSON")
    }

    fn mock_driver(address: std::net::SocketAddr) -> OpenRouterCompletionsModelDriver {
        OpenRouterCompletionsModelDriver {
            http_client: reqwest::Client::new(),
            api_key: "test-key".to_owned(),
            chat_completions_url: format!("http://{address}/chat/completions"),
            source: source(),
        }
    }

    async fn expect_single(stream: &mut ModelOutputStream) -> ModelDriverOutput {
        let batch = stream
            .next()
            .await
            .expect("the stream should yield a batch")
            .expect("the batch should be valid");
        let mut outputs = batch.into_outputs();
        assert_eq!(outputs.len(), 1, "the batch should hold one output");
        outputs.remove(0)
    }

    #[tokio::test]
    async fn invoke_emits_outputs_and_a_terminal_model_response() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server should bind");
        let address = listener
            .local_addr()
            .expect("the mock server address should be available");
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("the mock server should accept");
            let request = read_request(&connection);
            assert_eq!(request["model"], "openrouter/auto");
            assert_eq!(request["stream"], true);
            assert_eq!(request["messages"][0]["content"], "Hello");
            let response_body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Answer\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            write!(
                connection,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            )
            .expect("the mock response should write");
        });
        let driver = mock_driver(address);

        let conversation = test_conversation("Hello");
        let mut model_events = driver
            .invoke(driver_request(&conversation))
            .await
            .expect("the invocation should establish its stream");
        let first = expect_single(&mut model_events).await;
        assert!(matches!(first, ModelDriverOutput::ModelSpecificEvent(_)));
        let second = expect_single(&mut model_events).await;
        assert!(matches!(second, ModelDriverOutput::AssistantResponse(_)));
        let terminal = expect_single(&mut model_events).await;
        assert!(matches!(
            terminal,
            ModelDriverOutput::ModelResponse(response)
                if matches!(response.outcome(), ModelOutcome::Succeeded)
        ));
        assert!(model_events.next().await.is_none());
        server.join().expect("the mock server should stop");
    }

    #[tokio::test]
    async fn invoke_emits_a_tool_request_from_streamed_tool_calls() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server should bind");
        let address = listener
            .local_addr()
            .expect("the mock server address should be available");
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("the mock server should accept");
            read_request(&connection);
            let tool_call_body = concat!(
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            write!(
                connection,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{tool_call_body}",
                tool_call_body.len()
            )
            .expect("the tool call response should write");
        });
        let driver = mock_driver(address);

        let conversation = test_conversation("Run pwd");
        let mut model_events = driver
            .invoke(driver_request(&conversation))
            .await
            .expect("the invocation should establish its stream");
        let tool_request = expect_single(&mut model_events).await;
        assert!(matches!(
            tool_request,
            ModelDriverOutput::ToolRequest(request)
                if request.tool_name().as_str() == "shell"
        ));
        let terminal = expect_single(&mut model_events).await;
        assert!(matches!(
            terminal,
            ModelDriverOutput::ModelResponse(response)
                if matches!(response.outcome(), ModelOutcome::Succeeded)
        ));
        assert!(model_events.next().await.is_none());
        server.join().expect("the mock server should stop");
    }

    #[tokio::test]
    async fn an_early_http_failure_produces_a_failed_terminal() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server should bind");
        let address = listener
            .local_addr()
            .expect("the mock server address should be available");
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("the mock server should accept");
            read_request(&connection);
            let body = "unauthorized";
            write!(
                connection,
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("the mock response should write");
        });
        let driver = mock_driver(address);

        let conversation = test_conversation("Hello");
        let mut model_events = driver
            .invoke(driver_request(&conversation))
            .await
            .expect("the invocation should establish a stream");
        let batch = model_events
            .next()
            .await
            .expect("the stream should yield a failure batch")
            .expect("the failure batch should be valid")
            .into_outputs();
        assert_eq!(batch.len(), 1);
        assert!(matches!(
            &batch[0],
            ModelDriverOutput::ModelResponse(response)
                if matches!(
                    response.outcome(),
                    ModelOutcome::Failed { failure }
                        if failure.category() == FailureCategory::Authentication
                )
        ));
        assert!(model_events.next().await.is_none());
        server.join().expect("the mock server should stop");
    }

    #[tokio::test]
    async fn a_context_limit_http_response_becomes_a_failed_terminal() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server should bind");
        let address = listener
            .local_addr()
            .expect("the mock server address should be available");
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("the mock server should accept");
            read_request(&connection);
            let body =
                json!({ "error": { "message": "maximum context length exceeded" } }).to_string();
            write!(
                connection,
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("the mock response should write");
        });
        let driver = mock_driver(address);

        let conversation = test_conversation("Hello");
        let mut model_events = driver
            .invoke(driver_request(&conversation))
            .await
            .expect("the context-limit outcome should establish a semantic stream");
        let batch = model_events
            .next()
            .await
            .expect("the stream should yield a terminal batch")
            .expect("the terminal batch should be valid")
            .into_outputs();
        assert_eq!(batch.len(), 1);
        assert!(matches!(
            &batch[0],
            ModelDriverOutput::ModelResponse(response)
                if matches!(
                    response.outcome(),
                    ModelOutcome::Failed { failure }
                        if failure.category() == FailureCategory::ContextLimitExceeded
                            && failure.message() == "The model context limit was exceeded."
                )
        ));
        assert!(model_events.next().await.is_none());
        server.join().expect("the mock server should stop");
    }
}
