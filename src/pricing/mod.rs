use crate::types::{ModelTotals, Usage};
use std::collections::{BTreeMap, HashMap};

mod catalog;

use catalog::LiteLLMModelPricing;
pub use catalog::PricingCatalog;
#[cfg(test)]
use catalog::load_cached_catalog_any_age;

#[derive(Debug, Clone, Copy)]
pub struct ModelPricing {
    pub input_cost_per_million: f64,
    pub cached_input_cost_per_million: f64,
    pub output_cost_per_million: f64,
}

const GPT_5_PRICING: ModelPricing = ModelPricing {
    input_cost_per_million: 1.25,
    cached_input_cost_per_million: 0.125,
    output_cost_per_million: 10.0,
};

const GPT_5_2_CODEX_PRICING: ModelPricing = ModelPricing {
    input_cost_per_million: 1.75,
    cached_input_cost_per_million: 0.175,
    output_cost_per_million: 14.0,
};

const GPT_5_4_PRICING: ModelPricing = ModelPricing {
    input_cost_per_million: 2.50,
    cached_input_cost_per_million: 0.25,
    output_cost_per_million: 15.0,
};

const GPT_5_4_MINI_PRICING: ModelPricing = ModelPricing {
    input_cost_per_million: 0.75,
    cached_input_cost_per_million: 0.075,
    output_cost_per_million: 4.50,
};

const GPT_5_5_PRICING: ModelPricing = ModelPricing {
    input_cost_per_million: 5.00,
    cached_input_cost_per_million: 0.50,
    output_cost_per_million: 30.0,
};

const GPT_5_6_LUNA_PRICING: ModelPricing = ModelPricing {
    input_cost_per_million: 1.00,
    cached_input_cost_per_million: 0.10,
    output_cost_per_million: 6.00,
};

const GPT_5_6_TERRA_PRICING: ModelPricing = ModelPricing {
    input_cost_per_million: 2.50,
    cached_input_cost_per_million: 0.25,
    output_cost_per_million: 15.00,
};

const GPT_5_6_SOL_PRICING: ModelPricing = ModelPricing {
    input_cost_per_million: 5.00,
    cached_input_cost_per_million: 0.50,
    output_cost_per_million: 30.00,
};

const ZERO_COST_PRICING: ModelPricing = ModelPricing {
    input_cost_per_million: 0.0,
    cached_input_cost_per_million: 0.0,
    output_cost_per_million: 0.0,
};

impl PricingCatalog {
    pub fn pricing_for_model(&self, model: &str) -> ModelPricing {
        resolve_model_pricing(&self.models, model)
    }
}

pub fn pricing_for_model(model: &str) -> ModelPricing {
    resolve_model_pricing(&HashMap::new(), model)
}

fn resolve_model_pricing(
    models: &HashMap<String, LiteLLMModelPricing>,
    model: &str,
) -> ModelPricing {
    if let Some(pricing) = pinned_model_pricing(model) {
        return pricing;
    }

    if let Some(pricing) = direct_or_prefixed_lookup(models, model) {
        if let Some(resolved) = to_model_pricing(pricing) {
            return resolved;
        }
    }

    if let Some(alias) = model_alias(model) {
        if let Some(pricing) = pinned_model_pricing(alias) {
            return pricing;
        }
        if let Some(pricing) = direct_or_prefixed_lookup(models, alias) {
            if let Some(resolved) = to_model_pricing(pricing) {
                return resolved;
            }
        }
        return fallback_model_pricing(alias);
    }

    fallback_model_pricing(model)
}

fn pinned_model_pricing(model: &str) -> Option<ModelPricing> {
    match model {
        "gpt-5.3-codex-spark" => Some(ZERO_COST_PRICING),
        _ => None,
    }
}

fn fallback_model_pricing(model: &str) -> ModelPricing {
    match model {
        "gpt-5.6-luna" => GPT_5_6_LUNA_PRICING,
        "gpt-5.6-terra" => GPT_5_6_TERRA_PRICING,
        "gpt-5.6-sol" => GPT_5_6_SOL_PRICING,
        "gpt-5.5" => GPT_5_5_PRICING,
        "gpt-5.4-mini" => GPT_5_4_MINI_PRICING,
        "gpt-5.4" | "gpt-5.4-codex" => GPT_5_4_PRICING,
        "gpt-5.2-codex" | "gpt-5.3-codex" => GPT_5_2_CODEX_PRICING,
        "gpt-5" | "gpt-5-codex" => GPT_5_PRICING,
        _ => ZERO_COST_PRICING,
    }
}

fn direct_or_prefixed_lookup<'a>(
    models: &'a HashMap<String, LiteLLMModelPricing>,
    model: &str,
) -> Option<&'a LiteLLMModelPricing> {
    for candidate in [
        model.to_string(),
        format!("openai/{model}"),
        format!("azure/{model}"),
        format!("openrouter/openai/{model}"),
    ] {
        if let Some(pricing) = models.get(&candidate) {
            return Some(pricing);
        }
    }

    models.iter().find_map(|(key, value)| {
        if key.eq_ignore_ascii_case(model)
            || key
                .strip_prefix("openai/")
                .is_some_and(|value| value.eq_ignore_ascii_case(model))
            || key
                .strip_prefix("azure/")
                .is_some_and(|value| value.eq_ignore_ascii_case(model))
            || key
                .strip_prefix("openrouter/openai/")
                .is_some_and(|value| value.eq_ignore_ascii_case(model))
        {
            Some(value)
        } else {
            None
        }
    })
}

fn model_alias(model: &str) -> Option<&'static str> {
    match model {
        "gpt-5-codex" => Some("gpt-5"),
        "gpt-5.3-codex" => Some("gpt-5.2-codex"),
        _ => None,
    }
}

fn to_model_pricing(pricing: &LiteLLMModelPricing) -> Option<ModelPricing> {
    let input = pricing.input_cost_per_token?;
    let output = pricing.output_cost_per_token?;
    let cached = pricing.cache_read_input_token_cost.unwrap_or(input);

    Some(ModelPricing {
        input_cost_per_million: input * 1_000_000.0,
        cached_input_cost_per_million: cached * 1_000_000.0,
        output_cost_per_million: output * 1_000_000.0,
    })
}

pub fn usage_cost_usd(catalog: &PricingCatalog, model: &str, usage: &Usage) -> f64 {
    let pricing = catalog.pricing_for_model(model);

    let cached_input_tokens = usage.cached_input_tokens.min(usage.input_tokens);
    let non_cached_input_tokens = usage.input_tokens.saturating_sub(cached_input_tokens);

    (non_cached_input_tokens as f64 / 1_000_000.0) * pricing.input_cost_per_million
        + (cached_input_tokens as f64 / 1_000_000.0) * pricing.cached_input_cost_per_million
        + (usage.output_tokens as f64 / 1_000_000.0) * pricing.output_cost_per_million
}

pub fn totals_cost_usd(catalog: &PricingCatalog, models: &BTreeMap<String, ModelTotals>) -> f64 {
    models
        .iter()
        .map(|(model, totals)| usage_cost_usd(catalog, model, &totals.usage))
        .sum()
}

#[cfg(test)]
#[path = "../../tests/unit/pricing_tests.rs"]
mod tests;
