use super::wire::{
    EventPayload, LineEnvelope, LineKindHint, SessionMetaPayload, TOKEN_COUNT_PATTERN,
    line_kind_hint, normalized_line_key, parse_timestamp_unix_ms, trim_ascii_whitespace,
};
use anyhow::{Context, Result};
use memchr::memmem;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

const INHERITED_PREFIX_END_GAP_MS: i64 = 1_000;

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

    pub(super) fn consume_if_duplicate(&mut self, trimmed: &[u8]) -> bool {
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
pub(super) struct InheritedPrefixState {
    first_session_meta_seen: bool,
    first_session_is_forked: bool,
    embedded_source_meta_seen: bool,
    skipping: bool,
    saw_prefix_token_count: bool,
    previous_timestamp_unix_ms: Option<i64>,
}

impl InheritedPrefixState {
    pub(super) fn observe_session_meta(
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

    pub(super) fn observe_non_token_line(&mut self, timestamp_unix_ms: Option<i64>) {
        self.end_prefix_if_live_gap(timestamp_unix_ms);
        self.observe_timestamp(timestamp_unix_ms);
    }

    pub(super) fn observe_token_count(&mut self, timestamp_unix_ms: Option<i64>) -> bool {
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
