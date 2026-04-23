//! Configuration loading and validation for PennyPrompt.

use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;
use url::Url;

pub const PRESET_INDIE: &str = "indie";
pub const PRESET_TEAM: &str = "team";
pub const PRESET_EXPLORE: &str = "explore";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Observe,
    Guard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeType {
    Global,
    Project,
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowType {
    Day,
    Week,
    Month,
    Total,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopAction {
    Alert,
    Pause,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bind: String,
    pub admin_socket: String,
    pub database_path: String,
    pub mode: Mode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultsConfig {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributionConfig {
    pub auto_detect_project: bool,
    pub session_window_minutes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub enabled: bool,
    pub base_url: String,
    pub api_key_env: String,
    pub api_format: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvidersConfig {
    pub anthropic: ProviderConfig,
    pub openai: ProviderConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetConfig {
    pub scope_type: ScopeType,
    pub scope_id: String,
    pub window_type: WindowType,
    pub hard_limit_usd: Option<f64>,
    pub soft_limit_usd: Option<f64>,
    #[serde(default = "default_action_on_hard")]
    pub action_on_hard: String,
    #[serde(default = "default_action_on_soft")]
    pub action_on_soft: String,
    #[serde(default)]
    pub preset_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectConfig {
    pub enabled: bool,
    pub burn_rate_alert_usd_per_hour: f64,
    pub loop_window_seconds: u64,
    pub loop_threshold_similar_requests: u32,
    pub loop_action: LoopAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub defaults: DefaultsConfig,
    pub attribution: AttributionConfig,
    pub providers: ProvidersConfig,
    pub budgets: Vec<BudgetConfig>,
    pub detect: DetectConfig,
}

#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    pub repository_root: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub preset: Option<String>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("current directory error: {0}")]
    CurrentDir(String),
    #[error("missing default config at {0}")]
    MissingDefaultConfig(String),
    #[error("preset '{preset}' not found at {path}")]
    MissingPreset { preset: String, path: String },
    #[error("invalid preset '{0}', expected one of: indie|team|explore")]
    InvalidPreset(String),
    #[error("failed to read file {path}: {source}")]
    ReadFile {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse TOML from {path}: {source}")]
    ParseToml {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("validation error: {0}")]
    Validation(String),
}

pub fn load_config(opts: LoadOptions) -> Result<AppConfig, ConfigError> {
    let repo_root = resolve_repo_root(opts.repository_root)?;
    let default_path = repo_root.join("config").join("default.toml");
    if !default_path.exists() {
        return Err(ConfigError::MissingDefaultConfig(
            default_path.display().to_string(),
        ));
    }

    let mut merged = read_partial(&default_path)?;

    if let Some(preset) = opts.preset.as_deref() {
        validate_preset_name(preset)?;
        let preset_path = repo_root.join("presets").join(format!("{preset}.toml"));
        if !preset_path.exists() {
            return Err(ConfigError::MissingPreset {
                preset: preset.to_string(),
                path: preset_path.display().to_string(),
            });
        }
        let mut preset_partial = read_partial(&preset_path)?;
        tag_preset_budget_source(&mut preset_partial, preset);
        merged.merge_from(preset_partial);
    }

    if let Some(config_path) = resolve_user_config_path(opts.config_path)? {
        if config_path.exists() {
            let user_partial = read_partial(&config_path)?;
            merged.merge_from(user_partial);
        }
    }

    apply_env_overrides(&mut merged)?;
    let config = merged.require_all()?;
    validate_config(&config)?;
    Ok(config)
}

fn default_action_on_hard() -> String {
    "block".to_string()
}

fn default_action_on_soft() -> String {
    "warn".to_string()
}

fn tag_preset_budget_source(cfg: &mut PartialAppConfig, preset: &str) {
    let Some(budgets) = cfg.budgets.as_mut() else {
        return;
    };
    let source = format!("preset:{preset}");
    for budget in budgets {
        if budget.preset_source.is_none() {
            budget.preset_source = Some(source.clone());
        }
    }
}

fn resolve_repo_root(repository_root: Option<PathBuf>) -> Result<PathBuf, ConfigError> {
    if let Some(path) = repository_root {
        return Ok(path);
    }
    env::current_dir().map_err(|e| ConfigError::CurrentDir(e.to_string()))
}

pub fn resolve_user_config_path(
    config_path: Option<PathBuf>,
) -> Result<Option<PathBuf>, ConfigError> {
    if config_path.is_some() {
        return Ok(config_path);
    }
    if let Ok(env_path) = env::var("PENNY_CONFIG") {
        if !env_path.trim().is_empty() {
            return Ok(Some(PathBuf::from(env_path)));
        }
    }
    let home = env::var("HOME").ok();
    Ok(home.map(|h| PathBuf::from(h).join(".config/pennyprompt/config.toml")))
}

fn validate_preset_name(preset: &str) -> Result<(), ConfigError> {
    match preset {
        PRESET_INDIE | PRESET_TEAM | PRESET_EXPLORE => Ok(()),
        _ => Err(ConfigError::InvalidPreset(preset.to_string())),
    }
}

fn read_partial(path: &Path) -> Result<PartialAppConfig, ConfigError> {
    let raw = fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
        path: path.display().to_string(),
        source,
    })?;
    toml::from_str(&raw).map_err(|source| ConfigError::ParseToml {
        path: path.display().to_string(),
        source,
    })
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn apply_env_overrides(cfg: &mut PartialAppConfig) -> Result<(), ConfigError> {
    if let Ok(v) = env::var("PENNY_SERVER_BIND") {
        cfg.server.get_or_insert_with(Default::default).bind = Some(v);
    }
    if let Ok(v) = env::var("PENNY_SERVER_MODE") {
        let mode = match v.trim().to_lowercase().as_str() {
            "observe" => Mode::Observe,
            "guard" => Mode::Guard,
            _ => {
                return Err(ConfigError::Validation(
                    "PENNY_SERVER_MODE must be observe|guard".to_string(),
                ));
            }
        };
        cfg.server.get_or_insert_with(Default::default).mode = Some(mode);
    }
    if let Ok(v) = env::var("PENNY_DEFAULTS_PROVIDER") {
        cfg.defaults.get_or_insert_with(Default::default).provider = Some(v);
    }
    if let Ok(v) = env::var("PENNY_DEFAULTS_MODEL") {
        cfg.defaults.get_or_insert_with(Default::default).model = Some(v);
    }
    if let Ok(v) = env::var("PENNY_ATTRIBUTION_AUTO_DETECT_PROJECT") {
        let parsed = parse_bool(&v).ok_or_else(|| {
            ConfigError::Validation(
                "PENNY_ATTRIBUTION_AUTO_DETECT_PROJECT must be boolean".to_string(),
            )
        })?;
        cfg.attribution
            .get_or_insert_with(Default::default)
            .auto_detect_project = Some(parsed);
    }
    if let Ok(v) = env::var("PENNY_ATTRIBUTION_SESSION_WINDOW_MINUTES") {
        let parsed = v.trim().parse::<u32>().map_err(|_| {
            ConfigError::Validation(
                "PENNY_ATTRIBUTION_SESSION_WINDOW_MINUTES must be integer".to_string(),
            )
        })?;
        cfg.attribution
            .get_or_insert_with(Default::default)
            .session_window_minutes = Some(parsed);
    }
    Ok(())
}

fn validate_config(config: &AppConfig) -> Result<(), ConfigError> {
    if config.server.bind.trim().is_empty() {
        return Err(ConfigError::Validation(
            "server.bind is required".to_string(),
        ));
    }
    if config.attribution.session_window_minutes == 0 {
        return Err(ConfigError::Validation(
            "attribution.session_window_minutes must be > 0".to_string(),
        ));
    }
    validate_provider("providers.anthropic", &config.providers.anthropic)?;
    validate_provider("providers.openai", &config.providers.openai)?;
    if !config.providers.anthropic.enabled && !config.providers.openai.enabled {
        return Err(ConfigError::Validation(
            "at least one provider must be enabled".to_string(),
        ));
    }
    if config.budgets.is_empty() {
        return Err(ConfigError::Validation(
            "at least one budget entry is required".to_string(),
        ));
    }
    for (idx, budget) in config.budgets.iter().enumerate() {
        if budget.scope_id.trim().is_empty() {
            return Err(ConfigError::Validation(format!(
                "budgets[{idx}].scope_id is required"
            )));
        }
        if let Some(hard) = budget.hard_limit_usd {
            if hard <= 0.0 {
                return Err(ConfigError::Validation(format!(
                    "budgets[{idx}].hard_limit_usd must be > 0"
                )));
            }
        }
        if let Some(soft) = budget.soft_limit_usd {
            if soft <= 0.0 {
                return Err(ConfigError::Validation(format!(
                    "budgets[{idx}].soft_limit_usd must be > 0"
                )));
            }
        }
        if let (Some(soft), Some(hard)) = (budget.soft_limit_usd, budget.hard_limit_usd) {
            if soft > hard {
                return Err(ConfigError::Validation(format!(
                    "budgets[{idx}].soft_limit_usd cannot exceed hard_limit_usd"
                )));
            }
        }
    }
    Ok(())
}

fn validate_provider(name: &str, provider: &ProviderConfig) -> Result<(), ConfigError> {
    if provider.enabled {
        Url::parse(&provider.base_url)
            .map_err(|e| ConfigError::Validation(format!("{name}.base_url is invalid URL: {e}")))?;
        if provider.api_key_env.trim().is_empty() {
            return Err(ConfigError::Validation(format!(
                "{name}.api_key_env is required when provider is enabled"
            )));
        }
        if provider.api_format.trim().is_empty() {
            return Err(ConfigError::Validation(format!(
                "{name}.api_format is required when provider is enabled"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PartialAppConfig {
    server: Option<PartialServerConfig>,
    defaults: Option<PartialDefaultsConfig>,
    attribution: Option<PartialAttributionConfig>,
    providers: Option<PartialProvidersConfig>,
    budgets: Option<Vec<BudgetConfig>>,
    detect: Option<PartialDetectConfig>,
}

impl PartialAppConfig {
    fn merge_from(&mut self, other: PartialAppConfig) {
        merge_partial(&mut self.server, other.server, |a, b| a.merge_from(b));
        merge_partial(&mut self.defaults, other.defaults, |a, b| a.merge_from(b));
        merge_partial(&mut self.attribution, other.attribution, |a, b| {
            a.merge_from(b)
        });
        merge_partial(&mut self.providers, other.providers, |a, b| a.merge_from(b));
        merge_partial(&mut self.detect, other.detect, |a, b| a.merge_from(b));

        if other.budgets.is_some() {
            self.budgets = other.budgets;
        }
    }

    fn require_all(self) -> Result<AppConfig, ConfigError> {
        Ok(AppConfig {
            server: self
                .server
                .ok_or_else(|| ConfigError::Validation("missing [server]".to_string()))?
                .require()?,
            defaults: self
                .defaults
                .ok_or_else(|| ConfigError::Validation("missing [defaults]".to_string()))?
                .require()?,
            attribution: self
                .attribution
                .ok_or_else(|| ConfigError::Validation("missing [attribution]".to_string()))?
                .require()?,
            providers: self
                .providers
                .ok_or_else(|| ConfigError::Validation("missing [providers]".to_string()))?
                .require()?,
            budgets: self
                .budgets
                .ok_or_else(|| ConfigError::Validation("missing [[budgets]]".to_string()))?,
            detect: self
                .detect
                .ok_or_else(|| ConfigError::Validation("missing [detect]".to_string()))?
                .require()?,
        })
    }
}

fn merge_partial<T, F>(target: &mut Option<T>, incoming: Option<T>, merge_fn: F)
where
    F: FnOnce(&mut T, T),
{
    match (target.as_mut(), incoming) {
        (Some(current), Some(next)) => merge_fn(current, next),
        (None, Some(next)) => *target = Some(next),
        _ => {}
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PartialServerConfig {
    bind: Option<String>,
    admin_socket: Option<String>,
    database_path: Option<String>,
    mode: Option<Mode>,
}

impl PartialServerConfig {
    fn merge_from(&mut self, other: PartialServerConfig) {
        if other.bind.is_some() {
            self.bind = other.bind;
        }
        if other.admin_socket.is_some() {
            self.admin_socket = other.admin_socket;
        }
        if other.database_path.is_some() {
            self.database_path = other.database_path;
        }
        if other.mode.is_some() {
            self.mode = other.mode;
        }
    }

    fn require(self) -> Result<ServerConfig, ConfigError> {
        Ok(ServerConfig {
            bind: self
                .bind
                .ok_or_else(|| ConfigError::Validation("missing server.bind".to_string()))?,
            admin_socket: self.admin_socket.ok_or_else(|| {
                ConfigError::Validation("missing server.admin_socket".to_string())
            })?,
            database_path: self.database_path.ok_or_else(|| {
                ConfigError::Validation("missing server.database_path".to_string())
            })?,
            mode: self
                .mode
                .ok_or_else(|| ConfigError::Validation("missing server.mode".to_string()))?,
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PartialDefaultsConfig {
    provider: Option<String>,
    model: Option<String>,
}

impl PartialDefaultsConfig {
    fn merge_from(&mut self, other: PartialDefaultsConfig) {
        if other.provider.is_some() {
            self.provider = other.provider;
        }
        if other.model.is_some() {
            self.model = other.model;
        }
    }

    fn require(self) -> Result<DefaultsConfig, ConfigError> {
        Ok(DefaultsConfig {
            provider: self
                .provider
                .ok_or_else(|| ConfigError::Validation("missing defaults.provider".to_string()))?,
            model: self
                .model
                .ok_or_else(|| ConfigError::Validation("missing defaults.model".to_string()))?,
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PartialAttributionConfig {
    auto_detect_project: Option<bool>,
    session_window_minutes: Option<u32>,
}

impl PartialAttributionConfig {
    fn merge_from(&mut self, other: PartialAttributionConfig) {
        if other.auto_detect_project.is_some() {
            self.auto_detect_project = other.auto_detect_project;
        }
        if other.session_window_minutes.is_some() {
            self.session_window_minutes = other.session_window_minutes;
        }
    }

    fn require(self) -> Result<AttributionConfig, ConfigError> {
        Ok(AttributionConfig {
            auto_detect_project: self.auto_detect_project.ok_or_else(|| {
                ConfigError::Validation("missing attribution.auto_detect_project".to_string())
            })?,
            session_window_minutes: self.session_window_minutes.ok_or_else(|| {
                ConfigError::Validation("missing attribution.session_window_minutes".to_string())
            })?,
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PartialProviderConfig {
    enabled: Option<bool>,
    base_url: Option<String>,
    api_key_env: Option<String>,
    api_format: Option<String>,
}

impl PartialProviderConfig {
    fn merge_from(&mut self, other: PartialProviderConfig) {
        if other.enabled.is_some() {
            self.enabled = other.enabled;
        }
        if other.base_url.is_some() {
            self.base_url = other.base_url;
        }
        if other.api_key_env.is_some() {
            self.api_key_env = other.api_key_env;
        }
        if other.api_format.is_some() {
            self.api_format = other.api_format;
        }
    }

    fn require(self, name: &str) -> Result<ProviderConfig, ConfigError> {
        Ok(ProviderConfig {
            enabled: self.enabled.ok_or_else(|| {
                ConfigError::Validation(format!("missing providers.{name}.enabled"))
            })?,
            base_url: self.base_url.ok_or_else(|| {
                ConfigError::Validation(format!("missing providers.{name}.base_url"))
            })?,
            api_key_env: self.api_key_env.ok_or_else(|| {
                ConfigError::Validation(format!("missing providers.{name}.api_key_env"))
            })?,
            api_format: self.api_format.ok_or_else(|| {
                ConfigError::Validation(format!("missing providers.{name}.api_format"))
            })?,
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PartialProvidersConfig {
    anthropic: Option<PartialProviderConfig>,
    openai: Option<PartialProviderConfig>,
}

impl PartialProvidersConfig {
    fn merge_from(&mut self, other: PartialProvidersConfig) {
        if self.anthropic.is_none() {
            self.anthropic = other.anthropic;
        } else if let (Some(current), Some(incoming)) = (&mut self.anthropic, other.anthropic) {
            current.merge_from(incoming);
        }

        if self.openai.is_none() {
            self.openai = other.openai;
        } else if let (Some(current), Some(incoming)) = (&mut self.openai, other.openai) {
            current.merge_from(incoming);
        }
    }

    fn require(self) -> Result<ProvidersConfig, ConfigError> {
        Ok(ProvidersConfig {
            anthropic: self
                .anthropic
                .ok_or_else(|| ConfigError::Validation("missing providers.anthropic".to_string()))?
                .require("anthropic")?,
            openai: self
                .openai
                .ok_or_else(|| ConfigError::Validation("missing providers.openai".to_string()))?
                .require("openai")?,
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PartialDetectConfig {
    enabled: Option<bool>,
    burn_rate_alert_usd_per_hour: Option<f64>,
    loop_window_seconds: Option<u64>,
    loop_threshold_similar_requests: Option<u32>,
    loop_action: Option<LoopAction>,
}

impl PartialDetectConfig {
    fn merge_from(&mut self, other: PartialDetectConfig) {
        if other.enabled.is_some() {
            self.enabled = other.enabled;
        }
        if other.burn_rate_alert_usd_per_hour.is_some() {
            self.burn_rate_alert_usd_per_hour = other.burn_rate_alert_usd_per_hour;
        }
        if other.loop_window_seconds.is_some() {
            self.loop_window_seconds = other.loop_window_seconds;
        }
        if other.loop_threshold_similar_requests.is_some() {
            self.loop_threshold_similar_requests = other.loop_threshold_similar_requests;
        }
        if other.loop_action.is_some() {
            self.loop_action = other.loop_action;
        }
    }

    fn require(self) -> Result<DetectConfig, ConfigError> {
        Ok(DetectConfig {
            enabled: self
                .enabled
                .ok_or_else(|| ConfigError::Validation("missing detect.enabled".to_string()))?,
            burn_rate_alert_usd_per_hour: self.burn_rate_alert_usd_per_hour.ok_or_else(|| {
                ConfigError::Validation("missing detect.burn_rate_alert_usd_per_hour".to_string())
            })?,
            loop_window_seconds: self.loop_window_seconds.ok_or_else(|| {
                ConfigError::Validation("missing detect.loop_window_seconds".to_string())
            })?,
            loop_threshold_similar_requests: self.loop_threshold_similar_requests.ok_or_else(
                || {
                    ConfigError::Validation(
                        "missing detect.loop_threshold_similar_requests".to_string(),
                    )
                },
            )?,
            loop_action: self
                .loop_action
                .ok_or_else(|| ConfigError::Validation("missing detect.loop_action".to_string()))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(path, content).expect("write test file");
    }

    fn test_repo() -> TempDir {
        let dir = TempDir::new().expect("temp repo");
        write_file(
            &dir.path().join("config/default.toml"),
            r#"
[server]
bind = "127.0.0.1:8585"
admin_socket = "/tmp/penny.sock"
database_path = "/tmp/penny.db"
mode = "guard"

[defaults]
provider = "anthropic"
model = "claude-sonnet-4-6"

[attribution]
auto_detect_project = true
session_window_minutes = 30

[providers.anthropic]
enabled = true
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
api_format = "anthropic"

[providers.openai]
enabled = true
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
api_format = "openai"

[[budgets]]
scope_type = "global"
scope_id = "*"
window_type = "month"
hard_limit_usd = 30.0
soft_limit_usd = 20.0

[detect]
enabled = true
burn_rate_alert_usd_per_hour = 10.0
loop_window_seconds = 120
loop_threshold_similar_requests = 8
loop_action = "pause"
"#,
        );
        write_file(
            &dir.path().join("presets/indie.toml"),
            r#"
[server]
mode = "guard"

[defaults]
model = "claude-sonnet-4-6"

[[budgets]]
scope_type = "global"
scope_id = "*"
window_type = "day"
hard_limit_usd = 10.0
"#,
        );
        write_file(
            &dir.path().join("presets/team.toml"),
            r#"
[server]
mode = "guard"

[[budgets]]
scope_type = "global"
scope_id = "*"
window_type = "month"
hard_limit_usd = 100.0
"#,
        );
        write_file(
            &dir.path().join("presets/explore.toml"),
            r#"
[server]
mode = "observe"

[[budgets]]
scope_type = "global"
scope_id = "*"
window_type = "month"
soft_limit_usd = 10.0
"#,
        );
        dir
    }

    fn clear_test_env() {
        env::remove_var("PENNY_CONFIG");
        env::remove_var("PENNY_SERVER_BIND");
        env::remove_var("PENNY_SERVER_MODE");
        env::remove_var("PENNY_DEFAULTS_PROVIDER");
        env::remove_var("PENNY_DEFAULTS_MODEL");
        env::remove_var("PENNY_ATTRIBUTION_AUTO_DETECT_PROJECT");
        env::remove_var("PENNY_ATTRIBUTION_SESSION_WINDOW_MINUTES");
    }

    #[test]
    fn loads_valid_config_from_default_file() {
        let _guard = env_lock().lock().expect("env lock");
        clear_test_env();
        let repo = test_repo();

        let cfg = load_config(LoadOptions {
            repository_root: Some(repo.path().to_path_buf()),
            ..Default::default()
        })
        .expect("load config");

        assert_eq!(cfg.server.bind, "127.0.0.1:8585");
        assert_eq!(cfg.defaults.model, "claude-sonnet-4-6");
        assert_eq!(cfg.budgets.len(), 1);
    }

    #[test]
    fn applies_preset_over_default() {
        let _guard = env_lock().lock().expect("env lock");
        clear_test_env();
        let repo = test_repo();

        let cfg = load_config(LoadOptions {
            repository_root: Some(repo.path().to_path_buf()),
            preset: Some(PRESET_INDIE.to_string()),
            ..Default::default()
        })
        .expect("load with preset");

        assert_eq!(cfg.server.mode, Mode::Guard);
        assert_eq!(cfg.budgets[0].window_type, WindowType::Day);
        assert_eq!(cfg.budgets[0].hard_limit_usd, Some(10.0));
    }

    #[test]
    fn applies_env_overrides() {
        let _guard = env_lock().lock().expect("env lock");
        clear_test_env();
        let repo = test_repo();

        env::set_var("PENNY_DEFAULTS_MODEL", "gpt-5.2");
        env::set_var("PENNY_SERVER_BIND", "0.0.0.0:8585");
        env::set_var("PENNY_ATTRIBUTION_SESSION_WINDOW_MINUTES", "45");

        let cfg = load_config(LoadOptions {
            repository_root: Some(repo.path().to_path_buf()),
            ..Default::default()
        })
        .expect("load with env overrides");

        assert_eq!(cfg.defaults.model, "gpt-5.2");
        assert_eq!(cfg.server.bind, "0.0.0.0:8585");
        assert_eq!(cfg.attribution.session_window_minutes, 45);
    }

    #[test]
    fn rejects_invalid_provider_url() {
        let _guard = env_lock().lock().expect("env lock");
        clear_test_env();
        let repo = test_repo();
        write_file(
            &repo.path().join("broken.toml"),
            r#"
[providers.anthropic]
base_url = "not-an-url"
"#,
        );

        let err = load_config(LoadOptions {
            repository_root: Some(repo.path().to_path_buf()),
            config_path: Some(repo.path().join("broken.toml")),
            ..Default::default()
        })
        .expect_err("should fail validation");

        let rendered = err.to_string();
        assert!(
            rendered.contains("providers.anthropic.base_url is invalid URL"),
            "unexpected error: {rendered}"
        );
    }
}
