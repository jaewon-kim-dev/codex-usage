use anyhow::{Context, Result, anyhow};
use chrono::{NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use clap::{Parser, Subcommand};
use codex_usage::pricing::{PricingCatalog, totals_cost_usd};
use codex_usage::report::{GroupBy, SessionRow, aggregate_sessions, aggregate_usage};
use codex_usage::scanner::{ScanOptions, scan_full_daily_rows, scan_sessions};
use codex_usage::types::{ModelTotals, ReportRow};
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
        && (cli.refresh_cache || !cache_path.exists());

    if use_full_daily_fast_path {
        let rows = scan_full_daily_rows(&session_root, &cache_path, timezone)?;
        return render_usage_rows("daily", rows, cli.json, &pricing_catalog);
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
            aggregate_usage(&sessions, timezone, GroupBy::Day, since, until),
            cli.json,
            &pricing_catalog,
        )?,
        Command::Monthly => render_usage_rows(
            "monthly",
            aggregate_usage(&sessions, timezone, GroupBy::Month, since, until),
            cli.json,
            &pricing_catalog,
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
) -> Result<()> {
    if json_output {
        let totals = totals_from_report_rows(&rows, pricing_catalog);
        let mut payload = Map::<String, Value>::new();
        payload.insert(kind.to_string(), serde_json::to_value(rows)?);
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
        Cell::new("Models"),
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
            Cell::new(format_number(row.usage.input_tokens)),
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
                "sessions": rows,
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
            Cell::new(format_number(row.usage.input_tokens)),
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
struct TotalsPayload {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
    cost_usd: f64,
}

fn totals_from_report_rows(rows: &[ReportRow], pricing_catalog: &PricingCatalog) -> TotalsPayload {
    rows.iter().fold(
        TotalsPayload {
            input_tokens: 0,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 0,
            cost_usd: 0.0,
        },
        |mut totals, row| {
            totals.input_tokens += row.usage.input_tokens;
            totals.cached_input_tokens += row.usage.cached_input_tokens;
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
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 0,
            cost_usd: 0.0,
        },
        |mut totals, row| {
            totals.input_tokens += row.usage.input_tokens;
            totals.cached_input_tokens += row.usage.cached_input_tokens;
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
