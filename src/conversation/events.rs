mod automation;
mod context;
mod data;
mod error;
mod event;
mod failure;
mod model;
mod record;
#[cfg(test)]
mod tests;
mod tool_response;
mod turn_end;
mod turn_start;
mod user;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::conversation::ConversationId;

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ConversationEventId(Uuid);

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
