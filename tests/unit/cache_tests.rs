use super::{load_cache, load_cache_state, save_cache, write_atomically};
use crate::types::{CachedSessionSummary, SessionSummary, Usage, UsageEvent};
use std::io::{self, Write};

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

    save_cache(&cache_path, std::slice::from_ref(&entry)).expect("save cache");
    let restored = load_cache(&cache_path).expect("load cache");

    assert_eq!(restored, vec![entry]);
}

#[test]
fn treats_corrupt_session_cache_as_empty() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let cache_path = temp_dir.path().join("session-cache.bin");
    std::fs::write(&cache_path, []).expect("write corrupt session cache");

    assert!(
        load_cache(&cache_path)
            .expect("load session cache")
            .is_empty()
    );
    assert!(
        load_cache_state(&cache_path)
            .expect("load cache state")
            .needs_rewrite
    );
}

#[test]
fn preserves_existing_cache_when_replacement_write_fails() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let cache_path = temp_dir.path().join("session-cache.bin");
    std::fs::write(&cache_path, b"valid cache").expect("write existing cache");

    let result = write_atomically(&cache_path, |file| {
        file.write_all(b"partial replacement")?;
        Err(io::Error::other("simulated write failure"))
    });

    assert!(result.is_err());
    assert_eq!(
        std::fs::read(&cache_path).expect("read existing cache"),
        b"valid cache"
    );
}
