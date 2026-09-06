use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ConversationEventClass;

pub(crate) trait ConversationEventExtension: Send {
    fn class(&self) -> ConversationEventClass;

    fn driver_name(&self) -> &str;

    fn driver_version(&self) -> &str;

    fn event_type(&self) -> &str;

    fn event_schema_version(&self) -> u32;

    fn description(&self) -> &str;

    fn serialize_payload(&self) -> Result<Value, ConversationEventError>;

    fn to_envelope(&self) -> Result<DriverEventEnvelope, ConversationEventError> {
        DriverEventEnvelope::new(
            self.class(),
            self.driver_name().to_owned(),
            self.driver_version().to_owned(),
            self.event_type().to_owned(),
            self.event_schema_version(),
            self.description().to_owned(),
            self.serialize_payload()?,
        )
        .map_err(ConversationEventError::InvalidEnvelope)
    }
}

#[allow(dead_code)]
pub(crate) trait DriverEventReader {
    fn read_event(
        &self,
        envelope: &DriverEventEnvelope,
    ) -> Result<Box<dyn ConversationEventExtension>, DriverEventReadError>;
}

#[allow(dead_code)]
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum DriverEventReadError {
    UnsupportedDriver,
    UnsupportedEvent,
    InvalidPayload(String),
}

impl Display for DriverEventReadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedDriver => write!(formatter, "the driver does not own this event"),
            Self::UnsupportedEvent => write!(formatter, "the driver does not own this event type"),
            Self::InvalidPayload(message) => {
                write!(formatter, "invalid driver event payload: {message}")
            }
        }
    }
}

impl Error for DriverEventReadError {}

#[derive(Debug)]
pub(crate) enum ConversationEventError {
    InvalidEnvelope(InvalidDriverEventEnvelope),
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
pub(crate) struct DriverEventEnvelope {
    class: ConversationEventClass,
    driver: String,
    driver_version: String,
    event_type: String,
    event_schema_version: u32,
    description: String,
    payload: Value,
}

#[allow(dead_code)]
impl DriverEventEnvelope {
    pub(crate) fn new(
        class: ConversationEventClass,
        driver: String,
        driver_version: String,
        event_type: String,
        event_schema_version: u32,
        description: String,
        payload: Value,
    ) -> Result<Self, InvalidDriverEventEnvelope> {
        let event = Self {
            class,
            driver,
            driver_version,
            event_type,
            event_schema_version,
            description,
            payload,
        };
        event.ensure_valid()?;
        Ok(event)
    }

    pub(crate) fn driver(&self) -> &str {
        &self.driver
    }

    pub(crate) fn class(&self) -> ConversationEventClass {
        self.class
    }

    pub(crate) fn driver_version(&self) -> &str {
        &self.driver_version
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

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidDriverEventEnvelope> {
        if self.driver.trim().is_empty() {
            return Err(InvalidDriverEventEnvelope::DriverName);
        }
        if self.driver_version.trim().is_empty() {
            return Err(InvalidDriverEventEnvelope::DriverVersion);
        }
        if self.event_type.trim().is_empty() {
            return Err(InvalidDriverEventEnvelope::EventType);
        }
        if self.description.trim().is_empty() {
            return Err(InvalidDriverEventEnvelope::Description);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum InvalidDriverEventEnvelope {
    DriverName,
    DriverVersion,
    EventType,
    Description,
}

impl Display for InvalidDriverEventEnvelope {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::DriverName => "driver name must not be empty",
            Self::DriverVersion => "driver version must not be empty",
            Self::EventType => "driver event type must not be empty",
            Self::Description => "driver event description must not be empty",
        };
        write!(formatter, "{message}")
    }
}

impl Error for InvalidDriverEventEnvelope {}
