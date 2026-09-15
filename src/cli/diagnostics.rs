use crate::types::{SessionSummary, Usage, UsageEvent};
use chrono::{NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Serialize;

#[derive(Debug, Default, Serialize)]
pub(super) struct Diagnostics {
    pub(super) rewritten_timestamp_sessions: Vec<String>,
    pub(super) unresolved_history_usage: Vec<UnresolvedUsage>,
}

#[derive(Debug, Serialize)]
pub(super) struct UnresolvedUsage {
    session_path: String,
    usage: Usage,
}

impl Diagnostics {
    pub(super) fn from_sessions(
        sessions: &[SessionSummary],
        timezone: Tz,
        since: Option<NaiveDate>,
        until: Option<NaiveDate>,
    ) -> Self {
        let included = |event: &&UsageEvent| {
            Utc.timestamp_millis_opt(event.timestamp_unix_ms)
                .single()
                .is_some_and(|timestamp| {
                    let date = timestamp.with_timezone(&timezone).date_naive();
                    since.is_none_or(|since| date >= since)
                        && until.is_none_or(|until| date <= until)
                })
        };
        Self {
            rewritten_timestamp_sessions: sessions
                .iter()
                .filter(|session| {
                    session.has_rewritten_timestamps
                        && session
                            .events
                            .iter()
                            .chain(&session.unresolved_usage)
                            .any(|event| included(&event))
                })
                .map(|session| session.session_path.clone())
                .collect(),
            unresolved_history_usage: sessions
                .iter()
                .filter_map(|session| {
                    let events: Vec<_> = session.unresolved_usage.iter().filter(included).collect();
                    if events.is_empty() {
                        return None;
                    }
                    let mut usage = Usage::default();
                    for event in events {
                        usage.add_assign(&event.usage);
                    }
                    Some(UnresolvedUsage {
                        session_path: session.session_path.clone(),
                        usage,
                    })
                })
                .collect(),
        }
    }

    pub(super) fn warn(&self) {
        if !self.rewritten_timestamp_sessions.is_empty() {
            eprintln!(
                "warning: {} histories have rewritten event timestamps; daily/monthly dates reflect stored timestamps and cannot establish original usage dates. See JSON diagnostics.rewritten_timestamp_sessions.",
                self.rewritten_timestamp_sessions.len()
            );
        }
        if !self.unresolved_history_usage.is_empty() {
            eprintln!(
                "warning: {} child histories contain checkpoints or prefixes of unresolved ownership; excluded from totals. See JSON diagnostics.unresolved_history_usage. These snapshots may overlap and must not be summed as new usage.",
                self.unresolved_history_usage.len()
            );
        }
    }
}
