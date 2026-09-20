mod history;
mod id;
mod prompt;
#[cfg(test)]
mod tests;

pub(crate) use history::ConversationHistory;
pub(crate) use id::ConversationId;
pub(crate) use prompt::UserPrompt;

use crate::conversation_event::{ConversationEventRecord, ToolDefinition, UserMessageRequest};

pub(crate) trait Conversation {
    #[allow(dead_code)]
    fn id(&self) -> ConversationId;

    fn events(&self) -> &[ConversationEventRecord];

    fn pending_user_requests(&self) -> Vec<UserMessageRequest>;

    fn available_tools(&self) -> &[ToolDefinition];
}
