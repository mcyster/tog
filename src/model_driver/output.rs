use crate::conversation_event::{ConversationEventExtension, ConversationMessage, ToolRequest};

use super::EmptyModelDriverOutputBatch;

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
