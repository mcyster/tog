use serde_json::Value;

use tog::conversation::Usage;

pub(crate) fn parsed_usage(payload: &Value) -> Option<Usage> {
    let usage = payload
        .get("usage")
        .or_else(|| payload.pointer("/response/usage"))?;
    if !usage.is_object() {
        return None;
    }
    let token_count = |name: &str| usage.get(name).and_then(Value::as_u64);
    let cache_read_input_tokens = usage
        .pointer("/input_tokens_details/cached_tokens")
        .and_then(Value::as_u64);
    let cache_write_input_tokens = usage
        .pointer("/input_tokens_details/cache_write_tokens")
        .and_then(Value::as_u64);
    let reasoning_output_tokens = usage
        .pointer("/output_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64);
    let cost_usd = usage.get("cost").and_then(Value::as_f64);
    Usage::try_new(
        token_count("input_tokens"),
        token_count("output_tokens"),
        token_count("total_tokens"),
        cache_read_input_tokens,
        cache_write_input_tokens,
        reasoning_output_tokens,
        cost_usd,
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::parsed_usage;

    #[test]
    fn parsed_usage_maps_each_reported_value_and_keeps_missing_values_unknown() {
        let usage = parsed_usage(&json!({
            "type": "response.completed",
            "response": {
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 50,
                    "output_tokens_details": { "reasoning_tokens": 20 },
                    "input_tokens_details": { "cached_tokens": 10, "cache_write_tokens": 5 },
                    "total_tokens": 150,
                    "cost": 0.001234
                }
            }
        }))
        .expect("the usage should parse");

        assert_eq!(usage.input_tokens(), Some(100));
        assert_eq!(usage.output_tokens(), Some(50));
        assert_eq!(usage.total_tokens(), Some(150));
        assert_eq!(usage.cache_read_input_tokens(), Some(10));
        assert_eq!(usage.cache_write_input_tokens(), Some(5));
        assert_eq!(usage.reasoning_output_tokens(), Some(20));
        assert_eq!(usage.cost_usd(), Some(0.001234));
    }

    #[test]
    fn parsed_usage_requires_at_least_one_reported_value() {
        assert!(parsed_usage(&json!({ "response": {} })).is_none());
    }

    #[test]
    fn parsed_usage_rejects_an_inconsistent_report_as_unknown() {
        assert!(
            parsed_usage(&json!({
                "type": "response.completed",
                "response": {
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 50,
                        "total_tokens": 160
                    }
                }
            }))
            .is_none(),
            "an inconsistent provider report must not become a misleading usage record"
        );
    }
}
