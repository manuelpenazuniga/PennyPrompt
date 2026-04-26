//! Costing and estimation logic for PennyPrompt.

use std::{fs, path::Path};

use chrono::{DateTime, Utc};
use penny_store::{NewPricebookEntry, PricebookRepo, SqliteStore, StoreError};
use penny_types::{Confidence, CostRange, Money, MoneyError, TaskType, UsageSource};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tiktoken_rs::cl100k_base;

#[derive(Debug, Error)]
pub enum CostError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid datetime for {field}: {value}")]
    InvalidDatetime { field: &'static str, value: String },
    #[error("no active pricebook entry found for model `{0}`")]
    PriceNotFound(String),
    #[error("money conversion error: {0}")]
    Money(#[from] MoneyError),
    #[error("money arithmetic overflow")]
    MoneyOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenEstimate {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub source: UsageSource,
}

#[derive(Debug, Clone)]
pub struct PricingEngine<'a, R: PricebookRepo> {
    repo: &'a R,
}

impl<'a, R: PricebookRepo> PricingEngine<'a, R> {
    pub fn new(repo: &'a R) -> Self {
        Self { repo }
    }

    pub async fn calculate(
        &self,
        model_id: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<Money, CostError> {
        if input_tokens == 0 && output_tokens == 0 {
            return Ok(Money::ZERO);
        }

        let price = self
            .repo
            .get_price(model_id, Utc::now())
            .await?
            .ok_or_else(|| CostError::PriceNotFound(model_id.to_string()))?;

        let input_cost = prorate_mtok(input_tokens, price.input_per_mtok)?;
        let output_cost = prorate_mtok(output_tokens, price.output_per_mtok)?;
        input_cost
            .checked_add(output_cost)
            .ok_or(CostError::MoneyOverflow)
    }

    /// Estimate input/output tokens from OpenAI-compatible `messages`.
    ///
    /// Fallbacks:
    /// 1. Preferred path uses `tiktoken-rs` with `cl100k_base`.
    /// 2. If no textual content is extractable from `messages`, fallback to `chars/4`.
    /// 3. If tokenizer initialization fails, fallback to `chars/4`.
    ///
    /// Output tokens use heuristic `min(input * 0.3, 4096)`.
    pub fn estimate_tokens(&self, messages: &Value) -> TokenEstimate {
        estimate_tokens(messages)
    }

    pub async fn snapshot(&self, model_id: &str) -> Result<Value, CostError> {
        let entry = self
            .repo
            .get_price(model_id, Utc::now())
            .await?
            .ok_or_else(|| CostError::PriceNotFound(model_id.to_string()))?;

        Ok(serde_json::json!({
            "id": entry.id,
            "model_id": entry.model_id,
            "input_per_mtok": entry.input_per_mtok,
            "output_per_mtok": entry.output_per_mtok,
            "effective_from": entry.effective_from,
            "effective_until": entry.effective_until,
            "source": entry.source
        }))
    }

    pub async fn estimate_range(
        &self,
        model_id: &str,
        context_tokens: u64,
        task_type: TaskType,
    ) -> Result<CostRange, CostError> {
        let output_tokens = estimate_output_tokens(context_tokens);
        let one_round = self
            .calculate(model_id, context_tokens, output_tokens)
            .await?;
        let one_round_usd = one_round.to_usd();

        let (rounds, margin, confidence) = match task_type {
            TaskType::SinglePass => (1.0, 0.30, Confidence::High),
            TaskType::MultiRound => (3.0, 0.50, Confidence::Medium),
            TaskType::AgentTask => (5.0, 1.00, Confidence::Low),
        };

        let center = one_round_usd * rounds;
        let min = (center * (1.0 - margin)).max(0.0);
        let max = center * (1.0 + margin);

        Ok(CostRange {
            min_usd: min,
            max_usd: max,
            confidence,
        })
    }
}

pub fn estimate_tokens(messages: &Value) -> TokenEstimate {
    let content_text = extract_message_text(messages);
    if content_text.trim().is_empty() {
        let input = heuristic_chars_to_tokens(messages.to_string().chars().count());
        return TokenEstimate {
            input_tokens: input,
            output_tokens: estimate_output_tokens(input),
            source: UsageSource::Heuristic,
        };
    }

    match cl100k_base() {
        Ok(encoding) => {
            let input = encoding
                .encode_with_special_tokens(&content_text)
                .len()
                .try_into()
                .unwrap_or(u64::MAX);
            TokenEstimate {
                input_tokens: input,
                output_tokens: estimate_output_tokens(input),
                source: UsageSource::Estimated,
            }
        }
        Err(_) => {
            let input = heuristic_chars_to_tokens(content_text.chars().count());
            TokenEstimate {
                input_tokens: input,
                output_tokens: estimate_output_tokens(input),
                source: UsageSource::Heuristic,
            }
        }
    }
}

fn estimate_output_tokens(input_tokens: u64) -> u64 {
    ((input_tokens as f64 * 0.3).min(4096.0)).round() as u64
}

fn prorate_mtok(tokens: u64, usd_per_mtok: Money) -> Result<Money, CostError> {
    let numerator = i128::from(tokens) * i128::from(usd_per_mtok.micros());
    let micros = if numerator >= 0 {
        (numerator + 500_000) / 1_000_000
    } else {
        (numerator - 500_000) / 1_000_000
    };
    let micros_i64 = i64::try_from(micros).map_err(|_| CostError::MoneyOverflow)?;
    Ok(Money::from_micros(micros_i64))
}

fn heuristic_chars_to_tokens(chars: usize) -> u64 {
    chars.div_ceil(4).try_into().unwrap_or(u64::MAX)
}

fn extract_message_text(messages: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(items) = messages.as_array() {
        for item in items {
            if let Some(content) = item.get("content") {
                collect_text(content, &mut parts);
            } else {
                collect_text(item, &mut parts);
            }
        }
    } else {
        collect_text(messages, &mut parts);
    }

    parts.join("\n")
}

fn collect_text(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => out.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                collect_text(item, out);
            }
        }
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                out.push(text.to_string());
                return;
            }
            if let Some(content) = map.get("content") {
                collect_text(content, out);
            }
        }
        _ => {}
    }
}

#[derive(Debug, Deserialize)]
struct PricebookFile {
    provider_id: String,
    provider_name: Option<String>,
    api_format: Option<String>,
    source: Option<String>,
    entries: Vec<PricebookModelEntry>,
}

#[derive(Debug, Deserialize)]
struct PricebookModelEntry {
    model_id: String,
    external_name: Option<String>,
    display_name: Option<String>,
    class: Option<String>,
    input_per_mtok: f64,
    output_per_mtok: f64,
    effective_from: String,
    effective_until: Option<String>,
    source: Option<String>,
}

pub async fn import_pricebook_files<P: AsRef<Path>>(
    store: &SqliteStore,
    paths: &[P],
) -> Result<usize, CostError> {
    let mut imported_entries = Vec::new();

    for path in paths {
        let file_content = fs::read_to_string(path.as_ref())?;
        let parsed: PricebookFile = toml::from_str(&file_content)?;

        let provider_name = parsed
            .provider_name
            .unwrap_or_else(|| parsed.provider_id.to_string());
        let api_format = parsed.api_format.unwrap_or_else(|| "openai".to_string());

        sqlx::query(
            r#"
            INSERT INTO providers (id, name, base_url, api_format, enabled)
            VALUES (?1, ?2, ?3, ?4, 1)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                api_format = excluded.api_format
            "#,
        )
        .bind(&parsed.provider_id)
        .bind(provider_name)
        .bind("https://placeholder.local")
        .bind(api_format)
        .execute(store.pool())
        .await?;

        for entry in parsed.entries {
            sqlx::query(
                r#"
                INSERT INTO models (id, provider_id, external_name, display_name, class)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(id) DO UPDATE SET
                    provider_id = excluded.provider_id,
                    external_name = excluded.external_name,
                    display_name = excluded.display_name,
                    class = excluded.class
                "#,
            )
            .bind(&entry.model_id)
            .bind(&parsed.provider_id)
            .bind(
                entry
                    .external_name
                    .unwrap_or_else(|| entry.model_id.clone()),
            )
            .bind(entry.display_name.unwrap_or_else(|| entry.model_id.clone()))
            .bind(entry.class.unwrap_or_else(|| "balanced".to_string()))
            .execute(store.pool())
            .await?;

            imported_entries.push(NewPricebookEntry {
                model_id: entry.model_id,
                input_per_mtok: Money::from_usd(entry.input_per_mtok)?,
                output_per_mtok: Money::from_usd(entry.output_per_mtok)?,
                effective_from: parse_datetime(&entry.effective_from, "effective_from")?,
                effective_until: entry
                    .effective_until
                    .as_deref()
                    .map(|value| parse_datetime(value, "effective_until"))
                    .transpose()?,
                source: entry
                    .source
                    .or(parsed.source.clone())
                    .unwrap_or_else(|| "local".to_string()),
            });
        }
    }

    let count = imported_entries.len();
    PricebookRepo::import(store, &imported_entries).await?;
    Ok(count)
}

fn parse_datetime(value: &str, field: &'static str) -> Result<DateTime<Utc>, CostError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| CostError::InvalidDatetime {
            field,
            value: value.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn approx_eq(left: f64, right: f64, tolerance: f64) {
        let delta = (left - right).abs();
        assert!(
            delta <= tolerance,
            "left={left}, right={right}, delta={delta}, tolerance={tolerance}"
        );
    }

    async fn setup_store_with_pricebook() -> SqliteStore {
        let store = SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store");
        let prices_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../prices")
            .canonicalize()
            .expect("resolve prices dir");
        let anthropic = prices_dir.join("anthropic.toml");
        let openai = prices_dir.join("openai.toml");
        import_pricebook_files(&store, &[anthropic, openai])
            .await
            .expect("import pricebooks");
        store
    }

    #[tokio::test]
    async fn calculate_uses_pricebook_rates() {
        let store = setup_store_with_pricebook().await;
        let engine = PricingEngine::new(&store);

        let cost = engine
            .calculate("claude-sonnet-4-6", 1_000_000, 1_000_000)
            .await
            .expect("calculate");

        assert_eq!(cost, Money::from_usd(18.0).expect("money"));
    }

    #[tokio::test]
    async fn snapshot_returns_price_entry() {
        let store = setup_store_with_pricebook().await;
        let engine = PricingEngine::new(&store);

        let snapshot = engine
            .snapshot("gpt-4.1")
            .await
            .expect("snapshot should exist");
        assert_eq!(snapshot["model_id"], "gpt-4.1");
        assert_eq!(snapshot["source"], "local");
    }

    #[tokio::test]
    async fn calculate_prefers_latest_effective_entry_for_model() {
        let store = SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create store");
        sqlx::query(
            "INSERT INTO providers (id, name, base_url, api_format, enabled) VALUES ('test-provider', 'Test Provider', 'https://example.invalid', 'openai', 1)",
        )
        .execute(store.pool())
        .await
        .expect("insert provider");
        sqlx::query(
            "INSERT INTO models (id, provider_id, external_name, display_name, class) VALUES ('test-model', 'test-provider', 'test-model', 'Test Model', 'balanced')",
        )
        .execute(store.pool())
        .await
        .expect("insert model");

        PricebookRepo::import(
            &store,
            &[
                NewPricebookEntry {
                    model_id: "test-model".to_string(),
                    input_per_mtok: Money::from_usd(1.0).expect("money"),
                    output_per_mtok: Money::from_usd(2.0).expect("money"),
                    effective_from: parse_datetime("2026-04-10T00:00:00Z", "effective_from")
                        .expect("datetime"),
                    effective_until: Some(
                        parse_datetime("2026-04-25T00:00:00Z", "effective_until")
                            .expect("datetime"),
                    ),
                    source: "local".to_string(),
                },
                NewPricebookEntry {
                    model_id: "test-model".to_string(),
                    input_per_mtok: Money::from_usd(3.0).expect("money"),
                    output_per_mtok: Money::from_usd(4.0).expect("money"),
                    effective_from: parse_datetime("2026-04-25T00:00:00Z", "effective_from")
                        .expect("datetime"),
                    effective_until: None,
                    source: "local".to_string(),
                },
            ],
        )
        .await
        .expect("import versioned entries");

        let engine = PricingEngine::new(&store);
        let cost = engine
            .calculate("test-model", 1_000_000, 1_000_000)
            .await
            .expect("calculate cost");

        assert_eq!(cost, Money::from_usd(7.0).expect("money"));
    }

    #[test]
    fn token_estimation_uses_cl100k_when_text_exists() {
        let payload = serde_json::json!([
            { "role": "user", "content": "hola mundo desde penny prompt" }
        ]);
        let result = estimate_tokens(&payload);
        assert!(result.input_tokens > 0);
        assert!(result.output_tokens <= 4096);
        assert_eq!(result.source, UsageSource::Estimated);
    }

    #[test]
    fn token_estimation_falls_back_to_heuristic_when_no_text_found() {
        let payload = serde_json::json!([
            { "role": "user", "content": [{ "type": "image", "url": "file:///tmp/a.png" }] }
        ]);
        let result = estimate_tokens(&payload);
        assert!(result.input_tokens > 0);
        assert_eq!(result.source, UsageSource::Heuristic);
    }

    #[tokio::test]
    async fn range_estimation_matches_task_profiles() {
        let store = setup_store_with_pricebook().await;
        let engine = PricingEngine::new(&store);

        let single = engine
            .estimate_range("claude-sonnet-4-6", 1_000, TaskType::SinglePass)
            .await
            .expect("single range");
        approx_eq(single.min_usd, 0.00525, 1e-6);
        approx_eq(single.max_usd, 0.00975, 1e-6);
        assert_eq!(single.confidence, Confidence::High);

        let multi = engine
            .estimate_range("claude-sonnet-4-6", 1_000, TaskType::MultiRound)
            .await
            .expect("multi range");
        approx_eq(multi.min_usd, 0.01125, 1e-6);
        approx_eq(multi.max_usd, 0.03375, 1e-6);
        assert_eq!(multi.confidence, Confidence::Medium);

        let agent = engine
            .estimate_range("claude-sonnet-4-6", 1_000, TaskType::AgentTask)
            .await
            .expect("agent range");
        approx_eq(agent.min_usd, 0.0, 1e-6);
        approx_eq(agent.max_usd, 0.075, 1e-6);
        assert_eq!(agent.confidence, Confidence::Low);
    }

    #[tokio::test]
    async fn pricebook_import_loads_at_least_six_models() {
        let store = SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create store");
        let prices_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../prices")
            .canonicalize()
            .expect("resolve prices dir");
        let imported = import_pricebook_files(
            &store,
            &[
                prices_dir.join("anthropic.toml"),
                prices_dir.join("openai.toml"),
            ],
        )
        .await
        .expect("import pricebook files");

        assert!(imported >= 6);
        let total_models: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM models")
            .fetch_one(store.pool())
            .await
            .expect("count models");
        assert!(total_models >= 6);
    }
}
