use super::{
    LiteLLMModelPricing, PricingCatalog, load_cached_catalog_any_age, pricing_for_model,
    resolve_model_pricing, usage_cost_usd,
};
use crate::types::Usage;
use std::collections::HashMap;

fn empty_catalog() -> PricingCatalog {
    PricingCatalog::default()
}

fn resolved_pricing(model: &str) -> super::ModelPricing {
    pricing_for_model(model)
}

#[test]
fn resolves_gpt_5_4_family_pricing() {
    let pricing = resolved_pricing("gpt-5.4-codex");
    assert_eq!(pricing.input_cost_per_million, 2.50);
    assert_eq!(pricing.cached_input_cost_per_million, 0.25);
    assert_eq!(pricing.output_cost_per_million, 15.0);
}

#[test]
fn resolves_gpt_5_4_mini_pricing() {
    let pricing = resolved_pricing("gpt-5.4-mini");
    assert_eq!(pricing.input_cost_per_million, 0.75);
    assert_eq!(pricing.cached_input_cost_per_million, 0.075);
    assert_eq!(pricing.output_cost_per_million, 4.50);
}

#[test]
fn resolves_gpt_5_5_pricing() {
    let pricing = resolved_pricing("gpt-5.5");
    assert_eq!(pricing.input_cost_per_million, 5.00);
    assert_eq!(pricing.cached_input_cost_per_million, 0.50);
    assert_eq!(pricing.output_cost_per_million, 30.00);
}

#[test]
fn resolves_gpt_5_6_family_pricing() {
    for (model, input, cached_input, output) in [
        ("gpt-5.6-luna", 1.00, 0.10, 6.00),
        ("gpt-5.6-terra", 2.50, 0.25, 15.00),
        ("gpt-5.6-sol", 5.00, 0.50, 30.00),
    ] {
        let pricing = resolved_pricing(model);
        assert_eq!(pricing.input_cost_per_million, input, "{model}");
        assert_eq!(
            pricing.cached_input_cost_per_million, cached_input,
            "{model}"
        );
        assert_eq!(pricing.output_cost_per_million, output, "{model}");
    }
}

#[test]
fn calculates_gpt_5_6_family_usage_cost() {
    let usage = Usage {
        input_tokens: 2_000_000,
        cached_input_tokens: 1_000_000,
        output_tokens: 1_000_000,
        reasoning_output_tokens: 0,
        total_tokens: 3_000_000,
    };

    for (model, expected) in [
        ("gpt-5.6-luna", 7.10),
        ("gpt-5.6-terra", 17.75),
        ("gpt-5.6-sol", 35.50),
    ] {
        let cost = usage_cost_usd(&empty_catalog(), model, &usage);
        assert!((cost - expected).abs() < f64::EPSILON, "{model}");
    }
}

#[test]
fn resolves_gpt_5_2_codex_family_pricing() {
    let pricing = resolved_pricing("gpt-5.3-codex");
    assert_eq!(pricing.input_cost_per_million, 1.75);
    assert_eq!(pricing.cached_input_cost_per_million, 0.175);
    assert_eq!(pricing.output_cost_per_million, 14.0);
}

#[test]
fn resolves_gpt_5_3_codex_spark_as_zero_cost() {
    let pricing = resolved_pricing("gpt-5.3-codex-spark");
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
fn prices_unknown_models_at_zero_cost() {
    let pricing = pricing_for_model("gpt-unknown-codex");

    assert_eq!(pricing.input_cost_per_million, 0.0);
    assert_eq!(pricing.cached_input_cost_per_million, 0.0);
    assert_eq!(pricing.output_cost_per_million, 0.0);
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

#[test]
fn treats_corrupt_pricing_cache_as_missing() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let cache_path = temp_dir.path().join("pricing.json");
    std::fs::write(&cache_path, []).expect("write corrupt pricing cache");

    assert!(
        load_cached_catalog_any_age(&cache_path)
            .expect("load pricing cache")
            .is_none()
    );
}

#[test]
fn decodes_litellm_raw_catalog_fields_from_provider_fixture() {
    // Source: https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json
    let models = serde_json::from_str::<HashMap<String, LiteLLMModelPricing>>(include_str!(
        "../fixtures/litellm-pricing-subset.json"
    ))
    .expect("decode provider fixture");

    let pricing = resolve_model_pricing(&models, "gpt-5.4");
    assert_eq!(pricing.input_cost_per_million, 2.5);
    assert_eq!(pricing.cached_input_cost_per_million, 0.25);
    assert_eq!(pricing.output_cost_per_million, 15.0);
}
