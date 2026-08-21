use crate::pricing::{PricingCatalog, totals_cost_usd};
use crate::report::{GroupBy, SessionRow, aggregate_sessions, aggregate_usage};
use crate::scanner::{ScanOptions, scan_sessions};
use crate::types::{ModelTotals, ReportRow};
use anyhow::{Context, Result, anyhow};
use chrono::{NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use clap::{Parser, Subcommand};
use comfy_table::{Cell, ContentArrangement, Table, presets::UTF8_FULL};
use directories::BaseDirs;
use std::collections::BTreeMap;
use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

mod payload;

use payload::{
    DailyOutput, MonthlyOutput, SessionsOutput, report_row_payloads, session_row_payloads,
    totals_from_report_rows, totals_from_session_rows,
};

const DEFAULT_CODEX_HOME_DIRNAME: &str = ".codex";
const DEFAULT_SESSIONS_SUBDIR: &str = "sessions";
const DEFAULT_CACHE_SUBDIR: &str = "codex-usage";
const DEFAULT_CACHE_FILENAME: &str = "session-cache-v4.bin";

#[derive(Debug, Parser)]
#[command(name = "codex-usage")]
#[command(about = "Fast Codex usage analyzer written in Rust")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long, global = true)]
    json: bool,

    #[arg(long, global = true)]
    since: Option<String>,

    #[arg(long, global = true)]
    until: Option<String>,

    #[arg(long, global = true, default_value = "Asia/Seoul")]
    timezone: String,

    #[arg(long, global = true)]
    codex_home: Option<PathBuf>,

    #[arg(long, global = true)]
    cache_path: Option<PathBuf>,

    #[arg(long, global = true)]
    refresh_cache: bool,

    #[arg(long, global = true)]
    split_by_model: bool,
}

#[derive(Debug, Subcommand, Clone, Copy)]
enum Command {
    Daily,
    Monthly,
    Sessions,
}

#[derive(Debug, Clone, Copy)]
enum ReportKind {
    Daily,
    Monthly,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let pricing_catalog = PricingCatalog::load()?;
    run_with_cli(cli, &pricing_catalog)
}

fn run_with_cli(cli: Cli, pricing_catalog: &PricingCatalog) -> Result<()> {
    let timezone = parse_timezone(&cli.timezone)?;
    let since = parse_filter_date(cli.since.as_deref())?;
    let until = parse_filter_date(cli.until.as_deref())?;
    let codex_home = resolve_codex_home(cli.codex_home.as_deref())?;
    let session_root = codex_home.join(DEFAULT_SESSIONS_SUBDIR);
    let cache_path = resolve_cache_path(cli.cache_path.as_deref())?;
    let sessions = scan_sessions(ScanOptions {
        session_root: &session_root,
        cache_path: &cache_path,
        since,
        until,
        refresh_cache: cli.refresh_cache,
    })?;

    match cli.command.unwrap_or(Command::Daily) {
        Command::Daily => render_usage_rows(
            ReportKind::Daily,
            aggregate_usage(
                &sessions,
                timezone,
                GroupBy::Day,
                since,
                until,
                cli.split_by_model,
            ),
            cli.json,
            pricing_catalog,
            cli.split_by_model,
        )?,
        Command::Monthly => render_usage_rows(
            ReportKind::Monthly,
            aggregate_usage(
                &sessions,
                timezone,
                GroupBy::Month,
                since,
                until,
                cli.split_by_model,
            ),
            cli.json,
            pricing_catalog,
            cli.split_by_model,
        )?,
        Command::Sessions => render_session_rows(
            aggregate_sessions(&sessions, timezone, since, until),
            cli.json,
            timezone,
            pricing_catalog,
        )?,
    }

    Ok(())
}

fn parse_filter_date(value: Option<&str>) -> Result<Option<NaiveDate>> {
    value
        .map(|value| {
            NaiveDate::parse_from_str(value, "%Y%m%d")
                .with_context(|| format!("invalid date {value}; expected YYYYMMDD"))
        })
        .transpose()
}

fn parse_timezone(value: &str) -> Result<Tz> {
    value
        .parse::<Tz>()
        .map_err(|_| anyhow!("invalid timezone {value}"))
}

fn resolve_codex_home(value: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = value {
        return Ok(path.to_path_buf());
    }
    if let Some(codex_home) = env::var_os("CODEX_HOME") {
        return Ok(PathBuf::from(codex_home));
    }
    let base_dirs = BaseDirs::new().context("failed to resolve home directory")?;
    Ok(base_dirs.home_dir().join(DEFAULT_CODEX_HOME_DIRNAME))
}

fn resolve_cache_path(value: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = value {
        return Ok(path.to_path_buf());
    }
    let base_dirs = BaseDirs::new().context("failed to resolve cache directory")?;
    Ok(base_dirs
        .cache_dir()
        .join(DEFAULT_CACHE_SUBDIR)
        .join(DEFAULT_CACHE_FILENAME))
}

fn render_usage_rows(
    kind: ReportKind,
    rows: Vec<ReportRow>,
    json_output: bool,
    pricing_catalog: &PricingCatalog,
    split_by_model: bool,
) -> Result<()> {
    if json_output {
        let totals = totals_from_report_rows(&rows, pricing_catalog);
        let payloads = report_row_payloads(&rows);
        match kind {
            ReportKind::Daily => write_json(&DailyOutput {
                daily: &payloads,
                totals: &totals,
            })?,
            ReportKind::Monthly => write_json(&MonthlyOutput {
                monthly: &payloads,
                totals: &totals,
            })?,
        }
        return Ok(());
    }

    if rows.is_empty() {
        println!("No Codex usage data found.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new(match kind {
            ReportKind::Monthly => "Month",
            ReportKind::Daily => "Date",
        }),
        Cell::new(if split_by_model { "Model" } else { "Models" }),
        Cell::new("Input"),
        Cell::new("Cache"),
        Cell::new("Output"),
        Cell::new("Reasoning"),
        Cell::new("Total"),
        Cell::new("Cost (USD)"),
    ]);

    for row in &rows {
        table.add_row(vec![
            Cell::new(&row.key),
            Cell::new(models_summary(&row.models)),
            Cell::new(format_number(row.usage.billable_input_tokens())),
            Cell::new(format_number(row.usage.cached_input_tokens)),
            Cell::new(format_number(row.usage.output_tokens)),
            Cell::new(format_number(row.usage.reasoning_output_tokens)),
            Cell::new(format_number(row.usage.total_tokens)),
            Cell::new(format_cost(totals_cost_usd(pricing_catalog, &row.models))),
        ]);
    }

    let totals = totals_from_report_rows(&rows, pricing_catalog);
    table.add_row(vec![
        Cell::new("Total"),
        Cell::new(""),
        Cell::new(format_number(totals.input_tokens)),
        Cell::new(format_number(totals.cached_input_tokens)),
        Cell::new(format_number(totals.output_tokens)),
        Cell::new(format_number(totals.reasoning_output_tokens)),
        Cell::new(format_number(totals.total_tokens)),
        Cell::new(format_cost(totals.cost_usd)),
    ]);

    println!("{table}");
    Ok(())
}

fn render_session_rows(
    rows: Vec<SessionRow>,
    json_output: bool,
    timezone: Tz,
    pricing_catalog: &PricingCatalog,
) -> Result<()> {
    if json_output {
        let totals = totals_from_session_rows(&rows, pricing_catalog);
        let payloads = session_row_payloads(&rows);
        write_json(&SessionsOutput {
            sessions: &payloads,
            totals: &totals,
        })?;
        return Ok(());
    }

    if rows.is_empty() {
        println!("No Codex usage data found.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Date"),
        Cell::new("Directory"),
        Cell::new("Session"),
        Cell::new("Models"),
        Cell::new("Input"),
        Cell::new("Cache"),
        Cell::new("Output"),
        Cell::new("Reasoning"),
        Cell::new("Total"),
        Cell::new("Cost (USD)"),
        Cell::new("Last Activity"),
    ]);

    for row in &rows {
        table.add_row(vec![
            Cell::new(&row.date_key),
            Cell::new(&row.directory),
            Cell::new(&row.session_file),
            Cell::new(models_summary(&row.models)),
            Cell::new(format_number(row.usage.billable_input_tokens())),
            Cell::new(format_number(row.usage.cached_input_tokens)),
            Cell::new(format_number(row.usage.output_tokens)),
            Cell::new(format_number(row.usage.reasoning_output_tokens)),
            Cell::new(format_number(row.usage.total_tokens)),
            Cell::new(format_cost(totals_cost_usd(pricing_catalog, &row.models))),
            Cell::new(format_activity(row.last_activity_unix_ms, timezone)),
        ]);
    }

    let totals = totals_from_session_rows(&rows, pricing_catalog);
    table.add_row(vec![
        Cell::new(""),
        Cell::new(""),
        Cell::new("Total"),
        Cell::new(""),
        Cell::new(format_number(totals.input_tokens)),
        Cell::new(format_number(totals.cached_input_tokens)),
        Cell::new(format_number(totals.output_tokens)),
        Cell::new(format_number(totals.reasoning_output_tokens)),
        Cell::new(format_number(totals.total_tokens)),
        Cell::new(format_cost(totals.cost_usd)),
        Cell::new(""),
    ]);

    println!("{table}");
    Ok(())
}

fn models_summary(models: &BTreeMap<String, ModelTotals>) -> String {
    models
        .iter()
        .map(|(model, totals)| {
            if totals.is_fallback {
                format!("{model}*")
            } else {
                model.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_activity(timestamp_unix_ms: i64, timezone: Tz) -> String {
    let Some(timestamp) = Utc.timestamp_millis_opt(timestamp_unix_ms).single() else {
        return "-".to_string();
    };
    timestamp
        .with_timezone(&timezone)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn format_cost(cost_usd: Option<f64>) -> String {
    cost_usd
        .map(|cost| format!("${cost:.4}"))
        .unwrap_or_else(|| "N/A".to_string())
}

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut chunks = Vec::new();
    for chunk in digits.as_bytes().rchunks(3) {
        chunks.push(std::str::from_utf8(chunk).unwrap_or_default().to_string());
    }
    chunks.reverse();
    chunks.join(",")
}

fn write_json(value: &impl serde::Serialize) -> Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer_pretty(&mut output, value)?;
    writeln!(output)?;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/main_tests.rs"]
mod tests;
