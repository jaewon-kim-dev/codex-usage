use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
}

impl Usage {
    pub fn add_assign(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.cached_input_tokens += other.cached_input_tokens;
        self.output_tokens += other.output_tokens;
        self.reasoning_output_tokens += other.reasoning_output_tokens;
        self.total_tokens += other.total_tokens;
    }

    pub fn billable_input_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_sub(self.cached_input_tokens.min(self.input_tokens))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageEvent {
    pub timestamp_unix_ms: i64,
    pub model: String,
    pub is_fallback_model: bool,
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub session_path: String,
    pub directory: Option<String>,
    pub events: Vec<UsageEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedSessionSummary {
    pub file_size: u64,
    pub modified_unix_ms: i64,
    pub session: SessionSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedManifestFile {
    pub relative_path: String,
    pub file_size: u64,
    pub modified_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedManifestDirectory {
    pub relative_dir: String,
    pub modified_unix_ms: i64,
    pub files: Vec<CachedManifestFile>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTotals {
    pub usage: Usage,
    pub is_fallback: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRow {
    pub key: String,
    pub usage: Usage,
    pub models: BTreeMap<String, ModelTotals>,
}

#[cfg(test)]
mod tests {
    use super::Usage;

    #[test]
    fn calculates_billable_input_tokens_without_cached_tokens() {
        let usage = Usage {
            input_tokens: 1_000,
            cached_input_tokens: 250,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 1_000,
        };

        assert_eq!(usage.billable_input_tokens(), 750);
    }
}
