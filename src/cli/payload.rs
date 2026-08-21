use crate::pricing::{PricingCatalog, totals_cost_usd};
use crate::report::SessionRow;
use crate::types::{ModelTotals, ReportRow, Usage};
use std::collections::BTreeMap;

#[derive(Debug, serde::Serialize)]
pub(super) struct UsagePayload {
    pub(super) input_tokens: u64,
    pub(super) cached_input_tokens: u64,
    pub(super) raw_input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) reasoning_output_tokens: u64,
    pub(super) total_tokens: u64,
}

impl From<&Usage> for UsagePayload {
    fn from(usage: &Usage) -> Self {
        Self {
            input_tokens: usage.billable_input_tokens(),
            cached_input_tokens: usage.cached_input_tokens,
            raw_input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_output_tokens: usage.reasoning_output_tokens,
            total_tokens: usage.total_tokens,
        }
    }
}

#[derive(Debug, serde::Serialize)]
struct ModelTotalsPayload {
    usage: UsagePayload,
    is_fallback: bool,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct ReportRowPayload {
    pub(super) key: String,
    pub(super) usage: UsagePayload,
    models: BTreeMap<String, ModelTotalsPayload>,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct SessionRowPayload {
    date_key: String,
    session_id: String,
    session_file: String,
    directory: String,
    last_activity_unix_ms: i64,
    usage: UsagePayload,
    models: BTreeMap<String, ModelTotalsPayload>,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct TotalsPayload {
    pub(super) input_tokens: u64,
    pub(super) cached_input_tokens: u64,
    pub(super) raw_input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) reasoning_output_tokens: u64,
    pub(super) total_tokens: u64,
    pub(super) cost_usd: f64,
}

#[derive(serde::Serialize)]
pub(super) struct DailyOutput<'a> {
    pub(super) daily: &'a [ReportRowPayload],
    pub(super) totals: &'a TotalsPayload,
}

#[derive(serde::Serialize)]
pub(super) struct MonthlyOutput<'a> {
    pub(super) monthly: &'a [ReportRowPayload],
    pub(super) totals: &'a TotalsPayload,
}

#[derive(serde::Serialize)]
pub(super) struct SessionsOutput<'a> {
    pub(super) sessions: &'a [SessionRowPayload],
    pub(super) totals: &'a TotalsPayload,
}

fn model_totals_payloads(
    models: &BTreeMap<String, ModelTotals>,
) -> BTreeMap<String, ModelTotalsPayload> {
    models
        .iter()
        .map(|(model, totals)| {
            (
                model.clone(),
                ModelTotalsPayload {
                    usage: UsagePayload::from(&totals.usage),
                    is_fallback: totals.is_fallback,
                },
            )
        })
        .collect()
}

pub(super) fn report_row_payloads(rows: &[ReportRow]) -> Vec<ReportRowPayload> {
    rows.iter()
        .map(|row| ReportRowPayload {
            key: row.key.clone(),
            usage: UsagePayload::from(&row.usage),
            models: model_totals_payloads(&row.models),
        })
        .collect()
}

pub(super) fn session_row_payloads(rows: &[SessionRow]) -> Vec<SessionRowPayload> {
    rows.iter()
        .map(|row| SessionRowPayload {
            date_key: row.date_key.clone(),
            session_id: row.session_id.clone(),
            session_file: row.session_file.clone(),
            directory: row.directory.clone(),
            last_activity_unix_ms: row.last_activity_unix_ms,
            usage: UsagePayload::from(&row.usage),
            models: model_totals_payloads(&row.models),
        })
        .collect()
}

pub(super) fn totals_from_report_rows(
    rows: &[ReportRow],
    pricing_catalog: &PricingCatalog,
) -> TotalsPayload {
    rows.iter()
        .fold(TotalsPayload::default(), |mut totals, row| {
            totals.add_usage(&row.usage);
            totals.add_cost(totals_cost_usd(pricing_catalog, &row.models));
            totals
        })
}

pub(super) fn totals_from_session_rows(
    rows: &[SessionRow],
    pricing_catalog: &PricingCatalog,
) -> TotalsPayload {
    rows.iter()
        .fold(TotalsPayload::default(), |mut totals, row| {
            totals.add_usage(&row.usage);
            totals.add_cost(totals_cost_usd(pricing_catalog, &row.models));
            totals
        })
}

impl Default for TotalsPayload {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            cached_input_tokens: 0,
            raw_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 0,
            cost_usd: 0.0,
        }
    }
}

impl TotalsPayload {
    fn add_usage(&mut self, usage: &Usage) {
        self.input_tokens += usage.billable_input_tokens();
        self.cached_input_tokens += usage.cached_input_tokens;
        self.raw_input_tokens += usage.input_tokens;
        self.output_tokens += usage.output_tokens;
        self.reasoning_output_tokens += usage.reasoning_output_tokens;
        self.total_tokens += usage.total_tokens;
    }

    fn add_cost(&mut self, cost_usd: f64) {
        self.cost_usd += cost_usd;
    }
}
