use super::{EmptyModelDriverOutputBatch, ModelDriverOutputBatch};

#[test]
fn an_empty_output_batch_is_rejected() {
    let error = ModelDriverOutputBatch::try_new(Vec::new())
        .err()
        .expect("an empty output batch should be rejected");

    assert_eq!(error, EmptyModelDriverOutputBatch);
}
