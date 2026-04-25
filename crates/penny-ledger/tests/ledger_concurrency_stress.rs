use std::sync::Arc;

use penny_ledger::{CostLedger, LedgerError};
use penny_store::{BudgetRepo, SqliteStore};
use penny_types::{Budget, Money, Reservation, ScopeType, WindowType};
use sqlx::{query, query_scalar};
use tempfile::tempdir;
use tokio::sync::Barrier;

fn sqlite_url(path: &std::path::Path) -> String {
    format!("sqlite://{}", path.display())
}

async fn connect_store(url: &str) -> SqliteStore {
    let store = SqliteStore::connect(url)
        .await
        .expect("connect shared sqlite");
    query("PRAGMA busy_timeout = 5000")
        .execute(store.pool())
        .await
        .expect("set busy timeout");
    store
}

async fn insert_budget(store: &SqliteStore, hard_limit: f64) -> Budget {
    BudgetRepo::upsert(
        store,
        &Budget {
            id: 0,
            scope_type: ScopeType::Global,
            scope_id: "*".to_string(),
            window_type: WindowType::Day,
            hard_limit_usd: Some(Money::from_usd(hard_limit).expect("money")),
            soft_limit_usd: None,
            action_on_hard: "block".to_string(),
            action_on_soft: "warn".to_string(),
            preset_source: Some("integration".to_string()),
        },
    )
    .await
    .expect("insert budget")
}

fn is_sqlite_lock(err: &LedgerError) -> bool {
    match err {
        LedgerError::Sqlx(inner) => inner
            .as_database_error()
            .and_then(|db_err| db_err.code())
            .is_some_and(|code| is_sqlite_lock_code(code.as_ref())),
        _ => false,
    }
}

fn sqlite_primary_code(code: &str) -> Option<i32> {
    code.parse::<i32>().ok().map(|raw| raw & 0xFF)
}

fn is_sqlite_lock_code(code: &str) -> bool {
    matches!(sqlite_primary_code(code), Some(5 | 6))
}

async fn reserve_with_retry(
    ledger: &CostLedger,
    request_id: &str,
    budget: Budget,
    amount: Money,
) -> Result<Reservation, LedgerError> {
    let mut attempts = 0_u32;
    loop {
        match ledger
            .reserve(request_id, std::slice::from_ref(&budget), amount)
            .await
        {
            Ok(result) => return Ok(result),
            Err(err) if attempts < 64 && is_sqlite_lock(&err) => {
                attempts += 1;
                tokio::task::yield_now().await;
            }
            Err(err) => return Err(err),
        }
    }
}

async fn release_with_retry(
    ledger: &CostLedger,
    request_id: &str,
) -> Result<Vec<i64>, LedgerError> {
    let mut attempts = 0_u32;
    loop {
        match ledger.release(request_id).await {
            Ok(result) => return Ok(result),
            Err(err) if attempts < 64 && is_sqlite_lock(&err) => {
                attempts += 1;
                tokio::task::yield_now().await;
            }
            Err(err) => return Err(err),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_reserve_stress_never_overspends_hard_limit() {
    let dir = tempdir().expect("temp dir");
    let db_path = dir.path().join("ledger-concurrency.db");
    let db_url = sqlite_url(&db_path);
    let setup_store = connect_store(&db_url).await;

    let hard_limit = Money::from_usd(5.0).expect("money");
    let reserve_amount = Money::from_usd(1.0).expect("money");
    let budget = insert_budget(&setup_store, 5.0).await;

    let task_count = 14_usize;
    let start = Arc::new(Barrier::new(task_count + 1));
    let mut handles = Vec::with_capacity(task_count);

    for idx in 0..task_count {
        let barrier = start.clone();
        let budget = budget.clone();
        let db_url = db_url.clone();
        handles.push(tokio::spawn(async move {
            let store = connect_store(&db_url).await;
            let ledger = CostLedger::new(store);
            barrier.wait().await;
            reserve_with_retry(
                &ledger,
                &format!("req_concurrent_{idx:02}"),
                budget,
                reserve_amount,
            )
            .await
        }));
    }
    start.wait().await;

    let mut granted = 0_i64;
    let mut denied = 0_i64;
    for handle in handles {
        let reservation = handle.await.expect("join reserve task").expect("reserve");
        match reservation {
            Reservation::Granted { .. } => granted += 1,
            Reservation::Denied { .. } => denied += 1,
        }
    }

    assert_eq!(granted, 5, "granted count must match hard limit capacity");
    assert_eq!(denied, 9, "remaining requests must be denied");

    let reserve_rows: i64 =
        query_scalar("SELECT COUNT(*) FROM cost_ledger WHERE entry_type = 'reserve'")
            .fetch_one(setup_store.pool())
            .await
            .expect("count reserve rows");
    assert_eq!(reserve_rows, granted);

    let overspend_rows: i64 =
        query_scalar("SELECT COUNT(*) FROM cost_ledger WHERE running_total_micros > ?1")
            .bind(hard_limit.micros())
            .fetch_one(setup_store.pool())
            .await
            .expect("count overspend rows");
    assert_eq!(overspend_rows, 0, "no running total may exceed hard limit");

    let final_running_total: i64 =
        query_scalar("SELECT running_total_micros FROM cost_ledger ORDER BY id DESC LIMIT 1")
            .fetch_one(setup_store.pool())
            .await
            .expect("final running total");
    assert_eq!(final_running_total, hard_limit.micros());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn mixed_concurrent_reserve_and_release_preserves_running_total_invariants() {
    let dir = tempdir().expect("temp dir");
    let db_path = dir.path().join("ledger-concurrency-mixed.db");
    let db_url = sqlite_url(&db_path);
    let setup_store = connect_store(&db_url).await;

    let hard_limit = Money::from_usd(2.0).expect("money");
    let reserve_amount = Money::from_usd(1.0).expect("money");
    let budget = insert_budget(&setup_store, 2.0).await;

    let seed_ledger = CostLedger::new(setup_store.clone());
    for request_id in ["seed_a", "seed_b"] {
        let reservation = seed_ledger
            .reserve(request_id, std::slice::from_ref(&budget), reserve_amount)
            .await
            .expect("seed reserve");
        assert!(matches!(reservation, Reservation::Granted { .. }));
    }

    let start = Arc::new(Barrier::new(7));
    let mut release_handles = Vec::new();
    let mut reserve_handles = Vec::new();

    for request_id in ["seed_a", "seed_b"] {
        let barrier = start.clone();
        let db_url = db_url.clone();
        release_handles.push(tokio::spawn(async move {
            let store = connect_store(&db_url).await;
            let ledger = CostLedger::new(store);
            barrier.wait().await;
            release_with_retry(&ledger, request_id).await
        }));
    }

    for idx in 0..4 {
        let barrier = start.clone();
        let budget = budget.clone();
        let db_url = db_url.clone();
        reserve_handles.push(tokio::spawn(async move {
            let store = connect_store(&db_url).await;
            let ledger = CostLedger::new(store);
            barrier.wait().await;
            reserve_with_retry(
                &ledger,
                &format!("mixed_req_{idx:02}"),
                budget,
                reserve_amount,
            )
            .await
        }));
    }
    start.wait().await;

    let mut successful_releases = 0_i64;
    for handle in release_handles {
        let released_ids = handle.await.expect("join release task").expect("release");
        assert_eq!(released_ids.len(), 1, "each seed request must release once");
        successful_releases += 1;
    }

    let mut granted_reserves = 0_i64;
    for handle in reserve_handles {
        let reservation = handle.await.expect("join reserve task").expect("reserve");
        if matches!(reservation, Reservation::Granted { .. }) {
            granted_reserves += 1;
        }
    }

    let release_rows: i64 =
        query_scalar("SELECT COUNT(*) FROM cost_ledger WHERE entry_type = 'release'")
            .fetch_one(setup_store.pool())
            .await
            .expect("count release rows");
    assert_eq!(release_rows, 2, "release rows must persist exactly twice");
    assert_eq!(
        successful_releases, 2,
        "both seeded requests must be released"
    );

    assert!(
        granted_reserves <= 2,
        "new reserve grants cannot exceed released capacity"
    );

    let overspend_rows: i64 =
        query_scalar("SELECT COUNT(*) FROM cost_ledger WHERE running_total_micros > ?1")
            .bind(hard_limit.micros())
            .fetch_one(setup_store.pool())
            .await
            .expect("count overspend rows");
    assert_eq!(overspend_rows, 0);

    let min_total: i64 = query_scalar("SELECT MIN(running_total_micros) FROM cost_ledger")
        .fetch_one(setup_store.pool())
        .await
        .expect("min running total");
    assert!(min_total >= 0, "running total must not become negative");

    let max_total: i64 = query_scalar("SELECT MAX(running_total_micros) FROM cost_ledger")
        .fetch_one(setup_store.pool())
        .await
        .expect("max running total");
    assert!(
        max_total <= hard_limit.micros(),
        "running total must stay within hard limit"
    );
}

#[test]
fn sqlite_lock_code_classification_handles_primary_and_extended_values() {
    assert!(
        is_sqlite_lock_code("5"),
        "SQLITE_BUSY should be treated as lock"
    );
    assert!(
        is_sqlite_lock_code("6"),
        "SQLITE_LOCKED should be treated as lock"
    );
    assert!(
        is_sqlite_lock_code("261"),
        "SQLITE_BUSY_RECOVERY should map to SQLITE_BUSY primary code"
    );
    assert!(
        is_sqlite_lock_code("262"),
        "SQLITE_LOCKED_SHAREDCACHE should map to SQLITE_LOCKED primary code"
    );
    assert!(
        !is_sqlite_lock_code("2067"),
        "UNIQUE constraint code must not be treated as lock"
    );
    assert!(
        !is_sqlite_lock_code("not-a-number"),
        "non numeric codes are not SQLite lock codes"
    );
}
