use std::collections::VecDeque;
use std::str::FromStr;

use futures_util::future::BoxFuture;
use futures_util::stream::{self, BoxStream};
use futures_util::{FutureExt, StreamExt};
use reqwest::Client;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::conversation::{
    AssistantResponse, Conversation, ConversationEventKind, ConversationProblem,
    ConversationRecordKind, ConversationTurnId, DriverEventEnvelope, InvalidAssistantResponse,
    InvalidConversationProblem, InvalidModelCommunication, InvalidModelData, ModelCommunication,
    ModelData, ModelDetails, ModelEvent, ModelEventImportance, ModelId, ModelInvocationId,
    ModelIssue, ModelSource, ProviderId, TurnOutcome, UserContent,
};
use crate::model_driver::{
    DriverEvent, DriverEventDecodeError, DriverEventDecoder, ModelDriver, ModelDriverError,
    ModelDriverOutput, ModelOutputStream,
};

type ResponseByteStream = BoxStream<'static, Result<Vec<u8>, ModelDriverError>>;
type ProviderOutputStream = BoxStream<'static, Result<ModelDriverEvent, ModelDriverError>>;

const OPEN_AI_DRIVER_VERSION: &str = "1";

enum ModelDriverEvent {
    Model {
        event: ModelEvent,
        data: Option<ModelData>,
    },
    Problem {
        problem: ModelIssue,
        data: Option<ModelData>,
    },
}

pub(crate) struct OpenAiModelDriver {
    http_client: Client,
    api_key: String,
    responses_url: String,
    source: ModelSource,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OpenAiInvocationRequested {
    invocation_id: ModelInvocationId,
    turn_id: ConversationTurnId,
    model: ModelSource,
}

impl DriverEvent for OpenAiInvocationRequested {
    fn to_envelope(&self) -> Result<DriverEventEnvelope, ModelDriverError> {
        let payload = serde_json::to_value(self)
            .map_err(|error| ModelDriverError::InvalidResponse(error.to_string()))?;
        DriverEventEnvelope::new(
            "openai".to_owned(),
            OPEN_AI_DRIVER_VERSION.to_owned(),
            "model_invocation_requested".to_owned(),
            1,
            "OpenAI model invocation was requested.".to_owned(),
            payload,
        )
        .map_err(|error| ModelDriverError::InvalidResponse(error.to_string()))
    }
}

impl OpenAiModelDriver {
    pub(crate) fn from_environment(model: ModelId) -> Result<Self, ModelDriverError> {
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
            ModelDriverError::Authentication("OPENAI_API_KEY must be set".to_owned())
        })?;
        let base_url = std::env::var("TOG_OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_owned());
        Ok(Self {
            http_client: Client::new(),
            api_key,
            responses_url: format!("{}/responses", base_url.trim_end_matches('/')),
            source: ModelSource::new(
                ProviderId::from_str("openai")
                    .expect("the OpenAI provider identifier should be valid"),
                model,
            ),
        })
    }
}

impl ModelDriver for OpenAiModelDriver {
    fn source(&self) -> &ModelSource {
        &self.source
    }

    fn invoke<'invoke>(
        &'invoke self,
        conversation: &'invoke Conversation,
        turn_id: ConversationTurnId,
    ) -> BoxFuture<'invoke, Result<ModelOutputStream, ModelDriverError>> {
        let invocation_id = ModelInvocationId::new();
        let invocation_event = OpenAiInvocationRequested {
            invocation_id,
            turn_id,
            model: self.source.clone(),
        };
        let mut request_body = Map::new();
        request_body.insert(
            "model".to_owned(),
            Value::String(self.source.model().as_str().to_owned()),
        );
        request_body.insert("input".to_owned(), semantic_input(conversation));
        request_body.insert("reasoning".to_owned(), json!({ "summary": "auto" }));
        request_body.insert("stream".to_owned(), Value::Bool(true));
        request_body.insert("store".to_owned(), Value::Bool(true));

        let request = self
            .http_client
            .post(&self.responses_url)
            .bearer_auth(&self.api_key)
            .json(&request_body)
            .build();
        let http_client = self.http_client.clone();
        let source = self.source.clone();

        async move {
            let request = match request {
                Ok(request) => request,
                Err(error) => {
                    let error = ModelDriverError::InvalidRequest(error.to_string());
                    return Ok(invocation_error_stream(invocation_event, error));
                }
            };
            let response = match http_client.execute(request).await {
                Ok(response) => response,
                Err(error) => {
                    return Ok(invocation_error_stream(
                        invocation_event,
                        ModelDriverError::Transport(error.to_string()),
                    ));
                }
            };
            let response_status = response.status();
            if !response_status.is_success() {
                let response_body = response
                    .text()
                    .await
                    .map_err(|error| ModelDriverError::Transport(error.to_string()))?;
                return match classify_response_failure(response_status, response_body) {
                    Ok(issue) => Ok(conversation_event_stream(
                        model_issue_stream(issue),
                        turn_id,
                        invocation_id,
                        source,
                        Box::new(invocation_event),
                    )),
                    Err(error) => Ok(invocation_error_stream(invocation_event, error)),
                };
            }

            let response_bytes = response
                .bytes_stream()
                .map(|result| {
                    result
                        .map(|bytes| bytes.to_vec())
                        .map_err(|error| ModelDriverError::Transport(error.to_string()))
                })
                .boxed();
            Ok(conversation_event_stream(
                model_output_stream(response_bytes),
                turn_id,
                invocation_id,
                source,
                Box::new(invocation_event),
            ))
        }
        .boxed()
    }
}

impl DriverEventDecoder for OpenAiModelDriver {
    fn decode_event(
        &self,
        envelope: &DriverEventEnvelope,
    ) -> Result<Box<dyn DriverEvent>, DriverEventDecodeError> {
        if envelope.driver() != "openai" || envelope.driver_version() != OPEN_AI_DRIVER_VERSION {
            return Err(DriverEventDecodeError::UnsupportedDriver);
        }
        if envelope.event_type() != "model_invocation_requested"
            || envelope.event_schema_version() != 1
        {
            return Err(DriverEventDecodeError::UnsupportedEvent);
        }
        serde_json::from_value::<OpenAiInvocationRequested>(envelope.payload().clone())
            .map(|event| Box::new(event) as Box<dyn DriverEvent>)
            .map_err(|error| DriverEventDecodeError::InvalidPayload(error.to_string()))
    }
}

fn semantic_input(conversation: &Conversation) -> Value {
    Value::Array(
        conversation
            .events()
            .iter()
            .filter_map(|conversation_event| match &conversation_event.kind {
                ConversationRecordKind::Event(ConversationEventKind::User { content, .. }) => {
                    let text = content
                        .iter()
                        .map(|content| match content {
                            UserContent::Text(text) => text.as_str(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some(json!({ "role": "user", "content": text }))
                }
                ConversationRecordKind::Event(ConversationEventKind::Assistant {
                    response,
                    ..
                }) => Some(json!({ "role": "assistant", "content": response.message() })),
                ConversationRecordKind::Event(
                    ConversationEventKind::UserMessageRequested { .. }
                    | ConversationEventKind::TurnRequested { .. }
                    | ConversationEventKind::Communication { .. }
                    | ConversationEventKind::Problem { .. }
                    | ConversationEventKind::TurnCompleted { .. },
                )
                | ConversationRecordKind::Driver(_) => None,
            })
            .collect(),
    )
}

fn classify_response_failure(
    status: StatusCode,
    body: String,
) -> Result<ModelIssue, ModelDriverError> {
    if status == StatusCode::BAD_REQUEST && is_context_limit_error(&body) {
        return ModelIssue::try_context_limit_exceeded(
            "The model context limit was exceeded.".to_owned(),
        )
        .map_err(invalid_conversation_problem);
    }

    Err(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ModelDriverError::Authentication(body),
        StatusCode::TOO_MANY_REQUESTS => ModelDriverError::RateLimited(body),
        StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => {
            ModelDriverError::InvalidRequest(body)
        }
        _ => ModelDriverError::Provider(format!("OpenAI Responses returned {status}: {body}")),
    })
}

fn is_context_limit_error(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .is_some_and(|payload| is_context_limit_payload(&payload))
}

fn is_context_limit_payload(payload: &Value) -> bool {
    ["/code", "/error/code", "/response/error/code"]
        .into_iter()
        .filter_map(|pointer| payload.pointer(pointer).and_then(Value::as_str))
        .any(|code| matches!(code, "context_length_exceeded" | "context_window_exceeded"))
}

fn model_issue_stream(issue: ModelIssue) -> ProviderOutputStream {
    stream::once(async move {
        Ok(ModelDriverEvent::Problem {
            problem: issue,
            data: None,
        })
    })
    .boxed()
}

struct ConversationEventStreamState {
    provider_events: ProviderOutputStream,
    turn_id: ConversationTurnId,
    invocation_id: ModelInvocationId,
    source: ModelSource,
    invocation_event: Option<Box<dyn DriverEvent>>,
    turn_failed: bool,
    terminated: bool,
}

fn invocation_error_stream(
    invocation_event: OpenAiInvocationRequested,
    error: ModelDriverError,
) -> ModelOutputStream {
    stream::iter([
        Ok(ModelDriverOutput::Driver(
            Box::new(invocation_event) as Box<dyn DriverEvent>
        )),
        Err(error),
    ])
    .boxed()
}

fn conversation_event_stream(
    provider_events: ProviderOutputStream,
    turn_id: ConversationTurnId,
    invocation_id: ModelInvocationId,
    source: ModelSource,
    invocation_event: Box<dyn DriverEvent>,
) -> ModelOutputStream {
    stream::unfold(
        ConversationEventStreamState {
            provider_events,
            turn_id,
            invocation_id,
            source,
            invocation_event: Some(invocation_event),
            turn_failed: false,
            terminated: false,
        },
        |mut state| async move {
            if state.terminated {
                return None;
            }
            if let Some(invocation_event) = state.invocation_event.take() {
                return Some((Ok(ModelDriverOutput::Driver(invocation_event)), state));
            }
            match state.provider_events.next().await {
                Some(Ok(driver_event)) => {
                    let driver_output = translate_model_driver_event(
                        driver_event,
                        state.turn_id,
                        state.invocation_id,
                        &state.source,
                    );
                    let driver_output = match driver_output {
                        Ok(driver_output) => driver_output,
                        Err(error) => {
                            state.terminated = true;
                            return Some((Err(error), state));
                        }
                    };
                    if matches!(
                        &driver_output,
                        ModelDriverOutput::Event(ConversationEventKind::Problem { .. })
                    ) {
                        state.turn_failed = true;
                    }
                    Some((Ok(driver_output), state))
                }
                Some(Err(error)) => {
                    state.terminated = true;
                    Some((Err(error), state))
                }
                None => {
                    state.terminated = true;
                    Some((
                        Ok(ModelDriverOutput::Event(
                            ConversationEventKind::TurnCompleted {
                                turn_id: state.turn_id,
                                outcome: if state.turn_failed {
                                    TurnOutcome::Failed
                                } else {
                                    TurnOutcome::Succeeded
                                },
                            },
                        )),
                        state,
                    ))
                }
            }
        },
    )
    .boxed()
}

fn translate_model_driver_event(
    driver_event: ModelDriverEvent,
    turn_id: ConversationTurnId,
    invocation_id: ModelInvocationId,
    source: &ModelSource,
) -> Result<ModelDriverOutput, ModelDriverError> {
    let kind = match driver_event {
        ModelDriverEvent::Model { event, data } => match event {
            ModelEvent::Assistant(response) => ConversationEventKind::Assistant {
                turn_id,
                invocation_id,
                model: ModelDetails::new(source.clone(), data).map_err(invalid_model_data)?,
                response,
            },
            ModelEvent::Communication(communication) => ConversationEventKind::Communication {
                turn_id,
                invocation_id,
                model: ModelDetails::new(source.clone(), data).map_err(invalid_model_data)?,
                communication,
            },
        },
        ModelDriverEvent::Problem { problem, data } => ConversationEventKind::Problem {
            turn_id: Some(turn_id),
            invocation_id: Some(invocation_id),
            model: Some(ModelDetails::new(source.clone(), data).map_err(invalid_model_data)?),
            problem: ConversationProblem::Issue(problem),
        },
    };
    Ok(ModelDriverOutput::Event(kind))
}

#[derive(Default)]
struct ResponseState {
    assistant_outputs: Vec<AccumulatedText>,
    refusal_outputs: Vec<AccumulatedText>,
    reasoning_outputs: Vec<AccumulatedText>,
    reasoning_summaries: Vec<AccumulatedText>,
    completed: bool,
}

struct AccumulatedText {
    key: String,
    streamed_text: String,
    completed_text: Option<String>,
    emitted: bool,
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
    fn push(&mut self, bytes: &[u8]) -> Result<(), ModelDriverError> {
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

    fn finish(&mut self) -> Result<(), ModelDriverError> {
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

    fn process_line(&mut self, line: Vec<u8>) -> Result<(), ModelDriverError> {
        let line = String::from_utf8(line)
            .map_err(|error| ModelDriverError::InvalidResponse(error.to_string()))?;
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

struct OpenAiStreamState {
    response_bytes: ResponseByteStream,
    decoder: ServerSentEventDecoder,
    response: ResponseState,
    model_outputs: VecDeque<ModelDriverEvent>,
    response_end: Option<ResponseEnd>,
    terminated: bool,
}

#[derive(Clone, Copy)]
enum ResponseEnd {
    BodyEnded,
    DoneSentinel,
}

fn model_output_stream(response_bytes: ResponseByteStream) -> ProviderOutputStream {
    let state = OpenAiStreamState {
        response_bytes,
        decoder: ServerSentEventDecoder::default(),
        response: ResponseState::default(),
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
                match process_event(server_sent_event, &mut state.response) {
                    Ok(ProcessEventResult::Outputs(model_outputs)) => {
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
                if state.response.completed {
                    return None;
                }
                let error = match response_end {
                    ResponseEnd::BodyEnded => ModelDriverError::StreamInterrupted(
                        "the response body ended before response.completed".to_owned(),
                    ),
                    ResponseEnd::DoneSentinel => ModelDriverError::InvalidResponse(
                        "response.completed was not received before [DONE]".to_owned(),
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

fn process_event(
    server_sent_event: ServerSentEvent,
    response_state: &mut ResponseState,
) -> Result<ProcessEventResult, ModelDriverError> {
    if server_sent_event.data == "[DONE]" {
        return Ok(ProcessEventResult::Done);
    }

    let payload: Value = serde_json::from_str(&server_sent_event.data)
        .map_err(|error| ModelDriverError::InvalidResponse(error.to_string()))?;
    let payload_event_type = match payload.get("type") {
        Some(Value::String(event_type)) => Some(event_type.clone()),
        Some(_) => {
            return Err(ModelDriverError::InvalidResponse(
                "an OpenAI stream event contained a non-string type".to_owned(),
            ));
        }
        None => None,
    };
    if let (Some(payload_event_type), Some(server_sent_event_name)) =
        (&payload_event_type, &server_sent_event.name)
        && payload_event_type != server_sent_event_name
    {
        return Err(ModelDriverError::InvalidResponse(format!(
            "OpenAI stream event type {server_sent_event_name} did not match payload type {payload_event_type}"
        )));
    }
    let event_type = payload_event_type
        .or(server_sent_event.name)
        .unwrap_or_else(|| "unknown".to_owned());

    if response_state.completed
        && !matches!(
            event_type.as_str(),
            "error" | "response.failed" | "response.completed"
        )
    {
        return Err(ModelDriverError::InvalidResponse(format!(
            "OpenAI stream emitted {event_type} after response.completed"
        )));
    }

    let model_events = match event_type.as_str() {
        "response.output_text.delta" => {
            let key = semantic_output_key(&payload, &["output_index", "content_index"])?;
            append_delta(
                &payload,
                accumulated_text(&mut response_state.assistant_outputs, key)?,
            )?;
            Vec::new()
        }
        "response.refusal.delta" => {
            let key = semantic_output_key(&payload, &["output_index", "content_index"])?;
            append_delta(
                &payload,
                accumulated_text(&mut response_state.refusal_outputs, key)?,
            )?;
            Vec::new()
        }
        "response.output_text.done" => {
            let key = semantic_output_key(&payload, &["output_index", "content_index"])?;
            complete_text(
                &payload,
                "text",
                accumulated_text(&mut response_state.assistant_outputs, key.clone())?,
            )?;
            emit_assistant_response(&mut response_state.assistant_outputs, &key)?
                .into_iter()
                .collect()
        }
        "response.refusal.done" => {
            let key = semantic_output_key(&payload, &["output_index", "content_index"])?;
            complete_text(
                &payload,
                "refusal",
                accumulated_text(&mut response_state.refusal_outputs, key.clone())?,
            )?;
            emit_refusal(&mut response_state.refusal_outputs, &key)?
                .into_iter()
                .collect()
        }
        "response.reasoning_text.delta" => {
            let key = semantic_output_key(&payload, &["output_index", "content_index"])?;
            append_delta(
                &payload,
                accumulated_text(&mut response_state.reasoning_outputs, key)?,
            )?;
            Vec::new()
        }
        "response.reasoning_text.done" => {
            let key = semantic_output_key(&payload, &["output_index", "content_index"])?;
            complete_text(
                &payload,
                "text",
                accumulated_text(&mut response_state.reasoning_outputs, key.clone())?,
            )?;
            emit_reasoning(&mut response_state.reasoning_outputs, &key)?
                .into_iter()
                .collect()
        }
        "response.reasoning_summary_text.delta" => {
            let key = semantic_output_key(&payload, &["output_index", "summary_index"])?;
            append_delta(
                &payload,
                accumulated_text(&mut response_state.reasoning_summaries, key)?,
            )?;
            Vec::new()
        }
        "response.reasoning_summary_text.done" => {
            let key = semantic_output_key(&payload, &["output_index", "summary_index"])?;
            complete_text(
                &payload,
                "text",
                accumulated_text(&mut response_state.reasoning_summaries, key.clone())?,
            )?;
            emit_reasoning_summary(&mut response_state.reasoning_summaries, &key)?
                .into_iter()
                .collect()
        }
        "response.completed" => complete_response(response_state, &payload)?,
        "error" | "response.failed" if is_context_limit_payload(&payload) => {
            response_state.completed = true;
            vec![model_context_limit_exceeded()?]
        }
        "error" | "response.failed" => {
            return Err(ModelDriverError::Provider(format!(
                "OpenAI stream emitted {event_type}: {payload}"
            )));
        }
        _ => Vec::new(),
    };
    Ok(ProcessEventResult::Outputs(model_events))
}

fn complete_response(
    response_state: &mut ResponseState,
    payload: &Value,
) -> Result<Vec<ModelDriverEvent>, ModelDriverError> {
    if response_state.completed {
        return Err(ModelDriverError::InvalidResponse(
            "response.completed was received more than once".to_owned(),
        ));
    }
    if !payload.get("response").is_some_and(Value::is_object) {
        return Err(ModelDriverError::InvalidResponse(
            "response.completed did not contain an OpenAI response object".to_owned(),
        ));
    }

    let mut model_events = Vec::new();
    model_events.extend(emit_remaining_reasoning(
        &mut response_state.reasoning_outputs,
    )?);
    model_events.extend(emit_remaining_reasoning_summaries(
        &mut response_state.reasoning_summaries,
    )?);
    model_events.extend(emit_remaining_assistant_responses(
        &mut response_state.assistant_outputs,
    )?);
    model_events.extend(emit_remaining_refusals(
        &mut response_state.refusal_outputs,
    )?);
    let completed_content = completed_response_content(payload)?;
    for completed_output in completed_content.assistant_outputs {
        if !completed_output_already_emitted(
            &response_state.assistant_outputs,
            &completed_output.key,
        ) {
            model_events.push(assistant_response(completed_output.text)?);
        }
    }
    for completed_refusal in completed_content.refusals {
        if !completed_output_already_emitted(
            &response_state.refusal_outputs,
            &completed_refusal.key,
        ) {
            model_events.push(model_refusal(completed_refusal.text)?);
        }
    }
    let has_completed_model_output = response_state
        .assistant_outputs
        .iter()
        .chain(&response_state.refusal_outputs)
        .any(|output| output.emitted)
        || model_events.iter().any(|event| {
            matches!(
                event,
                ModelDriverEvent::Model {
                    event: ModelEvent::Assistant(_),
                    ..
                } | ModelDriverEvent::Problem { .. }
            )
        });
    if !has_completed_model_output {
        return Err(ModelDriverError::InvalidResponse(
            "the completed response contained no model message".to_owned(),
        ));
    }
    response_state.completed = true;
    Ok(model_events)
}

fn emit_reasoning(
    reasoning_outputs: &mut [AccumulatedText],
    key: &str,
) -> Result<Option<ModelDriverEvent>, ModelDriverError> {
    let output = un_emitted_text(reasoning_outputs, key)?;
    let Some(reasoning_text) = preferred_text(&output.streamed_text, &output.completed_text) else {
        return Ok(None);
    };
    let model_event =
        model_communication(reasoning_text, "reasoning", ModelEventImportance::Detailed)?;
    output.emitted = true;
    Ok(Some(model_event))
}

fn emit_reasoning_summary(
    reasoning_summaries: &mut [AccumulatedText],
    key: &str,
) -> Result<Option<ModelDriverEvent>, ModelDriverError> {
    let output = un_emitted_text(reasoning_summaries, key)?;
    let Some(reasoning_summary) = preferred_text(&output.streamed_text, &output.completed_text)
    else {
        return Ok(None);
    };
    let model_event = model_communication(
        reasoning_summary,
        "reasoning_summary",
        ModelEventImportance::Interesting,
    )?;
    output.emitted = true;
    Ok(Some(model_event))
}

fn emit_assistant_response(
    assistant_outputs: &mut [AccumulatedText],
    key: &str,
) -> Result<Option<ModelDriverEvent>, ModelDriverError> {
    let output = un_emitted_text(assistant_outputs, key)?;
    let assistant_text = preferred_text(&output.streamed_text, &output.completed_text);
    let Some(assistant_text) = assistant_text else {
        return Ok(None);
    };
    let model_event = assistant_response(assistant_text)?;
    output.emitted = true;
    Ok(Some(model_event))
}

fn emit_refusal(
    refusal_outputs: &mut [AccumulatedText],
    key: &str,
) -> Result<Option<ModelDriverEvent>, ModelDriverError> {
    let output = un_emitted_text(refusal_outputs, key)?;
    let refusal = preferred_text(&output.streamed_text, &output.completed_text);
    let Some(refusal) = refusal else {
        return Ok(None);
    };
    let model_event = model_refusal(refusal)?;
    output.emitted = true;
    Ok(Some(model_event))
}

fn emit_remaining_reasoning(
    outputs: &mut [AccumulatedText],
) -> Result<Vec<ModelDriverEvent>, ModelDriverError> {
    let keys = un_emitted_keys(outputs);
    keys.into_iter()
        .filter_map(|key| emit_reasoning(outputs, &key).transpose())
        .collect()
}

fn emit_remaining_reasoning_summaries(
    outputs: &mut [AccumulatedText],
) -> Result<Vec<ModelDriverEvent>, ModelDriverError> {
    let keys = un_emitted_keys(outputs);
    keys.into_iter()
        .filter_map(|key| emit_reasoning_summary(outputs, &key).transpose())
        .collect()
}

fn emit_remaining_assistant_responses(
    outputs: &mut [AccumulatedText],
) -> Result<Vec<ModelDriverEvent>, ModelDriverError> {
    let keys = un_emitted_keys(outputs);
    keys.into_iter()
        .filter_map(|key| emit_assistant_response(outputs, &key).transpose())
        .collect()
}

fn emit_remaining_refusals(
    outputs: &mut [AccumulatedText],
) -> Result<Vec<ModelDriverEvent>, ModelDriverError> {
    let keys = un_emitted_keys(outputs);
    keys.into_iter()
        .filter_map(|key| emit_refusal(outputs, &key).transpose())
        .collect()
}

fn assistant_response(message: String) -> Result<ModelDriverEvent, ModelDriverError> {
    AssistantResponse::new(message)
        .map(ModelEvent::Assistant)
        .map(|event| ModelDriverEvent::Model { event, data: None })
        .map_err(invalid_assistant_response)
}

fn model_refusal(message: String) -> Result<ModelDriverEvent, ModelDriverError> {
    ModelIssue::try_refusal(message)
        .map(|problem| ModelDriverEvent::Problem {
            problem,
            data: None,
        })
        .map_err(invalid_conversation_problem)
}

fn model_context_limit_exceeded() -> Result<ModelDriverEvent, ModelDriverError> {
    ModelIssue::try_context_limit_exceeded("The model context limit was exceeded.".to_owned())
        .map(|problem| ModelDriverEvent::Problem {
            problem,
            data: None,
        })
        .map_err(invalid_conversation_problem)
}

fn semantic_output_key(payload: &Value, indexes: &[&str]) -> Result<String, ModelDriverError> {
    let mut key_parts = Vec::new();
    for index_name in indexes {
        if let Some(index) = payload.get(index_name) {
            let index = index.as_u64().ok_or_else(|| {
                ModelDriverError::InvalidResponse(format!(
                    "an OpenAI semantic event contained an invalid {index_name}"
                ))
            })?;
            key_parts.push(format!("{index_name}={index}"));
        }
    }
    if !key_parts.is_empty() && key_parts.len() != indexes.len() {
        return Err(ModelDriverError::InvalidResponse(
            "an OpenAI semantic event contained incomplete output indexes".to_owned(),
        ));
    }
    if key_parts.len() == indexes.len() {
        return Ok(key_parts.join(";"));
    }

    match payload.get("item_id") {
        Some(Value::String(item_id)) => Ok(format!("item_id={item_id}")),
        Some(_) => Err(ModelDriverError::InvalidResponse(
            "an OpenAI semantic event contained a non-string item_id".to_owned(),
        )),
        None => Ok("default".to_owned()),
    }
}

fn accumulated_text(
    outputs: &mut Vec<AccumulatedText>,
    key: String,
) -> Result<&mut AccumulatedText, ModelDriverError> {
    if let Some(position) = outputs.iter().position(|output| output.key == key) {
        return Ok(&mut outputs[position]);
    }
    if (key == "default" && !outputs.is_empty())
        || (key != "default" && outputs.iter().any(|output| output.key == "default"))
    {
        return Err(ModelDriverError::InvalidResponse(
            "OpenAI semantic output identity changed while streaming".to_owned(),
        ));
    }
    outputs.push(AccumulatedText {
        key,
        streamed_text: String::new(),
        completed_text: None,
        emitted: false,
    });
    Ok(outputs
        .last_mut()
        .expect("the accumulated output was just added"))
}

fn un_emitted_text<'a>(
    outputs: &'a mut [AccumulatedText],
    key: &str,
) -> Result<&'a mut AccumulatedText, ModelDriverError> {
    let output = outputs
        .iter_mut()
        .find(|output| output.key == key)
        .expect("the accumulated output should exist");
    if output.emitted {
        return Err(ModelDriverError::InvalidResponse(format!(
            "OpenAI emitted semantic output {key} more than once"
        )));
    }
    Ok(output)
}

fn un_emitted_keys(outputs: &[AccumulatedText]) -> Vec<String> {
    outputs
        .iter()
        .filter(|output| !output.emitted)
        .map(|output| output.key.clone())
        .collect()
}

fn complete_text(
    payload: &Value,
    field: &str,
    output: &mut AccumulatedText,
) -> Result<(), ModelDriverError> {
    if output.emitted {
        return Err(ModelDriverError::InvalidResponse(format!(
            "OpenAI emitted semantic output {} more than once",
            output.key
        )));
    }
    output.completed_text = text_field(payload, field)?;
    Ok(())
}

fn model_communication(
    message: String,
    subtype: &str,
    importance: ModelEventImportance,
) -> Result<ModelDriverEvent, ModelDriverError> {
    ModelCommunication::new(message, importance, subtype.to_owned())
        .map(ModelEvent::Communication)
        .map(|event| ModelDriverEvent::Model { event, data: None })
        .map_err(invalid_model_communication)
}

fn invalid_assistant_response(error: InvalidAssistantResponse) -> ModelDriverError {
    ModelDriverError::InvalidResponse(error.to_string())
}

fn invalid_model_communication(error: InvalidModelCommunication) -> ModelDriverError {
    ModelDriverError::InvalidResponse(error.to_string())
}

fn invalid_conversation_problem(error: InvalidConversationProblem) -> ModelDriverError {
    ModelDriverError::InvalidResponse(error.to_string())
}

fn invalid_model_data(error: InvalidModelData) -> ModelDriverError {
    ModelDriverError::InvalidResponse(error.to_string())
}

fn append_delta(payload: &Value, output: &mut AccumulatedText) -> Result<(), ModelDriverError> {
    if output.emitted {
        return Err(ModelDriverError::InvalidResponse(format!(
            "OpenAI emitted a delta after completing semantic output {}",
            output.key
        )));
    }
    let delta = payload
        .get("delta")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ModelDriverError::InvalidResponse(
                "an OpenAI delta event did not contain a string delta".to_owned(),
            )
        })?;
    output.streamed_text.push_str(delta);
    Ok(())
}

fn text_field(payload: &Value, field: &str) -> Result<Option<String>, ModelDriverError> {
    let Some(value) = payload.get(field) else {
        return Ok(None);
    };
    value
        .as_str()
        .map(|text| Some(text.to_owned()))
        .ok_or_else(|| {
            ModelDriverError::InvalidResponse(format!(
                "an OpenAI completion event contained a non-string {field} field"
            ))
        })
}

fn preferred_text(streamed_text: &str, completed_text: &Option<String>) -> Option<String> {
    completed_text
        .clone()
        .or_else(|| (!streamed_text.is_empty()).then(|| streamed_text.to_owned()))
}

#[derive(Default)]
struct CompletedResponseContent {
    assistant_outputs: Vec<CompletedText>,
    refusals: Vec<CompletedText>,
}

struct CompletedText {
    key: String,
    text: String,
}

fn completed_response_content(
    payload: &Value,
) -> Result<CompletedResponseContent, ModelDriverError> {
    let Some(output_value) = payload
        .get("response")
        .and_then(|response| response.get("output"))
    else {
        return Ok(CompletedResponseContent::default());
    };
    let output = output_value.as_array().ok_or_else(|| {
        ModelDriverError::InvalidResponse(
            "the completed OpenAI response contained non-array output".to_owned(),
        )
    })?;
    let mut assistant_outputs = Vec::new();
    let mut refusals = Vec::new();
    for (output_index, output_item) in output.iter().enumerate() {
        let Some(content_value) = output_item.get("content") else {
            continue;
        };
        let content = content_value.as_array().ok_or_else(|| {
            ModelDriverError::InvalidResponse(
                "the completed OpenAI response contained non-array content".to_owned(),
            )
        })?;
        for (content_index, content_item) in content.iter().enumerate() {
            let key = format!("output_index={output_index};content_index={content_index}");
            match content_item.get("type").and_then(Value::as_str) {
                Some("output_text") => {
                    let content_text = content_item
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            ModelDriverError::InvalidResponse(
                                "completed OpenAI output text was not a string".to_owned(),
                            )
                        })?;
                    assistant_outputs.push(CompletedText {
                        key,
                        text: content_text.to_owned(),
                    });
                }
                Some("refusal") => {
                    let refusal = content_item
                        .get("refusal")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            ModelDriverError::InvalidResponse(
                                "completed OpenAI refusal was not a string".to_owned(),
                            )
                        })?;
                    refusals.push(CompletedText {
                        key,
                        text: refusal.to_owned(),
                    });
                }
                _ => {}
            }
        }
    }
    Ok(CompletedResponseContent {
        assistant_outputs,
        refusals,
    })
}

fn completed_output_already_emitted(outputs: &[AccumulatedText], key: &str) -> bool {
    outputs
        .iter()
        .any(|output| output.emitted && (output.key == "default" || output.key == key))
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::str::FromStr;
    use std::thread;

    use futures_util::StreamExt;
    use futures_util::stream;
    use reqwest::StatusCode;
    use serde_json::{Map, Value, json};
    use time::OffsetDateTime;

    use crate::conversation::{
        AssistantResponse, Conversation, ConversationEvent, ConversationEventId,
        ConversationEventKind, ConversationId, ConversationProblem, ConversationRecordKind,
        ConversationTurnId, ModelCommunication, ModelData, ModelDetails, ModelEvent,
        ModelEventImportance, ModelId, ModelInvocationId, ModelIssue, ModelSource, ProviderId,
        UserContent,
    };
    use crate::model_driver::{ModelDriver, ModelDriverError, ModelDriverOutput};

    use super::{
        ModelDriverEvent, OpenAiModelDriver, ResponseByteStream, classify_response_failure,
        model_communication, model_output_stream, semantic_input,
    };

    fn conversation_event(
        conversation_id: ConversationId,
        position: u64,
        kind: ConversationEventKind,
    ) -> ConversationEvent {
        ConversationEvent {
            conversation_id,
            position,
            id: ConversationEventId::new(),
            timestamp: OffsetDateTime::UNIX_EPOCH,
            schema_version: 7,
            kind: ConversationRecordKind::Event(kind),
        }
    }

    #[test]
    fn semantic_input_projects_canonical_events_and_ignores_model_data() {
        let conversation_id = ConversationId::new();
        let source = ModelSource::new(
            ProviderId::from_str("openai").expect("the provider identifier should be valid"),
            ModelId::from_str("gpt-5.6").expect("the model identifier should be valid"),
        );
        let model_data = ModelData::new(Map::from_iter([(
            "native".to_owned(),
            Value::String("ignored".to_owned()),
        )]))
        .expect("the model data should be valid");
        let turn_id = ConversationTurnId::new();
        let invocation_id = ModelInvocationId::new();
        let conversation = Conversation::from_events(vec![
            conversation_event(
                conversation_id,
                0,
                ConversationEventKind::User {
                    caused_by: None,
                    content: vec![UserContent::Text("Hello".to_owned())],
                },
            ),
            conversation_event(
                conversation_id,
                1,
                ConversationEventKind::Communication {
                    turn_id,
                    invocation_id,
                    model: ModelDetails::new(source.clone(), Some(model_data.clone()))
                        .expect("the model details should be valid"),
                    communication: ModelCommunication::new(
                        "Reasoning".to_owned(),
                        ModelEventImportance::Detailed,
                        "reasoning".to_owned(),
                    )
                    .expect("the model communication should be valid"),
                },
            ),
            conversation_event(
                conversation_id,
                2,
                ConversationEventKind::Assistant {
                    turn_id,
                    invocation_id,
                    model: ModelDetails::new(source, Some(model_data))
                        .expect("the model details should be valid"),
                    response: AssistantResponse::new("Hello.".to_owned())
                        .expect("the assistant response should be valid"),
                },
            ),
        ])
        .expect("the conversation should be valid");

        assert_eq!(
            semantic_input(&conversation),
            json!([
                { "role": "user", "content": "Hello" },
                { "role": "assistant", "content": "Hello." }
            ])
        );
    }

    fn response_byte_stream(chunks: Vec<Vec<u8>>) -> ResponseByteStream {
        stream::iter(chunks.into_iter().map(Ok::<Vec<u8>, ModelDriverError>)).boxed()
    }

    fn one_byte_chunks(input: &str) -> Vec<Vec<u8>> {
        input.as_bytes().iter().map(|byte| vec![*byte]).collect()
    }

    async fn collect_events(input: &str) -> Vec<Result<ModelEvent, ModelDriverError>> {
        model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]))
            .map(|result| {
                result.and_then(|driver_event| match driver_event {
                    ModelDriverEvent::Model { event, .. } => Ok(event),
                    ModelDriverEvent::Problem { .. } => Err(ModelDriverError::InvalidResponse(
                        "the test expected a model event, not a model problem".to_owned(),
                    )),
                })
            })
            .collect()
            .await
    }

    async fn collect_outputs(input: &str) -> Vec<Result<ModelDriverEvent, ModelDriverError>> {
        model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]))
            .collect()
            .await
    }

    fn expect_model_event(driver_event: ModelDriverEvent) -> ModelEvent {
        match driver_event {
            ModelDriverEvent::Model { event, .. } => event,
            ModelDriverEvent::Problem { .. } => panic!("the output should be a model event"),
        }
    }

    fn expect_event(output: ModelDriverOutput) -> ConversationEventKind {
        match output {
            ModelDriverOutput::Event(event) => event,
            ModelDriverOutput::Driver(_) => panic!("the output should be a conversation event"),
        }
    }

    fn test_conversation() -> Conversation {
        let conversation_id = ConversationId::new();
        Conversation::from_events(vec![conversation_event(
            conversation_id,
            0,
            ConversationEventKind::User {
                caused_by: None,
                content: vec![UserContent::Text("Hello".to_owned())],
            },
        )])
        .expect("the conversation should be valid")
    }

    fn source() -> ModelSource {
        ModelSource::new(
            ProviderId::from_str("openai").expect("the provider identifier should be valid"),
            ModelId::from_str("gpt-5.6").expect("the model identifier should be valid"),
        )
    }

    #[tokio::test]
    async fn invoke_returns_a_future_that_establishes_one_conversation_event_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server should bind");
        let address = listener
            .local_addr()
            .expect("the mock server address should be available");
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("the mock server should accept");
            read_request(&connection);
            let response_body = concat!(
                "data: {\"type\":\"response.reasoning_text.done\",\"text\":\"Reasoning\"}\n\n",
                "data: {\"type\":\"response.output_text.done\",\"text\":\"Answer\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{}}\n\n",
                "data: [DONE]\n\n"
            );
            write!(
                connection,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            )
            .expect("the mock response should write");
        });
        let driver = OpenAiModelDriver {
            http_client: reqwest::Client::new(),
            api_key: "test-key".to_owned(),
            responses_url: format!("http://{address}/responses"),
            source: source(),
        };

        let mut model_events = driver
            .invoke(&test_conversation(), ConversationTurnId::new())
            .await
            .expect("the invocation should establish its stream");
        let invocation = model_events
            .next()
            .await
            .expect("the stream should yield an invocation event")
            .expect("the invocation event should be valid");
        assert!(matches!(invocation, ModelDriverOutput::Driver(_)));
        let first_event = expect_event(
            model_events
                .next()
                .await
                .expect("the stream should yield reasoning")
                .expect("the reasoning should be valid"),
        );
        let second_event = expect_event(
            model_events
                .next()
                .await
                .expect("the stream should yield an answer")
                .expect("the answer should be valid"),
        );

        assert!(matches!(
            &first_event,
            ConversationEventKind::Communication { .. }
        ));
        assert!(matches!(
            &second_event,
            ConversationEventKind::Assistant { .. }
        ));
        let ConversationEventKind::Communication { model, .. } = &first_event else {
            panic!("the first event should be a model event");
        };
        assert_eq!(model.source(), &source());
        let ConversationEventKind::Assistant { model, .. } = &second_event else {
            panic!("the second event should be a model event");
        };
        assert_eq!(model.source(), &source());
        assert!(matches!(
            &first_event,
            ConversationEventKind::Communication { model, .. } if model.data().is_none()
        ));
        assert!(matches!(
            &second_event,
            ConversationEventKind::Assistant { model, .. } if model.data().is_none()
        ));
        assert!(matches!(
            model_events.next().await,
            Some(Ok(ModelDriverOutput::Event(
                ConversationEventKind::TurnCompleted { .. }
            )))
        ));
        assert!(model_events.next().await.is_none());
        server.join().expect("the mock server should stop");
    }

    #[tokio::test]
    async fn an_early_http_failure_produces_no_model_stream() {
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
        let driver = OpenAiModelDriver {
            http_client: reqwest::Client::new(),
            api_key: "test-key".to_owned(),
            responses_url: format!("http://{address}/responses"),
            source: source(),
        };

        let result = driver
            .invoke(&test_conversation(), ConversationTurnId::new())
            .await;

        let mut model_events = result.expect("the invocation should establish a stream");
        assert!(matches!(
            model_events.next().await,
            Some(Ok(ModelDriverOutput::Driver(_)))
        ));
        assert!(matches!(
            model_events.next().await,
            Some(Err(ModelDriverError::Authentication(_)))
        ));
        server.join().expect("the mock server should stop");
    }

    #[tokio::test]
    async fn a_context_limit_http_response_becomes_a_model_issue_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server should bind");
        let address = listener
            .local_addr()
            .expect("the mock server address should be available");
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("the mock server should accept");
            read_request(&connection);
            let body = json!({
                "error": {
                    "code": "context_length_exceeded",
                    "message": "raw provider details"
                }
            })
            .to_string();
            write!(
                connection,
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("the mock response should write");
        });
        let driver = OpenAiModelDriver {
            http_client: reqwest::Client::new(),
            api_key: "test-key".to_owned(),
            responses_url: format!("http://{address}/responses"),
            source: source(),
        };

        let mut model_events = driver
            .invoke(&test_conversation(), ConversationTurnId::new())
            .await
            .expect("the context-limit outcome should establish a semantic stream");
        assert!(matches!(
            model_events.next().await,
            Some(Ok(ModelDriverOutput::Driver(_)))
        ));
        let model_event = expect_event(
            model_events
                .next()
                .await
                .expect("the stream should yield a context-limit issue")
                .expect("the context-limit issue should be valid"),
        );

        assert!(matches!(
            &model_event,
            ConversationEventKind::Problem {
                problem: ConversationProblem::Issue(ModelIssue::ContextLimitExceeded { .. }),
                ..
            }
        ));
        let ConversationEventKind::Problem { problem, .. } = &model_event else {
            panic!("the output should be a model issue");
        };
        assert_eq!(problem.message(), "The model context limit was exceeded.");
        assert!(matches!(
            model_events.next().await,
            Some(Ok(ModelDriverOutput::Event(
                ConversationEventKind::TurnCompleted { .. }
            )))
        ));
        assert!(model_events.next().await.is_none());
        server.join().expect("the mock server should stop");
    }

    #[tokio::test]
    async fn arbitrary_byte_boundaries_crlf_multiline_data_and_event_fields_parse() {
        let input = concat!(
            "event: response.output_text.delta\r\n",
            "data: {\"delta\":\r\n",
            "data: \"Hello\"}\r\n\r\n",
            "event: response.output_text.done\r\n",
            "data: {}\r\n\r\n",
            "event: response.completed\r\n",
            "data: {\"response\":{}}\r\n\r\n",
            "data: [DONE]\r\n\r\n"
        );
        let mut model_events = model_output_stream(response_byte_stream(one_byte_chunks(input)));

        let model_event = model_events
            .next()
            .await
            .expect("the stream should yield an event")
            .expect("the event should be valid");

        assert_eq!(expect_model_event(model_event).message(), "Hello");
        assert!(model_events.next().await.is_none());
    }

    #[tokio::test]
    async fn a_completed_refusal_is_a_model_issue_not_an_assistant_response() {
        let input = concat!(
            "data: {\"type\":\"response.refusal.delta\",\"delta\":\"I cannot \"}\n\n",
            "data: {\"type\":\"response.refusal.done\",\"refusal\":\"I cannot comply.\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{}}\n\n"
        );

        let events = collect_outputs(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the refusal stream should parse");

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ModelDriverEvent::Problem {
                problem: ModelIssue::Refusal { .. },
                ..
            }
        ));
        let ModelDriverEvent::Problem { problem: issue, .. } = &events[0] else {
            panic!("the output should be a refusal issue");
        };
        assert_eq!(issue.message(), "I cannot comply.");
    }

    #[tokio::test]
    async fn several_sse_events_in_one_chunk_yield_reasoning_before_the_answer() {
        let input = concat!(
            "data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"Detailed \"}\n\n",
            "data: {\"type\":\"response.reasoning_text.done\",\"text\":\"Detailed thought\"}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.done\",\"text\":\"Summary\"}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"text\":\"Answer\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{}}\n\n"
        );

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the response stream should parse");

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].message(), "Detailed thought");
        assert_eq!(events[0].importance(), ModelEventImportance::Detailed);
        assert_eq!(events[1].message(), "Summary");
        assert_eq!(events[1].importance(), ModelEventImportance::Interesting);
        assert_eq!(events[2].message(), "Answer");
        assert_eq!(events[2].importance(), ModelEventImportance::Important);
    }

    #[tokio::test]
    async fn a_late_stream_failure_follows_the_completed_model_event() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"text\":\"Hello\"}\n\n",
            "data: {\"type\":\"error\",\"message\":\"late failure\"}\n\n"
        );
        let mut model_events =
            model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        let completed_event = model_events
            .next()
            .await
            .expect("the stream should yield an event")
            .expect("the completed event should be valid");
        assert_eq!(expect_model_event(completed_event).message(), "Hello");
        assert!(matches!(
            model_events.next().await,
            Some(Err(ModelDriverError::Provider(_)))
        ));
        assert!(model_events.next().await.is_none());
    }

    #[tokio::test]
    async fn context_limit_stream_failure_is_a_model_issue() {
        let input = concat!(
            "data: {\"type\":\"error\",\"code\":\"context_length_exceeded\",\"message\":\"raw details\"}\n\n",
            "data: [DONE]\n\n"
        );
        let events = collect_outputs(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the context-limit failure should be semantic");

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ModelDriverEvent::Problem {
                problem: ModelIssue::ContextLimitExceeded { .. },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn premature_body_end_is_a_stream_interruption() {
        let input = "data: {\"type\":\"response.output_text.done\",\"text\":\"Hello\"}\n\n";
        let mut model_events =
            model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        assert!(matches!(
            model_events.next().await,
            Some(Ok(ModelDriverEvent::Model {
                event: ModelEvent::Assistant(_),
                ..
            }))
        ));
        assert!(matches!(
            model_events.next().await,
            Some(Err(ModelDriverError::StreamInterrupted(_)))
        ));
    }

    #[tokio::test]
    async fn missing_response_completed_is_a_stream_error_even_after_done() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"text\":\"Hello\"}\n\n",
            "data: [DONE]\n\n"
        );
        let mut model_events =
            model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        assert!(matches!(
            model_events.next().await,
            Some(Ok(ModelDriverEvent::Model {
                event: ModelEvent::Assistant(_),
                ..
            }))
        ));
        assert!(matches!(
            model_events.next().await,
            Some(Err(ModelDriverError::InvalidResponse(_)))
        ));
    }

    #[tokio::test]
    async fn response_completed_fallback_does_not_duplicate_a_done_event() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"text\":\"Answer\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"content\":[{\"type\":\"output_text\",\"text\":\"Answer\"}]}]}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the response stream should parse");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message(), "Answer");
    }

    #[tokio::test]
    async fn response_completed_adds_unstreamed_indexed_output_without_duplicates() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"output_index\":0,\"content_index\":0,\"text\":\"First\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"content\":[{\"type\":\"output_text\",\"text\":\"First\"},{\"type\":\"output_text\",\"text\":\"Second\"}]}]}}\n\n"
        );

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the completed response fallback should parse");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].message(), "First");
        assert_eq!(events[1].message(), "Second");
    }

    #[tokio::test]
    async fn distinct_indexed_output_completions_yield_distinct_semantic_events() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"output_index\":0,\"content_index\":0,\"text\":\"First\"}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"output_index\":1,\"content_index\":0,\"text\":\"Second\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{}}\n\n"
        );

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the indexed output should parse");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].message(), "First");
        assert_eq!(events[1].message(), "Second");
    }

    #[tokio::test]
    async fn duplicate_indexed_completion_is_a_stream_error_after_the_completed_event() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"output_index\":0,\"content_index\":0,\"text\":\"Answer\"}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"output_index\":0,\"content_index\":0,\"text\":\"Answer\"}\n\n"
        );
        let mut events = model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        assert!(matches!(
            events.next().await,
            Some(Ok(ModelDriverEvent::Model {
                event: ModelEvent::Assistant(_),
                ..
            }))
        ));
        assert!(matches!(
            events.next().await,
            Some(Err(ModelDriverError::InvalidResponse(_)))
        ));
    }

    #[tokio::test]
    async fn response_completed_supplies_final_object_fallback_at_end_of_stream() {
        let input = "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"content\":[{\"type\":\"output_text\",\"text\":\"Fallback\"}]}]}}";

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the buffered final event should parse");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message(), "Fallback");
    }

    #[tokio::test]
    async fn malformed_json_is_a_stream_error() {
        let mut events =
            model_output_stream(response_byte_stream(vec![b"data: not-json\n\n".to_vec()]));

        assert!(matches!(
            events.next().await,
            Some(Err(ModelDriverError::InvalidResponse(_)))
        ));
    }

    #[test]
    fn invalid_model_communication_maps_to_invalid_response() {
        let empty_message = model_communication(
            "   ".to_owned(),
            "reasoning",
            ModelEventImportance::Detailed,
        );
        let empty_subtype = model_communication(
            "reasoning".to_owned(),
            "   ",
            ModelEventImportance::Detailed,
        );

        assert!(matches!(
            empty_message,
            Err(ModelDriverError::InvalidResponse(_))
        ));
        assert!(matches!(
            empty_subtype,
            Err(ModelDriverError::InvalidResponse(_))
        ));
    }

    #[test]
    fn response_statuses_map_to_typed_driver_errors() {
        assert!(matches!(
            classify_response_failure(StatusCode::UNAUTHORIZED, "unauthorized".to_owned()),
            Err(ModelDriverError::Authentication(_))
        ));
        assert!(matches!(
            classify_response_failure(StatusCode::TOO_MANY_REQUESTS, "slow down".to_owned()),
            Err(ModelDriverError::RateLimited(_))
        ));
        assert!(matches!(
            classify_response_failure(StatusCode::BAD_REQUEST, "bad request".to_owned()),
            Err(ModelDriverError::InvalidRequest(_))
        ));
        assert!(matches!(
            classify_response_failure(StatusCode::INTERNAL_SERVER_ERROR, "failed".to_owned()),
            Err(ModelDriverError::Provider(_))
        ));
    }

    fn read_request(connection: &TcpStream) {
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
    }
}
