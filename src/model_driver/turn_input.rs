use crate::conversation::Conversation;
use crate::conversation_event::{ConversationTurnId, UserMessageRequest};

pub(crate) struct TurnInput<'conversation> {
    conversation: &'conversation Conversation,
    pending_user_requests: Vec<UserMessageRequest>,
    turn_id: ConversationTurnId,
}

impl<'conversation> TurnInput<'conversation> {
    pub(crate) fn new(
        conversation: &'conversation Conversation,
        turn_id: ConversationTurnId,
    ) -> Self {
        Self {
            conversation,
            pending_user_requests: conversation.pending_user_requests(),
            turn_id,
        }
    }

    pub(crate) fn conversation(&self) -> &'conversation Conversation {
        self.conversation
    }

    pub(crate) fn pending_user_requests(&self) -> &[UserMessageRequest] {
        &self.pending_user_requests
    }

    pub(crate) fn turn_id(&self) -> ConversationTurnId {
        self.turn_id
    }
}
