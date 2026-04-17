use std::{path::PathBuf, time::Duration};

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use comfy_table::{presets::UTF8_FULL, Attribute, Cell, Table};
use penny_config::{load_config, LoadOptions};
use penny_store::SqliteStore;
use sqlx::{query, Row};
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(name = "pennyprompt")]
#[command(about = "PennyPrompt operator CLI")]
struct Cli {
    #[arg(long)]
    database: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Report {
        #[command(subcommand)]
        command: ReportCommands,
    },
}

#[derive(Debug, Subcommand)]
enum ReportCommands {
    Summary {
        #[arg(long)]
        since: Option<String>,
        #[arg(long, value_enum, default_value_t = SummaryBy::Project)]
        by: SummaryBy,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SummaryBy {
    Project,
    Model,
    Session,
}

impl SummaryBy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Model => "model",
            Self::Session => "session",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct SummaryRow {
    group_key: String,
    request_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    total_cost_usd: f64,
}

#[derive(Debug, Error)]
enum CliError {
    #[error("config error: {0}")]
    Config(String),
    #[error("store error: {0}")]
    Store(String),
    #[error("invalid --since value `{0}`. Expected format like 30m, 12h, or 7d")]
    InvalidSince(String),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli).await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), CliError> {
    let store = open_store(cli.database).await?;
    match cli.command {
        Commands::Report { command } => match command {
            ReportCommands::Summary { since, by } => {
                let cutoff = since
                    .as_deref()
                    .map(parse_since_duration)
                    .transpose()?
                    .map(|duration| {
                        chrono::Duration::from_std(duration)
                            .map(|chrono_duration| Utc::now() - chrono_duration)
                            .map_err(|_| {
                                CliError::InvalidSince(
                                    since.clone().unwrap_or_else(|| "unknown".to_string()),
                                )
                            })
                    })
                    .transpose()?;
                let rows = fetch_summary_rows(&store, by, cutoff).await?;
                print_summary_table(by, since.as_deref(), &rows);
            }
        },
    }
    Ok(())
}

async fn open_store(database_override: Option<PathBuf>) -> Result<SqliteStore, CliError> {
    let database_path = if let Some(path) = database_override {
        path
    } else {
        let config = load_config(LoadOptions::default())
            .map_err(|error| CliError::Config(error.to_string()))?;
        PathBuf::from(config.server.database_path)
    };
    let expanded = expand_tilde(database_path);
    let db_url = if expanded.starts_with("sqlite:") {
        expanded
    } else {
        format!("sqlite://{}", expanded)
    };
    SqliteStore::connect(&db_url)
        .await
        .map_err(|error| CliError::Store(error.to_string()))
}

fn expand_tilde(path: PathBuf) -> String {
    let raw = path.to_string_lossy().to_string();
    if raw == "~" {
        return std::env::var("HOME").unwrap_or(raw);
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    raw
}

fn parse_since_duration(raw: &str) -> Result<Duration, CliError> {
    let value = raw.trim().to_lowercase();
    if value.len() < 2 {
        return Err(CliError::InvalidSince(raw.to_string()));
    }

    let unit = value
        .chars()
        .last()
        .ok_or_else(|| CliError::InvalidSince(raw.to_string()))?;
    let amount_raw = &value[..value.len() - 1];
    let amount: u64 = amount_raw
        .parse()
        .map_err(|_| CliError::InvalidSince(raw.to_string()))?;

    const SECONDS_PER_MINUTE: u64 = 60;
    const SECONDS_PER_HOUR: u64 = 60 * 60;
    const SECONDS_PER_DAY: u64 = 60 * 60 * 24;

    let seconds = match unit {
        'm' => amount.checked_mul(SECONDS_PER_MINUTE),
        'h' => amount.checked_mul(SECONDS_PER_HOUR),
        'd' => amount.checked_mul(SECONDS_PER_DAY),
        _ => return Err(CliError::InvalidSince(raw.to_string())),
    }
    .ok_or_else(|| CliError::InvalidSince(raw.to_string()))?;

    Ok(Duration::from_secs(seconds))
}

async fn fetch_summary_rows(
    store: &SqliteStore,
    by: SummaryBy,
    since: Option<DateTime<Utc>>,
) -> Result<Vec<SummaryRow>, CliError> {
    let since_text = since.map(|ts| ts.to_rfc3339());
    let (group_key_expr, extra_join) = match by {
        SummaryBy::Project => (
            "COALESCE(projects.name, requests.project_id)",
            "LEFT JOIN projects ON projects.id = requests.project_id",
        ),
        SummaryBy::Model => ("requests.model_used", ""),
        SummaryBy::Session => ("COALESCE(requests.session_id, '(none)')", ""),
    };

    let sql = format!(
        r#"
            SELECT
                {group_key_expr} AS group_key,
                COUNT(requests.id) AS request_count,
                CAST(COALESCE(SUM(request_usage.input_tokens), 0) AS INTEGER) AS input_tokens,
                CAST(COALESCE(SUM(request_usage.output_tokens), 0) AS INTEGER) AS output_tokens,
                COALESCE(SUM(request_usage.cost_usd), 0.0) AS total_cost_usd
            FROM requests
            JOIN request_usage ON request_usage.request_id = requests.id
            {extra_join}
            WHERE (?1 IS NULL OR datetime(requests.started_at) >= datetime(?1))
            GROUP BY group_key
            ORDER BY total_cost_usd DESC, group_key ASC
        "#
    );

    let rows = query(&sql)
        .bind(since_text)
        .fetch_all(store.pool())
        .await
        .map_err(|error| CliError::Store(error.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| SummaryRow {
            group_key: row.get("group_key"),
            request_count: row.get("request_count"),
            input_tokens: row.get("input_tokens"),
            output_tokens: row.get("output_tokens"),
            total_cost_usd: row.get("total_cost_usd"),
        })
        .collect())
}

fn print_summary_table(by: SummaryBy, since: Option<&str>, rows: &[SummaryRow]) {
    if rows.is_empty() {
        match since {
            Some(range) => println!(
                "No usage rows found for report summary (by {} since {}).",
                by.as_str(),
                range
            ),
            None => println!(
                "No usage rows found for report summary (by {}).",
                by.as_str()
            ),
        }
        return;
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL).set_header([
        Cell::new(by.as_str()).add_attribute(Attribute::Bold),
        Cell::new("requests").add_attribute(Attribute::Bold),
        Cell::new("input_tokens").add_attribute(Attribute::Bold),
        Cell::new("output_tokens").add_attribute(Attribute::Bold),
        Cell::new("cost_usd").add_attribute(Attribute::Bold),
    ]);

    for row in rows {
        table.add_row([
            Cell::new(&row.group_key),
            Cell::new(row.request_count),
            Cell::new(row.input_tokens),
            Cell::new(row.output_tokens),
            Cell::new(format!("{:.6}", row.total_cost_usd)),
        ]);
    }

    let total_requests: i64 = rows.iter().map(|row| row.request_count).sum();
    let total_input: i64 = rows.iter().map(|row| row.input_tokens).sum();
    let total_output: i64 = rows.iter().map(|row| row.output_tokens).sum();
    let total_cost: f64 = rows.iter().map(|row| row.total_cost_usd).sum();
    table.add_row([
        Cell::new("TOTAL").add_attribute(Attribute::Bold),
        Cell::new(total_requests).add_attribute(Attribute::Bold),
        Cell::new(total_input).add_attribute(Attribute::Bold),
        Cell::new(total_output).add_attribute(Attribute::Bold),
        Cell::new(format!("{:.6}", total_cost)).add_attribute(Attribute::Bold),
    ]);

    match since {
        Some(range) => println!("Report summary by {} since {}", by.as_str(), range),
        None => println!("Report summary by {}", by.as_str()),
    }
    println!("{table}");
}

#[cfg(test)]
mod tests {
    use chrono::Duration as ChronoDuration;
    use penny_store::{NewRequest, ProjectRepo, RequestRepo, UsageRecord};
    use penny_types::{Money, UsageSource};

    use super::*;

    async fn setup_store() -> SqliteStore {
        SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store")
    }

    struct RequestUsageFixture<'a> {
        request_id: &'a str,
        project_seed: &'a str,
        model_used: &'a str,
        started_at: DateTime<Utc>,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    }

    async fn insert_request_usage(store: &SqliteStore, fixture: RequestUsageFixture<'_>) {
        let project_id = ProjectRepo::upsert_by_path(store, fixture.project_seed)
            .await
            .expect("upsert project");
        RequestRepo::insert(
            store,
            &NewRequest {
                id: fixture.request_id.to_string(),
                session_id: None,
                project_id,
                model_requested: fixture.model_used.to_string(),
                model_used: fixture.model_used.to_string(),
                provider_id: "mock".to_string(),
                started_at: fixture.started_at,
                is_streaming: false,
            },
        )
        .await
        .expect("insert request");
        RequestRepo::insert_usage(
            store,
            &UsageRecord {
                request_id: fixture.request_id.to_string(),
                input_tokens: fixture.input_tokens,
                output_tokens: fixture.output_tokens,
                cost_usd: Money::from_usd(fixture.cost_usd).expect("money fixture"),
                source: UsageSource::Provider,
                pricing_snapshot: serde_json::json!({ "source": "test" }),
            },
        )
        .await
        .expect("insert usage");
    }

    #[test]
    fn parse_since_duration_supports_minutes_hours_days() {
        assert_eq!(
            parse_since_duration("30m").expect("parse 30m"),
            Duration::from_secs(30 * 60)
        );
        assert_eq!(
            parse_since_duration("12h").expect("parse 12h"),
            Duration::from_secs(12 * 60 * 60)
        );
        assert_eq!(
            parse_since_duration("7d").expect("parse 7d"),
            Duration::from_secs(7 * 24 * 60 * 60)
        );
    }

    #[test]
    fn parse_since_duration_rejects_invalid_values() {
        assert!(parse_since_duration("xyz").is_err());
        assert!(parse_since_duration("10").is_err());
        assert!(parse_since_duration("3w").is_err());
    }

    #[tokio::test]
    async fn summary_by_project_aggregates_totals() {
        let store = setup_store().await;
        let now = Utc::now();
        insert_request_usage(
            &store,
            RequestUsageFixture {
                request_id: "req_a1",
                project_seed: "/tmp/proj-a",
                model_used: "claude-sonnet-4-6",
                started_at: now,
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: 1.5,
            },
        )
        .await;
        insert_request_usage(
            &store,
            RequestUsageFixture {
                request_id: "req_a2",
                project_seed: "/tmp/proj-a",
                model_used: "gpt-4.1",
                started_at: now,
                input_tokens: 200,
                output_tokens: 100,
                cost_usd: 2.0,
            },
        )
        .await;
        insert_request_usage(
            &store,
            RequestUsageFixture {
                request_id: "req_b1",
                project_seed: "/tmp/proj-b",
                model_used: "gpt-4.1",
                started_at: now,
                input_tokens: 300,
                output_tokens: 120,
                cost_usd: 3.0,
            },
        )
        .await;

        let rows = fetch_summary_rows(&store, SummaryBy::Project, None)
            .await
            .expect("fetch summary");
        let total_cost: f64 = rows.iter().map(|row| row.total_cost_usd).sum();
        assert_eq!(rows.len(), 2);
        assert!((total_cost - 6.5).abs() < 1e-9);
        let total_requests: i64 = rows.iter().map(|row| row.request_count).sum();
        assert_eq!(total_requests, 3);
    }

    #[tokio::test]
    async fn summary_since_filters_out_old_usage_rows() {
        let store = setup_store().await;
        let now = Utc::now();
        insert_request_usage(
            &store,
            RequestUsageFixture {
                request_id: "req_old",
                project_seed: "/tmp/proj-old",
                model_used: "claude-sonnet-4-6",
                started_at: now - ChronoDuration::days(3),
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: 1.0,
            },
        )
        .await;
        insert_request_usage(
            &store,
            RequestUsageFixture {
                request_id: "req_new",
                project_seed: "/tmp/proj-new",
                model_used: "claude-sonnet-4-6",
                started_at: now - ChronoDuration::hours(2),
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: 2.0,
            },
        )
        .await;

        let rows = fetch_summary_rows(
            &store,
            SummaryBy::Project,
            Some(now - ChronoDuration::days(1)),
        )
        .await
        .expect("fetch summary");
        assert_eq!(rows.len(), 1);
        assert!((rows[0].total_cost_usd - 2.0).abs() < 1e-9);
        assert_eq!(rows[0].request_count, 1);
    }
}
