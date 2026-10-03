use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::conversation::events::ConversationEventId;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AssistantResponse {
    model_request_id: ConversationEventId,
    content: String,
}

impl AssistantResponse {
    pub fn new(
        model_request_id: ConversationEventId,
        content: String,
    ) -> Result<Self, InvalidAssistantResponse> {
        let response = Self {
            model_request_id,
            content,
        };
        response.ensure_valid()?;
        Ok(response)
    }

    pub fn model_request_id(&self) -> ConversationEventId {
        self.model_request_id
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidAssistantResponse> {
        if self.content.trim().is_empty() {
            return Err(InvalidAssistantResponse::EmptyContent);
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum InvalidAssistantResponse {
    EmptyContent,
}

impl Display for InvalidAssistantResponse {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyContent => write!(formatter, "assistant response content must not be empty"),
        }
    }
}

impl Error for InvalidAssistantResponse {}
