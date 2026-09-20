use time::OffsetDateTime;

use super::{
    ConversationEventClass, ConversationEventEnvelope, ConversationEventId, ConversationEventKind,
    ConversationEventRecord, InvalidConversationEventKind, StoredConversationEventKind,
};
use crate::conversation::ConversationId;

const SCHEMA_VERSION: u32 = 13;

impl ConversationEventRecord {
    #[allow(dead_code)]
    pub(crate) fn class(&self) -> ConversationEventClass {
        self.kind.class()
    }

    pub(crate) fn new(
        conversation_id: ConversationId,
        position: u64,
        kind: ConversationEventKind,
    ) -> Self {
        Self {
            conversation_id,
            position,
            id: ConversationEventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            schema_version: SCHEMA_VERSION,
            kind: StoredConversationEventKind::Shared(kind),
        }
    }

    pub(crate) fn new_extension(
        conversation_id: ConversationId,
        position: u64,
        event: ConversationEventEnvelope,
    ) -> Self {
        Self {
            conversation_id,
            position,
            id: ConversationEventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            schema_version: SCHEMA_VERSION,
            kind: StoredConversationEventKind::Extension(event),
        }
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidConversationEventKind> {
        self.kind.ensure_valid()
    }
}

impl StoredConversationEventKind {
    #[allow(dead_code)]
    pub(crate) fn class(&self) -> ConversationEventClass {
        match self {
            Self::Shared(event) => event.class(),
            Self::Extension(event) => event.class(),
        }
    }

    pub(super) fn ensure_valid(&self) -> Result<(), InvalidConversationEventKind> {
        match self {
            Self::Shared(event) => event.ensure_valid(),
            Self::Extension(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEventKind::Envelope),
        }
    }
}
