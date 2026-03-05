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
mod tests {
    use super::{load_cache, load_manifest, save_cache, save_manifest};
    use crate::types::{
        CachedManifestDirectory, CachedManifestFile, CachedSessionSummary, SessionSummary, Usage,
        UsageEvent,
    };

    #[test]
    fn roundtrips_cached_session_summaries() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let cache_path = temp_dir.path().join("session-cache.bin");
        let entry = CachedSessionSummary {
            file_size: 4096,
            modified_unix_ms: 1_772_723_600_000,
            session: SessionSummary {
                session_id: "2026/03/06/rollout-1".to_string(),
                session_path: "2026/03/06/rollout-1.jsonl".to_string(),
                directory: Some("/Users/jaewon/sources/front-web-www".to_string()),
                events: vec![UsageEvent {
                    timestamp_unix_ms: 1_772_723_600_000,
                    model: "gpt-5.2-codex".to_string(),
                    is_fallback_model: false,
                    usage: Usage {
                        input_tokens: 1200,
                        cached_input_tokens: 100,
                        output_tokens: 300,
                        reasoning_output_tokens: 40,
                        total_tokens: 1500,
                    },
                }],
            },
        };

        save_cache(&cache_path, &[entry.clone()]).expect("save cache");
        let restored = load_cache(&cache_path).expect("load cache");

        assert_eq!(restored, vec![entry]);
    }

    #[test]
    fn roundtrips_manifest_entries() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp_dir.path().join("manifest-cache.bin");
        let entry = CachedManifestDirectory {
            relative_dir: "2026/03/06".to_string(),
            modified_unix_ms: 1_772_723_600_000,
            files: vec![CachedManifestFile {
                relative_path: "2026/03/06/rollout-1.jsonl".to_string(),
                file_size: 4096,
                modified_unix_ms: 1_772_723_600_000,
            }],
        };

        save_manifest(&manifest_path, &[entry.clone()]).expect("save manifest");
        let restored = load_manifest(&manifest_path).expect("load manifest");

        assert_eq!(restored, vec![entry]);
    }
}
