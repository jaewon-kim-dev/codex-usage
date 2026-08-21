use super::{Cli, report_row_payloads, should_use_full_daily_fast_path, totals_from_report_rows};
use clap::Parser;
use codex_usage::pricing::PricingCatalog;
use codex_usage::types::{ReportRow, Usage};
use std::collections::BTreeMap;

#[test]
fn serializes_input_and_cached_input_separately() {
    let rows = vec![ReportRow {
        key: "2026-03-06".to_string(),
        usage: Usage {
            input_tokens: 1_000,
            cached_input_tokens: 250,
            output_tokens: 50,
            reasoning_output_tokens: 10,
            total_tokens: 1_050,
        },
        models: BTreeMap::new(),
    }];

    let payloads = report_row_payloads(&rows);

    assert_eq!(payloads[0].usage.input_tokens, 750);
    assert_eq!(payloads[0].usage.cached_input_tokens, 250);
    assert_eq!(payloads[0].usage.raw_input_tokens, 1_000);
}

#[test]
fn reports_totals_with_billable_and_raw_input_tokens() {
    let rows = vec![ReportRow {
        key: "2026-03-06".to_string(),
        usage: Usage {
            input_tokens: 1_000,
            cached_input_tokens: 250,
            output_tokens: 50,
            reasoning_output_tokens: 10,
            total_tokens: 1_050,
        },
        models: BTreeMap::new(),
    }];

    let totals = totals_from_report_rows(&rows, &PricingCatalog::default());

    assert_eq!(totals.input_tokens, 750);
    assert_eq!(totals.cached_input_tokens, 250);
    assert_eq!(totals.raw_input_tokens, 1_000);
}

#[test]
fn refresh_cache_uses_the_session_cache_rebuild_path() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let cache_path = temp_dir.path().join("session-cache.bin");
    let cli = Cli::try_parse_from(["codex-usage", "--refresh-cache"]).expect("parse cli");

    assert!(!should_use_full_daily_fast_path(&cli, &cache_path));
}
