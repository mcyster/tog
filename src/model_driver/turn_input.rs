use crate::conversation::{
    Conversation, ConversationEventId, ConversationEventRecord, ConversationView, latest_toolset,
};
use crate::toolset::Toolset;

pub(crate) struct TurnInput<'conversation> {
    conversation: ConversationView<'conversation>,
    model_request_id: ConversationEventId,
    input_through: u64,
}

impl<'conversation> TurnInput<'conversation> {
    pub(crate) fn new(
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

    pub(crate) fn events(&self) -> &[ConversationEventRecord] {
        self.conversation.events_through(self.input_through)
    }

    pub(crate) fn toolset(&self) -> Option<&Toolset> {
        latest_toolset(self.events())
    }

    pub(crate) fn model_request_id(&self) -> ConversationEventId {
        self.model_request_id
    }
}
