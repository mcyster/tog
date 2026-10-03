use crate::conversation::{
    Conversation, ConversationEventId, ConversationView, Tools, latest_tools,
};
use crate::conversation_event_store::ConversationEventRecord;

pub struct TurnInput<'conversation> {
    conversation: ConversationView<'conversation>,
    model_request_id: ConversationEventId,
    input_through: u64,
}

impl<'conversation> TurnInput<'conversation> {
    pub fn new(
        conversation: &'conversation dyn Conversation,
        model_request_id: ConversationEventId,
        input_through: u64,
    ) -> Self {
        Self {
            conversation: ConversationView::new(conversation),
            model_request_id,
            input_through,
        }
    }

    pub fn events(&self) -> &[ConversationEventRecord] {
        self.conversation.events_through(self.input_through)
    }

    pub fn tools(&self) -> Option<&Tools> {
        latest_tools(self.events())
    }

    pub fn model_request_id(&self) -> ConversationEventId {
        self.model_request_id
    }
}
