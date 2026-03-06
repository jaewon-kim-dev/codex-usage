use crate::report::{GroupBy, accumulate_event};
use crate::types::{ReportRow, SessionSummary, Usage, UsageEvent};
use anyhow::{Context, Result};
use chrono::DateTime;
use chrono::NaiveDate;
use chrono_tz::Tz;
use memchr::memmem;
use serde::Deserialize;
use serde_json::value::RawValue;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

const LEGACY_FALLBACK_MODEL: &str = "gpt-5";
const EVENT_MSG_PATTERN: &[u8] = br#""type":"event_msg""#;
const TURN_CONTEXT_PATTERN: &[u8] = br#""type":"turn_context""#;
const SESSION_META_PATTERN: &[u8] = br#""type":"session_meta""#;

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
    cwd: Option<String>,
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

pub fn parse_session_file(session_root: &Path, file_path: &Path) -> Result<SessionSummary> {
    let mut events = Vec::new();
    let (session_path, directory) = scan_session_file_internal(session_root, file_path, |event| {
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
    let mut rows = BTreeMap::<String, ReportRow>::new();
    let _ = scan_session_file_internal(session_root, file_path, |event| {
        accumulate_event(&mut rows, &event, timezone, &group_by, since, until, false);
    })?;
    Ok(rows.into_values().collect())
}

fn scan_session_file_internal(
    session_root: &Path,
    file_path: &Path,
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
                if let Some(payload) = envelope.payload {
                    if let Ok(meta) = serde_json::from_str::<SessionMetaPayload>(payload.get()) {
                        directory = meta.cwd.filter(|cwd| !cwd.trim().is_empty()).or(directory);
                    }
                }
            }
            "turn_context" => {
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
                if event_payload.kind != "token_count" {
                    continue;
                }

                let info = event_payload.info;
                let last_usage = info
                    .as_ref()
                    .and_then(|info| info.last_token_usage)
                    .map(RawUsage::normalize);
                let total_usage = info
                    .as_ref()
                    .and_then(|info| info.total_token_usage)
                    .map(RawUsage::normalize);

                let Some(usage) = last_usage.or_else(|| {
                    total_usage
                        .as_ref()
                        .map(|current| subtract_usage(current.clone(), previous_totals.clone()))
                }) else {
                    continue;
                };

                if let Some(total_usage) = total_usage.as_ref() {
                    previous_totals = Some(total_usage.clone());
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
mod tests {
    use super::{LineKindHint, line_kind_hint, parse_session_file};
    use std::fs;

    #[test]
    fn parses_last_token_usage_and_directory_from_session_meta() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let sessions_dir = temp_dir.path().join("sessions/2026/03/06");
        fs::create_dir_all(&sessions_dir).expect("create session dir");
        let file_path = sessions_dir.join("rollout-1.jsonl");
        fs::write(
            &file_path,
            [
                r#"{"timestamp":"2026-03-05T23:59:00Z","type":"session_meta","payload":{"cwd":"/Users/jaewon/sources/front-web-www"}}"#,
                r#"{"timestamp":"2026-03-05T23:59:01Z","type":"turn_context","payload":{"model":"gpt-5.2-codex"}}"#,
                r#"{"timestamp":"2026-03-05T23:59:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1200,"cached_input_tokens":100,"output_tokens":300,"reasoning_output_tokens":40,"total_tokens":1500}}}}"#,
            ]
            .join("\n"),
        )
        .expect("write session file");

        let summary =
            parse_session_file(&temp_dir.path().join("sessions"), &file_path).expect("parse");

        assert_eq!(summary.session_id, "2026/03/06/rollout-1");
        assert_eq!(
            summary.directory.as_deref(),
            Some("/Users/jaewon/sources/front-web-www")
        );
        assert_eq!(summary.events.len(), 1);
        assert_eq!(summary.events[0].model, "gpt-5.2-codex");
        assert_eq!(summary.events[0].usage.total_tokens, 1500);
        assert_eq!(summary.events[0].usage.cached_input_tokens, 100);
    }

    #[test]
    fn derives_deltas_from_total_token_usage_when_last_usage_is_missing() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let sessions_dir = temp_dir.path().join("sessions/2026/03/06");
        fs::create_dir_all(&sessions_dir).expect("create session dir");
        let file_path = sessions_dir.join("rollout-2.jsonl");
        fs::write(
            &file_path,
            [
                r#"{"timestamp":"2026-03-05T23:59:01Z","type":"turn_context","payload":{"model":"gpt-5.2-codex"}}"#,
                r#"{"timestamp":"2026-03-05T23:59:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":100,"output_tokens":200,"reasoning_output_tokens":20,"total_tokens":1200}}}}"#,
                r#"{"timestamp":"2026-03-05T23:59:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1600,"cached_input_tokens":300,"output_tokens":260,"reasoning_output_tokens":20,"total_tokens":1860}}}}"#,
            ]
            .join("\n"),
        )
        .expect("write session file");

        let summary =
            parse_session_file(&temp_dir.path().join("sessions"), &file_path).expect("parse");

        assert_eq!(summary.events.len(), 2);
        assert_eq!(summary.events[0].usage.total_tokens, 1200);
        assert_eq!(summary.events[1].usage.input_tokens, 600);
        assert_eq!(summary.events[1].usage.cached_input_tokens, 200);
        assert_eq!(summary.events[1].usage.output_tokens, 60);
        assert_eq!(summary.events[1].usage.total_tokens, 660);
    }

    #[test]
    fn keeps_latest_non_empty_directory_from_turn_context() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let sessions_dir = temp_dir.path().join("sessions/2026/03/06");
        fs::create_dir_all(&sessions_dir).expect("create session dir");
        let file_path = sessions_dir.join("rollout-3.jsonl");
        fs::write(
            &file_path,
            [
                r#"{"timestamp":"2026-03-05T23:59:00Z","type":"session_meta","payload":{"cwd":"/Users/jaewon/sources/repo-a"}}"#,
                r#"{"timestamp":"2026-03-05T23:59:01Z","type":"turn_context","payload":{"cwd":"/Users/jaewon/sources/repo-b","model":"gpt-5.2-codex"}}"#,
                r#"{"timestamp":"2026-03-05T23:59:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}}"#,
            ]
            .join("\n"),
        )
        .expect("write session file");

        let summary =
            parse_session_file(&temp_dir.path().join("sessions"), &file_path).expect("parse");

        assert_eq!(
            summary.directory.as_deref(),
            Some("/Users/jaewon/sources/repo-b")
        );
    }

    #[test]
    fn skips_non_usage_lines_with_fast_hint() {
        assert_eq!(
            line_kind_hint(br#"{"type":"response_item","payload":{"type":"message"}}"#),
            LineKindHint::Other
        );
        assert_eq!(
            line_kind_hint(br#"{"type":"turn_context","payload":{"model":"gpt-5.4-codex"}}"#),
            LineKindHint::TurnContext
        );
    }
}
