use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use comfy_table::{presets::UTF8_FULL, Attribute, Cell, Table};
use glob::glob;
use penny_config::{
    load_config, resolve_user_config_path, ConfigError, LoadOptions, PRESET_EXPLORE, PRESET_INDIE,
    PRESET_TEAM,
};
use penny_cost::{estimate_tokens, import_pricebook_files, PricingEngine};
use penny_store::{BudgetRepo, ProjectRepo, SessionRepo, SqliteStore};
use penny_types::{Budget, Confidence, Event, EventType, Money, ScopeType, TaskType, WindowType};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{query, Row};
use thiserror::Error;

const PRESET_INDIE_TOML: &str = include_str!("../../../presets/indie.toml");
const PRESET_TEAM_TOML: &str = include_str!("../../../presets/team.toml");
const PRESET_EXPLORE_TOML: &str = include_str!("../../../presets/explore.toml");
const PRICEBOOK_ANTHROPIC_TOML: &str = include_str!("../../../prices/anthropic.toml");
const PRICEBOOK_OPENAI_TOML: &str = include_str!("../../../prices/openai.toml");

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
    Init {
        #[arg(long, default_value = PRESET_INDIE)]
        preset: String,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    Doctor,
    Config {
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Prices {
        #[command(subcommand)]
        command: PricesCommands,
    },
    Budget {
        #[command(subcommand)]
        command: BudgetCommands,
    },
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
    Detect {
        #[command(subcommand)]
        command: DetectCommands,
    },
    Run {
        agent: String,
        #[arg(long, default_value_t = false)]
        execute: bool,
        #[arg(long)]
        project_id: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Tail {
        #[arg(long, default_value = "http://127.0.0.1:8586")]
        admin_url: String,
        #[arg(long)]
        since_id: Option<i64>,
        #[arg(long, default_value_t = 500)]
        poll_ms: u64,
        #[arg(long, default_value_t = 100)]
        limit: u32,
        #[arg(long, default_value_t = false)]
        once: bool,
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
        #[arg(long, value_enum, default_value_t = SummaryFormat::Table)]
        format: SummaryFormat,
    },
    Top {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
}

#[derive(Debug, Subcommand)]
enum DetectCommands {
    Status {
        #[arg(long, default_value = "http://127.0.0.1:8586")]
        admin_url: String,
    },
    Resume {
        session_id: String,
        #[arg(long)]
        request_id: Option<String>,
        #[arg(long, default_value = "http://127.0.0.1:8586")]
        admin_url: String,
    },
}

#[derive(Debug, Subcommand)]
enum PricesCommands {
    Show {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    Update,
}

#[derive(Debug, Subcommand)]
enum BudgetCommands {
    List,
    Set {
        #[arg(long, value_enum)]
        scope_type: BudgetScopeArg,
        #[arg(long)]
        scope_id: String,
        #[arg(long, value_enum)]
        window_type: BudgetWindowArg,
        #[arg(long)]
        hard_limit_usd: Option<f64>,
        #[arg(long)]
        soft_limit_usd: Option<f64>,
        #[arg(long, default_value = "block")]
        action_on_hard: String,
        #[arg(long, default_value = "warn")]
        action_on_soft: String,
    },
    Reset {
        #[arg(long, value_enum)]
        scope_type: BudgetScopeArg,
        #[arg(long)]
        scope_id: String,
        #[arg(long, value_enum)]
        window_type: BudgetWindowArg,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SummaryBy {
    Project,
    Model,
    Session,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SummaryFormat {
    Table,
    Json,
    Csv,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum EstimateTaskType {
    SinglePass,
    MultiRound,
    AgentTask,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BudgetScopeArg {
    Global,
    Project,
    Session,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BudgetWindowArg {
    Day,
    Week,
    Month,
    Total,
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

impl BudgetScopeArg {
    fn as_scope_type(self) -> ScopeType {
        match self {
            Self::Global => ScopeType::Global,
            Self::Project => ScopeType::Project,
            Self::Session => ScopeType::Session,
        }
    }
}

impl BudgetWindowArg {
    fn as_window_type(self) -> WindowType {
        match self {
            Self::Day => WindowType::Day,
            Self::Week => WindowType::Week,
            Self::Month => WindowType::Month,
            Self::Total => WindowType::Total,
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

struct StagingDirGuard {
    path: PathBuf,
}

impl StagingDirGuard {
    fn new(path: PathBuf) -> Result<Self, CliError> {
        fs::create_dir_all(&path).map_err(|error| {
            CliError::Cost(format!(
                "failed to create temp pricebook directory {}: {error}",
                path.display()
            ))
        })?;
        Ok(Self { path })
    }

    fn create_pricebook_staging_dir() -> Result<Self, CliError> {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "pennyprompt-pricebooks-{}-{seed}",
            std::process::id()
        ));
        Self::new(path)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for StagingDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug, Clone, PartialEq)]
struct TopRow {
    request_id: String,
    project_id: String,
    session_id: Option<String>,
    model: String,
    provider_id: String,
    started_at: String,
    input_tokens: i64,
    output_tokens: i64,
    cost_usd: f64,
    source: String,
}

#[derive(Debug, Clone, PartialEq)]
struct PricebookRow {
    model_id: String,
    source: String,
    input_per_mtok_usd: f64,
    output_per_mtok_usd: f64,
    effective_from: String,
}

#[derive(Debug)]
struct DoctorReport {
    config_ok: bool,
    db_ok: bool,
    anthropic_key_present: bool,
    openai_key_present: bool,
    active_pricebook_models: i64,
    budgets_configured: i64,
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

#[derive(Debug, Clone)]
struct TailCommand {
    admin_url: String,
    since_id: Option<i64>,
    poll_ms: u64,
    limit: u32,
    once: bool,
}

#[derive(Debug, Clone)]
struct RunCommand {
    agent: String,
    execute: bool,
    project_id: Option<String>,
    session_id: Option<String>,
    cwd: Option<PathBuf>,
    as_json: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct LaunchPlan {
    mode: String,
    agent: String,
    project_id: String,
    session_id: String,
    cwd: String,
    proxy_bind: String,
    admin_socket: String,
    database_path: String,
    config_path: String,
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

#[derive(Debug, Default)]
struct SseDecoder {
    buffer: Vec<u8>,
    current_event: Option<String>,
    current_data: Vec<String>,
}

#[derive(Debug, Default)]
struct SseMessage {
    event: Option<String>,
    data: String,
}

#[derive(Debug, Deserialize)]
struct DetectStatusResponse {
    enabled: bool,
    paused_sessions: Vec<DetectPausedSession>,
    active_alerts: Vec<DetectSessionAlert>,
}

#[derive(Debug, Deserialize)]
struct DetectPausedSession {
    session_id: String,
    reason: String,
    paused_at: DateTime<Utc>,
    triggered_by: Value,
}

#[derive(Debug, Deserialize)]
struct DetectSessionAlert {
    session_id: String,
    alert: Value,
    triggered_at: DateTime<Utc>,
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
    #[error("tail request failed: {0}")]
    TailRequest(String),
    #[error("tail endpoint returned {status}: {body}")]
    TailStatus { status: u16, body: String },
    #[error("tail stream parse error: {0}")]
    TailParse(String),
    #[error("detect request failed: {0}")]
    DetectRequest(String),
    #[error("detect endpoint returned {status}: {body}")]
    DetectStatus { status: u16, body: String },
    #[error("detect payload parse error: {0}")]
    DetectParse(String),
    #[error("run command error: {0}")]
    Run(String),
    #[error("init error: {0}")]
    Init(String),
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
        Commands::Init { preset, force } => {
            run_init(&preset, force)?;
        }
        Commands::Doctor => {
            run_doctor(&store).await?;
        }
        Commands::Config { json } => {
            run_config(json)?;
        }
        Commands::Prices { command } => match command {
            PricesCommands::Show { limit } => {
                run_prices_show(&store, limit).await?;
            }
            PricesCommands::Update => {
                run_prices_update(&store).await?;
            }
        },
        Commands::Budget { command } => match command {
            BudgetCommands::List => {
                run_budget_list(&store).await?;
            }
            BudgetCommands::Set {
                scope_type,
                scope_id,
                window_type,
                hard_limit_usd,
                soft_limit_usd,
                action_on_hard,
                action_on_soft,
            } => {
                run_budget_set(
                    &store,
                    scope_type,
                    &scope_id,
                    window_type,
                    hard_limit_usd,
                    soft_limit_usd,
                    &action_on_hard,
                    &action_on_soft,
                )
                .await?;
            }
            BudgetCommands::Reset {
                scope_type,
                scope_id,
                window_type,
            } => {
                run_budget_reset(&store, scope_type, &scope_id, window_type).await?;
            }
        },
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
        Commands::Run {
            agent,
            execute,
            project_id,
            session_id,
            cwd,
            json,
        } => {
            run_launcher(
                &store,
                RunCommand {
                    agent,
                    execute,
                    project_id,
                    session_id,
                    cwd,
                    as_json: json,
                },
            )
            .await?;
        }
        Commands::Detect { command } => match command {
            DetectCommands::Status { admin_url } => {
                run_detect_status(&admin_url).await?;
            }
            DetectCommands::Resume {
                session_id,
                request_id,
                admin_url,
            } => {
                run_detect_resume(&admin_url, &session_id, request_id.as_deref()).await?;
            }
        },
        Commands::Tail {
            admin_url,
            since_id,
            poll_ms,
            limit,
            once,
        } => {
            run_tail(TailCommand {
                admin_url,
                since_id,
                poll_ms,
                limit,
                once,
            })
            .await?;
        }
        Commands::Report { command } => match command {
            ReportCommands::Summary { since, by, format } => {
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
                print_summary(by, since.as_deref(), &rows, format)?;
            }
            ReportCommands::Top { limit } => {
                run_report_top(&store, limit).await?;
            }
        },
    }
    Ok(())
}

fn run_init(preset: &str, force: bool) -> Result<(), CliError> {
    validate_preset(preset)?;
    let target = resolve_user_config_target()?;
    if target.exists() && !force {
        return Err(CliError::Init(format!(
            "config already exists at {} (use --force to overwrite)",
            target.display()
        )));
    }

    let raw = preset_template(preset)?;

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            CliError::Init(format!("failed creating {}: {error}", parent.display()))
        })?;
    }
    fs::write(&target, raw)
        .map_err(|error| CliError::Init(format!("failed writing {}: {error}", target.display())))?;

    println!("Initialized config from preset `{preset}`");
    println!("  path: {}", target.display());
    println!("  next: run `pennyprompt doctor` then `pennyprompt report summary --since 1d`");
    Ok(())
}

async fn run_doctor(store: &SqliteStore) -> Result<(), CliError> {
    let config =
        load_config(LoadOptions::default()).map_err(|error| CliError::Config(error.to_string()))?;
    let db_ok = query_scalar_one(store, "SELECT 1").await.is_ok();
    let active_pricebook_models = query_scalar_i64(
        store,
        "SELECT COUNT(DISTINCT model_id) FROM pricebook_entries WHERE datetime(effective_from) <= datetime('now') AND (effective_until IS NULL OR datetime(effective_until) > datetime('now'))",
    )
    .await
    .unwrap_or(0);
    let budgets_configured = query_scalar_i64(store, "SELECT COUNT(*) FROM budgets")
        .await
        .unwrap_or(0);

    let anthropic_key_present = !config.providers.anthropic.api_key_env.trim().is_empty()
        && std::env::var(&config.providers.anthropic.api_key_env)
            .ok()
            .is_some_and(|value| !value.trim().is_empty());
    let openai_key_present = !config.providers.openai.api_key_env.trim().is_empty()
        && std::env::var(&config.providers.openai.api_key_env)
            .ok()
            .is_some_and(|value| !value.trim().is_empty());

    let report = DoctorReport {
        config_ok: true,
        db_ok,
        anthropic_key_present,
        openai_key_present,
        active_pricebook_models,
        budgets_configured,
    };
    print_doctor_report(&report, &config.server.database_path);
    Ok(())
}

fn run_config(as_json: bool) -> Result<(), CliError> {
    let config =
        load_config(LoadOptions::default()).map_err(|error| CliError::Config(error.to_string()))?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&config)
                .map_err(|error| CliError::Config(error.to_string()))?
        );
    } else {
        println!(
            "{}",
            toml::to_string_pretty(&config).map_err(|error| CliError::Config(error.to_string()))?
        );
    }
    Ok(())
}

async fn run_prices_show(store: &SqliteStore, limit: u32) -> Result<(), CliError> {
    let rows = fetch_pricebook_rows(store, limit).await?;
    if rows.is_empty() {
        println!("No active pricebook entries found.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL).set_header([
        Cell::new("model").add_attribute(Attribute::Bold),
        Cell::new("source").add_attribute(Attribute::Bold),
        Cell::new("input_per_mtok").add_attribute(Attribute::Bold),
        Cell::new("output_per_mtok").add_attribute(Attribute::Bold),
        Cell::new("effective_from").add_attribute(Attribute::Bold),
    ]);
    for row in rows {
        table.add_row([
            Cell::new(row.model_id),
            Cell::new(row.source),
            Cell::new(format!("{:.6}", row.input_per_mtok_usd)),
            Cell::new(format!("{:.6}", row.output_per_mtok_usd)),
            Cell::new(row.effective_from),
        ]);
    }

    println!("Active pricebook entries");
    println!("{table}");
    Ok(())
}

async fn run_prices_update(store: &SqliteStore) -> Result<(), CliError> {
    let staging_dir = StagingDirGuard::create_pricebook_staging_dir()?;
    let anthropic = staging_dir.file("anthropic.toml");
    let openai = staging_dir.file("openai.toml");
    fs::write(&anthropic, PRICEBOOK_ANTHROPIC_TOML).map_err(|error| {
        CliError::Cost(format!(
            "failed to stage embedded pricebook {}: {error}",
            anthropic.display()
        ))
    })?;
    fs::write(&openai, PRICEBOOK_OPENAI_TOML).map_err(|error| {
        CliError::Cost(format!(
            "failed to stage embedded pricebook {}: {error}",
            openai.display()
        ))
    })?;

    let imported = import_pricebook_files(store, &[anthropic.clone(), openai.clone()])
        .await
        .map_err(|error| CliError::Cost(error.to_string()))?;
    println!("Pricebook update completed");
    println!("  imported_entries: {imported}");
    Ok(())
}

async fn run_budget_list(store: &SqliteStore) -> Result<(), CliError> {
    let budgets = store
        .list_all()
        .await
        .map_err(|error| CliError::Store(error.to_string()))?;
    if budgets.is_empty() {
        println!("No budgets configured.");
        return Ok(());
    }
    let totals = fetch_latest_running_totals(store).await?;

    let mut table = Table::new();
    table.load_preset(UTF8_FULL).set_header([
        Cell::new("id").add_attribute(Attribute::Bold),
        Cell::new("scope").add_attribute(Attribute::Bold),
        Cell::new("window").add_attribute(Attribute::Bold),
        Cell::new("hard_limit").add_attribute(Attribute::Bold),
        Cell::new("soft_limit").add_attribute(Attribute::Bold),
        Cell::new("accumulated").add_attribute(Attribute::Bold),
    ]);
    for budget in budgets {
        let accumulated = totals.get(&budget.id).copied().unwrap_or(Money::ZERO);
        table.add_row([
            Cell::new(budget.id),
            Cell::new(format!("{:?}:{}", budget.scope_type, budget.scope_id)),
            Cell::new(format!("{:?}", budget.window_type)),
            Cell::new(
                budget
                    .hard_limit_usd
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "-".to_string()),
            ),
            Cell::new(
                budget
                    .soft_limit_usd
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "-".to_string()),
            ),
            Cell::new(accumulated.to_string()),
        ]);
    }

    println!("Budgets");
    println!("{table}");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_budget_set(
    store: &SqliteStore,
    scope_type: BudgetScopeArg,
    scope_id: &str,
    window_type: BudgetWindowArg,
    hard_limit_usd: Option<f64>,
    soft_limit_usd: Option<f64>,
    action_on_hard: &str,
    action_on_soft: &str,
) -> Result<(), CliError> {
    let hard_limit_usd = hard_limit_usd
        .map(Money::from_usd)
        .transpose()
        .map_err(|error| CliError::Store(error.to_string()))?;
    let soft_limit_usd = soft_limit_usd
        .map(Money::from_usd)
        .transpose()
        .map_err(|error| CliError::Store(error.to_string()))?;

    let budget = Budget {
        id: 0,
        scope_type: scope_type.as_scope_type(),
        scope_id: scope_id.to_string(),
        window_type: window_type.as_window_type(),
        hard_limit_usd,
        soft_limit_usd,
        action_on_hard: action_on_hard.to_string(),
        action_on_soft: action_on_soft.to_string(),
        preset_source: Some("cli".to_string()),
    };
    let stored = store
        .upsert(&budget)
        .await
        .map_err(|error| CliError::Store(error.to_string()))?;

    println!("Budget upserted");
    println!(
        "  id={} scope={:?}:{} window={:?} hard={:?} soft={:?}",
        stored.id,
        stored.scope_type,
        stored.scope_id,
        stored.window_type,
        stored.hard_limit_usd,
        stored.soft_limit_usd
    );
    Ok(())
}

async fn run_budget_reset(
    store: &SqliteStore,
    scope_type: BudgetScopeArg,
    scope_id: &str,
    window_type: BudgetWindowArg,
) -> Result<(), CliError> {
    let affected = query(
        r#"
        DELETE FROM budgets
        WHERE scope_type = ?1 AND scope_id = ?2 AND window_type = ?3
        "#,
    )
    .bind(scope_type_db(scope_type.as_scope_type()))
    .bind(scope_id)
    .bind(window_type_db(window_type.as_window_type()))
    .execute(store.pool())
    .await
    .map_err(|error| CliError::Store(error.to_string()))?
    .rows_affected();

    println!("Budget reset");
    println!("  removed_rows: {affected}");
    Ok(())
}

async fn run_report_top(store: &SqliteStore, limit: u32) -> Result<(), CliError> {
    let rows = fetch_top_rows(store, limit).await?;
    if rows.is_empty() {
        println!("No usage rows found for report top.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL).set_header([
        Cell::new("request_id").add_attribute(Attribute::Bold),
        Cell::new("project").add_attribute(Attribute::Bold),
        Cell::new("model").add_attribute(Attribute::Bold),
        Cell::new("tokens").add_attribute(Attribute::Bold),
        Cell::new("cost_usd").add_attribute(Attribute::Bold),
    ]);
    for row in rows {
        table.add_row([
            Cell::new(row.request_id),
            Cell::new(row.project_id),
            Cell::new(row.model),
            Cell::new(format!("{}/{}", row.input_tokens, row.output_tokens)),
            Cell::new(format!("{:.6}", row.cost_usd)),
        ]);
    }

    println!("Top requests by cost");
    println!("{table}");
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

async fn run_launcher(store: &SqliteStore, command: RunCommand) -> Result<(), CliError> {
    let agent = normalize_required_arg(&command.agent, "agent")?;
    if command.execute {
        return Err(CliError::Run(
            "execute mode is not implemented yet. Rerun without --execute for deterministic dry-run output.".to_string(),
        ));
    }

    let explicit_cwd = command.cwd.is_some();
    let cwd = resolve_launcher_cwd(command.cwd)?;
    let config = load_config(LoadOptions {
        repository_root: Some(cwd.clone()),
        ..Default::default()
    })
    .or_else(|error| match (explicit_cwd, error) {
        (true, ConfigError::MissingDefaultConfig(_)) => load_config(LoadOptions::default()),
        (_, other) => Err(other),
    })
    .map_err(|error| CliError::Config(error.to_string()))?;
    let project_id = resolve_launch_project_id(
        store,
        &cwd,
        command.project_id.as_deref(),
        config.attribution.auto_detect_project,
    )
    .await?;
    let session_id = resolve_launch_session_id(
        store,
        &project_id,
        command.session_id.as_deref(),
        config.attribution.session_window_minutes as u64,
    )
    .await?;
    let config_path = resolve_user_config_target()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "(unresolved)".to_string());
    let plan = LaunchPlan {
        mode: "dry_run".to_string(),
        agent,
        project_id,
        session_id,
        cwd: cwd.display().to_string(),
        proxy_bind: config.server.bind,
        admin_socket: config.server.admin_socket,
        database_path: config.server.database_path,
        config_path,
    };

    if command.as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&plan)
                .map_err(|error| CliError::Run(error.to_string()))?
        );
    } else {
        println!("{}", render_launch_plan(&plan));
    }
    Ok(())
}

async fn resolve_launch_project_id(
    store: &SqliteStore,
    cwd: &Path,
    project_override: Option<&str>,
    auto_detect_project: bool,
) -> Result<String, CliError> {
    if let Some(project_id) = project_override {
        return normalize_required_arg(project_id, "project_id");
    }
    if !auto_detect_project {
        return Ok("default".to_string());
    }

    let cwd_text = cwd.display().to_string();
    let project = store
        .get_by_path(&cwd_text)
        .await
        .map_err(|error| CliError::Store(error.to_string()))?;
    if let Some(project) = project {
        return Ok(project.id);
    }
    Ok(default_project_id_from_cwd(cwd))
}

async fn resolve_launch_session_id(
    store: &SqliteStore,
    project_id: &str,
    session_override: Option<&str>,
    session_window_minutes: u64,
) -> Result<String, CliError> {
    if let Some(session_id) = session_override {
        return normalize_required_arg(session_id, "session_id");
    }

    let session = store
        .find_active(project_id, session_window_minutes.max(1))
        .await
        .map_err(|error| CliError::Store(error.to_string()))?;
    Ok(session.unwrap_or_else(|| "session-auto".to_string()))
}

fn normalize_required_arg(raw: &str, field: &str) -> Result<String, CliError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(CliError::Run(format!("`{field}` must not be empty")));
    }
    Ok(value.to_string())
}

fn default_project_id_from_cwd(cwd: &Path) -> String {
    let seed = cwd
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("default");
    let mut slug = String::with_capacity(seed.len());
    for ch in seed.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };
        if normalized == '-' {
            if slug.ends_with('-') {
                continue;
            }
            slug.push('-');
            continue;
        }
        slug.push(normalized);
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "default".to_string()
    } else {
        slug.to_string()
    }
}

fn render_launch_plan(plan: &LaunchPlan) -> String {
    format!(
        "Run launcher plan\n  mode: {}\n  agent: {}\n  project_id: {}\n  session_id: {}\n  cwd: {}\n  proxy_bind: {}\n  admin_socket: {}\n  database_path: {}\n  config_path: {}",
        plan.mode,
        plan.agent,
        plan.project_id,
        plan.session_id,
        plan.cwd,
        plan.proxy_bind,
        plan.admin_socket,
        plan.database_path,
        plan.config_path
    )
}

async fn run_tail(command: TailCommand) -> Result<(), CliError> {
    let url = build_tail_url(
        &command.admin_url,
        command.since_id,
        command.poll_ms,
        command.limit,
        command.once,
    );
    let response = Client::new()
        .get(url)
        .send()
        .await
        .map_err(|error| CliError::TailRequest(error.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(CliError::TailStatus {
            status: status.as_u16(),
            body,
        });
    }

    let no_color = std::env::var_os("NO_COLOR").is_some();
    let mut response = response;
    let mut decoder = SseDecoder::default();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| CliError::TailRequest(error.to_string()))?
    {
        for message in decoder.push(&chunk) {
            print_sse_message(&message, no_color)?;
        }
    }
    for message in decoder.finish() {
        print_sse_message(&message, no_color)?;
    }

    Ok(())
}

async fn run_detect_status(admin_url: &str) -> Result<(), CliError> {
    let url = format!("{}/admin/detect/status", admin_url.trim_end_matches('/'));
    let response = Client::new()
        .get(url)
        .send()
        .await
        .map_err(|error| CliError::DetectRequest(error.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| CliError::DetectRequest(error.to_string()))?;
    if !status.is_success() {
        return Err(CliError::DetectStatus {
            status: status.as_u16(),
            body,
        });
    }

    let payload: DetectStatusResponse =
        serde_json::from_str(&body).map_err(|error| CliError::DetectParse(error.to_string()))?;

    println!("Detect status");
    println!("  enabled: {}", payload.enabled);
    println!("  paused_sessions: {}", payload.paused_sessions.len());
    for paused in payload.paused_sessions {
        println!(
            "  - session={} paused_at={} reason={} triggered_by={}",
            paused.session_id,
            paused.paused_at.to_rfc3339(),
            paused.reason,
            paused.triggered_by
        );
    }
    println!("  active_alerts: {}", payload.active_alerts.len());
    for alert in payload.active_alerts {
        println!(
            "  - session={} triggered_at={} alert={}",
            alert.session_id,
            alert.triggered_at.to_rfc3339(),
            alert.alert
        );
    }

    Ok(())
}

async fn run_detect_resume(
    admin_url: &str,
    session_id: &str,
    request_id: Option<&str>,
) -> Result<(), CliError> {
    let url = format!("{}/admin/detect/resume", admin_url.trim_end_matches('/'));
    let payload = json!({
        "session_id": session_id,
        "request_id": request_id,
    });
    let response = Client::new()
        .post(url)
        .json(&payload)
        .send()
        .await
        .map_err(|error| CliError::DetectRequest(error.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| CliError::DetectRequest(error.to_string()))?;
    if !status.is_success() {
        return Err(CliError::DetectStatus {
            status: status.as_u16(),
            body,
        });
    }

    let parsed: Value =
        serde_json::from_str(&body).map_err(|error| CliError::DetectParse(error.to_string()))?;
    let resumed = parsed
        .get("resumed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if resumed {
        println!(
            "Detect resume succeeded: session={} request_id={}",
            session_id,
            request_id.unwrap_or("(none)")
        );
    } else {
        println!("Detect resume response: {parsed}");
    }
    Ok(())
}

fn validate_preset(preset: &str) -> Result<(), CliError> {
    match preset {
        PRESET_INDIE | PRESET_TEAM | PRESET_EXPLORE => Ok(()),
        _ => Err(CliError::Init(format!(
            "invalid preset `{preset}` (expected: {PRESET_INDIE}|{PRESET_TEAM}|{PRESET_EXPLORE})"
        ))),
    }
}

fn resolve_user_config_target() -> Result<PathBuf, CliError> {
    resolve_user_config_path(None)
        .map_err(|error| CliError::Init(error.to_string()))?
        .ok_or_else(|| {
            CliError::Init("unable to resolve config path from PENNY_CONFIG or HOME".to_string())
        })
}

fn preset_template(preset: &str) -> Result<&'static str, CliError> {
    match preset {
        PRESET_INDIE => Ok(PRESET_INDIE_TOML),
        PRESET_TEAM => Ok(PRESET_TEAM_TOML),
        PRESET_EXPLORE => Ok(PRESET_EXPLORE_TOML),
        _ => Err(CliError::Init(format!(
            "invalid preset `{preset}` (expected: {PRESET_INDIE}|{PRESET_TEAM}|{PRESET_EXPLORE})"
        ))),
    }
}

fn resolve_launcher_cwd(path: Option<PathBuf>) -> Result<PathBuf, CliError> {
    let raw = if let Some(path) = path {
        path
    } else {
        std::env::current_dir().map_err(|error| CliError::Run(error.to_string()))?
    };
    fs::canonicalize(&raw)
        .map_err(|error| CliError::Run(format!("invalid --cwd path {}: {error}", raw.display())))
}

fn print_doctor_report(report: &DoctorReport, database_path: &str) {
    println!("Doctor");
    println!("  config: {}", if report.config_ok { "ok" } else { "fail" });
    println!("  database: {}", if report.db_ok { "ok" } else { "fail" });
    println!("  database_path: {database_path}");
    println!(
        "  anthropic_api_key: {}",
        if report.anthropic_key_present {
            "present"
        } else {
            "missing"
        }
    );
    println!(
        "  openai_api_key: {}",
        if report.openai_key_present {
            "present"
        } else {
            "missing"
        }
    );
    println!(
        "  active_pricebook_models: {}",
        report.active_pricebook_models
    );
    println!("  budgets_configured: {}", report.budgets_configured);
}

async fn fetch_pricebook_rows(
    store: &SqliteStore,
    limit: u32,
) -> Result<Vec<PricebookRow>, CliError> {
    let rows = query(
        r#"
        SELECT model_id, source, input_per_mtok_micros, output_per_mtok_micros, effective_from
        FROM pricebook_entries
        WHERE datetime(effective_from) <= datetime('now')
          AND (effective_until IS NULL OR datetime(effective_until) > datetime('now'))
        ORDER BY model_id ASC, datetime(effective_from) DESC
        LIMIT ?1
        "#,
    )
    .bind(i64::from(limit.clamp(1, 500)))
    .fetch_all(store.pool())
    .await
    .map_err(|error| CliError::Store(error.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| PricebookRow {
            model_id: row.get("model_id"),
            source: row.get("source"),
            input_per_mtok_usd: Money::from_micros(row.get("input_per_mtok_micros")).to_usd(),
            output_per_mtok_usd: Money::from_micros(row.get("output_per_mtok_micros")).to_usd(),
            effective_from: row.get("effective_from"),
        })
        .collect())
}

async fn fetch_top_rows(store: &SqliteStore, limit: u32) -> Result<Vec<TopRow>, CliError> {
    let rows = query(
        r#"
        SELECT
            requests.id AS request_id,
            requests.project_id,
            requests.session_id,
            requests.model_used,
            requests.provider_id,
            requests.started_at,
            request_usage.input_tokens,
            request_usage.output_tokens,
            request_usage.cost_micros,
            request_usage.source
        FROM requests
        JOIN request_usage ON request_usage.request_id = requests.id
        ORDER BY request_usage.cost_micros DESC, requests.started_at DESC
        LIMIT ?1
        "#,
    )
    .bind(i64::from(limit.clamp(1, 500)))
    .fetch_all(store.pool())
    .await
    .map_err(|error| CliError::Store(error.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| TopRow {
            request_id: row.get("request_id"),
            project_id: row.get("project_id"),
            session_id: row.get("session_id"),
            model: row.get("model_used"),
            provider_id: row.get("provider_id"),
            started_at: row.get("started_at"),
            input_tokens: row.get("input_tokens"),
            output_tokens: row.get("output_tokens"),
            cost_usd: Money::from_micros(row.get("cost_micros")).to_usd(),
            source: row.get("source"),
        })
        .collect())
}

async fn query_scalar_i64(store: &SqliteStore, sql: &str) -> Result<i64, CliError> {
    query(sql)
        .fetch_one(store.pool())
        .await
        .map(|row| row.get::<i64, _>(0))
        .map_err(|error| CliError::Store(error.to_string()))
}

async fn query_scalar_one(store: &SqliteStore, sql: &str) -> Result<i64, CliError> {
    query(sql)
        .fetch_one(store.pool())
        .await
        .map(|row| row.get::<i64, _>(0))
        .map_err(|error| CliError::Store(error.to_string()))
}

fn scope_type_db(scope_type: ScopeType) -> &'static str {
    match scope_type {
        ScopeType::Global => "global",
        ScopeType::Project => "project",
        ScopeType::Session => "session",
    }
}

fn window_type_db(window_type: WindowType) -> &'static str {
    match window_type {
        WindowType::Day => "day",
        WindowType::Week => "week",
        WindowType::Month => "month",
        WindowType::Total => "total",
    }
}

fn build_tail_url(
    admin_url: &str,
    since_id: Option<i64>,
    poll_ms: u64,
    limit: u32,
    once: bool,
) -> String {
    let mut url = format!(
        "{}/admin/events?poll_ms={}&limit={}",
        admin_url.trim_end_matches('/'),
        poll_ms.max(100),
        limit.clamp(1, 500)
    );
    if let Some(since_id) = since_id {
        url.push_str("&since_id=");
        url.push_str(&since_id.to_string());
    }
    if once {
        url.push_str("&once=true");
    }
    url
}

fn print_sse_message(message: &SseMessage, no_color: bool) -> Result<(), CliError> {
    let event_name = message.event.as_deref().unwrap_or("message");
    match event_name {
        "heartbeat" => Ok(()),
        "error" => {
            let payload = serde_json::from_str::<Value>(&message.data).unwrap_or(Value::Null);
            let code = payload
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("admin_error");
            let detail = payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or(message.data.as_str());
            let ts = Utc::now().format("%H:%M:%S");
            println!(
                "{}",
                style(
                    &format!("[{ts}] ADMIN_ERROR code={code} detail={detail}"),
                    "31",
                    no_color
                )
            );
            Ok(())
        }
        "event" => {
            let event = serde_json::from_str::<Event>(&message.data)
                .map_err(|error| CliError::TailParse(error.to_string()))?;
            println!("{}", format_event_line(&event, no_color));
            Ok(())
        }
        _ => {
            println!("{event_name}: {}", message.data);
            Ok(())
        }
    }
}

fn format_event_line(event: &Event, no_color: bool) -> String {
    let ts = event.created_at.format("%H:%M:%S");
    let request = short_id(event.request_id.as_deref());
    let session = short_id(event.session_id.as_deref());

    match event.event_type {
        EventType::BudgetCheck => {
            let estimated = event
                .detail
                .get("estimated_cost_usd")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            format!(
                "[{ts}] REQ request={} session={} est=${estimated:.6}",
                request, session
            )
        }
        EventType::BurnRateAlert => {
            let usd_per_hour = event
                .detail
                .get("usd_per_hour")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let threshold = event
                .detail
                .get("threshold")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            style(
                &format!(
                    "[{ts}] BURN_RATE session={} usd_per_hour=${usd_per_hour:.2} threshold=${threshold:.2}",
                    session
                ),
                "33",
                no_color,
            )
        }
        EventType::BudgetBlock => {
            let (scope, window, accumulated, limit) = budget_block_detail(&event.detail);
            style(
                &format!(
                    "[{ts}] BUDGET_BLOCK session={} scope={}/{} accumulated={} limit={}",
                    session, scope, window, accumulated, limit
                ),
                "31",
                no_color,
            )
        }
        EventType::LoopDetected => {
            let kind = event
                .detail
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            style(
                &format!(
                    "[{ts}] LOOP_DETECTED session={} request={} kind={kind}",
                    session, request
                ),
                "35",
                no_color,
            )
        }
        EventType::SessionPaused => style(
            &format!(
                "[{ts}] SESSION_PAUSED session={} reason={}",
                session,
                event
                    .detail
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ),
            "33",
            no_color,
        ),
        EventType::ProviderFailure => style(
            &format!(
                "[{ts}] PROVIDER_FAILURE request={} session={} provider={} status={}",
                request,
                session,
                event
                    .detail
                    .get("provider_id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                event
                    .detail
                    .get("http_status")
                    .and_then(Value::as_u64)
                    .unwrap_or_default()
            ),
            "31",
            no_color,
        ),
        _ => format!(
            "[{ts}] EVENT type={:?} request={} session={}",
            event.event_type, request, session
        ),
    }
}

fn budget_block_detail(detail: &Value) -> (String, String, String, String) {
    let nested = detail.get("detail").unwrap_or(detail);
    let scope = nested
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let window = nested
        .get("window")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let accumulated = nested
        .get("accumulated_usd")
        .map(Value::to_string)
        .unwrap_or_else(|| "0".to_string());
    let limit = nested
        .get("limit_usd")
        .map(Value::to_string)
        .unwrap_or_else(|| "0".to_string());
    (scope, window, accumulated, limit)
}

fn short_id(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "(none)".to_string();
    };
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(12).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}..")
    } else {
        prefix
    }
}

fn style(text: &str, ansi_code: &str, no_color: bool) -> String {
    if no_color {
        text.to_string()
    } else {
        format!("\u{1b}[{ansi_code}m{text}\u{1b}[0m")
    }
}

impl SseDecoder {
    fn push(&mut self, chunk: &[u8]) -> Vec<SseMessage> {
        self.buffer.extend_from_slice(chunk);
        let mut messages = Vec::new();

        while let Some(index) = self.buffer.iter().position(|&byte| byte == b'\n') {
            let mut line = String::from_utf8_lossy(&self.buffer[..index]).into_owned();
            self.buffer.drain(..=index);
            if line.ends_with('\r') {
                line.pop();
            }

            if line.is_empty() {
                if let Some(message) = self.take_message() {
                    messages.push(message);
                }
                continue;
            }

            if let Some(event) = line.strip_prefix("event:") {
                let event = event.strip_prefix(' ').unwrap_or(event);
                self.current_event = Some(event.to_string());
                continue;
            }

            if let Some(data) = line.strip_prefix("data:") {
                let data = data.strip_prefix(' ').unwrap_or(data);
                self.current_data.push(data.to_string());
            }
        }

        messages
    }

    fn finish(&mut self) -> Vec<SseMessage> {
        if !self.buffer.is_empty() {
            self.buffer.push(b'\n');
            return self.push(&[]);
        }
        self.take_message().into_iter().collect()
    }

    fn take_message(&mut self) -> Option<SseMessage> {
        if self.current_event.is_none() && self.current_data.is_empty() {
            return None;
        }

        let event = self.current_event.take();
        let data = self.current_data.join("\n");
        self.current_data.clear();
        Some(SseMessage { event, data })
    }
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

fn print_summary(
    by: SummaryBy,
    since: Option<&str>,
    rows: &[SummaryRow],
    format: SummaryFormat,
) -> Result<(), CliError> {
    match format {
        SummaryFormat::Table => {
            print_summary_table(by, since, rows);
            Ok(())
        }
        SummaryFormat::Json => {
            print_summary_json(rows)?;
            Ok(())
        }
        SummaryFormat::Csv => {
            print_summary_csv(rows);
            Ok(())
        }
    }
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

fn print_summary_json(rows: &[SummaryRow]) -> Result<(), CliError> {
    let rendered = render_summary_json(rows)?;
    println!("{rendered}");
    Ok(())
}

fn print_summary_csv(rows: &[SummaryRow]) {
    println!("{}", render_summary_csv(rows));
}

fn render_summary_json(rows: &[SummaryRow]) -> Result<String, CliError> {
    let payload = rows
        .iter()
        .map(|row| {
            json!({
                "group_key": row.group_key,
                "request_count": row.request_count,
                "input_tokens": row.input_tokens,
                "output_tokens": row.output_tokens,
                "total_cost_usd": row.total_cost_usd,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&payload)
        .map_err(|error| CliError::Store(format!("failed to render summary json: {error}")))
}

fn render_summary_csv(rows: &[SummaryRow]) -> String {
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push("group_key,request_count,input_tokens,output_tokens,total_cost_usd".to_string());
    for row in rows {
        lines.push(format!(
            "{},{},{},{},{:.6}",
            csv_escape(&row.group_key),
            row.request_count,
            row.input_tokens,
            row.output_tokens,
            row.total_cost_usd
        ));
    }
    lines.join("\n")
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        let escaped = value.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use chrono::{Duration as ChronoDuration, TimeZone};
    use penny_store::{NewRequest, ProjectRepo, RequestRepo, UsageRecord};
    use penny_types::{EventType, Money, Severity, UsageSource};
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

    fn event(event_type: EventType, detail: Value) -> Event {
        Event {
            id: 1,
            request_id: Some("req_1234567890abcdef".to_string()),
            session_id: Some("sess_abcdef1234567890".to_string()),
            event_type,
            severity: Severity::Warn,
            detail,
            created_at: Utc
                .with_ymd_and_hms(2026, 4, 18, 12, 0, 0)
                .single()
                .expect("timestamp"),
        }
    }

    fn golden(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("golden")
            .join(name);
        fs::read_to_string(path)
            .expect("golden file")
            .trim_end()
            .to_string()
    }

    #[test]
    fn format_event_line_formats_burn_rate() {
        let line = format_event_line(
            &event(
                EventType::BurnRateAlert,
                serde_json::json!({
                    "kind": "burn_rate",
                    "usd_per_hour": 12.34,
                    "threshold": 10.0
                }),
            ),
            true,
        );

        assert!(line.contains("BURN_RATE"));
        assert!(line.contains("usd_per_hour=$12.34"));
        assert!(!line.contains("\u{1b}["));
    }

    #[test]
    fn format_event_line_formats_budget_block_and_loop() {
        let block_line = format_event_line(
            &event(
                EventType::BudgetBlock,
                serde_json::json!({
                    "detail": {
                        "scope": "global:*",
                        "window": "day",
                        "accumulated_usd": 10.12,
                        "limit_usd": 10.0
                    }
                }),
            ),
            true,
        );
        assert!(block_line.contains("BUDGET_BLOCK"));
        assert!(block_line.contains("scope=global:*/day"));

        let loop_line = format_event_line(
            &event(
                EventType::LoopDetected,
                serde_json::json!({
                    "kind": "content_similarity",
                    "similar_count": 8
                }),
            ),
            true,
        );
        assert!(loop_line.contains("LOOP_DETECTED"));
        assert!(loop_line.contains("kind=content_similarity"));
    }

    #[test]
    fn sse_decoder_parses_chunked_event() {
        let mut decoder = SseDecoder::default();
        let first = b"event: event\ndata: {\"id\":1";
        let second = b",\"event_type\":\"budget_check\"}\n\n";

        assert!(decoder.push(first).is_empty());
        let messages = decoder.push(second);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].event.as_deref(), Some("event"));
        assert!(messages[0].data.contains("\"event_type\":\"budget_check\""));
    }

    #[test]
    fn sse_decoder_preserves_utf8_when_codepoint_is_split_across_chunks() {
        let mut decoder = SseDecoder::default();
        let first = b"event: event\ndata: {\"text\":\"caf\xc3";
        let second = b"\xa9\"}\n\n";

        assert!(decoder.push(first).is_empty());
        let messages = decoder.push(second);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].event.as_deref(), Some("event"));
        assert!(messages[0].data.contains("café"));
    }

    #[test]
    fn sse_decoder_removes_only_single_leading_space_after_field_separator() {
        let mut decoder = SseDecoder::default();
        let messages = decoder.push(b"event:  detect\ndata:  x\n\n");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].event.as_deref(), Some(" detect"));
        assert_eq!(messages[0].data, " x");
    }

    #[test]
    fn golden_burn_rate_line_matches_fixture() {
        let line = format_event_line(
            &event(
                EventType::BurnRateAlert,
                serde_json::json!({
                    "kind": "burn_rate",
                    "usd_per_hour": 12.34,
                    "threshold": 10.0
                }),
            ),
            true,
        );
        assert_eq!(line, golden("burn_rate_line.txt"));
    }

    #[test]
    fn golden_budget_block_line_matches_fixture() {
        let line = format_event_line(
            &event(
                EventType::BudgetBlock,
                serde_json::json!({
                    "detail": {
                        "scope": "global:*",
                        "window": "day",
                        "accumulated_usd": 10.12,
                        "limit_usd": 10.0
                    }
                }),
            ),
            true,
        );
        assert_eq!(line, golden("budget_block_line.txt"));
    }

    #[test]
    fn golden_loop_line_matches_fixture() {
        let line = format_event_line(
            &event(
                EventType::LoopDetected,
                serde_json::json!({
                    "kind": "content_similarity",
                    "similar_count": 8
                }),
            ),
            true,
        );
        assert_eq!(line, golden("loop_detected_line.txt"));
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

    #[test]
    fn render_summary_json_outputs_valid_rows_array() {
        let rows = vec![
            SummaryRow {
                group_key: "proj-a".to_string(),
                request_count: 2,
                input_tokens: 300,
                output_tokens: 150,
                total_cost_usd: 3.5,
            },
            SummaryRow {
                group_key: "proj-b".to_string(),
                request_count: 1,
                input_tokens: 100,
                output_tokens: 50,
                total_cost_usd: 1.0,
            },
        ];
        let rendered = render_summary_json(&rows).expect("render summary json");
        let parsed: Value = serde_json::from_str(&rendered).expect("parse rendered json");
        let items = parsed.as_array().expect("json array");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["group_key"], "proj-a");
        assert_eq!(items[0]["request_count"], 2);
        assert_eq!(items[1]["total_cost_usd"], 1.0);
    }

    #[test]
    fn render_summary_csv_includes_header_and_escapes_group_key() {
        let rows = vec![SummaryRow {
            group_key: "proj,\"a\"".to_string(),
            request_count: 2,
            input_tokens: 300,
            output_tokens: 150,
            total_cost_usd: 3.5,
        }];
        let rendered = render_summary_csv(&rows);
        let mut lines = rendered.lines();
        assert_eq!(
            lines.next(),
            Some("group_key,request_count,input_tokens,output_tokens,total_cost_usd")
        );
        assert_eq!(lines.next(), Some("\"proj,\"\"a\"\"\",2,300,150,3.500000"));
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn normalize_required_arg_rejects_empty_values() {
        let error = normalize_required_arg("   ", "agent").expect_err("must fail");
        assert!(matches!(error, CliError::Run(_)));
    }

    #[test]
    fn default_project_id_from_cwd_slugifies_deterministically() {
        let cwd = Path::new("/tmp/My Project__CLI!");
        assert_eq!(default_project_id_from_cwd(cwd), "my-project-cli");

        let only_symbols = Path::new("/tmp/___");
        assert_eq!(default_project_id_from_cwd(only_symbols), "default");
    }

    #[test]
    fn resolve_launcher_cwd_canonicalizes_existing_path() {
        let temp = tempdir().expect("temp dir");
        let nested = temp.path().join("workspace");
        fs::create_dir_all(&nested).expect("create nested dir");

        let resolved = resolve_launcher_cwd(Some(nested.clone())).expect("resolve cwd");
        let canonical = fs::canonicalize(&nested).expect("canonical path");
        assert_eq!(resolved, canonical);
    }

    #[test]
    fn resolve_launcher_cwd_rejects_missing_path() {
        let temp = tempdir().expect("temp dir");
        let missing = temp.path().join("does-not-exist");
        let error = resolve_launcher_cwd(Some(missing)).expect_err("missing path must fail");
        assert!(matches!(error, CliError::Run(_)));
    }

    #[test]
    fn preset_template_uses_embedded_templates() {
        let indie = preset_template(PRESET_INDIE).expect("indie preset");
        let team = preset_template(PRESET_TEAM).expect("team preset");
        let explore = preset_template(PRESET_EXPLORE).expect("explore preset");

        assert!(indie.contains("[server]"));
        assert!(team.contains("[server]"));
        assert!(explore.contains("[server]"));
    }

    #[test]
    fn render_launch_plan_text_is_stable() {
        let plan = LaunchPlan {
            mode: "dry_run".to_string(),
            agent: "codex".to_string(),
            project_id: "my-project".to_string(),
            session_id: "session-auto".to_string(),
            cwd: "/tmp/work".to_string(),
            proxy_bind: "127.0.0.1:8585".to_string(),
            admin_socket: "127.0.0.1:8586".to_string(),
            database_path: "~/.config/pennyprompt/pennyprompt.db".to_string(),
            config_path: "~/.config/pennyprompt/config.toml".to_string(),
        };

        let rendered = render_launch_plan(&plan);
        assert_eq!(
            rendered,
            "Run launcher plan\n  mode: dry_run\n  agent: codex\n  project_id: my-project\n  session_id: session-auto\n  cwd: /tmp/work\n  proxy_bind: 127.0.0.1:8585\n  admin_socket: 127.0.0.1:8586\n  database_path: ~/.config/pennyprompt/pennyprompt.db\n  config_path: ~/.config/pennyprompt/config.toml"
        );
    }

    #[test]
    fn staging_dir_guard_removes_directory_on_drop() {
        let root = tempdir().expect("temp root");
        let staging_path = root.path().join("pricebook-staging");
        {
            let guard = StagingDirGuard::new(staging_path.clone()).expect("create staging guard");
            fs::write(guard.file("probe.toml"), "ok").expect("write probe file");
            assert!(staging_path.exists());
        }
        assert!(
            !staging_path.exists(),
            "staging dir should be removed on drop"
        );
    }
}
