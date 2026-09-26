use time::OffsetDateTime;

use super::{ConversationEvent, ConversationEventId, ConversationEventRecord};

use crate::conversation::ConversationId;

const SCHEMA_VERSION: u32 = 13;

impl ConversationEventRecord {
    pub(crate) fn new(
        conversation_id: ConversationId,
        position: u64,
        event: ConversationEvent,
    ) -> Self {
        Self {
            conversation_id,
            position,
            id: ConversationEventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            schema_version: SCHEMA_VERSION,
            event,
        }
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), super::InvalidConversationEvent> {
        self.event.ensure_valid()
    }
}
