mod automation;
mod context;
mod error;
mod event;
mod failure;
mod model;
#[cfg(test)]
mod tests;
mod tool_response;
mod tools;
mod turn_end;
mod turn_start;
mod user;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::conversation::ConversationId;

pub use automation::{Automation, InvalidAutomation};
pub use context::{Context, InvalidContext};
pub use failure::{FailureCategory, InvalidOperationFailure, OperationFailure};
pub use model::{
    AssistantResponse, InvalidAssistantResponse, InvalidModelRequest, InvalidModelResponse,
    InvalidModelSpecificEvent, InvalidToolRequest, ModelData, ModelEvent, ModelId, ModelOutcome,
    ModelRequest, ModelResponse, ModelSource, ModelSpecificEvent, ProviderId, ToolRequest, Usage,
};
pub use tool_response::{InvalidToolOutcome, ToolOutcome, ToolResponse};
pub use tools::{InvalidTools, Tool, ToolAvailability, ToolDefinition, ToolName, Tools};
pub use turn_end::{InvalidTurnOutcome, TurnEnd, TurnOutcome};
pub use turn_start::TurnStart;
pub use user::{InvalidUser, User, UserContent};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ConversationEventId(Uuid);

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ConversationEvent {
    id: ConversationEventId,
    conversation_id: ConversationId,
    #[serde(with = "time::serde::rfc3339")]
    timestamp: OffsetDateTime,
    #[serde(flatten)]
    payload: ConversationEventPayload,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConversationEventPayload {
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
    Tools(Tools),
}

#[derive(Debug, Eq, PartialEq)]
pub enum InvalidConversationEvent {
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
    Tools(InvalidTools),
}
