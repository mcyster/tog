use std::error::Error;
use std::fmt::{Display, Formatter};

use super::{Conversation, ConversationId};
use crate::conversation_event::ConversationEventRecord;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConversationHistory {
    id: ConversationId,
    events: Vec<ConversationEventRecord>,
}

impl ConversationHistory {
    pub(crate) fn from_events(
        events: Vec<ConversationEventRecord>,
    ) -> Result<Self, InvalidConversation> {
        let conversation_id = events
            .first()
            .map(|event| event.conversation_id)
            .ok_or(InvalidConversation::Empty)?;

        let mut previous_position = None;
        for event in &events {
            if event.conversation_id != conversation_id {
                return Err(InvalidConversation::MixedConversationIds {
                    expected: conversation_id,
                    found: event.conversation_id,
                });
            }

            if let Some(previous_position) = previous_position
                && event.position <= previous_position
            {
                return Err(InvalidConversation::InvalidPosition {
                    expected: previous_position
                        .checked_add(1)
                        .ok_or(InvalidConversation::TooManyEvents)?,
                    found: event.position,
                });
            }
            previous_position = Some(event.position);

            event
                .ensure_valid()
                .map_err(|error| InvalidConversation::InvalidEvent {
                    position: event.position,
                    reason: error.to_string(),
                })?;
        }

        Ok(Self {
            id: conversation_id,
            events,
        })
    }
}

impl Conversation for ConversationHistory {
    fn id(&self) -> ConversationId {
        self.id
    }

    fn events(&self) -> &[ConversationEventRecord] {
        &self.events
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidConversation {
    Empty,
    MixedConversationIds {
        expected: ConversationId,
        found: ConversationId,
    },
    InvalidPosition {
        expected: u64,
        found: u64,
    },
    InvalidEvent {
        position: u64,
        reason: String,
    },
    TooManyEvents,
}

impl Display for InvalidConversation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(formatter, "a conversation must contain at least one event"),
            Self::MixedConversationIds { expected, found } => write!(
                formatter,
                "conversation event belongs to {found}, expected {expected}"
            ),
            Self::InvalidPosition { expected, found } => {
                write!(
                    formatter,
                    "expected conversation event position {expected}, found {found}"
                )
            }
            Self::InvalidEvent { position, reason } => {
                write!(
                    formatter,
                    "invalid conversation event at position {position}: {reason}"
                )
            }
            Self::TooManyEvents => write!(formatter, "conversation contains too many events"),
        }
    }
}

impl Error for InvalidConversation {}
