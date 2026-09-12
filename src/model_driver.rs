use std::error::Error;
use std::fmt::{Display, Formatter};

use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;

use crate::conversation::{
    Conversation, ConversationCommandId, ConversationEventExtension, ConversationEventReader,
    ConversationMessage, ConversationTurnId, ModelSource, UserMessageRequest,
};

pub(crate) type ModelOutputStream = BoxStream<'static, Result<ModelDriverOutput, ModelDriverError>>;

pub(crate) enum ModelDriverOutput {
    Message(ConversationMessage),
    Command(Box<dyn ConversationEventExtension>),
    #[allow(dead_code)]
    Extension(Box<dyn ConversationEventExtension>),
}

pub(crate) struct TurnInput<'conversation> {
    conversation: &'conversation Conversation,
    pending_user_requests: Vec<UserMessageRequest>,
    turn_id: ConversationTurnId,
}

impl<'conversation> TurnInput<'conversation> {
    pub(crate) fn new(
        conversation: &'conversation Conversation,
        turn_id: ConversationTurnId,
    ) -> Self {
        Self {
            conversation,
            pending_user_requests: conversation.pending_user_requests(),
            turn_id,
        }
    }

    pub(crate) fn conversation(&self) -> &'conversation Conversation {
        self.conversation
    }

    pub(crate) fn pending_user_requests(&self) -> &[UserMessageRequest] {
        &self.pending_user_requests
    }

    pub(crate) fn turn_id(&self) -> ConversationTurnId {
        self.turn_id
    }
}

pub(crate) trait ModelDriver: ConversationEventReader {
    fn source(&self) -> &ModelSource;

    fn invoke<'invoke>(
        &'invoke self,
        input: TurnInput<'invoke>,
    ) -> BoxFuture<'invoke, Result<ModelOutputStream, ModelDriverError>>;
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ModelDriverError {
    UnassociatedUserMessage,
    UnexpectedUserRequest { command_id: ConversationCommandId },
    IncompleteTurn,
}

impl Display for ModelDriverError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnassociatedUserMessage => write!(
                formatter,
                "model driver emitted a user message without a request association"
            ),
            Self::UnexpectedUserRequest { command_id } => write!(
                formatter,
                "model driver accepted unexpected user request {command_id}"
            ),
            Self::IncompleteTurn => write!(
                formatter,
                "the model driver ended without an assistant response or problem"
            ),
        }
    }
}

impl Error for ModelDriverError {}
