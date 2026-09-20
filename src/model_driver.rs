mod error;
mod output;
#[cfg(test)]
mod tests;
mod turn_input;

pub(crate) use output::{ModelDriverOutput, ModelDriverOutputBatch};
pub(crate) use turn_input::TurnInput;

use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;

use crate::conversation_event::{ConversationCommandId, ConversationEventReader, ModelSource};

pub(crate) type ModelOutputStream =
    BoxStream<'static, Result<ModelDriverOutputBatch, ModelDriverError>>;

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

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct EmptyModelDriverOutputBatch;
