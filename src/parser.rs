use crate::report::{GroupBy, accumulate_event};
use crate::types::{ReportRow, SessionSummary, Usage, UsageEvent};
use anyhow::{Context, Result};
use chrono::DateTime;
use chrono::NaiveDate;
use chrono_tz::Tz;
use memchr::memmem;
use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

const LEGACY_FALLBACK_MODEL: &str = "gpt-5";
const EVENT_MSG_PATTERN: &[u8] = br#""type":"event_msg""#;
const TOKEN_COUNT_PATTERN: &[u8] = br#""type":"token_count""#;
const TURN_CONTEXT_PATTERN: &[u8] = br#""type":"turn_context""#;
const SESSION_META_PATTERN: &[u8] = br#""type":"session_meta""#;
const INHERITED_PREFIX_END_GAP_MS: i64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKindHint {
    EventMsg,
    TurnContext,
    SessionMeta,
    Other,
}

#[derive(Debug, Deserialize)]
struct LineEnvelope {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    payload: Option<Box<RawValue>>,
}

#[derive(Debug, Deserialize)]
struct SessionMetaPayload {
    id: Option<String>,
    forked_from_id: Option<String>,
    cwd: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionFileIdentity {
    pub session_id: Option<String>,
    pub forked_from_id: Option<String>,
    pub started_at_unix_ms: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct TurnContextPayload {
    cwd: Option<String>,
    model: Option<String>,
    model_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EventPayload {
    #[serde(rename = "type")]
    kind: String,
    info: Option<TokenInfo>,
    model: Option<String>,
    model_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenInfo {
    last_token_usage: Option<RawUsage>,
    total_token_usage: Option<RawUsage>,
    model: Option<String>,
    model_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct RawUsage {
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_output_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

impl RawUsage {
    fn normalize(self) -> Usage {
        let input = self.input_tokens.unwrap_or_default();
        let cached = self
            .cached_input_tokens
            .or(self.cache_read_input_tokens)
            .unwrap_or_default()
            .min(input);
        let output = self.output_tokens.unwrap_or_default();
        let reasoning = self.reasoning_output_tokens.unwrap_or_default();
        let total = self.total_tokens.unwrap_or(input + output);

        Usage {
            input_tokens: input,
            cached_input_tokens: cached,
            output_tokens: output,
            reasoning_output_tokens: reasoning,
            total_tokens: total,
        }
    }
}

fn subtract_usage(current: Usage, previous: Option<Usage>) -> Usage {
    let previous = previous.unwrap_or_default();
    Usage {
        input_tokens: current.input_tokens.saturating_sub(previous.input_tokens),
        cached_input_tokens: current
            .cached_input_tokens
            .saturating_sub(previous.cached_input_tokens),
        output_tokens: current.output_tokens.saturating_sub(previous.output_tokens),
        reasoning_output_tokens: current
            .reasoning_output_tokens
            .saturating_sub(previous.reasoning_output_tokens),
        total_tokens: current.total_tokens.saturating_sub(previous.total_tokens),
    }
}

fn parse_timestamp_unix_ms(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

fn first_non_empty(candidates: &[Option<String>]) -> Option<String> {
    candidates
        .iter()
        .flatten()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while let Some(first) = bytes.first() {
        if first.is_ascii_whitespace() {
            bytes = &bytes[1..];
        } else {
            break;
        }
    }

    while let Some(last) = bytes.last() {
        if last.is_ascii_whitespace() {
            bytes = &bytes[..bytes.len() - 1];
        } else {
            break;
        }
    }

    bytes
}

fn line_kind_hint(bytes: &[u8]) -> LineKindHint {
    if memmem::find(bytes, EVENT_MSG_PATTERN).is_some() {
        return LineKindHint::EventMsg;
    }
    if memmem::find(bytes, TURN_CONTEXT_PATTERN).is_some() {
        return LineKindHint::TurnContext;
    }
    if memmem::find(bytes, SESSION_META_PATTERN).is_some() {
        return LineKindHint::SessionMeta;
    }
    LineKindHint::Other
}

fn normalized_line_key(trimmed: &[u8]) -> Option<Vec<u8>> {
    let mut value = serde_json::from_slice::<Value>(trimmed).ok()?;
    value.as_object_mut()?.remove("timestamp");
    serde_json::to_vec(&value).ok()
}

#[derive(Debug, Clone, Default)]
pub struct DuplicateLineFilter {
    remaining: HashMap<Vec<u8>, usize>,
}

impl DuplicateLineFilter {
    pub fn from_file(file_path: &Path, before_or_at_unix_ms: Option<i64>) -> Result<Self> {
        let file = File::open(file_path).with_context(|| {
            format!("failed to open parent session file {}", file_path.display())
        })?;
        let mut reader = BufReader::new(file);
        let mut remaining = HashMap::<Vec<u8>, usize>::new();
        let mut line_buffer = Vec::<u8>::with_capacity(8 * 1024);

        loop {
            line_buffer.clear();
            let bytes_read = reader
                .read_until(b'\n', &mut line_buffer)
                .with_context(|| {
                    format!(
                        "failed to read line from parent session file {}",
                        file_path.display()
                    )
                })?;
            if bytes_read == 0 {
                break;
            }

            let trimmed = trim_ascii_whitespace(&line_buffer);
            if trimmed.is_empty()
                || line_kind_hint(trimmed) != LineKindHint::EventMsg
                || memmem::find(trimmed, TOKEN_COUNT_PATTERN).is_none()
            {
                continue;
            }

            let Ok(envelope) = serde_json::from_slice::<LineEnvelope>(trimmed) else {
                continue;
            };
            if envelope.kind != "event_msg" {
                continue;
            }
            if let Some(cutoff) = before_or_at_unix_ms {
                let Some(timestamp) = envelope.timestamp.as_deref() else {
                    continue;
                };
                let Some(timestamp_unix_ms) = parse_timestamp_unix_ms(timestamp) else {
                    continue;
                };
                if timestamp_unix_ms > cutoff {
                    continue;
                }
            }
            let Some(payload) = envelope.payload else {
                continue;
            };
            let Ok(event_payload) = serde_json::from_str::<EventPayload>(payload.get()) else {
                continue;
            };
            if event_payload.kind != "token_count" {
                continue;
            }
            if let Some(key) = normalized_line_key(trimmed) {
                *remaining.entry(key).or_default() += 1;
            }
        }

        Ok(Self { remaining })
    }

    fn consume_if_duplicate(&mut self, trimmed: &[u8]) -> bool {
        let Some(key) = normalized_line_key(trimmed) else {
            return false;
        };
        let Some(count) = self.remaining.get_mut(&key) else {
            return false;
        };
        if *count == 0 {
            return false;
        }
        *count -= 1;
        true
    }
}

#[derive(Debug, Default)]
struct InheritedPrefixState {
    first_session_meta_seen: bool,
    first_session_is_forked: bool,
    embedded_source_meta_seen: bool,
    skipping: bool,
    saw_prefix_token_count: bool,
    previous_timestamp_unix_ms: Option<i64>,
}

impl InheritedPrefixState {
    fn observe_session_meta(
        &mut self,
        meta: &SessionMetaPayload,
        timestamp_unix_ms: Option<i64>,
    ) -> bool {
        self.observe_timestamp(timestamp_unix_ms);

        if !self.first_session_meta_seen {
            self.first_session_meta_seen = true;
            self.first_session_is_forked = meta
                .forked_from_id
                .as_ref()
                .is_some_and(|id| !id.trim().is_empty());
            return false;
        }

        if self.first_session_is_forked && !self.embedded_source_meta_seen {
            self.embedded_source_meta_seen = true;
            self.skipping = true;
            return true;
        }

        false
    }

    fn observe_non_token_line(&mut self, timestamp_unix_ms: Option<i64>) {
        self.end_prefix_if_live_gap(timestamp_unix_ms);
        self.observe_timestamp(timestamp_unix_ms);
    }

    fn observe_token_count(&mut self, timestamp_unix_ms: Option<i64>) -> bool {
        self.end_prefix_if_live_gap(timestamp_unix_ms);
        let is_prefix = self.skipping;
        if is_prefix {
            self.saw_prefix_token_count = true;
        }
        self.observe_timestamp(timestamp_unix_ms);
        is_prefix
    }

    fn end_prefix_if_live_gap(&mut self, timestamp_unix_ms: Option<i64>) {
        if !self.skipping || !self.saw_prefix_token_count {
            return;
        }
        let Some(current) = timestamp_unix_ms else {
            return;
        };
        let Some(previous) = self.previous_timestamp_unix_ms else {
            return;
        };
        if current.saturating_sub(previous) > INHERITED_PREFIX_END_GAP_MS {
            self.skipping = false;
        }
    }

    fn observe_timestamp(&mut self, timestamp_unix_ms: Option<i64>) {
        if let Some(timestamp) = timestamp_unix_ms {
            self.previous_timestamp_unix_ms = Some(timestamp);
        }
    }
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
            session_id: non_empty(meta.id),
            forked_from_id: non_empty(meta.forked_from_id),
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
    let (session_path, directory) =
        scan_session_file_internal(session_root, file_path, duplicate_filter, |event| {
            events.push(event);
        })?;

    Ok(SessionSummary {
        session_id: session_path.trim_end_matches(".jsonl").to_string(),
        session_path,
        directory,
        events,
    })
}

pub fn aggregate_session_file(
    session_root: &Path,
    file_path: &Path,
    timezone: Tz,
    group_by: GroupBy,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
) -> Result<Vec<ReportRow>> {
    aggregate_session_file_with_duplicate_filter(
        session_root,
        file_path,
        timezone,
        group_by,
        since,
        until,
        None,
    )
}

pub fn aggregate_session_file_with_duplicate_filter(
    session_root: &Path,
    file_path: &Path,
    timezone: Tz,
    group_by: GroupBy,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
    duplicate_filter: Option<DuplicateLineFilter>,
) -> Result<Vec<ReportRow>> {
    let mut rows = BTreeMap::<String, ReportRow>::new();
    let _ = scan_session_file_internal(session_root, file_path, duplicate_filter, |event| {
        accumulate_event(&mut rows, &event, timezone, &group_by, since, until, false);
    })?;
    Ok(rows.into_values().collect())
}

fn scan_session_file_internal(
    session_root: &Path,
    file_path: &Path,
    mut duplicate_filter: Option<DuplicateLineFilter>,
    mut on_event: impl FnMut(UsageEvent),
) -> Result<(String, Option<String>)> {
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
                            directory = meta.cwd.filter(|cwd| !cwd.trim().is_empty()).or(directory);
                        }
                    }
                }
            }
            "turn_context" => {
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
                    .or(last_usage)
                else {
                    continue;
                };

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
                    let model = LEGACY_FALLBACK_MODEL.to_string();
                    current_model = Some(model.clone());
                    model
                };

                on_event(UsageEvent {
                    timestamp_unix_ms,
                    model,
                    is_fallback_model,
                    usage,
                });
            }
            _ => {}
        }
    }

    Ok((relative_path, directory))
}

#[cfg(test)]
#[path = "../tests/unit/parser_tests.rs"]
mod tests;
