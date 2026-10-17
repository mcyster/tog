use crate::conversation::{AssistantResponse, ModelResponse, ModelSpecificEvent, ToolRequest};

use super::InvalidModelDriverOutputBatch;

pub enum ModelDriverOutput {
    AssistantResponse(AssistantResponse),
    ToolRequest(ToolRequest),
    ModelResponse(ModelResponse),
    ModelSpecificEvent(ModelSpecificEvent),
}

impl ModelDriverOutput {
    pub fn is_terminal_response(&self) -> bool {
        matches!(self, Self::ModelResponse(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelDriverOutputPersistence {
    Durable,
    Unpersisted,
}

pub struct ModelDriverOutputBatch {
    outputs: Vec<ModelDriverOutput>,
    persistence: ModelDriverOutputPersistence,
}

impl ModelDriverOutputBatch {
    pub fn try_new(outputs: Vec<ModelDriverOutput>) -> Result<Self, InvalidModelDriverOutputBatch> {
        Self::try_new_with_persistence(outputs, ModelDriverOutputPersistence::Durable)
    }

    pub fn try_new_unpersisted(
        outputs: Vec<ModelDriverOutput>,
    ) -> Result<Self, InvalidModelDriverOutputBatch> {
        Self::try_new_with_persistence(outputs, ModelDriverOutputPersistence::Unpersisted)
    }

    fn try_new_with_persistence(
        outputs: Vec<ModelDriverOutput>,
        persistence: ModelDriverOutputPersistence,
    ) -> Result<Self, InvalidModelDriverOutputBatch> {
        if outputs.is_empty() {
            return Err(InvalidModelDriverOutputBatch::Empty);
        }
        if persistence == ModelDriverOutputPersistence::Unpersisted
            && outputs
                .iter()
                .any(|output| !matches!(output, ModelDriverOutput::ModelSpecificEvent(_)))
        {
            return Err(InvalidModelDriverOutputBatch::UnpersistedOutputNotModelSpecific);
        }
        Ok(Self {
            outputs,
            persistence,
        })
    }

    pub fn persistence(&self) -> ModelDriverOutputPersistence {
        self.persistence
    }

    pub fn into_outputs(self) -> Vec<ModelDriverOutput> {
        self.outputs
    }
}

impl From<ModelDriverOutput> for ModelDriverOutputBatch {
    fn from(output: ModelDriverOutput) -> Self {
        Self {
            outputs: vec![output],
            persistence: ModelDriverOutputPersistence::Durable,
        }
    }
}
