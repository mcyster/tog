use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ConversationCommandId(Uuid);

impl ConversationCommandId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for ConversationCommandId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "conversation_command_{}", self.0.simple())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ConversationTurnId(Uuid);

impl ConversationTurnId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for ConversationTurnId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "conversation_turn_{}", self.0.simple())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ModelInvocationId(Uuid);

impl ModelInvocationId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for ModelInvocationId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "model_invocation_{}", self.0.simple())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ToolCallId(Uuid);

impl ToolCallId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for ToolCallId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "tool_call_{}", self.0.simple())
    }
}
