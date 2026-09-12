use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ConversationEventClass;

pub(crate) trait ConversationEventExtension: Send {
    fn class(&self) -> ConversationEventClass;

    fn namespace(&self) -> &str;

    fn namespace_version(&self) -> &str;

    fn event_type(&self) -> &str;

    fn event_schema_version(&self) -> u32;

    fn description(&self) -> &str;

    fn serialize_payload(&self) -> Result<Value, ConversationEventError>;

    fn to_envelope(&self) -> Result<ConversationEventEnvelope, ConversationEventError> {
        ConversationEventEnvelope::new(
            self.class(),
            self.namespace().to_owned(),
            self.namespace_version().to_owned(),
            self.event_type().to_owned(),
            self.event_schema_version(),
            self.description().to_owned(),
            self.serialize_payload()?,
        )
        .map_err(ConversationEventError::InvalidEnvelope)
    }
}

#[allow(dead_code)]
pub(crate) trait ConversationEventReader {
    fn read_event(
        &self,
        envelope: &ConversationEventEnvelope,
    ) -> Result<Box<dyn ConversationEventExtension>, ConversationEventReadError>;
}

#[allow(dead_code)]
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ConversationEventReadError {
    UnsupportedNamespace,
    UnsupportedEvent,
    InvalidPayload(String),
}

impl Display for ConversationEventReadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedNamespace => {
                write!(formatter, "the namespace does not own this event")
            }
            Self::UnsupportedEvent => {
                write!(formatter, "the namespace does not own this event type")
            }
            Self::InvalidPayload(message) => {
                write!(formatter, "invalid conversation event payload: {message}")
            }
        }
    }
}

impl Error for ConversationEventReadError {}

#[derive(Debug)]
pub(crate) enum ConversationEventError {
    InvalidEnvelope(InvalidConversationEventEnvelope),
    Serialization(String),
}

impl Display for ConversationEventError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEnvelope(error) => Display::fmt(error, formatter),
            Self::Serialization(message) => write!(
                formatter,
                "conversation event serialization failed: {message}"
            ),
        }
    }
}

impl Error for ConversationEventError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ConversationEventEnvelope {
    class: ConversationEventClass,
    namespace: String,
    namespace_version: String,
    event_type: String,
    event_schema_version: u32,
    description: String,
    payload: Value,
}

#[allow(dead_code)]
impl ConversationEventEnvelope {
    pub(crate) fn new(
        class: ConversationEventClass,
        namespace: String,
        namespace_version: String,
        event_type: String,
        event_schema_version: u32,
        description: String,
        payload: Value,
    ) -> Result<Self, InvalidConversationEventEnvelope> {
        let event = Self {
            class,
            namespace,
            namespace_version,
            event_type,
            event_schema_version,
            description,
            payload,
        };
        event.ensure_valid()?;
        Ok(event)
    }

    pub(crate) fn namespace(&self) -> &str {
        &self.namespace
    }

    pub(crate) fn class(&self) -> ConversationEventClass {
        self.class
    }

    pub(crate) fn namespace_version(&self) -> &str {
        &self.namespace_version
    }

    pub(crate) fn event_type(&self) -> &str {
        &self.event_type
    }

    pub(crate) fn event_schema_version(&self) -> u32 {
        self.event_schema_version
    }

    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    pub(crate) fn payload(&self) -> &Value {
        &self.payload
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidConversationEventEnvelope> {
        if self.namespace.trim().is_empty() {
            return Err(InvalidConversationEventEnvelope::Namespace);
        }
        if self.namespace_version.trim().is_empty() {
            return Err(InvalidConversationEventEnvelope::NamespaceVersion);
        }
        if self.event_type.trim().is_empty() {
            return Err(InvalidConversationEventEnvelope::EventType);
        }
        if self.description.trim().is_empty() {
            return Err(InvalidConversationEventEnvelope::Description);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum InvalidConversationEventEnvelope {
    Namespace,
    NamespaceVersion,
    EventType,
    Description,
}

impl Display for InvalidConversationEventEnvelope {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Namespace => "namespace must not be empty",
            Self::NamespaceVersion => "namespace version must not be empty",
            Self::EventType => "conversation event type must not be empty",
            Self::Description => "conversation event description must not be empty",
        };
        write!(formatter, "{message}")
    }
}

impl Error for InvalidConversationEventEnvelope {}
