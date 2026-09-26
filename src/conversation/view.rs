use super::Conversation;
use crate::conversation::events::{
    Context, ConversationEvent, ConversationEventRecord, ToolDefinition,
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

    pub(crate) fn events_through(&self, position: u64) -> &[ConversationEventRecord] {
        let events = self.events();
        events
            .iter()
            .position(|event| event.position > position)
            .map(|boundary| &events[..boundary])
            .unwrap_or(events)
    }

    #[allow(dead_code)]
    pub(crate) fn available_tools(&self) -> &[ToolDefinition] {
        latest_tools(self.events())
    }
}

pub(crate) fn latest_tools(events: &[ConversationEventRecord]) -> &[ToolDefinition] {
    events
        .iter()
        .rev()
        .find_map(|event| match &event.event {
            ConversationEvent::Context(Context::ToolsAvailable { tools }) => Some(tools.as_slice()),
            _ => None,
        })
        .unwrap_or(&[])
}
