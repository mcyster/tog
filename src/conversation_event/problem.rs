use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "category", content = "detail", rename_all = "snake_case")]
pub(crate) enum ConversationProblem {
    Issue(ModelIssue),
    Invocation(InvocationError),
}

impl ConversationProblem {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Issue(issue) => issue.message(),
            Self::Invocation(error) => error.message(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn retryable(&self) -> bool {
        match self {
            Self::Issue(issue) => issue.retryable(),
            Self::Invocation(error) => error.retryable(),
        }
    }

    pub(super) fn ensure_valid(&self) -> Result<(), InvalidConversationProblem> {
        match self {
            Self::Issue(issue) => issue.ensure_valid(),
            Self::Invocation(error) => error.ensure_valid(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ModelIssue {
    Refusal { message: ProblemMessage },
    ContextLimitExceeded { message: ProblemMessage },
}

impl ModelIssue {
    pub(crate) fn try_refusal(message: String) -> Result<Self, InvalidConversationProblem> {
        Ok(Self::Refusal {
            message: ProblemMessage::try_new(message)?,
        })
    }

    pub(crate) fn try_context_limit_exceeded(
        message: String,
    ) -> Result<Self, InvalidConversationProblem> {
        Ok(Self::ContextLimitExceeded {
            message: ProblemMessage::try_new(message)?,
        })
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Refusal { message } | Self::ContextLimitExceeded { message } => message.as_str(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn retryable(&self) -> bool {
        false
    }

    fn ensure_valid(&self) -> Result<(), InvalidConversationProblem> {
        match self {
            Self::Refusal { message } | Self::ContextLimitExceeded { message } => {
                message.ensure_valid()
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum InvocationError {
    Authentication { message: ProblemMessage },
    RateLimited { message: ProblemMessage },
    Transport { message: ProblemMessage },
    InvalidRequest { message: ProblemMessage },
    ProviderFailure { message: ProblemMessage },
    InvalidProviderResponse { message: ProblemMessage },
    StreamInterrupted { message: ProblemMessage },
}

impl InvocationError {
    pub(crate) fn try_authentication(message: String) -> Result<Self, InvalidConversationProblem> {
        Ok(Self::Authentication {
            message: ProblemMessage::try_new(message)?,
        })
    }

    pub(crate) fn try_rate_limited(message: String) -> Result<Self, InvalidConversationProblem> {
        Ok(Self::RateLimited {
            message: ProblemMessage::try_new(message)?,
        })
    }

    pub(crate) fn try_transport(message: String) -> Result<Self, InvalidConversationProblem> {
        Ok(Self::Transport {
            message: ProblemMessage::try_new(message)?,
        })
    }

    pub(crate) fn try_invalid_request(message: String) -> Result<Self, InvalidConversationProblem> {
        Ok(Self::InvalidRequest {
            message: ProblemMessage::try_new(message)?,
        })
    }

    pub(crate) fn try_provider_failure(
        message: String,
    ) -> Result<Self, InvalidConversationProblem> {
        Ok(Self::ProviderFailure {
            message: ProblemMessage::try_new(message)?,
        })
    }

    pub(crate) fn try_invalid_provider_response(
        message: String,
    ) -> Result<Self, InvalidConversationProblem> {
        Ok(Self::InvalidProviderResponse {
            message: ProblemMessage::try_new(message)?,
        })
    }

    pub(crate) fn try_stream_interrupted(
        message: String,
    ) -> Result<Self, InvalidConversationProblem> {
        Ok(Self::StreamInterrupted {
            message: ProblemMessage::try_new(message)?,
        })
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Authentication { message }
            | Self::RateLimited { message }
            | Self::Transport { message }
            | Self::InvalidRequest { message }
            | Self::ProviderFailure { message }
            | Self::InvalidProviderResponse { message }
            | Self::StreamInterrupted { message } => message.as_str(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::Transport { .. }
                | Self::ProviderFailure { .. }
                | Self::StreamInterrupted { .. }
        )
    }

    fn ensure_valid(&self) -> Result<(), InvalidConversationProblem> {
        match self {
            Self::Authentication { message }
            | Self::RateLimited { message }
            | Self::Transport { message }
            | Self::InvalidRequest { message }
            | Self::ProviderFailure { message }
            | Self::InvalidProviderResponse { message }
            | Self::StreamInterrupted { message } => message.ensure_valid(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ProblemMessage(String);

impl ProblemMessage {
    fn try_new(message: String) -> Result<Self, InvalidConversationProblem> {
        let message = Self(message);
        message.ensure_valid()?;
        Ok(message)
    }

    fn as_str(&self) -> &str {
        &self.0
    }

    fn ensure_valid(&self) -> Result<(), InvalidConversationProblem> {
        if self.0.trim().is_empty() {
            return Err(InvalidConversationProblem::EmptyMessage);
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidConversationProblem {
    EmptyMessage,
}

impl Display for InvalidConversationProblem {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMessage => {
                write!(formatter, "conversation problem message must not be empty")
            }
        }
    }
}

impl Error for InvalidConversationProblem {}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ConversationProblem, InvocationError, ModelIssue};

    #[test]
    fn conversation_problem_round_trips_with_a_closed_tagged_representation() {
        let problem = ConversationProblem::Issue(
            ModelIssue::try_refusal("I cannot help with that.".to_owned())
                .expect("the refusal should be valid"),
        );

        let serialized = serde_json::to_value(&problem).expect("the problem should serialize");
        let deserialized: ConversationProblem =
            serde_json::from_value(serialized.clone()).expect("the problem should deserialize");

        assert_eq!(
            serialized,
            json!({
                "category": "issue",
                "detail": {
                    "type": "refusal",
                    "message": "I cannot help with that."
                }
            })
        );
        assert_eq!(deserialized, problem);
        assert_eq!(problem.message(), "I cannot help with that.");
        assert!(!problem.retryable());
    }

    #[test]
    fn every_problem_has_an_intentional_message_and_retryability() {
        let problems = [
            (
                ConversationProblem::Issue(
                    ModelIssue::try_refusal("Refused.".to_owned())
                        .expect("the refusal should be valid"),
                ),
                "Refused.",
                false,
            ),
            (
                ConversationProblem::Issue(
                    ModelIssue::try_context_limit_exceeded("Context exceeded.".to_owned())
                        .expect("the context issue should be valid"),
                ),
                "Context exceeded.",
                false,
            ),
            (
                ConversationProblem::Invocation(
                    InvocationError::try_authentication("Authentication failed.".to_owned())
                        .expect("the authentication error should be valid"),
                ),
                "Authentication failed.",
                false,
            ),
            (
                ConversationProblem::Invocation(
                    InvocationError::try_rate_limited("Rate limited.".to_owned())
                        .expect("the rate limit should be valid"),
                ),
                "Rate limited.",
                true,
            ),
            (
                ConversationProblem::Invocation(
                    InvocationError::try_transport("Transport failed.".to_owned())
                        .expect("the transport error should be valid"),
                ),
                "Transport failed.",
                true,
            ),
            (
                ConversationProblem::Invocation(
                    InvocationError::try_invalid_request("Request invalid.".to_owned())
                        .expect("the invalid request should be valid"),
                ),
                "Request invalid.",
                false,
            ),
            (
                ConversationProblem::Invocation(
                    InvocationError::try_provider_failure("Provider failed.".to_owned())
                        .expect("the provider failure should be valid"),
                ),
                "Provider failed.",
                true,
            ),
            (
                ConversationProblem::Invocation(
                    InvocationError::try_invalid_provider_response("Response invalid.".to_owned())
                        .expect("the invalid provider response should be valid"),
                ),
                "Response invalid.",
                false,
            ),
            (
                ConversationProblem::Invocation(
                    InvocationError::try_stream_interrupted("Stream interrupted.".to_owned())
                        .expect("the stream interruption should be valid"),
                ),
                "Stream interrupted.",
                true,
            ),
        ];

        for (problem, expected_message, expected_retryability) in problems {
            assert_eq!(problem.message(), expected_message);
            assert_eq!(problem.retryable(), expected_retryability);
        }
    }

    #[test]
    fn conversation_problem_messages_must_not_be_blank() {
        assert!(ModelIssue::try_refusal("  ".to_owned()).is_err());
        assert!(ModelIssue::try_context_limit_exceeded(String::new()).is_err());
        assert!(InvocationError::try_authentication(" ".to_owned()).is_err());
        assert!(InvocationError::try_rate_limited("\n".to_owned()).is_err());
        assert!(InvocationError::try_transport("\t".to_owned()).is_err());
        assert!(InvocationError::try_invalid_request(String::new()).is_err());
        assert!(InvocationError::try_provider_failure("  ".to_owned()).is_err());
        assert!(InvocationError::try_invalid_provider_response("\r\n".to_owned()).is_err());
        assert!(InvocationError::try_stream_interrupted("\t".to_owned()).is_err());
    }

    #[test]
    fn conversation_problem_messages_preserve_surrounding_whitespace() {
        let issue = ConversationProblem::Issue(
            ModelIssue::try_refusal("  refusal\n".to_owned()).expect("the refusal should be valid"),
        );
        let invocation = ConversationProblem::Invocation(
            InvocationError::try_transport("  transport\n".to_owned())
                .expect("the transport error should be valid"),
        );

        assert_eq!(issue.message(), "  refusal\n");
        assert_eq!(invocation.message(), "  transport\n");
    }
}
