mod event;
mod extension;
mod id;
mod message;
mod model;
mod model_data;
mod problem;
mod record;
#[cfg(test)]
mod tests;
mod tool;

pub(crate) use extension::{
    ConversationEventEnvelope, ConversationEventError, ConversationEventExtension,
    ConversationEventReadError, ConversationEventReader, InvalidConversationEventEnvelope,
};
pub(crate) use id::{
    ConversationCommandId, ConversationEventId, ConversationTurnId, ModelInvocationId, ToolCallId,
};
pub(crate) use model::{ModelId, ModelSource, ProviderId};
pub(crate) use model_data::{InvalidModelData, ModelData};
pub(crate) use problem::{
    ConversationProblem, InvalidConversationProblem, InvocationError, ModelIssue,
};
#[allow(unused_imports)]
pub(crate) use tool::{
    InvalidToolData, ToolDefinition, ToolExecutionProblem, ToolExecutionProblemKind, ToolName,
    ToolOutcome, ToolRequest, ToolResponse,
};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::conversation::ConversationId;

pub(crate) enum ConversationEvent {
    Command(ConversationCommand),
    Fact(ConversationFact),
    Extension(Box<dyn ConversationEventExtension>),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConversationEventClass {
    Command,
    Fact,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct UserMessageRequest {
    pub(crate) command_id: ConversationCommandId,
    pub(crate) content: Vec<UserContent>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "class", content = "event", rename_all = "snake_case")]
pub(crate) enum ConversationEventKind {
    Command(ConversationCommand),
    Fact(ConversationFact),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ConversationCommand {
    UserMessageRequested(UserMessageRequest),
    TurnRequested {
        command_id: ConversationCommandId,
        turn_id: ConversationTurnId,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ConversationMessage {
    User {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caused_by: Option<ConversationCommandId>,
        content: Vec<UserContent>,
    },
    #[serde(rename = "assistant")]
    AssistantResponse {
        invocation_id: ModelInvocationId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<ModelData>,
        response: AssistantResponse,
    },
    Communication {
        invocation_id: ModelInvocationId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<ModelData>,
        communication: ModelCommunication,
    },
    Problem {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation_id: Option<ModelInvocationId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<ModelData>,
        problem: ConversationProblem,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub(crate) enum ConversationFact {
    Message {
        #[serde(flatten)]
        message: ConversationMessage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<ConversationTurnId>,
    },
    Lifecycle(ConversationLifecycle),
    ToolsAvailable {
        tools: Vec<ToolDefinition>,
    },
    ToolRequest {
        #[serde(flatten)]
        request: ToolRequest,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<ConversationTurnId>,
    },
    ToolResponse {
        #[serde(flatten)]
        response: ToolResponse,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<ConversationTurnId>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ConversationLifecycle {
    TurnCompleted {
        turn_id: ConversationTurnId,
        outcome: TurnOutcome,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TurnOutcome {
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ConversationEventRecord {
    pub(crate) conversation_id: ConversationId,
    pub(crate) position: u64,
    pub(crate) id: ConversationEventId,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) timestamp: OffsetDateTime,
    pub(crate) schema_version: u32,
    #[serde(flatten)]
    pub(crate) kind: StoredConversationEventKind,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub(crate) enum StoredConversationEventKind {
    Shared(ConversationEventKind),
    Extension(ConversationEventEnvelope),
}

#[derive(Debug)]
pub(crate) enum InvalidConversationEventKind {
    Assistant(InvalidAssistantResponse),
    ModelCommunication(InvalidModelCommunication),
    ConversationProblem(InvalidConversationProblem),
    ModelData(InvalidModelData),
    Tool(InvalidToolData),
    Envelope(InvalidConversationEventEnvelope),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ModelEvent {
    Assistant(AssistantResponse),
    Communication(ModelCommunication),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AssistantResponse {
    message: String,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidAssistantResponse {
    EmptyMessage,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ModelCommunication {
    message: String,
    importance: ModelEventImportance,
    subtype: String,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidModelCommunication {
    EmptyMessage,
    EmptySubtype,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ModelEventImportance {
    Detailed,
    Interesting,
    Important,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub(crate) enum UserContent {
    Text(String),
}
