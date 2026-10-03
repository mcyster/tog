use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    InvalidArguments,
    UnknownTool,
    TimedOut,
    ExecutionFailed,
    Refusal,
    ContextLimitExceeded,
    Authentication,
    RateLimited,
    InvalidRequest,
    InvalidProviderResponse,
    ProviderFailure,
    Transport,
    StreamInterrupted,
}

impl FailureCategory {
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited
                | Self::Transport
                | Self::ProviderFailure
                | Self::StreamInterrupted
                | Self::TimedOut
        )
    }

    #[allow(dead_code)]
    pub(crate) fn execution_known(self) -> bool {
        !matches!(
            self,
            Self::Transport | Self::StreamInterrupted | Self::TimedOut
        )
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OperationFailure {
    category: FailureCategory,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

impl OperationFailure {
    pub fn try_new(
        category: FailureCategory,
        message: String,
        details: Option<Value>,
    ) -> Result<Self, InvalidOperationFailure> {
        let failure = Self {
            category,
            message,
            details,
        };
        failure.ensure_valid()?;
        Ok(failure)
    }

    pub fn category(&self) -> FailureCategory {
        self.category
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn details(&self) -> Option<&Value> {
        self.details.as_ref()
    }

    pub fn retryable(&self) -> bool {
        self.category.retryable()
    }

    #[allow(dead_code)]
    pub(crate) fn execution_known(&self) -> bool {
        self.category.execution_known()
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidOperationFailure> {
        if self.message.trim().is_empty() {
            return Err(InvalidOperationFailure::EmptyMessage);
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum InvalidOperationFailure {
    EmptyMessage,
}

impl Display for InvalidOperationFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMessage => write!(formatter, "operation failure message must not be empty"),
        }
    }
}

impl Error for InvalidOperationFailure {}
