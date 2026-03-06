use crate::types::{CachedManifestDirectory, CachedSessionSummary};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub fn save_cache(cache_path: &Path, entries: &[CachedSessionSummary]) -> Result<()> {
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create cache directory {}", parent.display()))?;
    }

    let encoded = bincode::serde::encode_to_vec(entries, bincode::config::standard())
        .context("failed to encode cache entries")?;
    fs::write(cache_path, encoded)
        .with_context(|| format!("failed to write cache file {}", cache_path.display()))?;
    Ok(())
}

pub fn load_cache(cache_path: &Path) -> Result<Vec<CachedSessionSummary>> {
    if !cache_path.exists() {
        return Ok(Vec::new());
    }

    let bytes = fs::read(cache_path)
        .with_context(|| format!("failed to read cache file {}", cache_path.display()))?;
    let (entries, _) = bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
        .context("failed to decode cache entries")?;
    Ok(entries)
}

pub fn save_manifest(manifest_path: &Path, entries: &[CachedManifestDirectory]) -> Result<()> {
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create cache directory {}", parent.display()))?;
    }

    let encoded = bincode::serde::encode_to_vec(entries, bincode::config::standard())
        .context("failed to encode manifest entries")?;
    fs::write(manifest_path, encoded)
        .with_context(|| format!("failed to write manifest file {}", manifest_path.display()))?;
    Ok(())
}

pub fn load_manifest(manifest_path: &Path) -> Result<Vec<CachedManifestDirectory>> {
    if !manifest_path.exists() {
        return Ok(Vec::new());
    }

    let bytes = fs::read(manifest_path)
        .with_context(|| format!("failed to read manifest file {}", manifest_path.display()))?;
    let (entries, _) = bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
        .context("failed to decode manifest entries")?;
    Ok(entries)
}

#[cfg(test)]
#[path = "../tests/unit/cache_tests.rs"]
mod tests;
