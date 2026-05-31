use super::{
    DuplicateLineFilter, LineKindHint, line_kind_hint, parse_session_file,
    parse_session_file_with_duplicate_filter,
};
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

    let summary = parse_session_file(&temp_dir.path().join("sessions"), &file_path).expect("parse");

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

    let summary = parse_session_file(&temp_dir.path().join("sessions"), &file_path).expect("parse");

    assert_eq!(summary.events.len(), 2);
    assert_eq!(summary.events[0].usage.total_tokens, 1200);
    assert_eq!(summary.events[1].usage.input_tokens, 600);
    assert_eq!(summary.events[1].usage.cached_input_tokens, 200);
    assert_eq!(summary.events[1].usage.output_tokens, 60);
    assert_eq!(summary.events[1].usage.total_tokens, 660);
}

#[test]
fn suppresses_forked_parent_token_counts_but_keeps_total_usage_delta_baseline() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp_dir.path().join("sessions/2026/03/06");
    fs::create_dir_all(&sessions_dir).expect("create session dir");
    let parent_path = sessions_dir.join("rollout-parent.jsonl");
    let child_path = sessions_dir.join("rollout-child.jsonl");

    fs::write(
        &parent_path,
        [
            r#"{"timestamp":"2026-03-05T23:59:00Z","type":"session_meta","payload":{"id":"parent-session","cwd":"/Users/jaewon/sources"}}"#,
            r#"{"timestamp":"2026-03-05T23:59:01Z","type":"turn_context","payload":{"model":"gpt-5.2-codex"}}"#,
            r#"{"timestamp":"2026-03-05T23:59:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":100,"output_tokens":200,"reasoning_output_tokens":20,"total_tokens":1200}}}}"#,
        ]
        .join("\n"),
    )
    .expect("write parent file");
    fs::write(
        &child_path,
        [
            r#"{"timestamp":"2026-03-05T23:59:04Z","type":"session_meta","payload":{"id":"child-session","forked_from_id":"parent-session","cwd":"/Users/jaewon/sources"}}"#,
            r#"{"timestamp":"2026-03-05T23:59:04Z","type":"turn_context","payload":{"model":"gpt-5.2-codex"}}"#,
            r#"{"timestamp":"2026-03-05T23:59:05Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":100,"output_tokens":200,"reasoning_output_tokens":20,"total_tokens":1200}}}}"#,
            r#"{"timestamp":"2026-03-05T23:59:06Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1500,"cached_input_tokens":150,"output_tokens":250,"reasoning_output_tokens":20,"total_tokens":1750}}}}"#,
        ]
        .join("\n"),
    )
    .expect("write child file");

    let filter = DuplicateLineFilter::from_file(
        &parent_path,
        Some(
            chrono::DateTime::parse_from_rfc3339("2026-03-05T23:59:04Z")
                .expect("timestamp")
                .timestamp_millis(),
        ),
    )
    .expect("filter");
    let summary = parse_session_file_with_duplicate_filter(
        &temp_dir.path().join("sessions"),
        &child_path,
        Some(filter),
    )
    .expect("parse");

    assert_eq!(summary.events.len(), 1);
    assert_eq!(summary.events[0].usage.input_tokens, 500);
    assert_eq!(summary.events[0].usage.cached_input_tokens, 50);
    assert_eq!(summary.events[0].usage.output_tokens, 50);
    assert_eq!(summary.events[0].usage.total_tokens, 550);
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

    let summary = parse_session_file(&temp_dir.path().join("sessions"), &file_path).expect("parse");

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
