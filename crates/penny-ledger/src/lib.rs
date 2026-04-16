//! Atomic cost ledger operations for PennyPrompt.

use penny_store::{SqliteStore, StoreError};
use penny_types::{Budget, BudgetRemaining, Reservation};
use sqlx::{query, query_scalar, Row};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("invalid request: {0}")]
    InvalidRequest(&'static str),
}

#[derive(Debug, Clone)]
pub struct CostLedger {
    store: SqliteStore,
}

impl CostLedger {
    pub fn new(store: SqliteStore) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    pub async fn reserve(
        &self,
        request_id: &str,
        budgets: &[Budget],
        estimated_cost: f64,
    ) -> Result<Reservation, LedgerError> {
        self.reserve_internal(request_id, budgets, estimated_cost, true)
            .await
    }

    pub async fn reserve_allow_over_limit(
        &self,
        request_id: &str,
        budgets: &[Budget],
        estimated_cost: f64,
    ) -> Result<Reservation, LedgerError> {
        self.reserve_internal(request_id, budgets, estimated_cost, false)
            .await
    }

    async fn reserve_internal(
        &self,
        request_id: &str,
        budgets: &[Budget],
        estimated_cost: f64,
        enforce_hard_limits: bool,
    ) -> Result<Reservation, LedgerError> {
        if request_id.trim().is_empty() {
            return Err(LedgerError::InvalidRequest("request_id must not be empty"));
        }
        if !estimated_cost.is_finite() || estimated_cost < 0.0 {
            return Err(LedgerError::InvalidRequest(
                "estimated_cost must be a finite non-negative number",
            ));
        }

        if budgets.is_empty() {
            return Ok(Reservation::Granted {
                entries: Vec::new(),
                remaining_by_budget: Vec::new(),
            });
        }

        let mut conn = self.store.pool().acquire().await?;
        begin_immediate(&mut conn).await?;

        let mut entries = Vec::with_capacity(budgets.len());
        let mut remaining_by_budget = Vec::with_capacity(budgets.len());

        for budget in budgets {
            let current_total = latest_running_total(&mut conn, budget.id).await?;
            let new_total = current_total + estimated_cost;
            if enforce_hard_limits {
                if let Some(limit) = budget.hard_limit_usd {
                    if new_total > limit {
                        rollback_quietly(&mut conn).await;
                        return Ok(Reservation::Denied {
                            budget: budget.clone(),
                            accumulated: new_total,
                            limit,
                            reason: format!(
                                "budget id {} exceeded hard limit: {:.6} > {:.6}",
                                budget.id, new_total, limit
                            ),
                        });
                    }
                }
            }

            let entry_id = insert_ledger_entry(
                &mut conn,
                request_id,
                "reserve",
                budget.id,
                estimated_cost,
                new_total,
            )
            .await?;
            entries.push(entry_id);
            remaining_by_budget.push(BudgetRemaining {
                budget_id: budget.id,
                remaining_usd: budget
                    .hard_limit_usd
                    .map(|limit| limit - new_total)
                    .unwrap_or(f64::INFINITY),
            });
        }

        commit(&mut conn).await?;
        Ok(Reservation::Granted {
            entries,
            remaining_by_budget,
        })
    }

    pub async fn reconcile(
        &self,
        request_id: &str,
        actual_cost: f64,
    ) -> Result<Vec<i64>, LedgerError> {
        if request_id.trim().is_empty() {
            return Err(LedgerError::InvalidRequest("request_id must not be empty"));
        }
        if !actual_cost.is_finite() || actual_cost < 0.0 {
            return Err(LedgerError::InvalidRequest(
                "actual_cost must be a finite non-negative number",
            ));
        }

        let mut conn = self.store.pool().acquire().await?;
        begin_immediate(&mut conn).await?;

        let reserve_rows = query(
            r#"
            SELECT budget_id, amount_usd
            FROM cost_ledger
            WHERE request_id = ?1 AND entry_type = 'reserve'
            ORDER BY id
            "#,
        )
        .bind(request_id)
        .fetch_all(&mut *conn)
        .await?;

        let mut entry_ids = Vec::with_capacity(reserve_rows.len());
        for row in reserve_rows {
            let budget_id: i64 = row.get("budget_id");
            let reserved_amount: f64 = row.get("amount_usd");
            let diff = actual_cost - reserved_amount;
            let current_total = latest_running_total(&mut conn, budget_id).await?;
            let new_total = current_total + diff;
            let entry_id = insert_ledger_entry(
                &mut conn,
                request_id,
                "reconcile",
                budget_id,
                diff,
                new_total,
            )
            .await?;
            entry_ids.push(entry_id);
        }

        commit(&mut conn).await?;
        Ok(entry_ids)
    }

    pub async fn release(&self, request_id: &str) -> Result<Vec<i64>, LedgerError> {
        if request_id.trim().is_empty() {
            return Err(LedgerError::InvalidRequest("request_id must not be empty"));
        }

        let mut conn = self.store.pool().acquire().await?;
        begin_immediate(&mut conn).await?;

        let reserve_rows = query(
            r#"
            SELECT budget_id, amount_usd
            FROM cost_ledger
            WHERE request_id = ?1 AND entry_type = 'reserve'
            ORDER BY id
            "#,
        )
        .bind(request_id)
        .fetch_all(&mut *conn)
        .await?;

        let mut entry_ids = Vec::with_capacity(reserve_rows.len());
        for row in reserve_rows {
            let budget_id: i64 = row.get("budget_id");
            let reserved_amount: f64 = row.get("amount_usd");
            let release_amount = -reserved_amount;
            let current_total = latest_running_total(&mut conn, budget_id).await?;
            let new_total = current_total + release_amount;
            let entry_id = insert_ledger_entry(
                &mut conn,
                request_id,
                "release",
                budget_id,
                release_amount,
                new_total,
            )
            .await?;
            entry_ids.push(entry_id);
        }

        commit(&mut conn).await?;
        Ok(entry_ids)
    }
}

async fn latest_running_total(
    conn: &mut sqlx::pool::PoolConnection<sqlx::Sqlite>,
    budget_id: i64,
) -> Result<f64, sqlx::Error> {
    query_scalar(
        r#"
        SELECT running_total
        FROM cost_ledger
        WHERE budget_id = ?1
        ORDER BY id DESC
        LIMIT 1
        "#,
    )
    .bind(budget_id)
    .fetch_optional(&mut **conn)
    .await
    .map(|value: Option<f64>| value.unwrap_or(0.0))
}

async fn insert_ledger_entry(
    conn: &mut sqlx::pool::PoolConnection<sqlx::Sqlite>,
    request_id: &str,
    entry_type: &'static str,
    budget_id: i64,
    amount_usd: f64,
    running_total: f64,
) -> Result<i64, sqlx::Error> {
    query(
        r#"
        INSERT INTO cost_ledger (
            request_id, entry_type, budget_id, amount_usd, running_total
        )
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
    )
    .bind(request_id)
    .bind(entry_type)
    .bind(budget_id)
    .bind(amount_usd)
    .bind(running_total)
    .execute(&mut **conn)
    .await?;

    query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut **conn)
        .await
}

async fn begin_immediate(
    conn: &mut sqlx::pool::PoolConnection<sqlx::Sqlite>,
) -> Result<(), sqlx::Error> {
    query("BEGIN IMMEDIATE").execute(&mut **conn).await?;
    Ok(())
}

async fn commit(conn: &mut sqlx::pool::PoolConnection<sqlx::Sqlite>) -> Result<(), sqlx::Error> {
    query("COMMIT").execute(&mut **conn).await?;
    Ok(())
}

async fn rollback_quietly(conn: &mut sqlx::pool::PoolConnection<sqlx::Sqlite>) {
    let _ = query("ROLLBACK").execute(&mut **conn).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use penny_store::BudgetRepo;
    use penny_types::{ScopeType, WindowType};
    use sqlx::{query, query_scalar};

    async fn setup_store() -> SqliteStore {
        SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store")
    }

    async fn insert_budget(store: &SqliteStore, hard_limit: Option<f64>) -> Budget {
        BudgetRepo::upsert(
            store,
            &Budget {
                id: 0,
                scope_type: ScopeType::Global,
                scope_id: "*".to_string(),
                window_type: WindowType::Day,
                hard_limit_usd: hard_limit,
                soft_limit_usd: None,
                action_on_hard: "block".to_string(),
                action_on_soft: "warn".to_string(),
            },
        )
        .await
        .expect("insert budget")
    }

    #[tokio::test]
    async fn reserve_grants_and_persists_running_total() {
        let store = setup_store().await;
        let budget = insert_budget(&store, Some(10.0)).await;
        let ledger = CostLedger::new(store.clone());

        let reservation = ledger
            .reserve("req_01", std::slice::from_ref(&budget), 2.5)
            .await
            .expect("reserve should succeed");

        match reservation {
            Reservation::Granted {
                entries,
                remaining_by_budget,
            } => {
                assert_eq!(entries.len(), 1);
                assert_eq!(remaining_by_budget.len(), 1);
                assert_eq!(remaining_by_budget[0].budget_id, budget.id);
                assert!((remaining_by_budget[0].remaining_usd - 7.5).abs() < 1e-9);
            }
            Reservation::Denied { .. } => panic!("reservation unexpectedly denied"),
        }

        let running_total: f64 = query_scalar(
            r#"
            SELECT running_total
            FROM cost_ledger
            WHERE request_id = 'req_01' AND entry_type = 'reserve'
            LIMIT 1
            "#,
        )
        .fetch_one(store.pool())
        .await
        .expect("load running_total");
        assert!((running_total - 2.5).abs() < 1e-9);
    }

    #[tokio::test]
    async fn reserve_denied_rolls_back_all_budget_rows() {
        let store = setup_store().await;
        let budget_a = insert_budget(&store, Some(10.0)).await;
        let budget_b = insert_budget(&store, Some(1.0)).await;
        let ledger = CostLedger::new(store.clone());

        let reservation = ledger
            .reserve("req_denied", &[budget_a, budget_b], 2.0)
            .await
            .expect("reserve should return denied, not error");

        match reservation {
            Reservation::Denied { limit, .. } => assert_eq!(limit, 1.0),
            Reservation::Granted { .. } => panic!("reservation unexpectedly granted"),
        }

        let rows: i64 =
            query_scalar("SELECT COUNT(*) FROM cost_ledger WHERE request_id = 'req_denied'")
                .fetch_one(store.pool())
                .await
                .expect("count denied rows");
        assert_eq!(rows, 0);
    }

    #[tokio::test]
    async fn reserve_allow_over_limit_persists_entries_in_observe_style_flow() {
        let store = setup_store().await;
        let budget = insert_budget(&store, Some(1.0)).await;
        let ledger = CostLedger::new(store.clone());

        let reservation = ledger
            .reserve_allow_over_limit("req_observe", std::slice::from_ref(&budget), 2.0)
            .await
            .expect("reserve allow over limit should succeed");

        match reservation {
            Reservation::Granted {
                entries,
                remaining_by_budget,
            } => {
                assert_eq!(entries.len(), 1);
                assert_eq!(remaining_by_budget.len(), 1);
                assert!(remaining_by_budget[0].remaining_usd < 0.0);
            }
            Reservation::Denied { .. } => panic!("reservation should not deny in allow-over-limit"),
        }

        let row = query(
            r#"
            SELECT amount_usd, running_total
            FROM cost_ledger
            WHERE request_id = 'req_observe' AND entry_type = 'reserve'
            LIMIT 1
            "#,
        )
        .fetch_one(store.pool())
        .await
        .expect("load observe reserve row");

        let amount: f64 = row.get("amount_usd");
        let running_total: f64 = row.get("running_total");
        assert!((amount - 2.0).abs() < 1e-9);
        assert!((running_total - 2.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn reconcile_adds_diff_entries_against_reserves() {
        let store = setup_store().await;
        let budget = insert_budget(&store, Some(20.0)).await;
        let ledger = CostLedger::new(store.clone());

        let reserve_result = ledger
            .reserve("req_reconcile", std::slice::from_ref(&budget), 2.0)
            .await
            .expect("reserve");
        assert!(matches!(reserve_result, Reservation::Granted { .. }));

        let ids = ledger
            .reconcile("req_reconcile", 3.5)
            .await
            .expect("reconcile");
        assert_eq!(ids.len(), 1);

        let row = query(
            r#"
            SELECT amount_usd, running_total
            FROM cost_ledger
            WHERE request_id = 'req_reconcile' AND entry_type = 'reconcile'
            LIMIT 1
            "#,
        )
        .fetch_one(store.pool())
        .await
        .expect("load reconcile row");

        let amount: f64 = row.get("amount_usd");
        let running_total: f64 = row.get("running_total");
        assert!((amount - 1.5).abs() < 1e-9);
        assert!((running_total - 3.5).abs() < 1e-9);
    }

    #[tokio::test]
    async fn release_reverts_reserved_amount() {
        let store = setup_store().await;
        let budget = insert_budget(&store, Some(20.0)).await;
        let ledger = CostLedger::new(store.clone());

        let reserve_result = ledger
            .reserve("req_release", std::slice::from_ref(&budget), 2.0)
            .await
            .expect("reserve");
        assert!(matches!(reserve_result, Reservation::Granted { .. }));

        let ids = ledger.release("req_release").await.expect("release");
        assert_eq!(ids.len(), 1);

        let row = query(
            r#"
            SELECT amount_usd, running_total
            FROM cost_ledger
            WHERE request_id = 'req_release' AND entry_type = 'release'
            LIMIT 1
            "#,
        )
        .fetch_one(store.pool())
        .await
        .expect("load release row");

        let amount: f64 = row.get("amount_usd");
        let running_total: f64 = row.get("running_total");
        assert!((amount + 2.0).abs() < 1e-9);
        assert!(running_total.abs() < 1e-9);
    }
}
