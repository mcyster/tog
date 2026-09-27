use std::collections::HashMap;

use super::Conversation;
use crate::conversation::Context;
use crate::conversation::events::ConversationEventContent;
use crate::conversation_event_store::ConversationEventRecord;
use crate::toolset::Toolset;

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
    pub(crate) fn toolset(&self) -> Option<&Toolset> {
        latest_toolset(self.events())
    }

    #[allow(dead_code)]
    pub(crate) fn effective_contexts(
        &'conversation self,
    ) -> HashMap<&'conversation str, &'conversation Context> {
        let mut effective = HashMap::new();
        for event in self.events() {
            if let ConversationEventContent::Context(context) = event.event.content() {
                effective.insert(context.name(), context);
            }
        }
        effective
    }
}

pub(crate) fn latest_toolset(events: &[ConversationEventRecord]) -> Option<&Toolset> {
    events
        .iter()
        .rev()
        .find_map(|event| match event.event.content() {
            ConversationEventContent::Toolset(toolset) => Some(toolset.toolset()),
            _ => None,
        })
}
