use super::{ScanOptions, scan_sessions};
use crate::cache::load_cache;
use chrono::NaiveDate;
use std::fs;

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

    let full_sessions = scan_sessions(ScanOptions {
        session_root: &session_root,
        cache_path: &temp_dir.path().join("full-session-cache.bin"),
        since: None,
        until: None,
        refresh_cache: true,
    })
    .expect("scan full sessions");
    let total_tokens = full_sessions
        .iter()
        .flat_map(|session| &session.events)
        .map(|event| event.usage.total_tokens)
        .sum::<u64>();
    assert_eq!(total_tokens, 1750);
}

#[test]
fn date_filtered_scan_preserves_cached_sessions_outside_the_window() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let session_root = temp_dir.path().join("sessions");
    let cache_path = temp_dir.path().join("session-cache.bin");

    for (date, id) in [
        ("2026/01/01", "019eeee0-0000-7000-8000-000000000011"),
        ("2026/03/01", "019eeee0-0000-7000-8000-000000000012"),
    ] {
        let day_dir = session_root.join(date);
        fs::create_dir_all(&day_dir).expect("day dir");
        fs::write(
            day_dir.join(format!("rollout-{id}.jsonl")),
            [
                format!(
                    r#"{{"timestamp":"{}T00:00:00Z","type":"session_meta","payload":{{"id":"{id}"}}}}"#,
                    date.replace('/', "-")
                ),
                format!(
                    r#"{{"timestamp":"{}T00:00:01Z","type":"event_msg","payload":{{"type":"token_count","model":"gpt-5.4","info":{{"last_token_usage":{{"input_tokens":100,"output_tokens":20,"total_tokens":120}}}}}}}}"#,
                    date.replace('/', "-")
                ),
            ]
            .join("\n"),
        )
        .expect("session file");
    }

    scan_sessions(ScanOptions {
        session_root: &session_root,
        cache_path: &cache_path,
        since: None,
        until: None,
        refresh_cache: true,
    })
    .expect("full scan");

    scan_sessions(ScanOptions {
        session_root: &session_root,
        cache_path: &cache_path,
        since: Some(NaiveDate::from_ymd_opt(2026, 3, 1).expect("since")),
        until: Some(NaiveDate::from_ymd_opt(2026, 3, 1).expect("until")),
        refresh_cache: false,
    })
    .expect("filtered scan");

    assert_eq!(load_cache(&cache_path).expect("load cache").len(), 2);
}

#[cfg(unix)]
#[test]
fn unchanged_warm_scan_does_not_replace_the_session_cache() {
    use std::os::unix::fs::MetadataExt;

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let session_root = temp_dir.path().join("sessions");
    let day_dir = session_root.join("2026/03/01");
    let cache_path = temp_dir.path().join("session-cache.bin");
    fs::create_dir_all(&day_dir).expect("day dir");
    fs::write(
        day_dir.join("rollout-1.jsonl"),
        r#"{"timestamp":"2026-03-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","model":"gpt-5.4","info":{"last_token_usage":{"input_tokens":100,"output_tokens":20,"total_tokens":120}}}}"#,
    )
    .expect("session file");

    let options = || ScanOptions {
        session_root: &session_root,
        cache_path: &cache_path,
        since: None,
        until: None,
        refresh_cache: false,
    };
    scan_sessions(options()).expect("first scan");
    let first_inode = fs::metadata(&cache_path).expect("first cache").ino();

    scan_sessions(options()).expect("warm scan");
    let second_inode = fs::metadata(&cache_path).expect("second cache").ino();

    assert_eq!(second_inode, first_inode);
}
