pub use super::fork::DuplicateLineFilter;
use super::fork::InheritedPrefixState;
use super::wire::{
    EventPayload, LineEnvelope, LineKindHint, RawUsage, SessionMetaPayload, TurnContextPayload,
    first_non_empty, line_kind_hint, parse_timestamp_unix_ms, subtract_usage,
    trim_ascii_whitespace,
};
use crate::types::{SessionSummary, Usage, UsageEvent};
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

const UNKNOWN_MODEL: &str = "unknown";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionFileIdentity {
    pub session_id: Option<String>,
    pub forked_from_id: Option<String>,
    pub started_at_unix_ms: Option<i64>,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

pub fn read_session_file_identity(file_path: &Path) -> Result<SessionFileIdentity> {
    let file = File::open(file_path)
        .with_context(|| format!("failed to open session file {}", file_path.display()))?;
    let mut reader = BufReader::new(file);
    let mut line_buffer = Vec::<u8>::with_capacity(8 * 1024);

    loop {
        line_buffer.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut line_buffer)
            .with_context(|| format!("failed to read line from {}", file_path.display()))?;
        if bytes_read == 0 {
            break;
        }

        let trimmed = trim_ascii_whitespace(&line_buffer);
        if trimmed.is_empty() || line_kind_hint(trimmed) != LineKindHint::SessionMeta {
            continue;
        }

        let Ok(envelope) = serde_json::from_slice::<LineEnvelope>(trimmed) else {
            continue;
        };
        if envelope.kind != "session_meta" {
            continue;
        }

        let Some(payload) = envelope.payload else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<SessionMetaPayload>(payload.get()) else {
            continue;
        };

        return Ok(SessionFileIdentity {
            forked_from_id: meta.parent_id(),
            session_id: non_empty(meta.id),
            started_at_unix_ms: envelope
                .timestamp
                .as_deref()
                .and_then(parse_timestamp_unix_ms),
        });
    }

    Ok(SessionFileIdentity::default())
}

pub fn parse_session_file(session_root: &Path, file_path: &Path) -> Result<SessionSummary> {
    parse_session_file_with_duplicate_filter(session_root, file_path, None)
}

pub fn parse_session_file_with_duplicate_filter(
    session_root: &Path,
    file_path: &Path,
    duplicate_filter: Option<DuplicateLineFilter>,
) -> Result<SessionSummary> {
    let mut events = Vec::new();
    let mut unresolved_usage = Vec::new();
    let (session_path, directory, has_rewritten_timestamps) = scan_session_file_internal(
        session_root,
        file_path,
        duplicate_filter,
        |event, unresolved| {
            if unresolved {
                unresolved_usage.push(event);
            } else {
                events.push(event);
            }
        },
    )?;

    Ok(SessionSummary {
        session_id: session_path.trim_end_matches(".jsonl").to_string(),
        session_path,
        directory,
        events,
        unresolved_usage,
        has_rewritten_timestamps,
    })
}

fn scan_session_file_internal(
    session_root: &Path,
    file_path: &Path,
    mut duplicate_filter: Option<DuplicateLineFilter>,
    mut on_event: impl FnMut(UsageEvent, bool),
) -> Result<(String, Option<String>, bool)> {
    let relative_path = file_path
        .strip_prefix(session_root)
        .with_context(|| {
            format!(
                "failed to resolve {} relative to {}",
                file_path.display(),
                session_root.display()
            )
        })?
        .to_string_lossy()
        .replace('\\', "/");
    let file = File::open(file_path)
        .with_context(|| format!("failed to open session file {}", file_path.display()))?;
    let mut reader = BufReader::new(file);

    let mut directory: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut current_model_is_fallback = false;
    let mut previous_totals: Option<Usage> = None;
    let mut paginated_child = false;
    let mut paginated = false;
    let mut previous_task_start = None;
    let mut previous_usage_timestamp = None;
    let mut has_rewritten_timestamps = false;
    let mut saw_turn_context = false;
    let mut inherited_prefix = InheritedPrefixState::default();
    let mut line_buffer = Vec::<u8>::with_capacity(8 * 1024);

    loop {
        line_buffer.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut line_buffer)
            .with_context(|| format!("failed to read line from {}", file_path.display()))?;
        if bytes_read == 0 {
            break;
        }

        let trimmed = trim_ascii_whitespace(&line_buffer);
        if trimmed.is_empty() {
            continue;
        }

        let kind_hint = line_kind_hint(trimmed);
        if kind_hint == LineKindHint::Other {
            continue;
        }

        let Ok(trimmed_str) = std::str::from_utf8(trimmed) else {
            continue;
        };

        let envelope: LineEnvelope = match serde_json::from_str(trimmed_str) {
            Ok(envelope) => envelope,
            Err(_) => continue,
        };

        match envelope.kind.as_str() {
            "session_meta" => {
                let timestamp_unix_ms = envelope
                    .timestamp
                    .as_deref()
                    .and_then(parse_timestamp_unix_ms);
                if let Some(payload) = envelope.payload {
                    if let Ok(meta) = serde_json::from_str::<SessionMetaPayload>(payload.get()) {
                        let embedded_source_meta =
                            inherited_prefix.observe_session_meta(&meta, timestamp_unix_ms);
                        if !embedded_source_meta {
                            paginated = meta.history_mode.as_deref() == Some("paginated");
                            paginated_child = paginated && meta.parent_id().is_some();
                            directory = meta.cwd.filter(|cwd| !cwd.trim().is_empty()).or(directory);
                        }
                    }
                }
            }
            "turn_context" => {
                saw_turn_context = true;
                inherited_prefix.observe_non_token_line(
                    envelope
                        .timestamp
                        .as_deref()
                        .and_then(parse_timestamp_unix_ms),
                );
                if let Some(payload) = envelope.payload {
                    if let Ok(context) = serde_json::from_str::<TurnContextPayload>(payload.get()) {
                        directory = context
                            .cwd
                            .filter(|cwd| !cwd.trim().is_empty())
                            .or(directory);
                        if let Some(model) = first_non_empty(&[context.model, context.model_name]) {
                            current_model = Some(model);
                            current_model_is_fallback = false;
                        }
                    }
                }
            }
            "event_msg" => {
                let Some(timestamp) = envelope.timestamp.as_deref() else {
                    continue;
                };
                let Some(timestamp_unix_ms) = parse_timestamp_unix_ms(timestamp) else {
                    continue;
                };
                let Some(payload) = envelope.payload else {
                    continue;
                };
                let Ok(event_payload) = serde_json::from_str::<EventPayload>(payload.get()) else {
                    continue;
                };
                if paginated && event_payload.kind == "task_started" {
                    if let Some(started_at) = event_payload.started_at {
                        if let Some((previous_timestamp, previous_started_at)) = previous_task_start
                        {
                            has_rewritten_timestamps |= previous_timestamp == timestamp_unix_ms
                                && previous_started_at != started_at;
                        }
                        previous_task_start = Some((timestamp_unix_ms, started_at));
                    }
                }
                let is_inherited_prefix_usage_line = if event_payload.kind == "token_count" {
                    inherited_prefix.observe_token_count(Some(timestamp_unix_ms))
                } else {
                    inherited_prefix.observe_non_token_line(Some(timestamp_unix_ms));
                    false
                };
                if event_payload.kind != "token_count" {
                    continue;
                }
                let is_duplicate_usage_line = duplicate_filter
                    .as_mut()
                    .map(|filter| filter.consume_if_duplicate(trimmed))
                    .unwrap_or(false);

                let info = event_payload.info;
                let last_usage = info
                    .as_ref()
                    .and_then(|info| info.last_token_usage)
                    .map(RawUsage::normalize);
                let total_usage = info
                    .as_ref()
                    .and_then(|info| info.total_token_usage)
                    .map(RawUsage::normalize);

                let Some(usage) = total_usage
                    .as_ref()
                    .map(|current| subtract_usage(current.clone(), previous_totals.clone()))
                    .or(last_usage.clone())
                else {
                    continue;
                };

                if paginated
                    && previous_usage_timestamp == Some(timestamp_unix_ms)
                    && previous_totals
                        .as_ref()
                        .zip(total_usage.as_ref())
                        .is_some_and(|(previous, current)| previous != current)
                {
                    has_rewritten_timestamps = true;
                }
                previous_usage_timestamp = Some(timestamp_unix_ms);
                // Truncated child history may start with an inherited cumulative ledger.
                // Preserve unmatched checkpoints and context-less prefixes as unresolved,
                // while a first request whose last usage equals its total is self-contained.
                let unresolved_baseline = paginated_child
                    && (!saw_turn_context
                        || (previous_totals.is_none()
                            && total_usage.is_some()
                            && total_usage != last_usage));
                if let Some(total_usage) = total_usage.as_ref() {
                    previous_totals = Some(total_usage.clone());
                }

                if is_duplicate_usage_line || is_inherited_prefix_usage_line {
                    continue;
                }

                if usage.total_tokens == 0
                    && usage.input_tokens == 0
                    && usage.cached_input_tokens == 0
                    && usage.output_tokens == 0
                    && usage.reasoning_output_tokens == 0
                {
                    continue;
                }

                let extracted_model = first_non_empty(&[
                    event_payload.model,
                    event_payload.model_name,
                    info.as_ref().and_then(|info| info.model.clone()),
                    info.as_ref().and_then(|info| info.model_name.clone()),
                ]);
                let extracted_model_missing = extracted_model.is_none();

                let mut is_fallback_model = false;
                if let Some(model) = extracted_model.as_ref() {
                    current_model = Some(model.clone());
                    current_model_is_fallback = false;
                }

                let model = if let Some(model) = extracted_model.or_else(|| current_model.clone()) {
                    if extracted_model_missing && current_model_is_fallback {
                        is_fallback_model = true;
                    }
                    model
                } else {
                    is_fallback_model = true;
                    current_model_is_fallback = true;
                    let model = UNKNOWN_MODEL.to_string();
                    current_model = Some(model.clone());
                    model
                };

                on_event(
                    UsageEvent {
                        timestamp_unix_ms,
                        model,
                        is_fallback_model,
                        usage,
                    },
                    unresolved_baseline,
                );
            }
            _ => {}
        }
    }

    Ok((relative_path, directory, has_rewritten_timestamps))
}

#[cfg(test)]
#[path = "../../tests/unit/parser_tests.rs"]
mod tests;
