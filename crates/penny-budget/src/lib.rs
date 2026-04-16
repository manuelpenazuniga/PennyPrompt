//! Budget evaluation and routing decisions.

use std::collections::{HashMap, HashSet};

use chrono::{Datelike, Days, Duration, TimeZone, Utc, Weekday};
use penny_ledger::CostLedger;
use penny_store::{BudgetRepo, EventRepo, NewEvent, SqliteStore};
use penny_types::{
    Budget, BudgetBlockDetail, EventType, Mode, Money, NormalizedRequest, Reservation,
    RouteDecision, ScopeType, Severity, WindowType,
};
use serde_json::json;
use sqlx::{QueryBuilder, Row, Sqlite};

#[derive(Clone)]
pub struct BudgetEvaluator {
    store: SqliteStore,
    ledger: CostLedger,
    mode: Mode,
}

impl BudgetEvaluator {
    pub fn new(store: SqliteStore, mode: Mode) -> Self {
        let ledger = CostLedger::new(store.clone());
        Self {
            store,
            ledger,
            mode,
        }
    }

    pub async fn evaluate(
        &self,
        request: &NormalizedRequest,
        estimated_cost: Money,
    ) -> RouteDecision {
        let budgets = match self.lookup_applicable_budgets(request).await {
            Ok(budgets) => budgets,
            Err(error) => {
                return self
                    .failsafe_decision(
                        request,
                        "budget_lookup_failed",
                        format!("budget lookup failed: {error}"),
                    )
                    .await;
            }
        };

        if budgets.is_empty() {
            self.record_event(
                request,
                EventType::BudgetCheck,
                Severity::Info,
                json!({
                    "mode": self.mode_tag(),
                    "result": "allow",
                    "reason": "no_applicable_budgets",
                    "estimated_cost_usd": estimated_cost.to_usd()
                }),
            )
            .await;
            return RouteDecision::Allow {
                warnings: Vec::new(),
            };
        }

        let reserve_result = match self.mode {
            Mode::Guard => {
                self.ledger
                    .reserve(&request.id, &budgets, estimated_cost)
                    .await
            }
            Mode::Observe => {
                self.ledger
                    .reserve_allow_over_limit(&request.id, &budgets, estimated_cost)
                    .await
            }
        };

        let reservation = match reserve_result {
            Ok(reservation) => reservation,
            Err(error) => {
                return self
                    .failsafe_decision(
                        request,
                        "ledger_reserve_failed",
                        format!("ledger reserve failed: {error}"),
                    )
                    .await;
            }
        };

        match reservation {
            Reservation::Granted {
                entries,
                remaining_by_budget,
            } => {
                let mut warnings = self.compute_over_limit_warnings(&budgets, &remaining_by_budget);
                let soft_warnings = match self
                    .compute_soft_limit_warnings(&budgets, &remaining_by_budget)
                    .await
                {
                    Ok(warnings) => warnings,
                    Err(error) => {
                        return self
                            .failsafe_decision(
                                request,
                                "soft_limit_check_failed",
                                format!("soft limit check failed: {error}"),
                            )
                            .await;
                    }
                };
                warnings.extend(soft_warnings);

                if warnings.is_empty() {
                    self.record_event(
                        request,
                        EventType::BudgetCheck,
                        Severity::Info,
                        json!({
                            "mode": self.mode_tag(),
                            "result": "allow",
                            "entries": entries,
                            "estimated_cost_usd": estimated_cost.to_usd()
                        }),
                    )
                    .await;
                } else {
                    self.record_event(
                        request,
                        EventType::BudgetWarn,
                        Severity::Warn,
                        json!({
                            "mode": self.mode_tag(),
                            "result": if warnings.iter().any(|warning| warning.contains("hard limit exceeded")) {
                                "allow_budget_violation"
                            } else {
                                "allow_with_warnings"
                            },
                            "warnings": warnings,
                            "estimated_cost_usd": estimated_cost.to_usd()
                        }),
                    )
                    .await;
                }

                RouteDecision::Allow { warnings }
            }
            Reservation::Denied {
                budget,
                accumulated,
                limit,
                reason,
            } => {
                let detail = BudgetBlockDetail {
                    scope: scope_string(&budget),
                    window: budget.window_type.clone(),
                    accumulated_usd: accumulated,
                    limit_usd: limit,
                    resets_at: window_reset_at(&budget.window_type),
                };
                match self.mode {
                    Mode::Guard => {
                        self.record_event(
                            request,
                            EventType::BudgetBlock,
                            Severity::Warn,
                            json!({
                                "mode": self.mode_tag(),
                                "result": "block",
                                "reason": reason,
                                "detail": detail
                            }),
                        )
                        .await;
                        RouteDecision::Block { reason, detail }
                    }
                    Mode::Observe => {
                        let warning = format!("observe mode budget violation: {reason}");
                        self.record_event(
                            request,
                            EventType::BudgetWarn,
                            Severity::Warn,
                            json!({
                                "mode": self.mode_tag(),
                                "result": "allow_budget_violation",
                                "reason": reason,
                                "detail": detail
                            }),
                        )
                        .await;
                        RouteDecision::Allow {
                            warnings: vec![warning],
                        }
                    }
                }
            }
        }
    }

    async fn lookup_applicable_budgets(
        &self,
        request: &NormalizedRequest,
    ) -> Result<Vec<Budget>, String> {
        let mut budgets = BudgetRepo::list_applicable_for_request(
            &self.store,
            &request.project_id,
            &request.session_id,
        )
        .await
        .map_err(|error| error.to_string())?;
        budgets.sort_by_key(|budget| budget.id);
        Ok(budgets)
    }

    fn compute_over_limit_warnings(
        &self,
        budgets: &[Budget],
        remaining_by_budget: &[penny_types::BudgetRemaining],
    ) -> Vec<String> {
        let mut remaining_map = HashMap::with_capacity(remaining_by_budget.len());
        for item in remaining_by_budget {
            remaining_map.insert(item.budget_id, item.remaining_usd);
        }

        let mut warnings = Vec::new();
        for budget in budgets {
            let Some(limit) = budget.hard_limit_usd else {
                continue;
            };

            let Some(Some(remaining)) = remaining_map.get(&budget.id).copied() else {
                continue;
            };

            if remaining.is_negative() {
                let Some(accumulated) = limit.checked_sub(remaining) else {
                    continue;
                };
                warnings.push(format!(
                    "hard limit exceeded for {} ({:?}): {} / {}",
                    scope_string(budget),
                    budget.window_type,
                    accumulated,
                    limit
                ));
            }
        }

        warnings
    }

    async fn compute_soft_limit_warnings(
        &self,
        budgets: &[Budget],
        remaining_by_budget: &[penny_types::BudgetRemaining],
    ) -> Result<Vec<String>, String> {
        let mut remaining_map = HashMap::with_capacity(remaining_by_budget.len());
        for item in remaining_by_budget {
            remaining_map.insert(item.budget_id, item.remaining_usd);
        }

        let mut lookup_budget_ids = HashSet::new();
        for budget in budgets {
            if budget.hard_limit_usd.is_none() {
                lookup_budget_ids.insert(budget.id);
                continue;
            }

            if !matches!(remaining_map.get(&budget.id), Some(Some(_))) {
                lookup_budget_ids.insert(budget.id);
            }
        }

        let running_totals = self
            .latest_running_totals(&lookup_budget_ids.into_iter().collect::<Vec<_>>())
            .await
            .map_err(|error| error.to_string())?;

        let mut warnings = Vec::new();
        for budget in budgets {
            let Some(soft_limit) = budget.soft_limit_usd else {
                continue;
            };

            let accumulated = if let Some(limit) = budget.hard_limit_usd {
                match remaining_map.get(&budget.id) {
                    Some(Some(remaining)) => limit.checked_sub(*remaining).unwrap_or(Money::ZERO),
                    _ => *running_totals.get(&budget.id).unwrap_or(&Money::ZERO),
                }
            } else {
                *running_totals.get(&budget.id).unwrap_or(&Money::ZERO)
            };

            if accumulated >= soft_limit {
                warnings.push(format!(
                    "soft limit reached for {} ({:?}): {} / {}",
                    scope_string(budget),
                    budget.window_type,
                    accumulated,
                    soft_limit
                ));
            }
        }

        Ok(warnings)
    }

    async fn latest_running_totals(
        &self,
        budget_ids: &[i64],
    ) -> Result<HashMap<i64, Money>, sqlx::Error> {
        if budget_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut qb = QueryBuilder::<Sqlite>::new(
            r#"
            SELECT budget_id, running_total_micros
            FROM cost_ledger
            WHERE budget_id IN (
            "#,
        );
        {
            let mut separated = qb.separated(", ");
            for budget_id in budget_ids {
                separated.push_bind(*budget_id);
            }
        }
        qb.push(
            r#")
            ORDER BY budget_id ASC, id DESC
            "#,
        );

        let rows = qb.build().fetch_all(self.store.pool()).await?;
        let mut totals = HashMap::with_capacity(budget_ids.len());
        for row in rows {
            let budget_id: i64 = row.get("budget_id");
            let running_total: i64 = row.get("running_total_micros");
            totals.entry(budget_id).or_insert(running_total);
        }
        Ok(totals
            .into_iter()
            .map(|(budget_id, micros)| (budget_id, Money::from_micros(micros)))
            .collect())
    }

    async fn failsafe_decision(
        &self,
        request: &NormalizedRequest,
        code: &'static str,
        message: String,
    ) -> RouteDecision {
        self.record_event(
            request,
            EventType::ModeFailsafe,
            Severity::Error,
            json!({
                "mode": self.mode_tag(),
                "code": code,
                "message": message
            }),
        )
        .await;

        match self.mode {
            Mode::Guard => RouteDecision::Failsafe {
                mode: Mode::Guard,
                reason: format!("guard mode fail-closed: {message}"),
            },
            Mode::Observe => RouteDecision::Allow {
                warnings: vec![format!("observe mode failsafe: {message}")],
            },
        }
    }

    async fn record_event(
        &self,
        request: &NormalizedRequest,
        event_type: EventType,
        severity: Severity,
        detail: serde_json::Value,
    ) {
        let _ = EventRepo::insert(
            &self.store,
            &NewEvent {
                request_id: Some(request.id.clone()),
                session_id: Some(request.session_id.clone()),
                event_type,
                severity,
                detail,
            },
        )
        .await;
    }

    fn mode_tag(&self) -> &'static str {
        match self.mode {
            Mode::Observe => "observe",
            Mode::Guard => "guard",
        }
    }
}

fn scope_string(budget: &Budget) -> String {
    let scope = match budget.scope_type {
        ScopeType::Global => "global",
        ScopeType::Project => "project",
        ScopeType::Session => "session",
    };
    format!("{scope}:{}", budget.scope_id)
}

fn window_reset_at(window: &WindowType) -> Option<chrono::DateTime<Utc>> {
    let now = Utc::now();
    match window {
        WindowType::Total => None,
        WindowType::Day => {
            let next = now
                .date_naive()
                .checked_add_days(Days::new(1))
                .expect("next day should exist");
            Some(
                Utc.from_utc_datetime(
                    &next.and_hms_opt(0, 0, 0).expect("midnight should be valid"),
                ),
            )
        }
        WindowType::Week => {
            let current_weekday = now.weekday();
            let days_until_monday = match current_weekday {
                Weekday::Mon => 7,
                _ => {
                    (Weekday::Mon.num_days_from_monday() + 7
                        - current_weekday.num_days_from_monday()) as i64
                }
            };
            let next_monday = now.date_naive() + Duration::days(days_until_monday);
            Some(
                Utc.from_utc_datetime(
                    &next_monday
                        .and_hms_opt(0, 0, 0)
                        .expect("midnight should be valid"),
                ),
            )
        }
        WindowType::Month => {
            let (year, month) = (now.year(), now.month());
            let (next_year, next_month) = if month == 12 {
                (year + 1, 1)
            } else {
                (year, month + 1)
            };
            Some(
                Utc.with_ymd_and_hms(next_year, next_month, 1, 0, 0, 0)
                    .single()
                    .expect("first day of next month should be valid"),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use penny_store::EventQuery;
    use serde_json::json;
    use sqlx::query_scalar;

    async fn setup_store() -> SqliteStore {
        SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store")
    }

    async fn seed_budget(
        store: &SqliteStore,
        window_type: WindowType,
        hard_limit: Option<Money>,
        soft_limit: Option<Money>,
    ) -> Budget {
        BudgetRepo::upsert(
            store,
            &Budget {
                id: 0,
                scope_type: ScopeType::Global,
                scope_id: "*".to_string(),
                window_type,
                hard_limit_usd: hard_limit,
                soft_limit_usd: soft_limit,
                action_on_hard: "block".to_string(),
                action_on_soft: "warn".to_string(),
                preset_source: None,
            },
        )
        .await
        .expect("seed budget")
    }

    fn request(id: &str) -> NormalizedRequest {
        NormalizedRequest {
            id: id.to_string(),
            project_id: "project-alpha".to_string(),
            session_id: "session-1".to_string(),
            model_requested: "claude-sonnet-4-6".to_string(),
            model_resolved: "claude-sonnet-4-6".to_string(),
            provider_id: "mock".to_string(),
            messages: json!([{ "role": "user", "content": "hi" }]),
            stream: false,
            estimated_input_tokens: 100,
            estimated_output_tokens: 50,
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn soft_limit_reached_returns_allow_with_warnings() {
        let store = setup_store().await;
        seed_budget(
            &store,
            WindowType::Day,
            Some(Money::from_usd(10.0).expect("money")),
            Some(Money::from_usd(5.0).expect("money")),
        )
        .await;

        let evaluator = BudgetEvaluator::new(store.clone(), Mode::Guard);
        let decision = evaluator
            .evaluate(&request("req_warn"), Money::from_usd(6.0).expect("money"))
            .await;

        match decision {
            RouteDecision::Allow { warnings } => {
                assert_eq!(warnings.len(), 1);
                assert!(warnings[0].contains("soft limit reached"));
            }
            other => panic!("unexpected decision: {other:?}"),
        }

        let events = EventRepo::list(
            &store,
            EventQuery {
                request_id: Some("req_warn".to_string()),
                ..EventQuery::default()
            },
        )
        .await
        .expect("list events");
        assert!(events
            .iter()
            .any(|event| event.event_type == EventType::BudgetWarn));
        let warn_count = events
            .iter()
            .filter(|event| event.event_type == EventType::BudgetWarn)
            .count();
        assert_eq!(warn_count, 1);
    }

    #[tokio::test]
    async fn hard_limit_exceeded_blocks_with_window_metadata() {
        let store = setup_store().await;
        seed_budget(
            &store,
            WindowType::Day,
            Some(Money::from_usd(1.0).expect("money")),
            None,
        )
        .await;

        let evaluator = BudgetEvaluator::new(store.clone(), Mode::Guard);
        let decision = evaluator
            .evaluate(&request("req_block"), Money::from_usd(2.0).expect("money"))
            .await;

        match decision {
            RouteDecision::Block { detail, .. } => {
                assert_eq!(detail.window, WindowType::Day);
                assert_eq!(detail.limit_usd, Money::from_usd(1.0).expect("money"));
                assert!(detail.resets_at.is_some());
            }
            other => panic!("unexpected decision: {other:?}"),
        }

        let event_types: Vec<String> = query_scalar(
            "SELECT event_type FROM events WHERE request_id = 'req_block' ORDER BY id",
        )
        .fetch_all(store.pool())
        .await
        .expect("load block events");
        assert!(event_types.iter().any(|ty| ty == "budget_block"));
    }

    #[tokio::test]
    async fn guard_mode_is_fail_closed_on_budget_lookup_failure() {
        let store = setup_store().await;
        sqlx::query("DROP TABLE budgets")
            .execute(store.pool())
            .await
            .expect("drop budgets table");

        let evaluator = BudgetEvaluator::new(store, Mode::Guard);
        let decision = evaluator
            .evaluate(
                &request("req_guard_failsafe"),
                Money::from_usd(1.0).expect("money"),
            )
            .await;

        match decision {
            RouteDecision::Failsafe { mode, reason } => {
                assert_eq!(mode, Mode::Guard);
                assert!(reason.contains("fail-closed"));
            }
            other => panic!("unexpected decision: {other:?}"),
        }
    }

    #[tokio::test]
    async fn observe_mode_logs_failsafe_but_allows_traffic() {
        let store = setup_store().await;
        sqlx::query("DROP TABLE budgets")
            .execute(store.pool())
            .await
            .expect("drop budgets table");

        let evaluator = BudgetEvaluator::new(store.clone(), Mode::Observe);
        let decision = evaluator
            .evaluate(
                &request("req_observe_failsafe"),
                Money::from_usd(1.0).expect("money"),
            )
            .await;

        match decision {
            RouteDecision::Allow { warnings } => {
                assert_eq!(warnings.len(), 1);
                assert!(warnings[0].contains("observe mode failsafe"));
            }
            other => panic!("unexpected decision: {other:?}"),
        }

        let types: Vec<String> = query_scalar(
            "SELECT event_type FROM events WHERE request_id = 'req_observe_failsafe' ORDER BY id",
        )
        .fetch_all(store.pool())
        .await
        .expect("load failsafe events");
        assert!(types.iter().any(|ty| ty == "mode_failsafe"));
    }

    #[tokio::test]
    async fn observe_mode_does_not_block_on_budget_denial() {
        let store = setup_store().await;
        seed_budget(
            &store,
            WindowType::Day,
            Some(Money::from_usd(1.0).expect("money")),
            None,
        )
        .await;

        let evaluator = BudgetEvaluator::new(store.clone(), Mode::Observe);
        let decision = evaluator
            .evaluate(
                &request("req_observe_violation"),
                Money::from_usd(2.0).expect("money"),
            )
            .await;

        match decision {
            RouteDecision::Allow { warnings } => {
                assert_eq!(warnings.len(), 1);
                assert!(warnings[0].contains("hard limit exceeded"));
            }
            other => panic!("unexpected decision: {other:?}"),
        }

        let event_types: Vec<String> = query_scalar(
            "SELECT event_type FROM events WHERE request_id = 'req_observe_violation' ORDER BY id",
        )
        .fetch_all(store.pool())
        .await
        .expect("load observe violation events");
        assert!(event_types.iter().any(|ty| ty == "budget_warn"));
        assert!(!event_types.iter().any(|ty| ty == "budget_block"));

        let reserve_count: i64 = query_scalar(
            "SELECT COUNT(*) FROM cost_ledger WHERE request_id = 'req_observe_violation' AND entry_type = 'reserve'",
        )
        .fetch_one(store.pool())
        .await
        .expect("count observe reserve rows");
        assert_eq!(reserve_count, 1);

        let running_total: i64 = query_scalar(
            "SELECT running_total_micros FROM cost_ledger WHERE request_id = 'req_observe_violation' AND entry_type = 'reserve' LIMIT 1",
        )
        .fetch_one(store.pool())
        .await
        .expect("load observe running total");
        assert_eq!(running_total, Money::from_usd(2.0).expect("money").micros());
    }

    #[tokio::test]
    async fn evaluator_applies_all_budget_windows_without_duplicates() {
        let store = setup_store().await;
        seed_budget(
            &store,
            WindowType::Day,
            Some(Money::from_usd(100.0).expect("money")),
            None,
        )
        .await;
        seed_budget(
            &store,
            WindowType::Week,
            Some(Money::from_usd(100.0).expect("money")),
            None,
        )
        .await;
        seed_budget(
            &store,
            WindowType::Month,
            Some(Money::from_usd(100.0).expect("money")),
            None,
        )
        .await;
        seed_budget(
            &store,
            WindowType::Total,
            Some(Money::from_usd(100.0).expect("money")),
            None,
        )
        .await;

        let evaluator = BudgetEvaluator::new(store.clone(), Mode::Guard);
        let decision = evaluator
            .evaluate(
                &request("req_windows"),
                Money::from_usd(1.0).expect("money"),
            )
            .await;
        assert!(matches!(decision, RouteDecision::Allow { .. }));

        let reserve_count: i64 = query_scalar(
            "SELECT COUNT(*) FROM cost_ledger WHERE request_id = 'req_windows' AND entry_type = 'reserve'",
        )
        .fetch_one(store.pool())
        .await
        .expect("count reserve rows");
        assert_eq!(reserve_count, 4);
    }
}
