use penny_ledger::CostLedger;
use penny_store::{BudgetRepo, SqliteStore};
use penny_types::{Budget, Money, Reservation, ScopeType, WindowType};
use sqlx::query_scalar;

async fn setup_store() -> SqliteStore {
    SqliteStore::connect("sqlite::memory:")
        .await
        .expect("create in-memory store")
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

#[tokio::test]
async fn reserve_and_reconcile_round_trip_is_persisted() {
    let store = setup_store().await;
    let budget = insert_budget(&store, 10.0).await;
    let ledger = CostLedger::new(store.clone());

    let reservation = ledger
        .reserve(
            "req-alpha-integration",
            std::slice::from_ref(&budget),
            Money::from_usd(2.0).expect("money"),
        )
        .await
        .expect("reserve");
    assert!(matches!(reservation, Reservation::Granted { .. }));

    let reconcile_ids = ledger
        .reconcile(
            "req-alpha-integration",
            Money::from_usd(3.5).expect("money"),
        )
        .await
        .expect("reconcile");
    assert_eq!(reconcile_ids.len(), 1);

    let reserve_rows: i64 = query_scalar(
        "SELECT COUNT(*) FROM cost_ledger WHERE request_id = 'req-alpha-integration' AND entry_type = 'reserve'",
    )
    .fetch_one(store.pool())
    .await
    .expect("count reserve");
    assert_eq!(reserve_rows, 1);

    let reconcile_rows: i64 = query_scalar(
        "SELECT COUNT(*) FROM cost_ledger WHERE request_id = 'req-alpha-integration' AND entry_type = 'reconcile'",
    )
    .fetch_one(store.pool())
    .await
    .expect("count reconcile");
    assert_eq!(reconcile_rows, 1);

    let running_total_micros: i64 = query_scalar(
        "SELECT running_total_micros FROM cost_ledger WHERE request_id = 'req-alpha-integration' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(store.pool())
    .await
    .expect("running total");
    assert_eq!(
        running_total_micros,
        Money::from_usd(3.5).expect("money").micros()
    );
}
