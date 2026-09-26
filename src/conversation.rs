mod events;
mod history;
mod id;
#[cfg(test)]
mod tests;
mod view;

pub(crate) use events::{
    AssistantResponse, Context, ConversationEvent, ConversationEventId, ConversationEventRecord,
    FailureCategory, InvalidAssistantResponse, InvalidModelResponse, InvalidModelSpecificEvent,
    InvalidToolRequest, ModelData, ModelEvent, ModelId, ModelOutcome, ModelRequest, ModelResponse,
    ModelSource, ModelSpecificEvent, OperationFailure, ProviderId, ToolDefinition, ToolName,
    ToolOutcome, ToolRequest, ToolResponse, TurnEnd, TurnOutcome, TurnStart, Usage, User,
    UserContent,
};
pub(crate) use history::ConversationHistory;
pub(crate) use id::ConversationId;
pub(crate) use view::{ConversationView, latest_tools};

pub(crate) trait Conversation {
    #[allow(dead_code)]
    fn id(&self) -> ConversationId;

    fn events(&self) -> &[ConversationEventRecord];
}
