//! Observability setup and tracing helpers.

use std::env;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub const DEFAULT_LOG_FILTER: &str = "info,sqlx=warn,hyper=warn,reqwest=warn";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObserveConfig {
    /// Log level filter (RUST_LOG syntax).
    #[serde(default = "default_log_filter")]
    pub log_filter: String,
    /// Emit JSON logs when enabled.
    #[serde(default)]
    pub json: bool,
}

impl Default for ObserveConfig {
    fn default() -> Self {
        Self {
            log_filter: default_log_filter(),
            json: false,
        }
    }
}

#[derive(Debug, Error)]
pub enum InitError {
    #[error("invalid tracing filter `{filter}`: {source}")]
    InvalidFilter {
        filter: String,
        #[source]
        source: tracing_subscriber::filter::ParseError,
    },
    #[error("tracing already initialized: {0}")]
    AlreadyInitialized(#[from] tracing_subscriber::util::TryInitError),
}

pub fn default_log_filter() -> String {
    DEFAULT_LOG_FILTER.to_string()
}

/// Initializes global tracing exactly once for the process.
///
/// Resolution order:
/// 1. `PENNY_LOG` (if set and non-empty)
/// 2. `RUST_LOG` (if set and non-empty)
/// 3. `cfg.log_filter`
///
/// Structured mode can be toggled with `PENNY_OBSERVE_JSON=true|false|1|0|yes|no|on|off`.
pub fn init_tracing(cfg: &ObserveConfig) -> Result<(), InitError> {
    let filter_spec = env_filter_override().unwrap_or_else(|| cfg.log_filter.clone());
    let env_filter =
        EnvFilter::try_new(filter_spec.clone()).map_err(|source| InitError::InvalidFilter {
            filter: filter_spec,
            source,
        })?;
    let json = env_json_override().unwrap_or(cfg.json);
    let registry = tracing_subscriber::registry().with(env_filter);

    if json {
        registry.with(fmt::layer().json()).try_init()?;
    } else {
        registry.with(fmt::layer().with_target(false)).try_init()?;
    }
    Ok(())
}

fn env_filter_override() -> Option<String> {
    non_empty_env("PENNY_LOG").or_else(|| non_empty_env("RUST_LOG"))
}

fn env_json_override() -> Option<bool> {
    non_empty_env("PENNY_OBSERVE_JSON").and_then(|raw| parse_bool(&raw))
}

fn non_empty_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Canonical field names for structured log attributes.
pub mod fields {
    pub const REQUEST_ID: &str = "request_id";
    pub const SESSION_ID: &str = "session_id";
    pub const PROJECT_ID: &str = "project_id";
    pub const MODEL: &str = "model";
    pub const COST_USD: &str = "cost_usd";
    pub const EVENT_TYPE: &str = "event_type";
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn observe_config_toml_roundtrip() {
        let cfg = ObserveConfig {
            log_filter: "debug,penny_proxy=trace".to_string(),
            json: true,
        };
        let raw = toml::to_string(&cfg).expect("serialize");
        let decoded: ObserveConfig = toml::from_str(&raw).expect("deserialize");
        assert_eq!(decoded, cfg);
    }

    #[test]
    fn init_tracing_second_call_returns_already_initialized() {
        let _guard = env_lock().lock().expect("env lock");
        std::env::remove_var("PENNY_LOG");
        std::env::remove_var("RUST_LOG");
        std::env::remove_var("PENNY_OBSERVE_JSON");

        let first = init_tracing(&ObserveConfig::default());
        assert!(first.is_ok(), "first init should succeed: {first:?}");

        let second = init_tracing(&ObserveConfig::default());
        assert!(
            matches!(second, Err(InitError::AlreadyInitialized(_))),
            "expected AlreadyInitialized, got {second:?}"
        );
    }
}
