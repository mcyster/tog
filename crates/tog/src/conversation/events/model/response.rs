use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::conversation::events::{ConversationEventId, InvalidOperationFailure, OperationFailure};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ModelResponse {
    model_request_id: ConversationEventId,
    output_event_ids: Vec<ConversationEventId>,
    outcome: ModelOutcome,
    continuation: ModelContinuation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage: Option<Usage>,
}

impl ModelResponse {
    pub fn new(
        model_request_id: ConversationEventId,
        output_event_ids: Vec<ConversationEventId>,
        outcome: ModelOutcome,
        continuation: ModelContinuation,
        usage: Option<Usage>,
    ) -> Result<Self, InvalidModelResponse> {
        let response = Self {
            model_request_id,
            output_event_ids,
            outcome,
            continuation,
            usage,
        };
        response.ensure_valid()?;
        Ok(response)
    }

    pub fn model_request_id(&self) -> ConversationEventId {
        self.model_request_id
    }

    pub fn output_event_ids(&self) -> &[ConversationEventId] {
        &self.output_event_ids
    }

    pub fn outcome(&self) -> &ModelOutcome {
        &self.outcome
    }

    pub fn continuation(&self) -> ModelContinuation {
        self.continuation
    }

    pub fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    pub fn describe(&self) -> String {
        match &self.outcome {
            ModelOutcome::Succeeded => {
                let continuation = match self.continuation {
                    ModelContinuation::Complete => "complete",
                    ModelContinuation::Continue => "continue",
                };
                format!(
                    "succeeded {continuation} with {} outputs",
                    self.output_event_ids.len()
                )
            }
            ModelOutcome::Failed { failure } => {
                let continuation = match self.continuation {
                    ModelContinuation::Complete => "complete",
                    ModelContinuation::Continue => "continue",
                };
                format!("failed {continuation}: {}", failure.message())
            }
        }
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidModelResponse> {
        if let ModelOutcome::Failed { failure } = &self.outcome {
            failure
                .ensure_valid()
                .map_err(InvalidModelResponse::Failure)?;
        }
        if let Some(usage) = &self.usage {
            usage.ensure_valid().map_err(InvalidModelResponse::Usage)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelContinuation {
    Complete,
    Continue,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelOutcome {
    Succeeded,
    Failed { failure: OperationFailure },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_write_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
}

impl Usage {
    pub fn new(input_tokens: u64, output_tokens: u64) -> Self {
        Self {
            input_tokens: Some(input_tokens),
            output_tokens: Some(output_tokens),
            total_tokens: None,
            cache_read_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_output_tokens: None,
            cost_usd: None,
        }
    }

    pub fn try_new(
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        total_tokens: Option<u64>,
        cache_read_input_tokens: Option<u64>,
        cache_write_input_tokens: Option<u64>,
        reasoning_output_tokens: Option<u64>,
        cost_usd: Option<f64>,
    ) -> Result<Self, InvalidUsage> {
        let usage = Self {
            input_tokens,
            output_tokens,
            total_tokens,
            cache_read_input_tokens,
            cache_write_input_tokens,
            reasoning_output_tokens,
            cost_usd,
        };
        usage.ensure_valid()?;
        Ok(usage)
    }

    pub fn input_tokens(&self) -> Option<u64> {
        self.input_tokens
    }

    pub fn output_tokens(&self) -> Option<u64> {
        self.output_tokens
    }

    pub fn total_tokens(&self) -> Option<u64> {
        self.total_tokens
    }

    pub fn cache_read_input_tokens(&self) -> Option<u64> {
        self.cache_read_input_tokens
    }

    pub fn cache_write_input_tokens(&self) -> Option<u64> {
        self.cache_write_input_tokens
    }

    pub fn reasoning_output_tokens(&self) -> Option<u64> {
        self.reasoning_output_tokens
    }

    pub fn cost_usd(&self) -> Option<f64> {
        self.cost_usd
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidUsage> {
        if self
            .cost_usd
            .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
        {
            return Err(InvalidUsage::CostMustBeNonNegative);
        }
        if let Some(input_tokens) = self.input_tokens
            && (self
                .cache_read_input_tokens
                .is_some_and(|tokens| tokens > input_tokens)
                || self
                    .cache_write_input_tokens
                    .is_some_and(|tokens| tokens > input_tokens)
                || self
                    .cache_read_input_tokens
                    .zip(self.cache_write_input_tokens)
                    .is_some_and(|(cache_read, cache_write)| {
                        cache_read
                            .checked_add(cache_write)
                            .is_none_or(|total| total > input_tokens)
                    }))
        {
            return Err(InvalidUsage::CacheTokensExceedInput);
        }
        if self
            .output_tokens
            .zip(self.reasoning_output_tokens)
            .is_some_and(|(output_tokens, reasoning_tokens)| reasoning_tokens > output_tokens)
        {
            return Err(InvalidUsage::ReasoningTokensExceedOutput);
        }
        if let (Some(input_tokens), Some(output_tokens), Some(total_tokens)) =
            (self.input_tokens, self.output_tokens, self.total_tokens)
        {
            let computed = input_tokens
                .checked_add(output_tokens)
                .ok_or(InvalidUsage::TotalTokensMismatch)?;
            if computed != total_tokens {
                return Err(InvalidUsage::TotalTokensMismatch);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum InvalidUsage {
    CostMustBeNonNegative,
    CacheTokensExceedInput,
    ReasoningTokensExceedOutput,
    TotalTokensMismatch,
}

impl Display for InvalidUsage {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CostMustBeNonNegative => {
                write!(formatter, "usage cost must be finite and nonnegative")
            }
            Self::CacheTokensExceedInput => {
                write!(formatter, "cached input tokens cannot exceed input tokens")
            }
            Self::ReasoningTokensExceedOutput => {
                write!(formatter, "reasoning tokens cannot exceed output tokens")
            }
            Self::TotalTokensMismatch => {
                write!(
                    formatter,
                    "usage total tokens must equal input plus output tokens"
                )
            }
        }
    }
}

impl Error for InvalidUsage {}

#[derive(Debug, Eq, PartialEq)]
pub enum InvalidModelResponse {
    Failure(InvalidOperationFailure),
    Usage(InvalidUsage),
}

impl Display for InvalidModelResponse {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failure(error) => Display::fmt(error, formatter),
            Self::Usage(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidModelResponse {}
