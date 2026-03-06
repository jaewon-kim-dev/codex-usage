use super::{
    LiteLLMModelPricing, PricingCatalog, pricing_for_model, resolve_model_pricing, usage_cost_usd,
};
use crate::types::Usage;
use std::collections::HashMap;

fn empty_catalog() -> PricingCatalog {
    PricingCatalog::default()
}

#[test]
fn resolves_gpt_5_4_family_pricing() {
    let pricing = pricing_for_model("gpt-5.4-codex");
    assert_eq!(pricing.input_cost_per_million, 2.50);
    assert_eq!(pricing.cached_input_cost_per_million, 0.25);
    assert_eq!(pricing.output_cost_per_million, 15.0);
}

#[test]
fn resolves_gpt_5_2_codex_family_pricing() {
    let pricing = pricing_for_model("gpt-5.3-codex");
    assert_eq!(pricing.input_cost_per_million, 1.75);
    assert_eq!(pricing.cached_input_cost_per_million, 0.175);
    assert_eq!(pricing.output_cost_per_million, 14.0);
}

#[test]
fn resolves_gpt_5_3_codex_spark_as_zero_cost() {
    let pricing = pricing_for_model("gpt-5.3-codex-spark");
    assert_eq!(pricing.input_cost_per_million, 0.0);
    assert_eq!(pricing.cached_input_cost_per_million, 0.0);
    assert_eq!(pricing.output_cost_per_million, 0.0);
}

#[test]
fn calculates_gpt_5_4_usage_cost() {
    let cost = usage_cost_usd(
        &empty_catalog(),
        "gpt-5.4",
        &Usage {
            input_tokens: 1_000_000,
            cached_input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            reasoning_output_tokens: 0,
            total_tokens: 3_000_000,
        },
    );

    assert!((cost - 15.25).abs() < f64::EPSILON);
}

#[test]
fn does_not_double_count_cached_input_tokens() {
    let cost = usage_cost_usd(
        &empty_catalog(),
        "gpt-5",
        &Usage {
            input_tokens: 1_000,
            cached_input_tokens: 200,
            output_tokens: 500,
            reasoning_output_tokens: 0,
            total_tokens: 1_500,
        },
    );

    let expected =
        (800.0 / 1_000_000.0) * 1.25 + (200.0 / 1_000_000.0) * 0.125 + (500.0 / 1_000_000.0) * 10.0;
    assert!((cost - expected).abs() < f64::EPSILON);
}

#[test]
fn falls_back_to_gpt_5_2_codex_pricing_for_unknown_models() {
    let cost = usage_cost_usd(
        &empty_catalog(),
        "gpt-unknown-codex",
        &Usage {
            input_tokens: 1_000_000,
            cached_input_tokens: 0,
            output_tokens: 1_000_000,
            reasoning_output_tokens: 0,
            total_tokens: 2_000_000,
        },
    );

    assert!((cost - 15.75).abs() < f64::EPSILON);
}

#[test]
fn keeps_gpt_5_3_codex_spark_free_even_with_remote_catalog_data() {
    let mut models = HashMap::new();
    models.insert(
        "gpt-5.3-codex-spark".to_string(),
        LiteLLMModelPricing {
            input_cost_per_token: Some(9.9e-6),
            output_cost_per_token: Some(9.9e-5),
            cache_read_input_token_cost: Some(9.9e-7),
        },
    );

    let pricing = resolve_model_pricing(&models, "gpt-5.3-codex-spark");
    assert_eq!(pricing.input_cost_per_million, 0.0);
    assert_eq!(pricing.cached_input_cost_per_million, 0.0);
    assert_eq!(pricing.output_cost_per_million, 0.0);
}

#[test]
fn calculates_zero_cost_for_gpt_5_3_codex_spark_usage() {
    let cost = usage_cost_usd(
        &empty_catalog(),
        "gpt-5.3-codex-spark",
        &Usage {
            input_tokens: 1_000_000,
            cached_input_tokens: 500_000,
            output_tokens: 1_000_000,
            reasoning_output_tokens: 0,
            total_tokens: 2_000_000,
        },
    );

    assert!((cost - 0.0).abs() < f64::EPSILON);
}

#[test]
fn uses_remote_alias_pricing_for_gpt_5_3_codex() {
    let mut models = HashMap::new();
    models.insert(
        "gpt-5.2-codex".to_string(),
        LiteLLMModelPricing {
            input_cost_per_token: Some(1.9e-6),
            output_cost_per_token: Some(1.5e-5),
            cache_read_input_token_cost: Some(1.9e-7),
        },
    );

    let pricing = resolve_model_pricing(&models, "gpt-5.3-codex");
    assert_eq!(pricing.input_cost_per_million, 1.9);
    assert_eq!(pricing.cached_input_cost_per_million, 0.19);
    assert_eq!(pricing.output_cost_per_million, 15.0);
}

#[test]
fn does_not_fuzzily_match_other_model_names() {
    let mut models = HashMap::new();
    models.insert(
        "openai/gpt-5".to_string(),
        LiteLLMModelPricing {
            input_cost_per_token: Some(1.25 / 1_000_000.0),
            output_cost_per_token: Some(10.0 / 1_000_000.0),
            cache_read_input_token_cost: Some(0.125 / 1_000_000.0),
        },
    );
    models.insert(
        "openai/gpt-5-mini".to_string(),
        LiteLLMModelPricing {
            input_cost_per_token: Some(9.99 / 1_000_000.0),
            output_cost_per_token: Some(99.0 / 1_000_000.0),
            cache_read_input_token_cost: Some(0.99 / 1_000_000.0),
        },
    );

    let pricing = resolve_model_pricing(&models, "gpt-5");
    assert_eq!(pricing.input_cost_per_million, 1.25);
    assert_eq!(pricing.output_cost_per_million, 10.0);
}
