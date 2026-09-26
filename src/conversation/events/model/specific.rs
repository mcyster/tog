use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::conversation::events::ConversationEventId;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ModelSpecificEvent {
    model_request_id: ConversationEventId,
    provider_event_type: String,
    provider_payload_version: u32,
    payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl ModelSpecificEvent {
    pub(crate) fn new(
        model_request_id: ConversationEventId,
        provider_event_type: String,
        provider_payload_version: u32,
        payload: Value,
        message: Option<String>,
    ) -> Result<Self, InvalidModelSpecificEvent> {
        let event = Self {
            model_request_id,
            provider_event_type,
            provider_payload_version,
            payload,
            message,
        };
        event.ensure_valid()?;
        Ok(event)
    }

    pub(crate) fn model_request_id(&self) -> ConversationEventId {
        self.model_request_id
    }

    #[allow(dead_code)]
    pub(crate) fn provider_event_type(&self) -> &str {
        &self.provider_event_type
    }

    #[allow(dead_code)]
    pub(crate) fn provider_payload_version(&self) -> u32 {
        self.provider_payload_version
    }

    #[allow(dead_code)]
    pub(crate) fn payload(&self) -> &Value {
        &self.payload
    }

    pub(crate) fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidModelSpecificEvent> {
        if self.provider_event_type.trim().is_empty() {
            return Err(InvalidModelSpecificEvent::EmptyProviderEventType);
        }
        if let Some(message) = &self.message
            && message.trim().is_empty()
        {
            return Err(InvalidModelSpecificEvent::EmptyMessage);
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidModelSpecificEvent {
    EmptyProviderEventType,
    EmptyMessage,
}

impl Display for InvalidModelSpecificEvent {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyProviderEventType => {
                write!(formatter, "provider event type must not be empty")
            }
            Self::EmptyMessage => {
                write!(formatter, "model specific event message must not be empty")
            }
        }
    }
}

impl Error for InvalidModelSpecificEvent {}
