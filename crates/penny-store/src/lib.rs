//! Persistence layer for PennyPrompt.

use std::{path::Path, str::FromStr};

use chrono::{DateTime, NaiveDateTime, Utc};
use penny_types::{
    AccountedUsage, Budget, Event, EventType, Money, ProjectId, RequestId, ScopeType, SessionId,
    Severity, UsageSource, WindowType,
};
use serde_json::Value;
use sqlx::{
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    QueryBuilder, Row, Sqlite, SqlitePool,
};
use thiserror::Error;
use uuid::Uuid;

static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("invalid enum value for {field}: {value}")]
    InvalidEnum { field: &'static str, value: String },
    #[error("invalid timestamp value for {field}: {value}")]
    InvalidTimestamp { field: &'static str, value: String },
}

#[derive(Debug, Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    pub async fn connect(database_url: &str) -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;

        sqlx::query("PRAGMA journal_mode=WAL;")
            .execute(&pool)
            .await?;
        MIGRATOR.run(&pool).await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectRecord {
    pub id: ProjectId,
    pub name: String,
    pub path: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRecord {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub started_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestStatus {
    Pending,
    Completed,
    Failed,
    Cancelled,
}

impl RequestStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn is_terminal(&self) -> bool {
        !matches!(self, Self::Pending)
    }
}

#[derive(Debug, Clone)]
pub struct NewRequest {
    pub id: RequestId,
    pub session_id: Option<SessionId>,
    pub project_id: ProjectId,
    pub model_requested: String,
    pub model_used: String,
    pub provider_id: String,
    pub started_at: DateTime<Utc>,
    pub is_streaming: bool,
}

#[derive(Debug, Clone)]
pub struct UsageRecord {
    pub request_id: RequestId,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Money,
    pub source: UsageSource,
    pub pricing_snapshot: Value,
}

impl From<(RequestId, AccountedUsage)> for UsageRecord {
    fn from(value: (RequestId, AccountedUsage)) -> Self {
        let (request_id, usage) = value;
        Self {
            request_id,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cost_usd: usage.cost_usd,
            source: usage.source,
            pricing_snapshot: usage.pricing_snapshot,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewEvent {
    pub request_id: Option<RequestId>,
    pub session_id: Option<SessionId>,
    pub event_type: EventType,
    pub severity: Severity,
    pub detail: Value,
}

#[derive(Debug, Clone)]
pub struct EventQuery {
    pub request_id: Option<RequestId>,
    pub session_id: Option<SessionId>,
    pub event_type: Option<EventType>,
    pub limit: u32,
}

impl Default for EventQuery {
    fn default() -> Self {
        Self {
            request_id: None,
            session_id: None,
            event_type: None,
            limit: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PricebookEntryRecord {
    pub id: i64,
    pub model_id: String,
    pub input_per_mtok: Money,
    pub output_per_mtok: Money,
    pub effective_from: DateTime<Utc>,
    pub effective_until: Option<DateTime<Utc>>,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct NewPricebookEntry {
    pub model_id: String,
    pub input_per_mtok: Money,
    pub output_per_mtok: Money,
    pub effective_from: DateTime<Utc>,
    pub effective_until: Option<DateTime<Utc>>,
    pub source: String,
}

#[allow(async_fn_in_trait)]
pub trait ProjectRepo {
    async fn upsert_by_path(&self, path: &str) -> Result<ProjectId, StoreError>;
    async fn get_by_path(&self, path: &str) -> Result<Option<ProjectRecord>, StoreError>;
}

#[allow(async_fn_in_trait)]
pub trait SessionRepo {
    async fn create(&self, project_id: &str) -> Result<SessionId, StoreError>;
    async fn find_active(
        &self,
        project_id: &str,
        window_minutes: u64,
    ) -> Result<Option<SessionId>, StoreError>;
    async fn close(&self, session_id: &str) -> Result<(), StoreError>;
}

#[allow(async_fn_in_trait)]
pub trait RequestRepo {
    async fn insert(&self, request: &NewRequest) -> Result<(), StoreError>;
    async fn update_status(
        &self,
        request_id: &str,
        status: RequestStatus,
        upstream_ms: Option<i64>,
    ) -> Result<(), StoreError>;
    async fn insert_usage(&self, usage: &UsageRecord) -> Result<(), StoreError>;
}

#[allow(async_fn_in_trait)]
pub trait BudgetRepo {
    async fn list_applicable(
        &self,
        scope_type: ScopeType,
        scope_id: &str,
        window_type: WindowType,
    ) -> Result<Vec<Budget>, StoreError>;
    async fn list_applicable_for_request(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Vec<Budget>, StoreError>;
    async fn upsert(&self, budget: &Budget) -> Result<Budget, StoreError>;
    async fn list_all(&self) -> Result<Vec<Budget>, StoreError>;
}

#[allow(async_fn_in_trait)]
pub trait EventRepo {
    async fn insert(&self, event: &NewEvent) -> Result<i64, StoreError>;
    async fn list(&self, query: EventQuery) -> Result<Vec<Event>, StoreError>;
}

#[allow(async_fn_in_trait)]
pub trait PricebookRepo {
    async fn get_price(
        &self,
        model_id: &str,
        at: DateTime<Utc>,
    ) -> Result<Option<PricebookEntryRecord>, StoreError>;
    async fn import(&self, entries: &[NewPricebookEntry]) -> Result<(), StoreError>;
}

impl ProjectRepo for SqliteStore {
    async fn upsert_by_path(&self, path: &str) -> Result<ProjectId, StoreError> {
        let normalized_path = normalize_project_path(path);
        let project_id = slug_from_path(&normalized_path);
        let project_name = project_name_from_path(&normalized_path);

        sqlx::query(
            r#"
            INSERT INTO projects (id, name, path)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(path) DO UPDATE SET name = excluded.name
            "#,
        )
        .bind(&project_id)
        .bind(&project_name)
        .bind(&normalized_path)
        .execute(&self.pool)
        .await?;

        let existing_id: String = sqlx::query_scalar("SELECT id FROM projects WHERE path = ?1")
            .bind(&normalized_path)
            .fetch_one(&self.pool)
            .await?;

        Ok(existing_id)
    }

    async fn get_by_path(&self, path: &str) -> Result<Option<ProjectRecord>, StoreError> {
        let normalized_path = normalize_project_path(path);
        let row = sqlx::query(
            r#"
            SELECT id, name, path, created_at
            FROM projects
            WHERE path = ?1
            "#,
        )
        .bind(normalized_path)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| {
            Ok(ProjectRecord {
                id: row.get("id"),
                name: row.get("name"),
                path: row.get("path"),
                created_at: parse_db_datetime(
                    row.get::<String, _>("created_at"),
                    "projects.created_at",
                )?,
            })
        })
        .transpose()
    }
}

impl SessionRepo for SqliteStore {
    async fn create(&self, project_id: &str) -> Result<SessionId, StoreError> {
        let session_id = Uuid::now_v7().to_string();
        sqlx::query(
            r#"
            INSERT INTO sessions (id, project_id, source)
            VALUES (?1, ?2, 'auto')
            "#,
        )
        .bind(&session_id)
        .bind(project_id)
        .execute(&self.pool)
        .await?;
        Ok(session_id)
    }

    async fn find_active(
        &self,
        project_id: &str,
        window_minutes: u64,
    ) -> Result<Option<SessionId>, StoreError> {
        let modifier = format!("-{} minutes", window_minutes);
        let row = sqlx::query(
            r#"
            SELECT id
            FROM sessions
            WHERE project_id = ?1
              AND closed_at IS NULL
              AND started_at >= datetime('now', ?2)
            ORDER BY started_at DESC
            LIMIT 1
            "#,
        )
        .bind(project_id)
        .bind(modifier)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.get("id")))
    }

    async fn close(&self, session_id: &str) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            UPDATE sessions
            SET closed_at = datetime('now')
            WHERE id = ?1
            "#,
        )
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

impl RequestRepo for SqliteStore {
    async fn insert(&self, request: &NewRequest) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            INSERT INTO requests (
                id, session_id, project_id, model_requested, model_used, provider_id,
                started_at, status, is_streaming
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8)
            "#,
        )
        .bind(&request.id)
        .bind(&request.session_id)
        .bind(&request.project_id)
        .bind(&request.model_requested)
        .bind(&request.model_used)
        .bind(&request.provider_id)
        .bind(request.started_at.to_rfc3339())
        .bind(request.is_streaming)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_status(
        &self,
        request_id: &str,
        status: RequestStatus,
        upstream_ms: Option<i64>,
    ) -> Result<(), StoreError> {
        if status.is_terminal() {
            sqlx::query(
                r#"
                UPDATE requests
                SET status = ?1,
                    completed_at = datetime('now'),
                    upstream_ms = COALESCE(?2, upstream_ms)
                WHERE id = ?3
                "#,
            )
            .bind(status.as_str())
            .bind(upstream_ms)
            .bind(request_id)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query(
                r#"
                UPDATE requests
                SET status = ?1
                WHERE id = ?2
                "#,
            )
            .bind(status.as_str())
            .bind(request_id)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    async fn insert_usage(&self, usage: &UsageRecord) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            INSERT INTO request_usage (
                request_id, input_tokens, output_tokens, cost_usd, cost_micros, pricing_snapshot, source
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )
        .bind(&usage.request_id)
        .bind(i64_from_u64(usage.input_tokens))
        .bind(i64_from_u64(usage.output_tokens))
        .bind(usage.cost_usd.to_usd())
        .bind(usage.cost_usd.micros())
        .bind(usage.pricing_snapshot.to_string())
        .bind(usage_source_to_db(&usage.source))
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

impl BudgetRepo for SqliteStore {
    async fn list_applicable(
        &self,
        scope_type: ScopeType,
        scope_id: &str,
        window_type: WindowType,
    ) -> Result<Vec<Budget>, StoreError> {
        let scope_type_db = scope_type_to_db(&scope_type);
        let window_type_db = window_type_to_db(&window_type);
        let rows = sqlx::query(
            r#"
            SELECT id, scope_type, scope_id, window_type, hard_limit_micros, soft_limit_micros, action_on_hard, action_on_soft, preset_source
            FROM budgets
            WHERE window_type = ?1
              AND (
                (scope_type = 'global' AND scope_id = '*')
                OR (scope_type = ?2 AND scope_id = ?3)
              )
            ORDER BY id
            "#,
        )
        .bind(window_type_db)
        .bind(scope_type_db)
        .bind(scope_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(budget_from_row).collect()
    }

    async fn list_applicable_for_request(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Vec<Budget>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT id, scope_type, scope_id, window_type, hard_limit_micros, soft_limit_micros, action_on_hard, action_on_soft, preset_source
            FROM budgets
            WHERE window_type IN ('day', 'week', 'month', 'total')
              AND (
                (scope_type = 'global' AND scope_id = '*')
                OR (scope_type = 'project' AND scope_id = ?1)
                OR (scope_type = 'session' AND scope_id = ?2)
              )
            ORDER BY id
            "#,
        )
        .bind(project_id)
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(budget_from_row).collect()
    }

    async fn upsert(&self, budget: &Budget) -> Result<Budget, StoreError> {
        let scope_type_db = scope_type_to_db(&budget.scope_type);
        let window_type_db = window_type_to_db(&budget.window_type);

        if budget.id > 0 {
            let updated = sqlx::query(
                r#"
                UPDATE budgets
                SET scope_type = ?1,
                    scope_id = ?2,
                    window_type = ?3,
                    hard_limit_usd = ?4,
                    soft_limit_usd = ?5,
                    hard_limit_micros = ?6,
                    soft_limit_micros = ?7,
                    action_on_hard = ?8,
                    action_on_soft = ?9,
                    preset_source = ?10
                WHERE id = ?11
                "#,
            )
            .bind(scope_type_db)
            .bind(&budget.scope_id)
            .bind(window_type_db)
            .bind(budget.hard_limit_usd.map(Money::to_usd))
            .bind(budget.soft_limit_usd.map(Money::to_usd))
            .bind(budget.hard_limit_usd.map(Money::micros))
            .bind(budget.soft_limit_usd.map(Money::micros))
            .bind(&budget.action_on_hard)
            .bind(&budget.action_on_soft)
            .bind(&budget.preset_source)
            .bind(budget.id)
            .execute(&self.pool)
            .await?;

            if updated.rows_affected() > 0 {
                return Ok(budget.clone());
            }
        }

        let existing_id: Option<i64> = sqlx::query_scalar(
            r#"
            SELECT id
            FROM budgets
            WHERE scope_type = ?1 AND scope_id = ?2 AND window_type = ?3
            LIMIT 1
            "#,
        )
        .bind(scope_type_db)
        .bind(&budget.scope_id)
        .bind(window_type_db)
        .fetch_optional(&self.pool)
        .await?;

        let final_id = if let Some(id) = existing_id {
            sqlx::query(
                r#"
                UPDATE budgets
                SET hard_limit_usd = ?1,
                    soft_limit_usd = ?2,
                    hard_limit_micros = ?3,
                    soft_limit_micros = ?4,
                    action_on_hard = ?5,
                    action_on_soft = ?6,
                    preset_source = ?7
                WHERE id = ?8
                "#,
            )
            .bind(budget.hard_limit_usd.map(Money::to_usd))
            .bind(budget.soft_limit_usd.map(Money::to_usd))
            .bind(budget.hard_limit_usd.map(Money::micros))
            .bind(budget.soft_limit_usd.map(Money::micros))
            .bind(&budget.action_on_hard)
            .bind(&budget.action_on_soft)
            .bind(&budget.preset_source)
            .bind(id)
            .execute(&self.pool)
            .await?;
            id
        } else {
            sqlx::query(
                r#"
                INSERT INTO budgets (
                    scope_type, scope_id, window_type, hard_limit_usd, soft_limit_usd, hard_limit_micros, soft_limit_micros, action_on_hard, action_on_soft, preset_source
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                "#,
            )
            .bind(scope_type_db)
            .bind(&budget.scope_id)
            .bind(window_type_db)
            .bind(budget.hard_limit_usd.map(Money::to_usd))
            .bind(budget.soft_limit_usd.map(Money::to_usd))
            .bind(budget.hard_limit_usd.map(Money::micros))
            .bind(budget.soft_limit_usd.map(Money::micros))
            .bind(&budget.action_on_hard)
            .bind(&budget.action_on_soft)
            .bind(&budget.preset_source)
            .execute(&self.pool)
            .await?;
            sqlx::query_scalar("SELECT last_insert_rowid()")
                .fetch_one(&self.pool)
                .await?
        };

        let mut stored = budget.clone();
        stored.id = final_id;
        Ok(stored)
    }

    async fn list_all(&self) -> Result<Vec<Budget>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT id, scope_type, scope_id, window_type, hard_limit_micros, soft_limit_micros, action_on_hard, action_on_soft, preset_source
            FROM budgets
            ORDER BY id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(budget_from_row).collect()
    }
}

impl EventRepo for SqliteStore {
    async fn insert(&self, event: &NewEvent) -> Result<i64, StoreError> {
        sqlx::query(
            r#"
            INSERT INTO events (request_id, session_id, event_type, severity, detail)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
        )
        .bind(&event.request_id)
        .bind(&event.session_id)
        .bind(event_type_to_db(&event.event_type))
        .bind(severity_to_db(&event.severity))
        .bind(event.detail.to_string())
        .execute(&self.pool)
        .await?;

        let id = sqlx::query_scalar("SELECT last_insert_rowid()")
            .fetch_one(&self.pool)
            .await?;
        Ok(id)
    }

    async fn list(&self, query: EventQuery) -> Result<Vec<Event>, StoreError> {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT id, request_id, session_id, event_type, severity, detail, created_at FROM events",
        );
        let mut has_where = false;

        if let Some(request_id) = &query.request_id {
            qb.push(if has_where { " AND " } else { " WHERE " });
            qb.push("request_id = ");
            qb.push_bind(request_id);
            has_where = true;
        }

        if let Some(session_id) = &query.session_id {
            qb.push(if has_where { " AND " } else { " WHERE " });
            qb.push("session_id = ");
            qb.push_bind(session_id);
            has_where = true;
        }

        if let Some(event_type) = &query.event_type {
            qb.push(if has_where { " AND " } else { " WHERE " });
            qb.push("event_type = ");
            qb.push_bind(event_type_to_db(event_type));
        }

        qb.push(" ORDER BY id DESC LIMIT ");
        qb.push_bind(i64::from(query.limit.max(1)));

        let rows = qb.build().fetch_all(&self.pool).await?;
        rows.into_iter().map(event_from_row).collect()
    }
}

impl PricebookRepo for SqliteStore {
    async fn get_price(
        &self,
        model_id: &str,
        at: DateTime<Utc>,
    ) -> Result<Option<PricebookEntryRecord>, StoreError> {
        let at_iso = at.to_rfc3339();
        let row = sqlx::query(
            r#"
            SELECT id, model_id, input_per_mtok_micros, output_per_mtok_micros, effective_from, effective_until, source
            FROM pricebook_entries
            WHERE model_id = ?1
              AND datetime(effective_from) <= datetime(?2)
              AND (effective_until IS NULL OR datetime(effective_until) > datetime(?2))
            ORDER BY datetime(effective_from) DESC, id DESC
            LIMIT 1
            "#,
        )
        .bind(model_id)
        .bind(at_iso)
        .fetch_optional(&self.pool)
        .await?;

        row.map(pricebook_from_row).transpose()
    }

    async fn import(&self, entries: &[NewPricebookEntry]) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        for entry in entries {
            sqlx::query(
                r#"
                INSERT INTO pricebook_entries (
                    model_id, input_per_mtok, output_per_mtok, input_per_mtok_micros, output_per_mtok_micros, effective_from, effective_until, source
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                "#,
            )
            .bind(&entry.model_id)
            .bind(entry.input_per_mtok.to_usd())
            .bind(entry.output_per_mtok.to_usd())
            .bind(entry.input_per_mtok.micros())
            .bind(entry.output_per_mtok.micros())
            .bind(entry.effective_from.to_rfc3339())
            .bind(entry.effective_until.map(|ts| ts.to_rfc3339()))
            .bind(&entry.source)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

fn normalize_project_path(path: &str) -> String {
    let candidate = path.trim();
    if candidate.is_empty() {
        "default".to_string()
    } else {
        candidate.to_string()
    }
}

fn project_name_from_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("default")
        .to_string()
}

fn slug_from_path(path: &str) -> String {
    let source = project_name_from_path(path).to_lowercase();
    let mut slug = String::with_capacity(source.len());
    let mut prev_dash = false;
    for ch in source.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "default".to_string()
    } else {
        slug
    }
}

fn i64_from_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn parse_db_datetime(value: String, field: &'static str) -> Result<DateTime<Utc>, StoreError> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(&value) {
        return Ok(dt.with_timezone(&Utc));
    }

    if let Ok(dt) = NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S") {
        return Ok(DateTime::from_naive_utc_and_offset(dt, Utc));
    }

    Err(StoreError::InvalidTimestamp { field, value })
}

fn scope_type_to_db(scope_type: &ScopeType) -> &'static str {
    match scope_type {
        ScopeType::Global => "global",
        ScopeType::Project => "project",
        ScopeType::Session => "session",
    }
}

fn scope_type_from_db(value: &str) -> Result<ScopeType, StoreError> {
    match value {
        "global" => Ok(ScopeType::Global),
        "project" => Ok(ScopeType::Project),
        "session" => Ok(ScopeType::Session),
        _ => Err(StoreError::InvalidEnum {
            field: "budgets.scope_type",
            value: value.to_string(),
        }),
    }
}

fn window_type_to_db(window_type: &WindowType) -> &'static str {
    match window_type {
        WindowType::Day => "day",
        WindowType::Week => "week",
        WindowType::Month => "month",
        WindowType::Total => "total",
    }
}

fn window_type_from_db(value: &str) -> Result<WindowType, StoreError> {
    match value {
        "day" => Ok(WindowType::Day),
        "week" => Ok(WindowType::Week),
        "month" => Ok(WindowType::Month),
        "total" => Ok(WindowType::Total),
        _ => Err(StoreError::InvalidEnum {
            field: "budgets.window_type",
            value: value.to_string(),
        }),
    }
}

fn usage_source_to_db(source: &UsageSource) -> &'static str {
    match source {
        UsageSource::Provider => "provider",
        UsageSource::Estimated => "estimated",
        UsageSource::Heuristic => "heuristic",
    }
}

fn event_type_to_db(event_type: &EventType) -> &'static str {
    match event_type {
        EventType::BudgetCheck => "budget_check",
        EventType::BudgetBlock => "budget_block",
        EventType::BudgetWarn => "budget_warn",
        EventType::Reserve => "reserve",
        EventType::Reconcile => "reconcile",
        EventType::Release => "release",
        EventType::LoopDetected => "loop_detected",
        EventType::BurnRateAlert => "burn_rate_alert",
        EventType::SessionPaused => "session_paused",
        EventType::ProviderFailure => "provider_failure",
        EventType::ModeFailsafe => "mode_failsafe",
    }
}

fn event_type_from_db(value: &str) -> Result<EventType, StoreError> {
    match value {
        "budget_check" => Ok(EventType::BudgetCheck),
        "budget_block" => Ok(EventType::BudgetBlock),
        "budget_warn" => Ok(EventType::BudgetWarn),
        "reserve" => Ok(EventType::Reserve),
        "reconcile" => Ok(EventType::Reconcile),
        "release" => Ok(EventType::Release),
        "loop_detected" => Ok(EventType::LoopDetected),
        "burn_rate_alert" => Ok(EventType::BurnRateAlert),
        "session_paused" => Ok(EventType::SessionPaused),
        "provider_failure" => Ok(EventType::ProviderFailure),
        "mode_failsafe" => Ok(EventType::ModeFailsafe),
        _ => Err(StoreError::InvalidEnum {
            field: "events.event_type",
            value: value.to_string(),
        }),
    }
}

fn severity_to_db(severity: &Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Warn => "warn",
        Severity::Error => "error",
        Severity::Critical => "critical",
    }
}

fn severity_from_db(value: &str) -> Result<Severity, StoreError> {
    match value {
        "info" => Ok(Severity::Info),
        "warn" => Ok(Severity::Warn),
        "error" => Ok(Severity::Error),
        "critical" => Ok(Severity::Critical),
        _ => Err(StoreError::InvalidEnum {
            field: "events.severity",
            value: value.to_string(),
        }),
    }
}

fn budget_from_row(row: sqlx::sqlite::SqliteRow) -> Result<Budget, StoreError> {
    let scope_value: String = row.get("scope_type");
    let window_value: String = row.get("window_type");
    Ok(Budget {
        id: row.get("id"),
        scope_type: scope_type_from_db(&scope_value)?,
        scope_id: row.get("scope_id"),
        window_type: window_type_from_db(&window_value)?,
        hard_limit_usd: row
            .get::<Option<i64>, _>("hard_limit_micros")
            .map(Money::from_micros),
        soft_limit_usd: row
            .get::<Option<i64>, _>("soft_limit_micros")
            .map(Money::from_micros),
        action_on_hard: row.get("action_on_hard"),
        action_on_soft: row.get("action_on_soft"),
        preset_source: row.get("preset_source"),
    })
}

fn event_from_row(row: sqlx::sqlite::SqliteRow) -> Result<Event, StoreError> {
    let event_type: String = row.get("event_type");
    let severity: String = row.get("severity");
    let detail: String = row.get("detail");
    Ok(Event {
        id: row.get("id"),
        request_id: row.get("request_id"),
        session_id: row.get("session_id"),
        event_type: event_type_from_db(&event_type)?,
        severity: severity_from_db(&severity)?,
        detail: serde_json::from_str(&detail).map_err(|_| StoreError::InvalidEnum {
            field: "events.detail",
            value: detail,
        })?,
        created_at: parse_db_datetime(row.get("created_at"), "events.created_at")?,
    })
}

fn pricebook_from_row(row: sqlx::sqlite::SqliteRow) -> Result<PricebookEntryRecord, StoreError> {
    Ok(PricebookEntryRecord {
        id: row.get("id"),
        model_id: row.get("model_id"),
        input_per_mtok: Money::from_micros(row.get("input_per_mtok_micros")),
        output_per_mtok: Money::from_micros(row.get("output_per_mtok_micros")),
        effective_from: parse_db_datetime(
            row.get("effective_from"),
            "pricebook_entries.effective_from",
        )?,
        effective_until: row
            .get::<Option<String>, _>("effective_until")
            .map(|value| parse_db_datetime(value, "pricebook_entries.effective_until"))
            .transpose()?,
        source: row.get("source"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;
    use sqlx::Executor;

    async fn setup_store() -> SqliteStore {
        SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store")
    }

    async fn seed_project_and_session(store: &SqliteStore) -> (ProjectId, SessionId) {
        let project_id = store
            .upsert_by_path("/tmp/pennyprompt")
            .await
            .expect("upsert project");
        let session_id = store.create(&project_id).await.expect("create session");
        (project_id, session_id)
    }

    #[tokio::test]
    async fn project_repo_crud_and_migrations_run() {
        let store = setup_store().await;
        let tables: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'projects'",
        )
        .fetch_one(store.pool())
        .await
        .expect("projects table exists");
        assert_eq!(tables, 1);

        let first = store
            .upsert_by_path("/Users/me/code/PennyPrompt")
            .await
            .expect("insert project");
        let second = store
            .upsert_by_path("/Users/me/code/PennyPrompt")
            .await
            .expect("upsert project");
        assert_eq!(first, second);

        let loaded = store
            .get_by_path("/Users/me/code/PennyPrompt")
            .await
            .expect("load project")
            .expect("project exists");
        assert_eq!(loaded.id, first);
        assert_eq!(loaded.name, "PennyPrompt");
    }

    #[tokio::test]
    async fn session_repo_crud() {
        let store = setup_store().await;
        let project_id = store
            .upsert_by_path("/Users/me/code/PennyPrompt")
            .await
            .expect("project");
        let session_id = store.create(&project_id).await.expect("session");

        let active = store
            .find_active(&project_id, 30)
            .await
            .expect("find active");
        assert_eq!(active, Some(session_id.clone()));

        store.close(&session_id).await.expect("close");

        let active_after = store
            .find_active(&project_id, 30)
            .await
            .expect("find active");
        assert_eq!(active_after, None);
    }

    #[tokio::test]
    async fn request_repo_crud() {
        let store = setup_store().await;
        let (project_id, session_id) = seed_project_and_session(&store).await;

        let request_id = "req_01".to_string();
        let request = NewRequest {
            id: request_id.clone(),
            session_id: Some(session_id),
            project_id,
            model_requested: "claude-sonnet-4-6".into(),
            model_used: "claude-sonnet-4-6".into(),
            provider_id: "anthropic".into(),
            started_at: Utc::now(),
            is_streaming: false,
        };
        RequestRepo::insert(&store, &request)
            .await
            .expect("insert request");
        store
            .update_status(&request_id, RequestStatus::Completed, Some(1200))
            .await
            .expect("update status");
        store
            .insert_usage(&UsageRecord {
                request_id,
                input_tokens: 1234,
                output_tokens: 321,
                cost_usd: Money::from_usd(0.42).expect("money"),
                source: UsageSource::Provider,
                pricing_snapshot: json!({ "model": "claude-sonnet-4-6" }),
            })
            .await
            .expect("insert usage");

        let request_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM requests")
            .fetch_one(store.pool())
            .await
            .expect("count requests");
        let usage_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM request_usage")
            .fetch_one(store.pool())
            .await
            .expect("count usage");
        assert_eq!(request_count, 1);
        assert_eq!(usage_count, 1);
    }

    #[tokio::test]
    async fn budget_repo_crud() {
        let store = setup_store().await;
        let budget = Budget {
            id: 0,
            scope_type: ScopeType::Global,
            scope_id: "*".into(),
            window_type: WindowType::Day,
            hard_limit_usd: Some(Money::from_usd(10.0).expect("money")),
            soft_limit_usd: Some(Money::from_usd(8.0).expect("money")),
            action_on_hard: "block".into(),
            action_on_soft: "warn".into(),
            preset_source: None,
        };

        let stored = store.upsert(&budget).await.expect("insert budget");
        assert!(stored.id > 0);

        let mut changed = stored.clone();
        changed.hard_limit_usd = Some(Money::from_usd(12.5).expect("money"));
        store.upsert(&changed).await.expect("update budget");

        let applicable = store
            .list_applicable(ScopeType::Global, "*", WindowType::Day)
            .await
            .expect("list applicable");
        assert_eq!(applicable.len(), 1);
        assert_eq!(
            applicable[0].hard_limit_usd,
            Some(Money::from_usd(12.5).expect("money"))
        );

        let all = store.list_all().await.expect("list all");
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn budget_repo_list_applicable_for_request_returns_all_scope_matches() {
        let store = setup_store().await;

        let budgets = vec![
            Budget {
                id: 0,
                scope_type: ScopeType::Global,
                scope_id: "*".into(),
                window_type: WindowType::Day,
                hard_limit_usd: Some(Money::from_usd(100.0).expect("money")),
                soft_limit_usd: None,
                action_on_hard: "block".into(),
                action_on_soft: "warn".into(),
                preset_source: Some("preset:indie".into()),
            },
            Budget {
                id: 0,
                scope_type: ScopeType::Project,
                scope_id: "project-alpha".into(),
                window_type: WindowType::Week,
                hard_limit_usd: Some(Money::from_usd(200.0).expect("money")),
                soft_limit_usd: None,
                action_on_hard: "block".into(),
                action_on_soft: "warn".into(),
                preset_source: Some("preset:indie".into()),
            },
            Budget {
                id: 0,
                scope_type: ScopeType::Session,
                scope_id: "session-123".into(),
                window_type: WindowType::Month,
                hard_limit_usd: Some(Money::from_usd(300.0).expect("money")),
                soft_limit_usd: None,
                action_on_hard: "block".into(),
                action_on_soft: "warn".into(),
                preset_source: Some("preset:indie".into()),
            },
            Budget {
                id: 0,
                scope_type: ScopeType::Project,
                scope_id: "other-project".into(),
                window_type: WindowType::Total,
                hard_limit_usd: Some(Money::from_usd(400.0).expect("money")),
                soft_limit_usd: None,
                action_on_hard: "block".into(),
                action_on_soft: "warn".into(),
                preset_source: Some("preset:team".into()),
            },
        ];

        for budget in &budgets {
            store.upsert(budget).await.expect("upsert budget");
        }

        let found = store
            .list_applicable_for_request("project-alpha", "session-123")
            .await
            .expect("list applicable for request");

        let found_ids: Vec<i64> = found.into_iter().map(|budget| budget.id).collect();
        assert_eq!(found_ids.len(), 3);
    }

    #[tokio::test]
    async fn event_repo_crud() {
        let store = setup_store().await;
        let (_, session_id) = seed_project_and_session(&store).await;
        let event_id = EventRepo::insert(
            &store,
            &NewEvent {
                request_id: Some("req_01".into()),
                session_id: Some(session_id.clone()),
                event_type: EventType::BudgetWarn,
                severity: Severity::Warn,
                detail: json!({ "limit_usd": 10.0, "accumulated_usd": 9.5 }),
            },
        )
        .await
        .expect("insert event");
        assert!(event_id > 0);

        let events = store
            .list(EventQuery {
                request_id: None,
                session_id: Some(session_id),
                event_type: Some(EventType::BudgetWarn),
                limit: 10,
            })
            .await
            .expect("list events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::BudgetWarn);
    }

    #[tokio::test]
    async fn pricebook_repo_crud() {
        let store = setup_store().await;
        store
            .pool()
            .execute(
                sqlx::query(
                    "INSERT INTO providers (id, name, base_url, api_format, enabled) VALUES ('anthropic', 'Anthropic', 'https://api.anthropic.com', 'openai', 1)",
                ),
            )
            .await
            .expect("seed provider");
        store
            .pool()
            .execute(
                sqlx::query(
                    "INSERT INTO models (id, provider_id, external_name, display_name, class) VALUES ('claude-sonnet-4-6', 'anthropic', 'claude-sonnet-4-6', 'Claude Sonnet 4.6', 'balanced')",
                ),
            )
            .await
            .expect("seed model");

        store
            .import(&[
                NewPricebookEntry {
                    model_id: "claude-sonnet-4-6".into(),
                    input_per_mtok: Money::from_usd(3.0).expect("money"),
                    output_per_mtok: Money::from_usd(15.0).expect("money"),
                    effective_from: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap(),
                    effective_until: Some(
                        Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).single().unwrap(),
                    ),
                    source: "local".into(),
                },
                NewPricebookEntry {
                    model_id: "claude-sonnet-4-6".into(),
                    input_per_mtok: Money::from_usd(3.5).expect("money"),
                    output_per_mtok: Money::from_usd(16.0).expect("money"),
                    effective_from: Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).single().unwrap(),
                    effective_until: None,
                    source: "local".into(),
                },
            ])
            .await
            .expect("import pricebook");

        let price = store
            .get_price(
                "claude-sonnet-4-6",
                Utc.with_ymd_and_hms(2026, 4, 10, 12, 0, 0)
                    .single()
                    .unwrap(),
            )
            .await
            .expect("get price")
            .expect("price exists");

        assert_eq!(price.input_per_mtok, Money::from_usd(3.5).expect("money"));
        assert_eq!(price.output_per_mtok, Money::from_usd(16.0).expect("money"));
    }

    #[tokio::test]
    async fn wal_pragma_is_applied() {
        let store = setup_store().await;
        let mode: String = store
            .pool()
            .fetch_one(sqlx::query("PRAGMA journal_mode;"))
            .await
            .expect("read pragma")
            .get(0);
        assert!(
            mode.eq_ignore_ascii_case("wal") || mode.eq_ignore_ascii_case("memory"),
            "journal mode should be WAL for file DBs; :memory: stays MEMORY"
        );
    }
}
