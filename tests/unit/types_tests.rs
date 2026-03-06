use super::Usage;

#[test]
fn calculates_billable_input_tokens_without_cached_tokens() {
    let usage = Usage {
        input_tokens: 1_000,
        cached_input_tokens: 250,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 1_000,
    };

    assert_eq!(usage.billable_input_tokens(), 750);
}
