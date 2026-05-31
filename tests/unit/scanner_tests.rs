use super::{ScanOptions, discover_files, manifest_path_for, scan_full_daily_rows, scan_sessions};
use chrono::NaiveDate;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

#[test]
fn reuses_manifest_file_path_next_to_session_cache() {
    let manifest_path = manifest_path_for(Path::new("/tmp/session-cache-v1.bin"));
    assert_eq!(
        manifest_path,
        PathBuf::from("/tmp/session-cache-v1-manifest.bin")
    );
}

#[test]
fn discovers_files_and_populates_manifest() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let session_root = temp_dir.path().join("sessions");
    let day_dir = session_root.join("2026/03/06");
    fs::create_dir_all(&day_dir).expect("mkdirs");
    fs::write(day_dir.join("rollout-1.jsonl"), "{}\n").expect("write file");
    let cache_path = temp_dir.path().join("session-cache.bin");

    let files = discover_files(&session_root, &cache_path, None, None, true).expect("discover");

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].relative_path, "2026/03/06/rollout-1.jsonl");
    assert!(manifest_path_for(&cache_path).exists());
}

#[test]
fn refreshes_manifest_file_metadata_for_past_directories() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let session_root = temp_dir.path().join("sessions");
    let day_dir = session_root.join("2000/01/01");
    fs::create_dir_all(&day_dir).expect("mkdirs");
    let file_path = day_dir.join("rollout-1.jsonl");
    fs::write(&file_path, "{}\n").expect("write file");
    let cache_path = temp_dir.path().join("session-cache.bin");

    let first = discover_files(&session_root, &cache_path, None, None, true).expect("first");
    thread::sleep(Duration::from_millis(10));
    fs::write(&file_path, "{\"a\":1}\n{\"b\":2}\n").expect("rewrite file");

    let second = discover_files(&session_root, &cache_path, None, None, true).expect("second");

    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert!(second[0].file_size > first[0].file_size);
}

#[test]
fn filters_parent_history_when_parent_file_is_outside_date_window() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let session_root = temp_dir.path().join("sessions");
    let parent_dir = session_root.join("2026/02/01");
    let child_dir = session_root.join("2026/03/06");
    fs::create_dir_all(&parent_dir).expect("parent dir");
    fs::create_dir_all(&child_dir).expect("child dir");

    let parent_id = "019eeee0-0000-7000-8000-000000000001";
    let child_id = "019eeee0-0000-7000-8000-000000000002";
    let parent_path = parent_dir.join(format!("rollout-2026-02-01T00-00-00-{parent_id}.jsonl"));
    let child_path = child_dir.join(format!("rollout-2026-03-06T00-00-00-{child_id}.jsonl"));

    fs::write(
        &parent_path,
        [
            format!(
                r#"{{"timestamp":"2026-02-01T00:00:00Z","type":"session_meta","payload":{{"id":"{parent_id}","cwd":"/Users/jaewon/sources"}}}}"#
            ),
            r#"{"timestamp":"2026-02-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2-codex"}}"#.to_string(),
            r#"{"timestamp":"2026-02-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":100,"output_tokens":200,"reasoning_output_tokens":20,"total_tokens":1200}}}}"#.to_string(),
        ]
        .join("\n"),
    )
    .expect("write parent");
    fs::write(
        &child_path,
        [
            format!(
                r#"{{"timestamp":"2026-03-06T00:00:00Z","type":"session_meta","payload":{{"id":"{child_id}","forked_from_id":"{parent_id}","cwd":"/Users/jaewon/sources"}}}}"#
            ),
            r#"{"timestamp":"2026-03-06T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2-codex"}}"#.to_string(),
            r#"{"timestamp":"2026-03-06T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":100,"output_tokens":200,"reasoning_output_tokens":20,"total_tokens":1200}}}}"#.to_string(),
            r#"{"timestamp":"2026-03-06T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1500,"cached_input_tokens":150,"output_tokens":250,"reasoning_output_tokens":20,"total_tokens":1750}}}}"#.to_string(),
        ]
        .join("\n"),
    )
    .expect("write child");

    let sessions = scan_sessions(ScanOptions {
        session_root: &session_root,
        cache_path: &temp_dir.path().join("session-cache.bin"),
        since: Some(NaiveDate::from_ymd_opt(2026, 3, 6).expect("since")),
        until: Some(NaiveDate::from_ymd_opt(2026, 3, 6).expect("until")),
        refresh_cache: true,
    })
    .expect("scan");

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].events.len(), 1);
    assert_eq!(sessions[0].events[0].usage.total_tokens, 550);

    let rows = scan_full_daily_rows(
        &session_root,
        &temp_dir.path().join("daily-session-cache.bin"),
        chrono_tz::UTC,
    )
    .expect("scan full daily rows");
    let total_tokens = rows.iter().map(|row| row.usage.total_tokens).sum::<u64>();
    assert_eq!(total_tokens, 1750);
}
