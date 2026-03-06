use crate::types::{ModelTotals, ReportRow, SessionSummary, Usage, UsageEvent};
use chrono::{NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub enum GroupBy {
    Day,
    Month,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRow {
    pub date_key: String,
    pub session_id: String,
    pub session_file: String,
    pub directory: String,
    pub last_activity_unix_ms: i64,
    pub usage: Usage,
    pub models: BTreeMap<String, ModelTotals>,
}

fn event_date(timestamp_unix_ms: i64, timezone: Tz) -> Option<NaiveDate> {
    Utc.timestamp_millis_opt(timestamp_unix_ms)
        .single()
        .map(|timestamp| timestamp.with_timezone(&timezone).date_naive())
}

fn event_key(timestamp_unix_ms: i64, timezone: Tz, group_by: &GroupBy) -> Option<String> {
    let timestamp = Utc.timestamp_millis_opt(timestamp_unix_ms).single()?;
    let zoned = timestamp.with_timezone(&timezone);
    Some(match group_by {
        GroupBy::Day => zoned.format("%Y-%m-%d").to_string(),
        GroupBy::Month => zoned.format("%Y-%m").to_string(),
    })
}

fn row_storage_key(key: &str, event: &UsageEvent, split_by_model: bool) -> String {
    if split_by_model {
        format!("{key}\u{1f}{}", event.model)
    } else {
        key.to_string()
    }
}

pub fn accumulate_event(
    rows: &mut BTreeMap<String, ReportRow>,
    event: &UsageEvent,
    timezone: Tz,
    group_by: &GroupBy,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
    split_by_model: bool,
) {
    let Some(event_date) = event_date(event.timestamp_unix_ms, timezone) else {
        return;
    };
    if since.is_some_and(|since| event_date < since) {
        return;
    }
    if until.is_some_and(|until| event_date > until) {
        return;
    }
    let Some(key) = event_key(event.timestamp_unix_ms, timezone, group_by) else {
        return;
    };
    let storage_key = row_storage_key(&key, event, split_by_model);

    let row = rows.entry(storage_key).or_insert_with(|| ReportRow {
        key,
        usage: Usage::default(),
        models: BTreeMap::new(),
    });
    row.usage.add_assign(&event.usage);

    let model_totals = row
        .models
        .entry(event.model.clone())
        .or_insert_with(ModelTotals::default);
    model_totals.usage.add_assign(&event.usage);
    model_totals.is_fallback |= event.is_fallback_model;
}

pub fn aggregate_usage(
    sessions: &[SessionSummary],
    timezone: Tz,
    group_by: GroupBy,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
    split_by_model: bool,
) -> Vec<ReportRow> {
    let mut rows = BTreeMap::<String, ReportRow>::new();

    for session in sessions {
        for event in &session.events {
            accumulate_event(
                &mut rows,
                event,
                timezone,
                &group_by,
                since,
                until,
                split_by_model,
            );
        }
    }

    rows.into_values().collect()
}

pub fn aggregate_sessions(
    sessions: &[SessionSummary],
    timezone: Tz,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
) -> Vec<SessionRow> {
    let mut rows = Vec::new();

    for session in sessions {
        let mut usage = Usage::default();
        let mut models = BTreeMap::<String, ModelTotals>::new();
        let mut last_activity_unix_ms = i64::MIN;
        let mut date_key = None;

        for event in &session.events {
            let Some(event_date) = event_date(event.timestamp_unix_ms, timezone) else {
                continue;
            };
            if since.is_some_and(|since| event_date < since) {
                continue;
            }
            if until.is_some_and(|until| event_date > until) {
                continue;
            }

            usage.add_assign(&event.usage);
            let model_totals = models
                .entry(event.model.clone())
                .or_insert_with(ModelTotals::default);
            model_totals.usage.add_assign(&event.usage);
            model_totals.is_fallback |= event.is_fallback_model;

            if event.timestamp_unix_ms > last_activity_unix_ms {
                last_activity_unix_ms = event.timestamp_unix_ms;
                date_key = Some(event_date.format("%Y-%m-%d").to_string());
            }
        }

        if last_activity_unix_ms == i64::MIN {
            continue;
        }

        let session_file = session
            .session_path
            .rsplit('/')
            .next()
            .unwrap_or(session.session_path.as_str())
            .trim_end_matches(".jsonl")
            .to_string();
        rows.push(SessionRow {
            date_key: date_key.unwrap_or_default(),
            session_id: session.session_id.clone(),
            session_file,
            directory: session.directory.clone().unwrap_or_else(|| "-".to_string()),
            last_activity_unix_ms,
            usage,
            models,
        });
    }

    rows.sort_by(|left, right| {
        right
            .last_activity_unix_ms
            .cmp(&left.last_activity_unix_ms)
            .then_with(|| left.session_file.cmp(&right.session_file))
    });
    rows
}

#[cfg(test)]
mod tests {
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
}
