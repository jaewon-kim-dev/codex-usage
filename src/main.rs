use anyhow::{Context, Result, anyhow};
use chrono::{NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use clap::{Parser, Subcommand};
use codex_usage::pricing::{PricingCatalog, totals_cost_usd};
use codex_usage::report::{GroupBy, SessionRow, aggregate_sessions, aggregate_usage};
use codex_usage::scanner::{ScanOptions, scan_full_daily_rows, scan_sessions};
use codex_usage::types::{ModelTotals, ReportRow, Usage};
use comfy_table::{Cell, ContentArrangement, Table, presets::UTF8_FULL};
use directories::BaseDirs;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

const DEFAULT_CODEX_HOME_DIRNAME: &str = ".codex";
const DEFAULT_SESSIONS_SUBDIR: &str = "sessions";
const DEFAULT_CACHE_SUBDIR: &str = "codex-usage";
const DEFAULT_CACHE_FILENAME: &str = "session-cache-v1.bin";

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

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let timezone = parse_timezone(&cli.timezone)?;
    let since = parse_filter_date(cli.since.as_deref())?;
    let until = parse_filter_date(cli.until.as_deref())?;
    let codex_home = resolve_codex_home(cli.codex_home.as_deref())?;
    let session_root = codex_home.join(DEFAULT_SESSIONS_SUBDIR);
    let cache_path = resolve_cache_path(cli.cache_path.as_deref())?;
    let pricing_catalog = PricingCatalog::load()?;
    let use_full_daily_fast_path = cli.command.is_none()
        && since.is_none()
        && until.is_none()
        && !cli.split_by_model
        && (cli.refresh_cache || !cache_path.exists());

    if use_full_daily_fast_path {
        let rows = scan_full_daily_rows(&session_root, &cache_path, timezone)?;
        return render_usage_rows("daily", rows, cli.json, &pricing_catalog, false);
    }

    let sessions = scan_sessions(ScanOptions {
        session_root: &session_root,
        cache_path: &cache_path,
        since,
        until,
        refresh_cache: cli.refresh_cache,
    })?;

    match cli.command.unwrap_or(Command::Daily) {
        Command::Daily => render_usage_rows(
            "daily",
            aggregate_usage(
                &sessions,
                timezone,
                GroupBy::Day,
                since,
                until,
                cli.split_by_model,
            ),
            cli.json,
            &pricing_catalog,
            cli.split_by_model,
        )?,
        Command::Monthly => render_usage_rows(
            "monthly",
            aggregate_usage(
                &sessions,
                timezone,
                GroupBy::Month,
                since,
                until,
                cli.split_by_model,
            ),
            cli.json,
            &pricing_catalog,
            cli.split_by_model,
        )?,
        Command::Sessions => render_session_rows(
            aggregate_sessions(&sessions, timezone, since, until),
            cli.json,
            timezone,
            &pricing_catalog,
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
    kind: &str,
    rows: Vec<ReportRow>,
    json_output: bool,
    pricing_catalog: &PricingCatalog,
    split_by_model: bool,
) -> Result<()> {
    if json_output {
        let totals = totals_from_report_rows(&rows, pricing_catalog);
        let mut payload = Map::<String, Value>::new();
        payload.insert(
            kind.to_string(),
            serde_json::to_value(report_row_payloads(&rows))?,
        );
        payload.insert("totals".to_string(), serde_json::to_value(totals)?);
        println!("{}", serde_json::to_string_pretty(&Value::Object(payload))?);
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
            "monthly" => "Month",
            _ => "Date",
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
            Cell::new(format!(
                "${:.4}",
                totals_cost_usd(pricing_catalog, &row.models)
            )),
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
        Cell::new(format!("${:.4}", totals.cost_usd)),
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
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "sessions": session_row_payloads(&rows),
                "totals": totals,
            }))?
        );
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
            Cell::new(format!(
                "${:.4}",
                totals_cost_usd(pricing_catalog, &row.models)
            )),
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
        Cell::new(format!("${:.4}", totals.cost_usd)),
        Cell::new(""),
    ]);

    println!("{table}");
    Ok(())
}

#[derive(Debug, serde::Serialize)]
struct UsagePayload {
    input_tokens: u64,
    cached_input_tokens: u64,
    raw_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
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
struct ReportRowPayload {
    key: String,
    usage: UsagePayload,
    models: BTreeMap<String, ModelTotalsPayload>,
}

#[derive(Debug, serde::Serialize)]
struct SessionRowPayload {
    date_key: String,
    session_id: String,
    session_file: String,
    directory: String,
    last_activity_unix_ms: i64,
    usage: UsagePayload,
    models: BTreeMap<String, ModelTotalsPayload>,
}

#[derive(Debug, serde::Serialize)]
struct TotalsPayload {
    input_tokens: u64,
    cached_input_tokens: u64,
    raw_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
    cost_usd: f64,
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

fn report_row_payloads(rows: &[ReportRow]) -> Vec<ReportRowPayload> {
    rows.iter()
        .map(|row| ReportRowPayload {
            key: row.key.clone(),
            usage: UsagePayload::from(&row.usage),
            models: model_totals_payloads(&row.models),
        })
        .collect()
}

fn session_row_payloads(rows: &[SessionRow]) -> Vec<SessionRowPayload> {
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

fn totals_from_report_rows(rows: &[ReportRow], pricing_catalog: &PricingCatalog) -> TotalsPayload {
    rows.iter().fold(
        TotalsPayload {
            input_tokens: 0,
            cached_input_tokens: 0,
            raw_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 0,
            cost_usd: 0.0,
        },
        |mut totals, row| {
            totals.input_tokens += row.usage.billable_input_tokens();
            totals.cached_input_tokens += row.usage.cached_input_tokens;
            totals.raw_input_tokens += row.usage.input_tokens;
            totals.output_tokens += row.usage.output_tokens;
            totals.reasoning_output_tokens += row.usage.reasoning_output_tokens;
            totals.total_tokens += row.usage.total_tokens;
            totals.cost_usd += totals_cost_usd(pricing_catalog, &row.models);
            totals
        },
    )
}

fn totals_from_session_rows(
    rows: &[SessionRow],
    pricing_catalog: &PricingCatalog,
) -> TotalsPayload {
    rows.iter().fold(
        TotalsPayload {
            input_tokens: 0,
            cached_input_tokens: 0,
            raw_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 0,
            cost_usd: 0.0,
        },
        |mut totals, row| {
            totals.input_tokens += row.usage.billable_input_tokens();
            totals.cached_input_tokens += row.usage.cached_input_tokens;
            totals.raw_input_tokens += row.usage.input_tokens;
            totals.output_tokens += row.usage.output_tokens;
            totals.reasoning_output_tokens += row.usage.reasoning_output_tokens;
            totals.total_tokens += row.usage.total_tokens;
            totals.cost_usd += totals_cost_usd(pricing_catalog, &row.models);
            totals
        },
    )
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

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut chunks = Vec::new();
    for chunk in digits.as_bytes().rchunks(3) {
        chunks.push(std::str::from_utf8(chunk).unwrap_or_default().to_string());
    }
    chunks.reverse();
    chunks.join(",")
}

#[cfg(test)]
mod tests {
    use super::{report_row_payloads, totals_from_report_rows};
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
}
