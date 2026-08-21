use crate::types::Usage;
use chrono::DateTime;
use memchr::memmem;
use serde::Deserialize;
use serde_json::{Value, value::RawValue};

pub(super) const EVENT_MSG_PATTERN: &[u8] = br#""type":"event_msg""#;
pub(super) const TOKEN_COUNT_PATTERN: &[u8] = br#""type":"token_count""#;
const TURN_CONTEXT_PATTERN: &[u8] = br#""type":"turn_context""#;
const SESSION_META_PATTERN: &[u8] = br#""type":"session_meta""#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LineKindHint {
    EventMsg,
    TurnContext,
    SessionMeta,
    Other,
}

#[derive(Debug, Deserialize)]
pub(super) struct LineEnvelope {
    pub(super) timestamp: Option<String>,
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) payload: Option<Box<RawValue>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SessionMetaPayload {
    pub(super) id: Option<String>,
    pub(super) forked_from_id: Option<String>,
    pub(super) cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TurnContextPayload {
    pub(super) cwd: Option<String>,
    pub(super) model: Option<String>,
    pub(super) model_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct EventPayload {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) info: Option<TokenInfo>,
    pub(super) model: Option<String>,
    pub(super) model_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TokenInfo {
    pub(super) last_token_usage: Option<RawUsage>,
    pub(super) total_token_usage: Option<RawUsage>,
    pub(super) model: Option<String>,
    pub(super) model_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub(super) struct RawUsage {
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_output_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

impl RawUsage {
    pub(super) fn normalize(self) -> Usage {
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

pub(super) fn subtract_usage(current: Usage, previous: Option<Usage>) -> Usage {
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

pub(super) fn parse_timestamp_unix_ms(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

pub(super) fn first_non_empty(candidates: &[Option<String>]) -> Option<String> {
    candidates
        .iter()
        .flatten()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

pub(super) fn line_kind_hint(bytes: &[u8]) -> LineKindHint {
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

pub(super) fn normalized_line_key(trimmed: &[u8]) -> Option<Vec<u8>> {
    let mut value = serde_json::from_slice::<Value>(trimmed).ok()?;
    value.as_object_mut()?.remove("timestamp");
    serde_json::to_vec(&value).ok()
}
