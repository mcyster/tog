use serde::{Deserialize, Serialize};

use super::ConversationEventId;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TurnStart {
    user_id: Option<ConversationEventId>,
    input_through: u64,
}

impl TurnStart {
    pub fn new(user_id: Option<ConversationEventId>, input_through: u64) -> Self {
        Self {
            user_id,
            input_through,
        }
    }

    pub fn user_id(&self) -> Option<ConversationEventId> {
        self.user_id
    }

    pub fn input_through(&self) -> u64 {
        self.input_through
    }

    pub(super) fn ensure_valid(&self) -> Result<(), super::InvalidConversationEvent> {
        Ok(())
    }
}
