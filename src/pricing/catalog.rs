use crate::cache::write_atomically;
use anyhow::{Context, Result};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LITELLM_PRICING_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const DEFAULT_PRICING_CACHE_SUBDIR: &str = "codex-usage";
const DEFAULT_PRICING_CACHE_FILENAME: &str = "litellm-pricing-cache.json";
const DEFAULT_PRICING_TTL_SECS: u64 = 60 * 60 * 24;
const PRICING_FETCH_TIMEOUT_SECS: u64 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CachedPricingCatalog {
    fetched_unix_ms: u64,
    models: HashMap<String, LiteLLMModelPricing>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct LiteLLMModelPricing {
    pub(super) input_cost_per_token: Option<f64>,
    pub(super) output_cost_per_token: Option<f64>,
    pub(super) cache_read_input_token_cost: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct PricingCatalog {
    pub(super) models: HashMap<String, LiteLLMModelPricing>,
}

impl PricingCatalog {
    pub fn load() -> Result<Self> {
        let cache_path = default_pricing_cache_path()?;
        Self::load_at(&cache_path, now_unix_ms()?)
    }

    fn load_at(cache_path: &Path, now_unix_ms: u64) -> Result<Self> {
        if let Some(cached) = load_cached_catalog(cache_path, now_unix_ms)? {
            return Ok(Self {
                models: cached.models,
            });
        }

        match fetch_remote_catalog(now_unix_ms) {
            Ok(fetched) => {
                save_cached_catalog(cache_path, &fetched)?;
                Ok(Self {
                    models: fetched.models,
                })
            }
            Err(_) => Ok(Self {
                models: load_cached_catalog_any_age(cache_path)?
                    .map(|cached| cached.models)
                    .unwrap_or_default(),
            }),
        }
    }
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

fn load_cached_catalog(
    cache_path: &Path,
    now_unix_ms: u64,
) -> Result<Option<CachedPricingCatalog>> {
    let Some(cached) = load_cached_catalog_any_age(cache_path)? else {
        return Ok(None);
    };
    let age_ms = now_unix_ms.saturating_sub(cached.fetched_unix_ms);
    if age_ms > Duration::from_secs(DEFAULT_PRICING_TTL_SECS).as_millis() as u64 {
        return Ok(None);
    }
    Ok(Some(cached))
}

pub(super) fn load_cached_catalog_any_age(
    cache_path: &Path,
) -> Result<Option<CachedPricingCatalog>> {
    if !cache_path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(cache_path)
        .with_context(|| format!("failed to read pricing cache {}", cache_path.display()))?;
    Ok(serde_json::from_str::<CachedPricingCatalog>(&content).ok())
}

fn save_cached_catalog(cache_path: &Path, catalog: &CachedPricingCatalog) -> Result<()> {
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create cache directory {}", parent.display()))?;
    }
    let content =
        serde_json::to_string(catalog).context("failed to serialize pricing cache content")?;
    write_atomically(cache_path, |file| file.write_all(content.as_bytes()))
        .with_context(|| format!("failed to write pricing cache {}", cache_path.display()))?;
    Ok(())
}

fn fetch_remote_catalog(fetched_unix_ms: u64) -> Result<CachedPricingCatalog> {
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(PRICING_FETCH_TIMEOUT_SECS))
        .build()
        .context("failed to build LiteLLM pricing client")?
        .get(LITELLM_PRICING_URL)
        .send()
        .context("failed to fetch LiteLLM pricing catalog")?
        .error_for_status()
        .context("LiteLLM pricing catalog returned error status")?;
    let models = response
        .json::<HashMap<String, LiteLLMModelPricing>>()
        .context("failed to decode LiteLLM pricing catalog")?;

    Ok(CachedPricingCatalog {
        fetched_unix_ms,
        models,
    })
}
