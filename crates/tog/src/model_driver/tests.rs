use crate::conversation::{AssistantResponse, ConversationEventId, ModelSpecificEvent};

use super::{
    InvalidModelDriverOutputBatch, ModelDriverOutput, ModelDriverOutputBatch,
    ModelDriverOutputPersistence,
};

fn model_specific_event() -> ModelDriverOutput {
    ModelDriverOutput::ModelSpecificEvent(
        ModelSpecificEvent::new(
            ConversationEventId::new(),
            "reasoning".to_owned(),
            1,
            serde_json::json!({}),
            Some("Thinking.".to_owned()),
        )
        .expect("the model specific event should be valid"),
    )
}

#[test]
fn an_empty_output_batch_is_rejected() {
    let error = ModelDriverOutputBatch::try_new(Vec::new())
        .err()
        .expect("an empty output batch should be rejected");

    assert_eq!(error, InvalidModelDriverOutputBatch::Empty);
}

#[test]
fn an_output_batch_is_durable_by_default() {
    let batch = ModelDriverOutputBatch::try_new(vec![model_specific_event()])
        .expect("the batch should be valid");

    assert_eq!(batch.persistence(), ModelDriverOutputPersistence::Durable);
}

#[test]
fn a_model_specific_event_batch_can_be_unpersisted() {
    let batch = ModelDriverOutputBatch::try_new_unpersisted(vec![model_specific_event()])
        .expect("the unpersisted batch should be valid");

    assert_eq!(
        batch.persistence(),
        ModelDriverOutputPersistence::Unpersisted
    );
}

#[test]
fn an_unpersisted_batch_rejects_non_model_specific_output() {
    let assistant = ModelDriverOutput::AssistantResponse(
        AssistantResponse::new(ConversationEventId::new(), "Hello.".to_owned())
            .expect("the assistant response should be valid"),
    );

    let error = ModelDriverOutputBatch::try_new_unpersisted(vec![assistant])
        .err()
        .expect("an unpersisted batch must contain only model-specific events");

    assert_eq!(
        error,
        InvalidModelDriverOutputBatch::UnpersistedOutputNotModelSpecific
    );
}
