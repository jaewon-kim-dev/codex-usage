use crate::types::CachedSessionSummary;
use anyhow::{Context, Result};
use serde::Serialize;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

pub(crate) struct CacheLoad<T> {
    pub(crate) entries: T,
    pub(crate) needs_rewrite: bool,
}

pub(crate) fn write_atomically(
    cache_path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let parent = cache_path.parent().unwrap_or_else(|| Path::new("."));
    let mut temp_file = tempfile::NamedTempFile::new_in(parent)?;
    write(temp_file.as_file_mut())?;
    temp_file.as_file().sync_all()?;
    temp_file.persist(cache_path).map_err(|error| error.error)?;
    Ok(())
}

pub fn save_cache<T>(cache_path: &Path, entries: &T) -> Result<()>
where
    T: Serialize + ?Sized,
{
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create cache directory {}", parent.display()))?;
    }

    let encoded = bincode::serde::encode_to_vec(entries, bincode::config::standard())
        .context("failed to encode cache entries")?;
    write_atomically(cache_path, |file| file.write_all(&encoded))
        .with_context(|| format!("failed to write cache file {}", cache_path.display()))?;
    Ok(())
}

pub fn load_cache(cache_path: &Path) -> Result<Vec<CachedSessionSummary>> {
    Ok(load_cache_state(cache_path)?.entries)
}

pub(crate) fn load_cache_state(cache_path: &Path) -> Result<CacheLoad<Vec<CachedSessionSummary>>> {
    if !cache_path.exists() {
        return Ok(CacheLoad {
            entries: Vec::new(),
            needs_rewrite: true,
        });
    }

    let bytes = fs::read(cache_path)
        .with_context(|| format!("failed to read cache file {}", cache_path.display()))?;
    let Ok((entries, _)) = bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
    else {
        return Ok(CacheLoad {
            entries: Vec::new(),
            needs_rewrite: true,
        });
    };
    Ok(CacheLoad {
        entries,
        needs_rewrite: false,
    })
}

#[cfg(test)]
#[path = "../../tests/unit/cache_tests.rs"]
mod tests;
