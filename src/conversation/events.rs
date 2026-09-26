mod automation;
mod context;
mod data;
mod failure;
mod model;
mod record;
#[cfg(test)]
mod tests;
mod tool_response;
mod turn_end;
mod turn_start;
mod user;

pub(crate) use automation::{Automation, InvalidAutomation};
pub(crate) use context::{Context, InvalidContext};
pub(crate) use data::{Data, InvalidData};
pub(crate) use failure::{FailureCategory, InvalidOperationFailure, OperationFailure};
pub(crate) use model::{
    AssistantResponse, InvalidAssistantResponse, InvalidModelRequest, InvalidModelResponse,
    InvalidModelSpecificEvent, InvalidToolRequest, ModelData, ModelEvent, ModelId, ModelOutcome,
    ModelRequest, ModelResponse, ModelSource, ModelSpecificEvent, ProviderId, ToolDefinition,
    ToolName, ToolRequest, Usage,
};
pub(crate) use tool_response::{InvalidToolOutcome, ToolOutcome, ToolResponse};
pub(crate) use turn_end::{InvalidTurnOutcome, TurnEnd, TurnOutcome};
pub(crate) use turn_start::TurnStart;
pub(crate) use user::{InvalidUser, User, UserContent};

use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::conversation::ConversationId;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ConversationEventId(Uuid);

impl ConversationEventId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for ConversationEventId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "conversation_event_{}", self.0.simple())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ConversationEvent {
    User(User),
    TurnStart(TurnStart),
    TurnEnd(TurnEnd),
    AssistantResponse(AssistantResponse),
    ToolRequest(ToolRequest),
    ToolResponse(ToolResponse),
    ModelRequest(ModelRequest),
    ModelResponse(ModelResponse),
    ModelSpecificEvent(ModelSpecificEvent),
    Automation(Automation),
    Context(Context),
    Data(Data),
}

impl ConversationEvent {
    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidConversationEvent> {
        match self {
            Self::User(event) => event.ensure_valid().map_err(InvalidConversationEvent::User),
            Self::TurnStart(event) => event.ensure_valid(),
            Self::TurnEnd(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::TurnEnd),
            Self::AssistantResponse(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::AssistantResponse),
            Self::ToolRequest(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::ToolRequest),
            Self::ToolResponse(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::ToolResponse),
            Self::ModelRequest(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::ModelRequest),
            Self::ModelResponse(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::ModelResponse),
            Self::ModelSpecificEvent(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::ModelSpecificEvent),
            Self::Automation(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::Automation),
            Self::Context(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::Context),
            Self::Data(event) => event.ensure_valid().map_err(InvalidConversationEvent::Data),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ConversationEventRecord {
    pub(crate) conversation_id: ConversationId,
    pub(crate) position: u64,
    pub(crate) id: ConversationEventId,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) timestamp: time::OffsetDateTime,
    pub(crate) schema_version: u32,
    #[serde(flatten)]
    pub(crate) event: ConversationEvent,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidConversationEvent {
    User(InvalidUser),
    TurnEnd(InvalidTurnOutcome),
    AssistantResponse(InvalidAssistantResponse),
    ToolRequest(InvalidToolRequest),
    ToolResponse(InvalidToolOutcome),
    ModelRequest(InvalidModelRequest),
    ModelResponse(InvalidModelResponse),
    ModelSpecificEvent(InvalidModelSpecificEvent),
    Automation(InvalidAutomation),
    Context(InvalidContext),
    Data(InvalidData),
}

impl Display for InvalidConversationEvent {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User(error) => Display::fmt(error, formatter),
            Self::TurnEnd(error) => Display::fmt(error, formatter),
            Self::AssistantResponse(error) => Display::fmt(error, formatter),
            Self::ToolRequest(error) => Display::fmt(error, formatter),
            Self::ToolResponse(error) => Display::fmt(error, formatter),
            Self::ModelRequest(error) => Display::fmt(error, formatter),
            Self::ModelResponse(error) => Display::fmt(error, formatter),
            Self::ModelSpecificEvent(error) => Display::fmt(error, formatter),
            Self::Automation(error) => Display::fmt(error, formatter),
            Self::Context(error) => Display::fmt(error, formatter),
            Self::Data(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidConversationEvent {}
