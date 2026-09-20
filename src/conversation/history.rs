use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

use super::{Conversation, ConversationId};
use crate::conversation_event::{
    ConversationCommand, ConversationEventKind, ConversationEventRecord, ConversationFact,
    ConversationMessage, StoredConversationEventKind, ToolDefinition, UserMessageRequest,
};

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

    fn pending_user_requests(&self) -> Vec<UserMessageRequest> {
        let mut requests = Vec::new();
        let mut accepted_request_ids = HashSet::new();
        for event in &self.events {
            let StoredConversationEventKind::Shared(kind) = &event.kind else {
                continue;
            };
            match kind {
                ConversationEventKind::Command(ConversationCommand::UserMessageRequested(
                    request,
                )) => requests.push(request.clone()),
                ConversationEventKind::Fact(ConversationFact::Message {
                    message:
                        ConversationMessage::User {
                            caused_by: Some(command_id),
                            ..
                        },
                    ..
                }) => {
                    accepted_request_ids.insert(*command_id);
                }
                _ => {}
            }
        }
        requests
            .into_iter()
            .filter(|request| !accepted_request_ids.contains(&request.command_id))
            .collect()
    }

    fn available_tools(&self) -> &[ToolDefinition] {
        self.events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                StoredConversationEventKind::Shared(ConversationEventKind::Fact(
                    ConversationFact::ToolsAvailable { tools },
                )) => Some(tools.as_slice()),
                _ => None,
            })
            .unwrap_or(&[])
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
