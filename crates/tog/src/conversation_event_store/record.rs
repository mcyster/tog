use super::ConversationEventRecord;
use crate::conversation::ConversationEvent;
use crate::conversation::InvalidConversationEvent;

const SCHEMA_VERSION: u32 = 14;

impl ConversationEventRecord {
    pub fn new(position: u64, event: ConversationEvent) -> Self {
        Self {
            position,
            schema_version: SCHEMA_VERSION,
            event,
        }
    }

    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn event(&self) -> &ConversationEvent {
        &self.event
    }

    pub fn ensure_valid(&self) -> Result<(), InvalidConversationEvent> {
        self.event.ensure_valid()
    }
}
