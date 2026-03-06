use super::{GroupBy, aggregate_usage};
use crate::types::{SessionSummary, Usage, UsageEvent};
use chrono::{TimeZone, Utc};

#[test]
fn groups_usage_by_day_in_requested_timezone() {
    let sessions = vec![SessionSummary {
        session_id: "2026/03/06/rollout-1".to_string(),
        session_path: "2026/03/06/rollout-1.jsonl".to_string(),
        directory: Some("/Users/jaewon/sources/front-web-www".to_string()),
        events: vec![
            UsageEvent {
                timestamp_unix_ms: Utc
                    .with_ymd_and_hms(2026, 3, 5, 15, 30, 0)
                    .single()
                    .expect("timestamp")
                    .timestamp_millis(),
                model: "gpt-5.2-codex".to_string(),
                is_fallback_model: false,
                usage: Usage {
                    input_tokens: 100,
                    cached_input_tokens: 0,
                    output_tokens: 10,
                    reasoning_output_tokens: 0,
                    total_tokens: 110,
                },
            },
            UsageEvent {
                timestamp_unix_ms: Utc
                    .with_ymd_and_hms(2026, 3, 5, 17, 30, 0)
                    .single()
                    .expect("timestamp")
                    .timestamp_millis(),
                model: "gpt-5.2-codex".to_string(),
                is_fallback_model: false,
                usage: Usage {
                    input_tokens: 200,
                    cached_input_tokens: 0,
                    output_tokens: 20,
                    reasoning_output_tokens: 0,
                    total_tokens: 220,
                },
            },
        ],
    }];

    let rows = aggregate_usage(
        &sessions,
        chrono_tz::Asia::Seoul,
        GroupBy::Day,
        None,
        None,
        false,
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key, "2026-03-06");
    assert_eq!(rows[0].usage.input_tokens, 300);
    assert_eq!(rows[0].usage.total_tokens, 330);
}

#[test]
fn splits_daily_rows_by_model_when_requested() {
    let sessions = vec![SessionSummary {
        session_id: "2026/03/06/rollout-1".to_string(),
        session_path: "2026/03/06/rollout-1.jsonl".to_string(),
        directory: Some("/Users/jaewon/sources/front-web-www".to_string()),
        events: vec![
            UsageEvent {
                timestamp_unix_ms: Utc
                    .with_ymd_and_hms(2026, 3, 5, 15, 30, 0)
                    .single()
                    .expect("timestamp")
                    .timestamp_millis(),
                model: "gpt-5.2-codex".to_string(),
                is_fallback_model: false,
                usage: Usage {
                    input_tokens: 100,
                    cached_input_tokens: 0,
                    output_tokens: 10,
                    reasoning_output_tokens: 0,
                    total_tokens: 110,
                },
            },
            UsageEvent {
                timestamp_unix_ms: Utc
                    .with_ymd_and_hms(2026, 3, 5, 16, 30, 0)
                    .single()
                    .expect("timestamp")
                    .timestamp_millis(),
                model: "gpt-5.4".to_string(),
                is_fallback_model: false,
                usage: Usage {
                    input_tokens: 200,
                    cached_input_tokens: 0,
                    output_tokens: 20,
                    reasoning_output_tokens: 0,
                    total_tokens: 220,
                },
            },
        ],
    }];

    let rows = aggregate_usage(
        &sessions,
        chrono_tz::Asia::Seoul,
        GroupBy::Day,
        None,
        None,
        true,
    );

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].key, "2026-03-06");
    assert_eq!(rows[1].key, "2026-03-06");
    assert_eq!(rows[0].models.len(), 1);
    assert_eq!(rows[1].models.len(), 1);
}
