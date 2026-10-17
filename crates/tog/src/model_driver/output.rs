use crate::conversation::{AssistantResponse, ModelResponse, ModelSpecificEvent, ToolRequest};

use super::EmptyModelDriverOutputBatch;

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

pub struct ModelDriverOutputBatch {
    outputs: Vec<ModelDriverOutput>,
}

impl ModelDriverOutputBatch {
    pub fn try_new(outputs: Vec<ModelDriverOutput>) -> Result<Self, EmptyModelDriverOutputBatch> {
        if outputs.is_empty() {
            return Err(EmptyModelDriverOutputBatch);
        }
        Ok(Self { outputs })
    }

    pub fn into_outputs(self) -> Vec<ModelDriverOutput> {
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
