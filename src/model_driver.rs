use std::error::Error;
use std::fmt::{Display, Formatter};

use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;

use crate::conversation::{
    Conversation, ConversationEventKind, ConversationTurnId, DriverEventEnvelope, ModelSource,
};

pub(crate) type ModelOutputStream = BoxStream<'static, Result<ModelDriverOutput, ModelDriverError>>;

pub(crate) enum ModelDriverOutput {
    Event(ConversationEventKind),
    Driver(Box<dyn DriverEvent>),
}

pub(crate) trait DriverEvent: Send {
    fn to_envelope(&self) -> Result<DriverEventEnvelope, ModelDriverError>;
}

#[allow(dead_code)]
pub(crate) trait DriverEventDecoder {
    fn decode_event(
        &self,
        envelope: &DriverEventEnvelope,
    ) -> Result<Box<dyn DriverEvent>, DriverEventDecodeError>;
}

pub(crate) trait ModelDriver: DriverEventDecoder {
    fn source(&self) -> &ModelSource;

    fn invoke<'invoke>(
        &'invoke self,
        conversation: &'invoke Conversation,
        turn_id: ConversationTurnId,
    ) -> BoxFuture<'invoke, Result<ModelOutputStream, ModelDriverError>>;
}

#[allow(dead_code)]
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum DriverEventDecodeError {
    UnsupportedDriver,
    UnsupportedEvent,
    InvalidPayload(String),
}

impl Display for DriverEventDecodeError {
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

impl Error for DriverEventDecodeError {}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ModelDriverError {
    Authentication(String),
    RateLimited(String),
    Transport(String),
    InvalidRequest(String),
    InvalidResponse(String),
    StreamInterrupted(String),
    Provider(String),
    IncompleteTurn,
}

impl Display for ModelDriverError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Authentication(message) => write!(formatter, "authentication failed: {message}"),
            Self::RateLimited(message) => write!(formatter, "rate limited: {message}"),
            Self::Transport(message) => write!(formatter, "model transport failed: {message}"),
            Self::InvalidRequest(message) => write!(formatter, "invalid model request: {message}"),
            Self::InvalidResponse(message) => {
                write!(formatter, "invalid model response: {message}")
            }
            Self::StreamInterrupted(message) => {
                write!(
                    formatter,
                    "model response stream was interrupted: {message}"
                )
            }
            Self::Provider(message) => write!(formatter, "model provider failed: {message}"),
            Self::IncompleteTurn => write!(
                formatter,
                "the model driver ended without completing the turn"
            ),
        }
    }
}

impl Error for ModelDriverError {}
