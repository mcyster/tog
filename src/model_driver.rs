use std::error::Error;
use std::fmt::{Display, Formatter};

use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;

use crate::conversation::{
    Conversation, ConversationCommandId, ConversationEventExtension, ConversationEventReader,
    ConversationMessage, ConversationTurnId, ModelSource, ToolRequest, UserMessageRequest,
};

pub(crate) type ModelOutputStream =
    BoxStream<'static, Result<ModelDriverOutputBatch, ModelDriverError>>;

pub(crate) enum ModelDriverOutput {
    Message(ConversationMessage),
    ToolRequest(ToolRequest),
    Command(Box<dyn ConversationEventExtension>),
    #[allow(dead_code)]
    Extension(Box<dyn ConversationEventExtension>),
}

pub(crate) struct ModelDriverOutputBatch {
    outputs: Vec<ModelDriverOutput>,
}

impl ModelDriverOutputBatch {
    pub(crate) fn try_new(
        outputs: Vec<ModelDriverOutput>,
    ) -> Result<Self, EmptyModelDriverOutputBatch> {
        if outputs.is_empty() {
            return Err(EmptyModelDriverOutputBatch);
        }
        Ok(Self { outputs })
    }

    pub(crate) fn into_outputs(self) -> Vec<ModelDriverOutput> {
        self.outputs
    }
}

impl From<ModelDriverOutput> for ModelDriverOutputBatch {
    fn from(output: ModelDriverOutput) -> Self {
        Self {
            outputs: vec![output],
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct EmptyModelDriverOutputBatch;

impl Display for EmptyModelDriverOutputBatch {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "a model driver output batch must not be empty")
    }
}

impl Error for EmptyModelDriverOutputBatch {}

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

#[cfg(test)]
mod tests {
    use super::{EmptyModelDriverOutputBatch, ModelDriverOutputBatch};

    #[test]
    fn an_empty_output_batch_is_rejected() {
        let error = ModelDriverOutputBatch::try_new(Vec::new())
            .err()
            .expect("an empty output batch should be rejected");

        assert_eq!(error, EmptyModelDriverOutputBatch);
    }
}
