mod error;
mod output;
#[cfg(test)]
mod tests;
mod turn_input;

pub(crate) use output::{ModelDriverOutput, ModelDriverOutputBatch};
pub(crate) use turn_input::TurnInput;

use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;

use crate::conversation::{ConversationEventId, ModelSource};

pub(crate) type ModelOutputStream =
    BoxStream<'static, Result<ModelDriverOutputBatch, ModelDriverError>>;

pub(crate) trait ModelDriver {
    fn source(&self) -> &ModelSource;

    fn invoke<'invoke>(
        &'invoke self,
        input: TurnInput<'invoke>,
    ) -> BoxFuture<'invoke, Result<ModelOutputStream, ModelDriverError>>;
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ModelDriverError {
    UnexpectedModelRequestReference {
        expected: ConversationEventId,
        found: ConversationEventId,
    },
    TerminalResponseNotAlone {
        model_request_id: ConversationEventId,
    },
    OutputAfterTerminalResponse {
        model_request_id: ConversationEventId,
    },
}

#[allow(dead_code)]
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct EmptyModelDriverOutputBatch;
