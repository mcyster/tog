use std::error::Error;
use std::fmt::{Display, Formatter};

use super::{EmptyModelDriverOutputBatch, ModelDriverError};

impl Display for ModelDriverError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedModelRequestReference { expected, found } => write!(
                formatter,
                "model driver produced a model event for model request {found}, expected {expected}"
            ),
            Self::TerminalResponseNotAlone { model_request_id } => write!(
                formatter,
                "the terminal model response for {model_request_id} must be the only output in its batch"
            ),
            Self::OutputAfterTerminalResponse { model_request_id } => write!(
                formatter,
                "model driver produced output after the terminal model response for {model_request_id}"
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
