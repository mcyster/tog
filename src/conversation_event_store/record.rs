use super::ConversationEventRecord;
use crate::conversation::ConversationEvent;
use crate::conversation::InvalidConversationEvent;

const SCHEMA_VERSION: u32 = 13;

impl ConversationEventRecord {
    pub(crate) fn new(position: u64, event: ConversationEvent) -> Self {
        Self {
            position,
            schema_version: SCHEMA_VERSION,
            event,
        }
    }

    pub(crate) fn event(&self) -> &ConversationEvent {
        &self.event
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidConversationEvent> {
        self.event.ensure_valid()
    }
}
