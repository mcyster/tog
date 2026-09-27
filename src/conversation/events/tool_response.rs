use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ConversationEventId, InvalidOperationFailure, OperationFailure};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ToolResponse {
    tool_request_id: ConversationEventId,
    outcome: ToolOutcome,
}

impl ToolResponse {
    pub(crate) fn new(tool_request_id: ConversationEventId, outcome: ToolOutcome) -> Self {
        Self {
            tool_request_id,
            outcome,
        }
    }

    pub(crate) fn tool_request_id(&self) -> ConversationEventId {
        self.tool_request_id
    }

    pub(crate) fn outcome(&self) -> &ToolOutcome {
        &self.outcome
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidToolOutcome> {
        self.outcome.ensure_valid()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ToolOutcome {
    Succeeded { value: Value },
    Failed { failure: OperationFailure },
}

impl ToolOutcome {
    #[cfg(test)]
    pub(crate) fn succeeded(value: Value) -> Self {
        Self::Succeeded { value }
    }

    fn ensure_valid(&self) -> Result<(), InvalidToolOutcome> {
        match self {
            Self::Succeeded { .. } => Ok(()),
            Self::Failed { failure } => failure.ensure_valid().map_err(InvalidToolOutcome::Failure),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidToolOutcome {
    Failure(InvalidOperationFailure),
}

impl Display for InvalidToolOutcome {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failure(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidToolOutcome {}
