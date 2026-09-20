use crate::conversation::{Conversation, ConversationView};
use crate::conversation_event::{ConversationTurnId, UserMessageRequest};

pub(crate) struct TurnInput<'conversation> {
    conversation: ConversationView<'conversation>,
    pending_user_requests: Vec<UserMessageRequest>,
    turn_id: ConversationTurnId,
}

impl<'conversation> TurnInput<'conversation> {
    pub(crate) fn new(
        conversation: &'conversation dyn Conversation,
        turn_id: ConversationTurnId,
    ) -> Self {
        let conversation = ConversationView::new(conversation);
        let pending_user_requests = conversation.pending_user_requests();
        Self {
            conversation,
            pending_user_requests,
            turn_id,
        }
    }

    pub(crate) fn conversation(&self) -> &ConversationView<'conversation> {
        &self.conversation
    }

    pub(crate) fn pending_user_requests(&self) -> &[UserMessageRequest] {
        &self.pending_user_requests
    }

    pub(crate) fn turn_id(&self) -> ConversationTurnId {
        self.turn_id
    }
}
