use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::conversation::events::{ConversationEventId, InvalidOperationFailure, OperationFailure};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ModelResponse {
    model_request_id: ConversationEventId,
    output_event_ids: Vec<ConversationEventId>,
    outcome: ModelOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage: Option<Usage>,
}

impl ModelResponse {
    pub(crate) fn new(
        model_request_id: ConversationEventId,
        output_event_ids: Vec<ConversationEventId>,
        outcome: ModelOutcome,
        usage: Option<Usage>,
    ) -> Result<Self, InvalidModelResponse> {
        let response = Self {
            model_request_id,
            output_event_ids,
            outcome,
            usage,
        };
        response.ensure_valid()?;
        Ok(response)
    }

    pub(crate) fn model_request_id(&self) -> ConversationEventId {
        self.model_request_id
    }

    pub(crate) fn output_event_ids(&self) -> &[ConversationEventId] {
        &self.output_event_ids
    }

    pub(crate) fn outcome(&self) -> &ModelOutcome {
        &self.outcome
    }

    pub(crate) fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidModelResponse> {
        if let ModelOutcome::Failed { failure } = &self.outcome {
            failure
                .ensure_valid()
                .map_err(InvalidModelResponse::Failure)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ModelOutcome {
    Succeeded,
    Failed { failure: OperationFailure },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

impl Usage {
    pub(crate) fn new(input_tokens: u64, output_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidModelResponse {
    Failure(InvalidOperationFailure),
}

impl Display for InvalidModelResponse {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failure(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidModelResponse {}
