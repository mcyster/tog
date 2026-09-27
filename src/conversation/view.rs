use std::collections::HashMap;

use super::Conversation;
use crate::conversation::events::ConversationEventPayload;
use crate::conversation::{Context, Tools};
use crate::conversation_event_store::ConversationEventRecord;

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

    pub(crate) fn events_through(&self, position: u64) -> &[ConversationEventRecord] {
        let events = self.events();
        events
            .iter()
            .position(|event| event.position > position)
            .map(|boundary| &events[..boundary])
            .unwrap_or(events)
    }

    #[allow(dead_code)]
    pub(crate) fn tools(&self) -> Option<&Tools> {
        latest_tools(self.events())
    }

    #[allow(dead_code)]
    pub(crate) fn effective_contexts(
        &'conversation self,
    ) -> HashMap<&'conversation str, &'conversation Context> {
        let mut effective = HashMap::new();
        for event in self.events() {
            if let ConversationEventPayload::Context(context) = event.event.payload() {
                effective.insert(context.name(), context);
            }
        }
        effective
    }
}

pub(crate) fn latest_tools(events: &[ConversationEventRecord]) -> Option<&Tools> {
    events
        .iter()
        .rev()
        .find_map(|event| match event.event.payload() {
            ConversationEventPayload::Tools(tools) => Some(tools),
            _ => None,
        })
}
