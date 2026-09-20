mod history;
mod id;
mod prompt;
#[cfg(test)]
mod tests;
mod view;

pub(crate) use history::ConversationHistory;
pub(crate) use id::ConversationId;
pub(crate) use prompt::UserPrompt;
pub(crate) use view::ConversationView;

use crate::conversation_event::ConversationEventRecord;

pub(crate) trait Conversation {
    #[allow(dead_code)]
    fn id(&self) -> ConversationId;

    fn events(&self) -> &[ConversationEventRecord];
}
