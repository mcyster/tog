mod events;
mod history;
mod id;
#[cfg(test)]
mod tests;
mod view;

pub use events::{
    AssistantResponse, Context, ConversationEvent, ConversationEventId, ConversationEventPayload,
    FailureCategory, InvalidAssistantResponse, InvalidConversationEvent, InvalidModelResponse,
    InvalidModelSpecificEvent, InvalidToolRequest, ModelData, ModelEvent, ModelId, ModelOutcome,
    ModelRequest, ModelResponse, ModelSource, ModelSpecificEvent, OperationFailure, ProviderId,
    Tool, ToolAvailability, ToolDefinition, ToolName, ToolOutcome, ToolRequest, ToolResponse,
    Tools, TurnEnd, TurnOutcome, TurnStart, Usage, User, UserContent,
};
pub use history::ConversationHistory;
pub use id::ConversationId;
pub use view::{ConversationView, latest_tools};

use crate::conversation_event_store::ConversationEventRecord;

pub trait Conversation {
    #[allow(dead_code)]
    fn id(&self) -> ConversationId;

    fn events(&self) -> &[ConversationEventRecord];
}
