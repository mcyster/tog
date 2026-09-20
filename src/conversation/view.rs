use std::collections::HashSet;

use super::Conversation;
use crate::conversation_event::{
    ConversationCommand, ConversationEventKind, ConversationEventRecord, ConversationFact,
    ConversationMessage, StoredConversationEventKind, ToolDefinition, UserMessageRequest,
};

pub(crate) struct ConversationView<'conversation> {
    conversation: &'conversation dyn Conversation,
}

impl<'conversation> ConversationView<'conversation> {
    pub(crate) fn new(conversation: &'conversation dyn Conversation) -> Self {
        Self { conversation }
    }

    pub(crate) fn events(&self) -> &[ConversationEventRecord] {
        self.conversation.events()
    }

    pub(crate) fn pending_user_requests(&self) -> Vec<UserMessageRequest> {
        let mut requests = Vec::new();
        let mut accepted_request_ids = HashSet::new();
        for event in self.events() {
            let StoredConversationEventKind::Shared(kind) = &event.kind else {
                continue;
            };
            match kind {
                ConversationEventKind::Command(ConversationCommand::UserMessageRequested(
                    request,
                )) => requests.push(request.clone()),
                ConversationEventKind::Fact(ConversationFact::Message {
                    message:
                        ConversationMessage::User {
                            caused_by: Some(command_id),
                            ..
                        },
                    ..
                }) => {
                    accepted_request_ids.insert(*command_id);
                }
                _ => {}
            }
        }
        requests
            .into_iter()
            .filter(|request| !accepted_request_ids.contains(&request.command_id))
            .collect()
    }

    pub(crate) fn available_tools(&self) -> &[ToolDefinition] {
        self.events()
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                StoredConversationEventKind::Shared(ConversationEventKind::Fact(
                    ConversationFact::ToolsAvailable { tools },
                )) => Some(tools.as_slice()),
                _ => None,
            })
            .unwrap_or(&[])
    }
}
