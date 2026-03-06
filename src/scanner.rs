use crate::cache::{load_cache, load_manifest, save_cache, save_manifest};
use crate::parser::{aggregate_session_file, parse_session_file};
use crate::report::GroupBy;
use crate::types::{
    CachedManifestDirectory, CachedManifestFile, CachedSessionSummary, ReportRow, SessionSummary,
};
use anyhow::{Context, Result};
use chrono::{Duration, NaiveDate};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

pub struct ScanOptions<'a> {
    pub session_root: &'a Path,
    pub cache_path: &'a Path,
    pub since: Option<NaiveDate>,
    pub until: Option<NaiveDate>,
    pub refresh_cache: bool,
}

#[derive(Debug, Clone)]
struct FileCandidate {
    relative_path: String,
    absolute_path: PathBuf,
    file_size: u64,
    modified_unix_ms: i64,
}

#[derive(Debug, Clone)]
struct DirectoryCandidate {
    relative_dir: String,
    absolute_dir: PathBuf,
    modified_unix_ms: i64,
}

fn parse_date_from_relative_path(relative_path: &str) -> Option<NaiveDate> {
    let mut parts = relative_path.split('/');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

fn file_matches_date_window(
    relative_path: &str,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
) -> bool {
    let Some(file_date) = parse_date_from_relative_path(relative_path) else {
        return true;
    };

    if let Some(since) = since {
        if file_date < since - Duration::days(1) {
            return false;
        }
    }

    if let Some(until) = until {
        if file_date > until + Duration::days(1) {
            return false;
        }
    }

    true
}

fn discover_files(
    session_root: &Path,
    cache_path: &Path,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
    use_manifest_cache: bool,
) -> Result<Vec<FileCandidate>> {
    if !session_root.exists() {
        return Ok(Vec::new());
    }
    if !session_root.is_dir() {
        anyhow::bail!("{} is not a directory", session_root.display());
    }

    if !use_manifest_cache {
        return discover_files_direct(session_root, since, until);
    }

    let manifest_path = manifest_path_for(cache_path);
    let cached_manifest = load_manifest(&manifest_path)?
        .into_iter()
        .map(|entry| (entry.relative_dir.clone(), entry))
        .collect::<HashMap<_, _>>();
    let day_directories = discover_day_directories(session_root)?;
    let refreshed_directories = day_directories
        .par_iter()
        .map(|directory| {
            refresh_directory_manifest_entry(
                session_root,
                directory,
                cached_manifest.get(&directory.relative_dir),
            )
        })
        .collect::<Result<Vec<_>>>()?;

    save_manifest(&manifest_path, &refreshed_directories)?;

    let mut files = Vec::new();
    for directory in refreshed_directories {
        for file in directory.files {
            if !file_matches_date_window(&file.relative_path, since, until) {
                continue;
            }

            files.push(FileCandidate {
                relative_path: file.relative_path.clone(),
                absolute_path: session_root.join(&file.relative_path),
                file_size: file.file_size,
                modified_unix_ms: file.modified_unix_ms,
            });
        }
    }

    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn discover_files_direct(
    session_root: &Path,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
) -> Result<Vec<FileCandidate>> {
    let mut files = Vec::new();

    for entry in WalkDir::new(session_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("jsonl")
        {
            continue;
        }

        let relative_path = entry
            .path()
            .strip_prefix(session_root)
            .with_context(|| {
                format!(
                    "failed to resolve {} relative to {}",
                    entry.path().display(),
                    session_root.display()
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");

        if !file_matches_date_window(&relative_path, since, until) {
            continue;
        }

        let metadata = fs::metadata(entry.path())
            .with_context(|| format!("failed to stat {}", entry.path().display()))?;
        let modified_unix_ms = metadata
            .modified()
            .with_context(|| {
                format!(
                    "failed to read modified time for {}",
                    entry.path().display()
                )
            })?
            .duration_since(UNIX_EPOCH)
            .context("modified time predates unix epoch")?
            .as_millis() as i64;

        files.push(FileCandidate {
            relative_path,
            absolute_path: entry.path().to_path_buf(),
            file_size: metadata.len(),
            modified_unix_ms,
        });
    }

    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn manifest_path_for(cache_path: &Path) -> PathBuf {
    let parent = cache_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = cache_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("session-cache");
    parent.join(format!("{stem}-manifest.bin"))
}

fn discover_day_directories(session_root: &Path) -> Result<Vec<DirectoryCandidate>> {
    let mut directories = Vec::new();

    for entry in WalkDir::new(session_root)
        .follow_links(false)
        .min_depth(3)
        .max_depth(3)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_dir() {
            continue;
        }

        let relative_dir = entry
            .path()
            .strip_prefix(session_root)
            .with_context(|| {
                format!(
                    "failed to resolve {} relative to {}",
                    entry.path().display(),
                    session_root.display()
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        if parse_date_from_relative_path(&relative_dir).is_none() {
            continue;
        }

        let metadata = fs::metadata(entry.path())
            .with_context(|| format!("failed to stat {}", entry.path().display()))?;
        let modified_unix_ms = metadata
            .modified()
            .with_context(|| {
                format!(
                    "failed to resolve {} relative to {}",
                    entry.path().display(),
                    session_root.display()
                )
            })?
            .duration_since(UNIX_EPOCH)
            .context("modified time predates unix epoch")?
            .as_millis() as i64;

        directories.push(DirectoryCandidate {
            relative_dir,
            absolute_dir: entry.path().to_path_buf(),
            modified_unix_ms,
        });
    }

    directories.sort_by(|left, right| left.relative_dir.cmp(&right.relative_dir));
    Ok(directories)
}

fn refresh_directory_manifest_entry(
    session_root: &Path,
    directory: &DirectoryCandidate,
    cached: Option<&CachedManifestDirectory>,
) -> Result<CachedManifestDirectory> {
    let _ = cached;

    let mut files = Vec::new();
    for entry in fs::read_dir(&directory.absolute_dir).with_context(|| {
        format!(
            "failed to read directory {}",
            directory.absolute_dir.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "failed to read directory entry {}",
                directory.absolute_dir.display()
            )
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read file type for {}", path.display()))?;
        if !file_type.is_file() {
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
            continue;
        }

        let relative_path = path
            .strip_prefix(session_root)
            .with_context(|| {
                format!(
                    "failed to resolve {} relative to {}",
                    path.display(),
                    session_root.display()
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let metadata =
            fs::metadata(&path).with_context(|| format!("failed to stat {}", path.display()))?;
        let modified_unix_ms = metadata
            .modified()
            .with_context(|| format!("failed to read modified time for {}", path.display()))?
            .duration_since(UNIX_EPOCH)
            .context("modified time predates unix epoch")?
            .as_millis() as i64;

        files.push(CachedManifestFile {
            relative_path,
            file_size: metadata.len(),
            modified_unix_ms,
        });
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

    Ok(CachedManifestDirectory {
        relative_dir: directory.relative_dir.clone(),
        modified_unix_ms: directory.modified_unix_ms,
        files,
    })
}

fn is_cache_hit(candidate: &FileCandidate, cached: &CachedSessionSummary) -> bool {
    candidate.file_size == cached.file_size && candidate.modified_unix_ms == cached.modified_unix_ms
}

pub fn scan_sessions(options: ScanOptions<'_>) -> Result<Vec<SessionSummary>> {
    let use_manifest_cache = options.refresh_cache || !options.cache_path.exists();
    let files = discover_files(
        options.session_root,
        options.cache_path,
        options.since,
        options.until,
        use_manifest_cache,
    )?;
    let cached_entries = if options.refresh_cache {
        Vec::new()
    } else {
        load_cache(options.cache_path)?
    };
    let cached_by_path = cached_entries
        .into_iter()
        .map(|entry| (entry.session.session_path.clone(), entry))
        .collect::<HashMap<_, _>>();

    let parsed_entries = files
        .par_iter()
        .map(|candidate| {
            if let Some(cached) = cached_by_path.get(&candidate.relative_path) {
                if is_cache_hit(candidate, cached) {
                    return Ok(cached.clone());
                }
            }

            let session = parse_session_file(options.session_root, &candidate.absolute_path)?;
            Ok(CachedSessionSummary {
                file_size: candidate.file_size,
                modified_unix_ms: candidate.modified_unix_ms,
                session,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    save_cache(options.cache_path, &parsed_entries)?;

    Ok(parsed_entries
        .into_iter()
        .map(|entry| entry.session)
        .collect::<Vec<_>>())
}

pub fn scan_full_daily_rows(
    session_root: &Path,
    cache_path: &Path,
    timezone: chrono_tz::Tz,
) -> Result<Vec<ReportRow>> {
    let files = discover_files(session_root, cache_path, None, None, true)?;
    let per_file_rows = files
        .par_iter()
        .map(|candidate| {
            aggregate_session_file(
                session_root,
                &candidate.absolute_path,
                timezone,
                GroupBy::Day,
                None,
                None,
            )
        })
        .collect::<Result<Vec<_>>>()?;

    let mut merged = BTreeMap::<String, ReportRow>::new();
    for rows in per_file_rows {
        for row in rows {
            let merged_row = merged.entry(row.key.clone()).or_insert_with(|| ReportRow {
                key: row.key,
                usage: Default::default(),
                models: Default::default(),
            });
            merged_row.usage.add_assign(&row.usage);
            for (model, totals) in row.models {
                let model_totals = merged_row.models.entry(model).or_default();
                model_totals.usage.add_assign(&totals.usage);
                model_totals.is_fallback |= totals.is_fallback;
            }
        }
    }

    Ok(merged.into_values().collect())
}

#[cfg(test)]
#[path = "../tests/unit/scanner_tests.rs"]
mod tests;
