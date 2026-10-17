mod input;
mod stream;
mod usage;

use std::fmt::{Display, Formatter};
use std::str::FromStr;

use futures_util::future::BoxFuture;
use futures_util::{FutureExt, StreamExt};
use reqwest::Client;
use reqwest::StatusCode;
use serde_json::{Map, Value, json};

use tog::conversation::{
    FailureCategory, ModelId, ModelOutcome, ModelSource, OperationFailure, ProviderId,
};
use tog::model_driver::{ModelDriver, ModelDriverError, ModelOutputStream, TurnInput};

pub(crate) const PROVIDER_PAYLOAD_VERSION: u32 = 1;
pub(crate) const REASONING_ITEM_EVENT_TYPE: &str = "response_reasoning_item";

#[derive(Debug, Eq, PartialEq)]
pub enum OpenRouterResponsesError {
    Authentication(String),
    RateLimited(String),
    Transport(String),
    InvalidRequest(String),
    InvalidResponse(String),
    StreamInterrupted(String),
    Provider(String),
}

impl Display for OpenRouterResponsesError {
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

impl std::error::Error for OpenRouterResponsesError {}

pub struct OpenRouterResponsesModelDriver {
    http_client: Client,
    api_key: String,
    responses_url: String,
    source: ModelSource,
}

impl OpenRouterResponsesModelDriver {
    pub fn from_environment(model: ModelId) -> Result<Self, OpenRouterResponsesError> {
        let api_key = std::env::var("OPENROUTER_API_KEY").map_err(|_| {
            OpenRouterResponsesError::Authentication("OPENROUTER_API_KEY must be set".to_owned())
        })?;
        let base_url = std::env::var("TOG_OPENROUTER_BASE_URL")
            .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_owned());
        Ok(Self {
            http_client: Client::new(),
            api_key,
            responses_url: format!("{}/responses", base_url.trim_end_matches('/')),
            source: ModelSource::new(
                ProviderId::from_str("openrouter")
                    .expect("the OpenRouter provider identifier should be valid"),
                model,
            ),
        })
    }
}

impl ModelDriver for OpenRouterResponsesModelDriver {
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
        request_body.insert(
            "input".to_owned(),
            input::responses_input(events, &self.source),
        );
        let immediate_tools = input
            .tools()
            .map(|tools| {
                tools
                    .tools()
                    .iter()
                    .filter(|tool| {
                        tool.availability() == tog::conversation::ToolAvailability::Immediate
                    })
                    .map(|tool| input::provider_tool(tool.definition()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !immediate_tools.is_empty() {
            request_body.insert("tools".to_owned(), Value::Array(immediate_tools));
        }
        request_body.insert("reasoning".to_owned(), json!({ "summary": "auto" }));
        request_body.insert("stream".to_owned(), Value::Bool(true));

        let request = self
            .http_client
            .post(&self.responses_url)
            .bearer_auth(&self.api_key)
            .json(&request_body)
            .build();
        let http_client = self.http_client.clone();
        async move {
            let request = match request {
                Ok(request) => request,
                Err(error) => {
                    let error = OpenRouterResponsesError::InvalidRequest(error.to_string());
                    return Ok(stream::failed_terminal_stream(
                        model_request_id,
                        FailureStage::BeforeStream,
                        error,
                    ));
                }
            };
            let response = match http_client.execute(request).await {
                Ok(response) => response,
                Err(error) => {
                    return Ok(stream::failed_terminal_stream(
                        model_request_id,
                        FailureStage::BeforeStream,
                        OpenRouterResponsesError::Transport(error.to_string()),
                    ));
                }
            };
            let response_status = response.status();
            if !response_status.is_success() {
                let response_body = match response.text().await {
                    Ok(response_body) => response_body,
                    Err(error) => {
                        return Ok(stream::failed_terminal_stream(
                            model_request_id,
                            FailureStage::BeforeStream,
                            OpenRouterResponsesError::Transport(error.to_string()),
                        ));
                    }
                };
                return match classify_response_failure(response_status, response_body) {
                    Ok(()) => Ok(stream::context_limit_stream(model_request_id)),
                    Err(error) => Ok(stream::failed_terminal_stream(
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
                        .map_err(|error| OpenRouterResponsesError::Transport(error.to_string()))
                })
                .boxed();
            Ok(stream::model_output_stream(
                response_bytes,
                model_request_id,
            ))
        }
        .boxed()
    }
}

fn classify_response_failure(
    status: StatusCode,
    body: String,
) -> Result<(), OpenRouterResponsesError> {
    if status == StatusCode::BAD_REQUEST && is_context_limit_error(&body) {
        return Ok(());
    }

    Err(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            OpenRouterResponsesError::Authentication(body)
        }
        StatusCode::TOO_MANY_REQUESTS => OpenRouterResponsesError::RateLimited(body),
        StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => {
            OpenRouterResponsesError::InvalidRequest(body)
        }
        _ => OpenRouterResponsesError::Provider(format!(
            "OpenRouter Responses returned {status}: {body}"
        )),
    })
}

fn is_context_limit_error(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .is_some_and(|payload| is_context_limit_payload(&payload))
}

pub(crate) fn is_context_limit_payload(payload: &Value) -> bool {
    let error_code = payload
        .pointer("/code")
        .or_else(|| payload.pointer("/error/code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let error_message = payload
        .pointer("/error/message")
        .or_else(|| payload.pointer("/message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    [
        "context_length",
        "context_window",
        "maximum context",
        "too many tokens",
        "context_length_exceeded",
        "context_window_exceeded",
    ]
    .into_iter()
    .any(|needle| error_code.contains(needle) || error_message.contains(needle))
}

#[derive(Clone, Copy)]
pub(crate) enum FailureStage {
    BeforeStream,
    DuringStream,
}

pub(crate) fn provider_failure(
    error: &OpenRouterResponsesError,
    failure_stage: FailureStage,
) -> (FailureCategory, String) {
    match error {
        OpenRouterResponsesError::Authentication(_) => (
            FailureCategory::Authentication,
            "The model provider could not authenticate the invocation.".to_owned(),
        ),
        OpenRouterResponsesError::RateLimited(_) => (
            FailureCategory::RateLimited,
            "The model provider rate-limited the invocation.".to_owned(),
        ),
        OpenRouterResponsesError::Transport(_)
            if matches!(failure_stage, FailureStage::DuringStream) =>
        {
            (
                FailureCategory::StreamInterrupted,
                "The model response stream was interrupted.".to_owned(),
            )
        }
        OpenRouterResponsesError::Transport(_) => (
            FailureCategory::Transport,
            "The model provider could not be reached.".to_owned(),
        ),
        OpenRouterResponsesError::InvalidRequest(_) => (
            FailureCategory::InvalidRequest,
            "The model invocation request was invalid.".to_owned(),
        ),
        OpenRouterResponsesError::InvalidResponse(_) => (
            FailureCategory::InvalidProviderResponse,
            "The model provider returned an invalid response.".to_owned(),
        ),
        OpenRouterResponsesError::StreamInterrupted(_) => (
            FailureCategory::StreamInterrupted,
            "The model response stream was interrupted.".to_owned(),
        ),
        OpenRouterResponsesError::Provider(_) => (
            FailureCategory::ProviderFailure,
            "The model provider failed the invocation.".to_owned(),
        ),
    }
}

pub(crate) fn failed_outcome(category: FailureCategory, message: &str) -> ModelOutcome {
    ModelOutcome::Failed {
        failure: OperationFailure::try_new(category, message.to_owned(), None)
            .expect("the terminal failure should be valid"),
    }
}

#[cfg(test)]
mod tests;
