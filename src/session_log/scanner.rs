use crate::cache::{CacheLoad, load_cache_state, save_cache};
use crate::parser::{
    DuplicateLineFilter, parse_session_file_with_duplicate_filter, read_session_file_identity,
};
use crate::types::{CachedSessionSummary, SessionSummary};
use anyhow::{Context, Result};
use chrono::{Duration, NaiveDate};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
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
struct ChildParentSpec {
    child_relative_path: String,
    parent_id: String,
    child_started_at_unix_ms: Option<i64>,
}

enum SessionEntryPlan {
    Cached(CachedSessionSummary),
    Parse(FileCandidate),
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
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
) -> Result<Vec<FileCandidate>> {
    if !session_root.exists() {
        return Ok(Vec::new());
    }
    if !session_root.is_dir() {
        anyhow::bail!("{} is not a directory", session_root.display());
    }

    discover_files_direct(session_root, since, until)
}

fn discover_files_direct(
    session_root: &Path,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
) -> Result<Vec<FileCandidate>> {
    let mut files = Vec::new();

    for entry in WalkDir::new(session_root).follow_links(false) {
        let entry = entry.with_context(|| {
            format!(
                "failed to walk session directory {}",
                session_root.display()
            )
        })?;
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

fn is_cache_hit(candidate: &FileCandidate, cached: &CachedSessionSummary) -> bool {
    candidate.file_size == cached.file_size && candidate.modified_unix_ms == cached.modified_unix_ms
}

fn find_session_files_by_id(
    session_root: &Path,
    session_ids: &HashSet<String>,
) -> Result<HashMap<String, PathBuf>> {
    if session_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut resolved = HashMap::new();
    let suffixes = session_ids
        .iter()
        .map(|session_id| (format!("{session_id}.jsonl"), session_id))
        .collect::<Vec<_>>();
    for entry in WalkDir::new(session_root).follow_links(false) {
        let entry = entry.with_context(|| {
            format!(
                "failed to walk session directory {}",
                session_root.display()
            )
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|file_name| file_name.to_str()) else {
            continue;
        };
        let Some((_, candidate_id)) = suffixes
            .iter()
            .find(|(suffix, _)| file_name.ends_with(suffix))
        else {
            continue;
        };

        let identity = read_session_file_identity(path)?;
        if identity.session_id.as_deref() == Some(candidate_id.as_str()) {
            resolved.insert((*candidate_id).clone(), path.to_path_buf());
            if resolved.len() == session_ids.len() {
                break;
            }
        }
    }

    Ok(resolved)
}

fn duplicate_filters_for_files(
    session_root: &Path,
    files: &[FileCandidate],
) -> Result<HashMap<String, DuplicateLineFilter>> {
    let identities = files
        .par_iter()
        .map(|candidate| {
            read_session_file_identity(&candidate.absolute_path).map(|identity| {
                (
                    candidate.relative_path.clone(),
                    candidate.absolute_path.clone(),
                    identity,
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let parent_paths_by_id = identities
        .iter()
        .filter_map(|(_, absolute_path, identity)| {
            identity
                .session_id
                .as_ref()
                .map(|session_id| (session_id.clone(), absolute_path.clone()))
        })
        .collect::<HashMap<_, _>>();

    let child_specs = identities
        .iter()
        .filter_map(|(relative_path, _, identity)| {
            identity
                .forked_from_id
                .as_ref()
                .map(|parent_id| ChildParentSpec {
                    child_relative_path: relative_path.clone(),
                    parent_id: parent_id.clone(),
                    child_started_at_unix_ms: identity.started_at_unix_ms,
                })
        })
        .collect::<Vec<_>>();

    let unresolved_parent_ids = child_specs
        .iter()
        .filter(|spec| !parent_paths_by_id.contains_key(&spec.parent_id))
        .map(|spec| spec.parent_id.clone())
        .collect::<HashSet<_>>();
    let resolved_parent_paths = find_session_files_by_id(session_root, &unresolved_parent_ids)?;

    let entries = child_specs
        .par_iter()
        .map(|spec| {
            let parent_path = match parent_paths_by_id.get(&spec.parent_id) {
                Some(path) => Some(path.clone()),
                None => resolved_parent_paths.get(&spec.parent_id).cloned(),
            };
            let Some(parent_path) = parent_path else {
                return Ok(None);
            };

            let filter =
                DuplicateLineFilter::from_file(&parent_path, spec.child_started_at_unix_ms)?;
            Ok(Some((spec.child_relative_path.clone(), filter)))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(entries.into_iter().flatten().collect())
}

pub fn scan_sessions(options: ScanOptions<'_>) -> Result<Vec<SessionSummary>> {
    let cache_existed = options.cache_path.exists();
    let files = discover_files(options.session_root, options.since, options.until)?;
    let cached = if options.refresh_cache {
        CacheLoad {
            entries: Vec::new(),
            needs_rewrite: true,
        }
    } else {
        load_cache_state(options.cache_path)?
    };
    let cache_needs_rewrite = cached.needs_rewrite;
    let mut cached_by_path = cached
        .entries
        .into_iter()
        .map(|entry| (entry.session.session_path.clone(), entry))
        .collect::<HashMap<_, _>>();
    let mut plans = Vec::with_capacity(files.len());
    let mut files_to_parse = Vec::new();
    for candidate in &files {
        match cached_by_path.remove(&candidate.relative_path) {
            Some(cached) if is_cache_hit(candidate, &cached) => {
                plans.push(SessionEntryPlan::Cached(cached));
            }
            _ => {
                files_to_parse.push(candidate.clone());
                plans.push(SessionEntryPlan::Parse(candidate.clone()));
            }
        }
    }
    let removed_cached_session =
        options.since.is_none() && options.until.is_none() && !cached_by_path.is_empty();
    let duplicate_filters = duplicate_filters_for_files(options.session_root, &files_to_parse)?;

    let parsed_entries = plans
        .into_par_iter()
        .map(|plan| match plan {
            SessionEntryPlan::Cached(cached) => Ok(cached),
            SessionEntryPlan::Parse(candidate) => {
                let session = parse_session_file_with_duplicate_filter(
                    options.session_root,
                    &candidate.absolute_path,
                    duplicate_filters.get(&candidate.relative_path).cloned(),
                )?;
                Ok(CachedSessionSummary {
                    file_size: candidate.file_size,
                    modified_unix_ms: candidate.modified_unix_ms,
                    session,
                })
            }
        })
        .collect::<Result<Vec<_>>>()?;

    let cache_changed = options.refresh_cache
        || !cache_existed
        || cache_needs_rewrite
        || !files_to_parse.is_empty()
        || removed_cached_session;
    if cache_changed {
        let mut cache_entries = parsed_entries.iter().collect::<Vec<_>>();
        if options.since.is_some() || options.until.is_some() {
            cache_entries.extend(cached_by_path.values());
        }
        cache_entries
            .sort_by(|left, right| left.session.session_path.cmp(&right.session.session_path));
        save_cache(options.cache_path, &cache_entries)?;
    }

    Ok(parsed_entries
        .into_iter()
        .map(|entry| entry.session)
        .collect::<Vec<_>>())
}

#[cfg(test)]
#[path = "../../tests/unit/scanner_tests.rs"]
mod tests;
