use std::error::Error;
use std::fmt::{Display, Formatter};

use super::{EmptyModelDriverOutputBatch, ModelDriverError};

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

impl Display for EmptyModelDriverOutputBatch {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "a model driver output batch must not be empty")
    }
}

impl Error for EmptyModelDriverOutputBatch {}
