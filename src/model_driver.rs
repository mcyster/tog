use std::error::Error;
use std::fmt::{Display, Formatter};

use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;

use crate::conversation::{
    Conversation, ConversationRequest, ConversationTurnId, DriverConversationEvent,
    DriverEventReader, ModelSource,
};

pub(crate) type ModelOutputStream =
    BoxStream<'static, Result<DriverConversationEvent, ModelDriverError>>;

pub(crate) struct ModelDriverRequest<'conversation> {
    conversation: &'conversation Conversation,
    pending_user_requests: Vec<ConversationRequest>,
    turn_id: ConversationTurnId,
}

impl<'conversation> ModelDriverRequest<'conversation> {
    pub(crate) fn new(
        conversation: &'conversation Conversation,
        pending_user_requests: Vec<ConversationRequest>,
        turn_id: ConversationTurnId,
    ) -> Self {
        Self {
            conversation,
            pending_user_requests,
            turn_id,
        }
    }

    pub(crate) fn conversation(&self) -> &'conversation Conversation {
        self.conversation
    }

    pub(crate) fn pending_user_requests(&self) -> &[ConversationRequest] {
        &self.pending_user_requests
    }

    pub(crate) fn turn_id(&self) -> ConversationTurnId {
        self.turn_id
    }
}

pub(crate) trait ModelDriver: DriverEventReader {
    fn source(&self) -> &ModelSource;

    fn invoke<'invoke>(
        &'invoke self,
        request: ModelDriverRequest<'invoke>,
    ) -> BoxFuture<'invoke, Result<ModelOutputStream, ModelDriverError>>;
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ModelDriverError {
    WrongTurnIdentity {
        expected: ConversationTurnId,
        actual: ConversationTurnId,
    },
    MissingTurnIdentity,
    DisallowedEventKind {
        event_type: String,
    },
    OutputAfterCompletion {
        event_type: String,
    },
    IncompleteTurn,
}

impl Display for ModelDriverError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongTurnIdentity { expected, actual } => write!(
                formatter,
                "model driver event belonged to turn {actual}, expected turn {expected}"
            ),
            Self::MissingTurnIdentity => {
                write!(formatter, "model driver problem had no turn identity")
            }
            Self::DisallowedEventKind { event_type } => {
                write!(
                    formatter,
                    "model driver emitted disallowed event kind {event_type}"
                )
            }
            Self::OutputAfterCompletion { event_type } => {
                write!(
                    formatter,
                    "model driver emitted {event_type} after turn completion"
                )
            }
            Self::IncompleteTurn => write!(
                formatter,
                "the model driver ended without completing the turn"
            ),
        }
    }
}

impl Error for ModelDriverError {}
