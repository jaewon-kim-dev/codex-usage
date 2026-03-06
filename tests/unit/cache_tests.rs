use super::{load_cache, load_manifest, save_cache, save_manifest};
use crate::types::{
    CachedManifestDirectory, CachedManifestFile, CachedSessionSummary, SessionSummary, Usage,
    UsageEvent,
};

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

    save_cache(&cache_path, &[entry.clone()]).expect("save cache");
    let restored = load_cache(&cache_path).expect("load cache");

    assert_eq!(restored, vec![entry]);
}

#[test]
fn roundtrips_manifest_entries() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let manifest_path = temp_dir.path().join("manifest-cache.bin");
    let entry = CachedManifestDirectory {
        relative_dir: "2026/03/06".to_string(),
        modified_unix_ms: 1_772_723_600_000,
        files: vec![CachedManifestFile {
            relative_path: "2026/03/06/rollout-1.jsonl".to_string(),
            file_size: 4096,
            modified_unix_ms: 1_772_723_600_000,
        }],
    };

    save_manifest(&manifest_path, &[entry.clone()]).expect("save manifest");
    let restored = load_manifest(&manifest_path).expect("load manifest");

    assert_eq!(restored, vec![entry]);
}
