use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{
    ConversationEventClass, ConversationEventKind, DriverEventEnvelope,
    InvalidConversationEventKind,
};
use crate::conversation::{ConversationEventId, ConversationId};

pub(crate) const SCHEMA_VERSION: u32 = 11;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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

    pub(crate) fn new_driver(
        conversation_id: ConversationId,
        position: u64,
        event: DriverEventEnvelope,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub(crate) enum StoredConversationEventKind {
    Shared(ConversationEventKind),
    Extension(DriverEventEnvelope),
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
                .map_err(InvalidConversationEventKind::DriverEvent),
        }
    }
}
