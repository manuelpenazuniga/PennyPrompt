//! Shared domain types for PennyPrompt.

use chrono::{DateTime, Utc};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::{fmt, str::FromStr};
use thiserror::Error;

pub type RequestId = String;
pub type SessionId = String;
pub type ProjectId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Money(i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MoneyError {
    #[error("money value must be finite")]
    NonFinite,
    #[error("money value is out of range")]
    OutOfRange,
    #[error("money decimal string is invalid")]
    InvalidFormat,
}

impl Money {
    pub const SCALE: i64 = 1_000_000;
    pub const ZERO: Self = Self(0);

    pub fn from_micros(micros: i64) -> Self {
        Self(micros)
    }

    pub fn micros(self) -> i64 {
        self.0
    }

    pub fn is_negative(self) -> bool {
        self.0 < 0
    }

    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        self.0.checked_add(rhs.0).map(Self)
    }

    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.0.checked_sub(rhs.0).map(Self)
    }

    pub fn from_usd(value: f64) -> Result<Self, MoneyError> {
        if !value.is_finite() {
            return Err(MoneyError::NonFinite);
        }
        let scaled = (value * Self::SCALE as f64).round();
        if scaled < i64::MIN as f64 || scaled > i64::MAX as f64 {
            return Err(MoneyError::OutOfRange);
        }
        Ok(Self(scaled as i64))
    }

    pub fn to_usd(self) -> f64 {
        self.0 as f64 / Self::SCALE as f64
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let negative = self.0 < 0;
        let abs = self.0.unsigned_abs();
        let whole = abs / Money::SCALE as u64;
        let frac = abs % Money::SCALE as u64;
        if negative {
            write!(f, "-{whole}.{frac:06}")
        } else {
            write!(f, "{whole}.{frac:06}")
        }
    }
}

impl Serialize for Money {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(self.to_usd())
    }
}

impl<'de> Deserialize<'de> for Money {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MoneyVisitor;

        impl<'de> de::Visitor<'de> for MoneyVisitor {
            type Value = Money;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a decimal USD value as number or string")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Money::from_micros(
                    value
                        .checked_mul(Money::SCALE)
                        .ok_or_else(|| E::custom(MoneyError::OutOfRange.to_string()))?,
                ))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let signed = i64::try_from(value)
                    .map_err(|_| E::custom(MoneyError::OutOfRange.to_string()))?;
                self.visit_i64(signed)
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Money::from_usd(value).map_err(|err| E::custom(err.to_string()))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                parse_money_str(value).map_err(|err| E::custom(err.to_string()))
            }
        }

        deserializer.deserialize_any(MoneyVisitor)
    }
}

impl FromStr for Money {
    type Err = MoneyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_money_str(s)
    }
}

fn parse_money_str(value: &str) -> Result<Money, MoneyError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(MoneyError::InvalidFormat);
    }

    let (negative, digits) = match trimmed.as_bytes()[0] {
        b'-' => (true, &trimmed[1..]),
        b'+' => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };

    if digits.is_empty() {
        return Err(MoneyError::InvalidFormat);
    }

    let mut parts = digits.split('.');
    let whole_part = parts.next().ok_or(MoneyError::InvalidFormat)?;
    let frac_part = parts.next();
    if parts.next().is_some() {
        return Err(MoneyError::InvalidFormat);
    }

    let whole: i64 = whole_part.parse().map_err(|_| MoneyError::InvalidFormat)?;
    let frac = match frac_part {
        None => 0_i64,
        Some(raw) => {
            if raw.is_empty() || raw.len() > 6 || !raw.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(MoneyError::InvalidFormat);
            }
            let mut padded = raw.to_string();
            while padded.len() < 6 {
                padded.push('0');
            }
            padded
                .parse::<i64>()
                .map_err(|_| MoneyError::InvalidFormat)?
        }
    };

    let whole_scaled = whole
        .checked_mul(Money::SCALE)
        .ok_or(MoneyError::OutOfRange)?;
    let mut micros = whole_scaled
        .checked_add(frac)
        .ok_or(MoneyError::OutOfRange)?;

    if negative {
        micros = micros.checked_neg().ok_or(MoneyError::OutOfRange)?;
    }

    Ok(Money::from_micros(micros))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NormalizedRequest {
    pub id: RequestId,
    pub project_id: ProjectId,
    pub session_id: SessionId,
    pub model_requested: String,
    pub model_resolved: String,
    pub provider_id: String,
    pub messages: Value,
    pub stream: bool,
    pub estimated_input_tokens: u64,
    pub estimated_output_tokens: u64,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderResponse {
    pub status: u16,
    pub body: ResponseBody,
    pub upstream_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseBody {
    Complete(Value),
    Stream(StreamDescriptor),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamDescriptor {
    pub provider: String,
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountedUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Money,
    pub source: UsageSource,
    pub pricing_snapshot: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    Provider,
    Estimated,
    Heuristic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Budget {
    pub id: i64,
    pub scope_type: ScopeType,
    pub scope_id: String,
    pub window_type: WindowType,
    pub hard_limit_usd: Option<Money>,
    pub soft_limit_usd: Option<Money>,
    pub action_on_hard: String,
    pub action_on_soft: String,
    pub preset_source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScopeType {
    Global,
    Project,
    Session,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowType {
    Day,
    Week,
    Month,
    Total,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RouteDecision {
    Allow {
        warnings: Vec<String>,
    },
    Block {
        reason: String,
        detail: BudgetBlockDetail,
    },
    Failsafe {
        mode: Mode,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BudgetBlockDetail {
    pub scope: String,
    pub window: WindowType,
    pub accumulated_usd: Money,
    pub limit_usd: Money,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Observe,
    Guard,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LedgerEntry {
    pub id: i64,
    pub request_id: RequestId,
    pub entry_type: LedgerEntryType,
    pub budget_id: i64,
    pub amount_usd: Money,
    pub running_total: Money,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LedgerEntryType {
    Reserve,
    Reconcile,
    Release,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Reservation {
    Granted {
        entries: Vec<i64>,
        remaining_by_budget: Vec<BudgetRemaining>,
    },
    Denied {
        budget: Budget,
        accumulated: Money,
        limit: Money,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BudgetRemaining {
    pub budget_id: i64,
    pub remaining_usd: Option<Money>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestDigest {
    pub model: String,
    pub input_tokens: u64,
    pub cost_usd: Money,
    pub tool_name: Option<String>,
    pub tool_succeeded: bool,
    pub content_hash: u64,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DetectAlert {
    ToolLoop {
        tool_name: String,
        failure_count: u64,
    },
    BurnRate {
        usd_per_hour: f64,
        threshold: f64,
    },
    ContentLoop {
        similar_count: u64,
        window_seconds: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CostRange {
    pub min_usd: f64,
    pub max_usd: f64,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    SinglePass,
    MultiRound,
    AgentTask,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: i64,
    pub request_id: Option<RequestId>,
    pub session_id: Option<SessionId>,
    pub event_type: EventType,
    pub severity: Severity,
    pub detail: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    BudgetCheck,
    BudgetBlock,
    BudgetWarn,
    Reserve,
    Reconcile,
    Release,
    LoopDetected,
    BurnRateAlert,
    SessionPaused,
    ProviderFailure,
    ModeFailsafe,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
    Error,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Error)]
#[serde(tag = "type", content = "message", rename_all = "snake_case")]
pub enum PennyError {
    #[error("config error: {0}")]
    Config(String),
    #[error("store error: {0}")]
    Store(String),
    #[error("cost error: {0}")]
    Cost(String),
    #[error("ledger error: {0}")]
    Ledger(String),
    #[error("budget error: {0}")]
    Budget(String),
    #[error("detect error: {0}")]
    Detect(String),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("proxy error: {0}")]
    Proxy(String),
    #[error("admin error: {0}")]
    Admin(String),
    #[error("observe error: {0}")]
    Observe(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("time parse error: {0}")]
    Time(String),
    #[error("unknown error: {0}")]
    Unknown(String),
}

impl From<serde_json::Error> for PennyError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialization(value.to_string())
    }
}

impl From<chrono::ParseError> for PennyError {
    fn from(value: chrono::ParseError) -> Self {
        Self::Time(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde::de::DeserializeOwned;
    use std::fmt::Debug;

    fn assert_round_trip<T>(value: &T)
    where
        T: Serialize + DeserializeOwned + PartialEq + Debug,
    {
        let json = serde_json::to_string(value).expect("serialize test value");
        let decoded: T = serde_json::from_str(&json).expect("deserialize test value");
        assert_eq!(*value, decoded);
    }

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 10, 12, 30, 0)
            .single()
            .expect("valid static timestamp")
    }

    #[test]
    fn normalized_request_round_trip() {
        let value = NormalizedRequest {
            id: "req_01".into(),
            project_id: "penny".into(),
            session_id: "sess_01".into(),
            model_requested: "claude-sonnet-4-6".into(),
            model_resolved: "claude-sonnet-4-6".into(),
            provider_id: "anthropic".into(),
            messages: serde_json::json!([{ "role": "user", "content": "hello" }]),
            stream: false,
            estimated_input_tokens: 1200,
            estimated_output_tokens: 300,
            timestamp: ts(),
        };
        assert_round_trip(&value);
    }

    #[test]
    fn provider_response_round_trip() {
        let value = ProviderResponse {
            status: 200,
            body: ResponseBody::Complete(serde_json::json!({ "ok": true })),
            upstream_ms: 42,
        };
        assert_round_trip(&value);
    }

    #[test]
    fn route_decision_and_budget_round_trip() {
        let budget = Budget {
            id: 10,
            scope_type: ScopeType::Global,
            scope_id: "*".into(),
            window_type: WindowType::Day,
            hard_limit_usd: Some(Money::from_usd(10.0).expect("money")),
            soft_limit_usd: Some(Money::from_usd(8.0).expect("money")),
            action_on_hard: "block".into(),
            action_on_soft: "warn".into(),
            preset_source: Some("preset:indie".into()),
        };
        assert_round_trip(&budget);

        let decision = RouteDecision::Block {
            reason: "budget exceeded".into(),
            detail: BudgetBlockDetail {
                scope: "global:*".into(),
                window: WindowType::Day,
                accumulated_usd: Money::from_usd(10.12).expect("money"),
                limit_usd: Money::from_usd(10.0).expect("money"),
                resets_at: Some(ts()),
            },
        };
        assert_round_trip(&decision);
    }

    #[test]
    fn ledger_and_reservation_round_trip() {
        let entry = LedgerEntry {
            id: 1,
            request_id: "req_01".into(),
            entry_type: LedgerEntryType::Reserve,
            budget_id: 10,
            amount_usd: Money::from_usd(0.25).expect("money"),
            running_total: Money::from_usd(1.75).expect("money"),
            created_at: ts(),
        };
        assert_round_trip(&entry);

        let reservation = Reservation::Granted {
            entries: vec![1, 2],
            remaining_by_budget: vec![BudgetRemaining {
                budget_id: 10,
                remaining_usd: Some(Money::from_usd(8.25).expect("money")),
            }],
        };
        assert_round_trip(&reservation);
    }

    #[test]
    fn detect_and_cost_types_round_trip() {
        let digest = RequestDigest {
            model: "claude-sonnet-4-6".into(),
            input_tokens: 2000,
            cost_usd: Money::from_usd(0.43).expect("money"),
            tool_name: Some("shell".into()),
            tool_succeeded: false,
            content_hash: 998877,
            timestamp: ts(),
        };
        assert_round_trip(&digest);

        let alert = DetectAlert::BurnRate {
            usd_per_hour: 14.2,
            threshold: 10.0,
        };
        assert_round_trip(&alert);

        let range = CostRange {
            min_usd: 0.2,
            max_usd: 0.8,
            confidence: Confidence::Medium,
        };
        assert_round_trip(&range);
    }

    #[test]
    fn event_and_error_round_trip() {
        let event = Event {
            id: 1,
            request_id: Some("req_01".into()),
            session_id: Some("sess_01".into()),
            event_type: EventType::Reserve,
            severity: Severity::Info,
            detail: serde_json::json!({ "cost_usd": 0.25 }),
            created_at: ts(),
        };
        assert_round_trip(&event);

        let error = PennyError::Budget("hard limit exceeded".into());
        assert_round_trip(&error);
    }

    #[test]
    fn money_round_trip_and_parse() {
        let value = Money::from_usd(12.345678).expect("money");
        assert_eq!(value.to_string(), "12.345678");
        assert_round_trip(&value);

        let parsed = "0.42".parse::<Money>().expect("parse money");
        assert_eq!(parsed, Money::from_usd(0.42).expect("money"));
    }
}
