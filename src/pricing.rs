use crate::types::{ModelTotals, Usage};
use anyhow::{Context, Result};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

const ZERO_COST_PRICING: ModelPricing = ModelPricing {
    input_cost_per_million: 0.0,
    cached_input_cost_per_million: 0.0,
    output_cost_per_million: 0.0,
};

const LITELLM_PRICING_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const DEFAULT_PRICING_CACHE_SUBDIR: &str = "codex-usage";
const DEFAULT_PRICING_CACHE_FILENAME: &str = "litellm-pricing-cache.json";
const DEFAULT_PRICING_TTL_SECS: u64 = 60 * 60 * 24;
const PRICING_FETCH_TIMEOUT_SECS: u64 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedPricingCatalog {
    fetched_unix_ms: u64,
    models: HashMap<String, LiteLLMModelPricing>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LiteLLMModelPricing {
    input_cost_per_token: Option<f64>,
    output_cost_per_token: Option<f64>,
    cache_read_input_token_cost: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct PricingCatalog {
    models: HashMap<String, LiteLLMModelPricing>,
}

impl Default for PricingCatalog {
    fn default() -> Self {
        Self {
            models: HashMap::new(),
        }
    }
}

impl PricingCatalog {
    pub fn load() -> Result<Self> {
        let cache_path = default_pricing_cache_path()?;
        if let Some(cached) = load_cached_catalog(&cache_path)? {
            return Ok(Self {
                models: cached.models,
            });
        }

        let fetched = fetch_remote_catalog()
            .map(Some)
            .or_else(|_| load_cached_catalog_any_age(&cache_path))?
            .unwrap_or_else(|| CachedPricingCatalog {
                fetched_unix_ms: 0,
                models: HashMap::new(),
            });
        save_cached_catalog(&cache_path, &fetched)?;

        Ok(Self {
            models: fetched.models,
        })
    }

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
        "gpt-5.4" | "gpt-5.4-codex" => GPT_5_4_PRICING,
        "gpt-5.2-codex" | "gpt-5.3-codex" => GPT_5_2_CODEX_PRICING,
        "gpt-5" | "gpt-5-codex" => GPT_5_PRICING,
        _ => GPT_5_2_CODEX_PRICING,
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

fn default_pricing_cache_path() -> Result<PathBuf> {
    let base_dirs = BaseDirs::new().context("failed to resolve cache directory")?;
    Ok(base_dirs
        .cache_dir()
        .join(DEFAULT_PRICING_CACHE_SUBDIR)
        .join(DEFAULT_PRICING_CACHE_FILENAME))
}

fn now_unix_ms() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time predates unix epoch")?
        .as_millis() as u64)
}

fn load_cached_catalog(cache_path: &Path) -> Result<Option<CachedPricingCatalog>> {
    let Some(cached) = load_cached_catalog_any_age(cache_path)? else {
        return Ok(None);
    };
    let age_ms = now_unix_ms()?.saturating_sub(cached.fetched_unix_ms);
    if age_ms > Duration::from_secs(DEFAULT_PRICING_TTL_SECS).as_millis() as u64 {
        return Ok(None);
    }
    Ok(Some(cached))
}

fn load_cached_catalog_any_age(cache_path: &Path) -> Result<Option<CachedPricingCatalog>> {
    if !cache_path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(cache_path)
        .with_context(|| format!("failed to read pricing cache {}", cache_path.display()))?;
    let cached = serde_json::from_str::<CachedPricingCatalog>(&content)
        .with_context(|| format!("failed to parse pricing cache {}", cache_path.display()))?;
    Ok(Some(cached))
}

fn save_cached_catalog(cache_path: &Path, catalog: &CachedPricingCatalog) -> Result<()> {
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create cache directory {}", parent.display()))?;
    }
    let content =
        serde_json::to_string(catalog).context("failed to serialize pricing cache content")?;
    fs::write(cache_path, content)
        .with_context(|| format!("failed to write pricing cache {}", cache_path.display()))?;
    Ok(())
}

fn fetch_remote_catalog() -> Result<CachedPricingCatalog> {
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(PRICING_FETCH_TIMEOUT_SECS))
        .build()
        .context("failed to build LiteLLM pricing client")?
        .get(LITELLM_PRICING_URL)
        .send()
        .context("failed to fetch LiteLLM pricing catalog")?
        .error_for_status()
        .context("LiteLLM pricing catalog returned error status")?;
    let raw_models = response
        .json::<HashMap<String, LiteLLMModelPricing>>()
        .context("failed to decode LiteLLM pricing catalog")?;

    Ok(CachedPricingCatalog {
        fetched_unix_ms: now_unix_ms()?,
        models: raw_models,
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
mod tests {
    use super::{
        LiteLLMModelPricing, PricingCatalog, pricing_for_model, resolve_model_pricing,
        usage_cost_usd,
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

        let expected = (800.0 / 1_000_000.0) * 1.25
            + (200.0 / 1_000_000.0) * 0.125
            + (500.0 / 1_000_000.0) * 10.0;
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
}
