use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use comfy_table::{presets::UTF8_FULL, Attribute, Cell, Table};
use glob::glob;
use penny_config::{load_config, LoadOptions};
use penny_cost::{estimate_tokens, PricingEngine};
use penny_store::{BudgetRepo, ProjectRepo, SqliteStore};
use penny_types::{Confidence, Money, ScopeType, TaskType, WindowType};
use serde_json::Value;
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
    Estimate {
        #[arg(long)]
        model: String,
        #[arg(long, value_enum, default_value_t = EstimateTaskType::SinglePass)]
        task_type: EstimateTaskType,
        #[arg(long)]
        context_tokens: Option<u64>,
        #[arg(long = "context-files")]
        context_files: Vec<String>,
        #[arg(long)]
        project_id: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long, default_value_t = 200)]
        max_files: usize,
    },
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

#[derive(Debug, Clone, Copy, ValueEnum)]
enum EstimateTaskType {
    SinglePass,
    MultiRound,
    AgentTask,
}

impl EstimateTaskType {
    fn as_task_type(self) -> TaskType {
        match self {
            Self::SinglePass => TaskType::SinglePass,
            Self::MultiRound => TaskType::MultiRound,
            Self::AgentTask => TaskType::AgentTask,
        }
    }

    fn as_label(self) -> &'static str {
        match self {
            Self::SinglePass => "single_pass",
            Self::MultiRound => "multi_round",
            Self::AgentTask => "agent_task",
        }
    }
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

#[derive(Debug, Clone)]
struct ContextEstimate {
    tokens: u64,
    file_count: usize,
    source: String,
}

#[derive(Debug, Clone)]
struct RoutePreview {
    provider_id: String,
    provider_name: String,
    api_format: String,
    external_model: String,
    enabled: bool,
}

#[derive(Debug, Clone)]
struct BudgetEstimateSummary {
    status: String,
    max_estimated_cost_usd: Money,
    rows: Vec<BudgetEstimateRow>,
}

#[derive(Debug, Clone)]
struct BudgetEstimateRow {
    budget_id: i64,
    scope_type: ScopeType,
    scope_id: String,
    window_type: WindowType,
    accumulated_usd: Money,
    projected_usd: Money,
    hard_limit_usd: Option<Money>,
    soft_limit_usd: Option<Money>,
    hard_limit_exceeded: bool,
    soft_limit_reached: bool,
}

#[derive(Debug, Clone)]
struct EstimateCommand {
    model: String,
    task_type: EstimateTaskType,
    context_tokens: Option<u64>,
    context_files: Vec<String>,
    project_id: Option<String>,
    session_id: Option<String>,
    max_files: usize,
}

#[derive(Debug)]
struct EstimateSummary<'a> {
    model: &'a str,
    task_type: EstimateTaskType,
    context: &'a ContextEstimate,
    confidence: &'a Confidence,
    min_usd: f64,
    max_usd: f64,
    route_preview: Option<&'a RoutePreview>,
    budget: &'a BudgetEstimateSummary,
    pricebook_snapshot: &'a Value,
    project_id: Option<&'a str>,
    session_id: Option<&'a str>,
}

#[derive(Debug, Error)]
enum CliError {
    #[error("config error: {0}")]
    Config(String),
    #[error("store error: {0}")]
    Store(String),
    #[error("cost error: {0}")]
    Cost(String),
    #[error("invalid --since value `{0}`. Expected format like 30m, 12h, or 7d")]
    InvalidSince(String),
    #[error("estimate requires --context-tokens or --context-files")]
    MissingEstimateContext,
    #[error("invalid context glob `{pattern}`: {reason}")]
    InvalidContextGlob { pattern: String, reason: String },
    #[error("no files matched --context-files")]
    NoContextFilesMatched,
    #[error("failed to read context file `{path}`: {reason}")]
    ContextFileRead { path: String, reason: String },
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
        Commands::Estimate {
            model,
            task_type,
            context_tokens,
            context_files,
            project_id,
            session_id,
            max_files,
        } => {
            run_estimate(
                &store,
                EstimateCommand {
                    model,
                    task_type,
                    context_tokens,
                    context_files,
                    project_id,
                    session_id,
                    max_files,
                },
            )
            .await?;
        }
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

async fn run_estimate(store: &SqliteStore, command: EstimateCommand) -> Result<(), CliError> {
    let context = estimate_context(
        command.context_tokens,
        &command.context_files,
        command.max_files,
    )?;
    let resolved_project_id = match command.project_id {
        Some(project_id) => Some(project_id),
        None => autodetect_project_id(store).await?,
    };

    let engine = PricingEngine::new(store);
    let range = engine
        .estimate_range(
            command.model.as_str(),
            context.tokens,
            command.task_type.as_task_type(),
        )
        .await
        .map_err(|error| CliError::Cost(error.to_string()))?;
    let pricebook_snapshot = engine
        .snapshot(command.model.as_str())
        .await
        .map_err(|error| CliError::Cost(error.to_string()))?;
    let route_preview = fetch_route_preview(store, command.model.as_str())
        .await
        .map_err(|error| CliError::Store(error.to_string()))?;
    let max_estimated_cost_usd =
        Money::from_usd(range.max_usd).map_err(|error| CliError::Cost(error.to_string()))?;
    let budget = build_budget_estimate_summary(
        store,
        resolved_project_id.as_deref(),
        command.session_id.as_deref(),
        max_estimated_cost_usd,
    )
    .await?;

    print_estimate_summary(EstimateSummary {
        model: command.model.as_str(),
        task_type: command.task_type,
        context: &context,
        confidence: &range.confidence,
        min_usd: range.min_usd,
        max_usd: range.max_usd,
        route_preview: route_preview.as_ref(),
        budget: &budget,
        pricebook_snapshot: &pricebook_snapshot,
        project_id: resolved_project_id.as_deref(),
        session_id: command.session_id.as_deref(),
    });
    Ok(())
}

async fn autodetect_project_id(store: &SqliteStore) -> Result<Option<String>, CliError> {
    let cwd = std::env::current_dir().map_err(|error| CliError::Config(error.to_string()))?;
    let project = store
        .get_by_path(&cwd.to_string_lossy())
        .await
        .map_err(|error| CliError::Store(error.to_string()))?;
    Ok(project.map(|project| project.id))
}

fn estimate_context(
    context_tokens: Option<u64>,
    context_files: &[String],
    max_files: usize,
) -> Result<ContextEstimate, CliError> {
    if let Some(tokens) = context_tokens {
        return Ok(ContextEstimate {
            tokens,
            file_count: 0,
            source: "manual".to_string(),
        });
    }

    if context_files.is_empty() {
        return Err(CliError::MissingEstimateContext);
    }

    let files = expand_context_patterns(context_files, max_files)?;
    if files.is_empty() {
        return Err(CliError::NoContextFilesMatched);
    }

    let mut content = String::new();
    for path in &files {
        let raw = fs::read(path).map_err(|error| CliError::ContextFileRead {
            path: path.to_string_lossy().to_string(),
            reason: error.to_string(),
        })?;
        content.push_str(&String::from_utf8_lossy(&raw));
        content.push('\n');
    }

    let estimate = estimate_tokens(&Value::String(content));
    Ok(ContextEstimate {
        tokens: estimate.input_tokens,
        file_count: files.len(),
        source: format!("{:?}", estimate.source).to_lowercase(),
    })
}

fn expand_context_patterns(
    patterns: &[String],
    max_files: usize,
) -> Result<Vec<PathBuf>, CliError> {
    let mut files = BTreeSet::<PathBuf>::new();
    for pattern in patterns {
        let entries = glob(pattern).map_err(|error| CliError::InvalidContextGlob {
            pattern: pattern.clone(),
            reason: error.to_string(),
        })?;
        for entry in entries {
            let path = entry.map_err(|error| CliError::InvalidContextGlob {
                pattern: pattern.clone(),
                reason: error.to_string(),
            })?;
            collect_paths(path.as_path(), &mut files, max_files)?;
            if files.len() >= max_files {
                return Ok(files.into_iter().collect());
            }
        }
    }

    Ok(files.into_iter().collect())
}

fn collect_paths(
    path: &Path,
    files: &mut BTreeSet<PathBuf>,
    max_files: usize,
) -> Result<(), CliError> {
    if files.len() >= max_files {
        return Ok(());
    }

    if path.is_file() {
        files.insert(path.to_path_buf());
        return Ok(());
    }

    if !path.is_dir() {
        return Ok(());
    }

    let entries = fs::read_dir(path).map_err(|error| CliError::ContextFileRead {
        path: path.to_string_lossy().to_string(),
        reason: error.to_string(),
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| CliError::ContextFileRead {
            path: path.to_string_lossy().to_string(),
            reason: error.to_string(),
        })?;
        collect_paths(entry.path().as_path(), files, max_files)?;
        if files.len() >= max_files {
            break;
        }
    }

    Ok(())
}

async fn fetch_route_preview(
    store: &SqliteStore,
    model_id: &str,
) -> Result<Option<RoutePreview>, sqlx::Error> {
    let row = query(
        r#"
        SELECT
            providers.id AS provider_id,
            providers.name AS provider_name,
            providers.api_format AS api_format,
            providers.enabled AS provider_enabled,
            models.external_name AS external_model
        FROM models
        JOIN providers ON providers.id = models.provider_id
        WHERE models.id = ?1
        LIMIT 1
        "#,
    )
    .bind(model_id)
    .fetch_optional(store.pool())
    .await?;

    Ok(row.map(|row| RoutePreview {
        provider_id: row.get("provider_id"),
        provider_name: row.get("provider_name"),
        api_format: row.get("api_format"),
        external_model: row.get("external_model"),
        enabled: row.get::<i64, _>("provider_enabled") != 0,
    }))
}

async fn build_budget_estimate_summary(
    store: &SqliteStore,
    project_id: Option<&str>,
    session_id: Option<&str>,
    max_estimated_cost_usd: Money,
) -> Result<BudgetEstimateSummary, CliError> {
    let mut budgets = if let (Some(project_id), Some(session_id)) = (project_id, session_id) {
        store
            .list_applicable_for_request(project_id, session_id)
            .await
            .map_err(|error| CliError::Store(error.to_string()))?
    } else {
        store
            .list_all()
            .await
            .map_err(|error| CliError::Store(error.to_string()))?
            .into_iter()
            .filter(|budget| match budget.scope_type {
                ScopeType::Global => budget.scope_id == "*",
                ScopeType::Project => project_id.is_some_and(|id| id == budget.scope_id),
                ScopeType::Session => session_id.is_some_and(|id| id == budget.scope_id),
            })
            .collect::<Vec<_>>()
    };
    budgets.sort_by_key(|budget| budget.id);

    if budgets.is_empty() {
        return Ok(BudgetEstimateSummary {
            status: "no_budgets".to_string(),
            max_estimated_cost_usd,
            rows: Vec::new(),
        });
    }

    let running_totals = fetch_latest_running_totals(store).await?;
    let mut rows = Vec::with_capacity(budgets.len());
    let mut hard_limit_exceeded = false;
    let mut soft_limit_reached = false;

    for budget in budgets {
        let accumulated_usd = running_totals
            .get(&budget.id)
            .copied()
            .unwrap_or(Money::ZERO);
        let projected_usd = accumulated_usd
            .checked_add(max_estimated_cost_usd)
            .unwrap_or(Money::from_micros(i64::MAX));
        let hard_exceeded = budget
            .hard_limit_usd
            .map(|limit| projected_usd > limit)
            .unwrap_or(false);
        let soft_reached = budget
            .soft_limit_usd
            .map(|limit| projected_usd >= limit)
            .unwrap_or(false);

        hard_limit_exceeded |= hard_exceeded;
        soft_limit_reached |= soft_reached;

        rows.push(BudgetEstimateRow {
            budget_id: budget.id,
            scope_type: budget.scope_type,
            scope_id: budget.scope_id,
            window_type: budget.window_type,
            accumulated_usd,
            projected_usd,
            hard_limit_usd: budget.hard_limit_usd,
            soft_limit_usd: budget.soft_limit_usd,
            hard_limit_exceeded: hard_exceeded,
            soft_limit_reached: soft_reached,
        });
    }

    let status = if hard_limit_exceeded {
        "over_hard_limit"
    } else if soft_limit_reached {
        "soft_limit_reached"
    } else {
        "within_limit"
    };

    Ok(BudgetEstimateSummary {
        status: status.to_string(),
        max_estimated_cost_usd,
        rows,
    })
}

async fn fetch_latest_running_totals(store: &SqliteStore) -> Result<HashMap<i64, Money>, CliError> {
    let rows = query(
        r#"
        SELECT ledger.budget_id, ledger.running_total_micros
        FROM cost_ledger AS ledger
        JOIN (
            SELECT budget_id, MAX(id) AS latest_id
            FROM cost_ledger
            GROUP BY budget_id
        ) AS latest ON latest.latest_id = ledger.id
        "#,
    )
    .fetch_all(store.pool())
    .await
    .map_err(|error| CliError::Store(error.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.get::<i64, _>("budget_id"),
                Money::from_micros(row.get("running_total_micros")),
            )
        })
        .collect())
}

fn print_estimate_summary(summary: EstimateSummary<'_>) {
    println!("Estimate");
    println!("  model: {}", summary.model);
    println!("  task_type: {}", summary.task_type.as_label());
    println!(
        "  context: {} tokens (source: {}, files: {})",
        summary.context.tokens, summary.context.source, summary.context.file_count
    );
    println!(
        "  range_usd: {:.6} - {:.6}",
        summary.min_usd, summary.max_usd
    );
    println!(
        "  confidence: {}",
        match summary.confidence {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    );
    if let Some(route) = summary.route_preview {
        println!(
            "  route_preview: provider={} ({}, api={}) external_model={} enabled={}",
            route.provider_id,
            route.provider_name,
            route.api_format,
            route.external_model,
            route.enabled
        );
    } else {
        println!("  route_preview: unavailable (model not found in routing table)");
    }
    println!(
        "  budget: status={} max_estimated_cost_usd={}",
        summary.budget.status, summary.budget.max_estimated_cost_usd
    );
    if let Some(project_id) = summary.project_id {
        println!("  project_id: {project_id}");
    }
    if let Some(session_id) = summary.session_id {
        println!("  session_id: {session_id}");
    }
    for row in &summary.budget.rows {
        println!(
            "  budget_row: id={} scope={:?}:{} window={:?} accumulated={} projected={} hard_limit={:?} soft_limit={:?} hard_exceeded={} soft_reached={}",
            row.budget_id,
            row.scope_type,
            row.scope_id,
            row.window_type,
            row.accumulated_usd,
            row.projected_usd,
            row.hard_limit_usd,
            row.soft_limit_usd,
            row.hard_limit_exceeded,
            row.soft_limit_reached
        );
    }
    println!("  pricebook_snapshot: {}", summary.pricebook_snapshot);
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
    use std::fs;

    use chrono::Duration as ChronoDuration;
    use penny_store::{NewRequest, ProjectRepo, RequestRepo, UsageRecord};
    use penny_types::{Money, UsageSource};
    use tempfile::tempdir;

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

    #[test]
    fn estimate_context_from_context_files_glob() {
        let dir = tempdir().expect("temp dir");
        let root = dir.path();
        fs::create_dir_all(root.join("src/nested")).expect("mkdir");
        fs::write(root.join("src/lib.rs"), "fn a() { println!(\"hello\"); }").expect("write");
        fs::write(root.join("src/nested/mod.rs"), "pub fn b() {}").expect("write");

        let pattern = format!("{}/src/**/*.rs", root.display());
        let estimate = estimate_context(None, &[pattern], 100).expect("estimate context");
        assert_eq!(estimate.file_count, 2);
        assert!(estimate.tokens > 0);
    }

    #[test]
    fn estimate_context_requires_tokens_or_files() {
        let error = estimate_context(None, &[], 100).expect_err("missing context must fail");
        assert!(matches!(error, CliError::MissingEstimateContext));
    }

    #[test]
    fn estimate_context_fails_when_no_files_match() {
        let error = estimate_context(None, &[String::from("/tmp/does-not-exist/**/*.rs")], 100)
            .expect_err("expected no matches");
        assert!(matches!(error, CliError::NoContextFilesMatched));
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
