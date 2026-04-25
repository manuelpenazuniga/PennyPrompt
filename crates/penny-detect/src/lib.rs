//! Runaway loop and burn-rate detection.

use std::{
    collections::{HashMap, VecDeque},
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use chrono::{DateTime, Duration, Utc};
use penny_config::{DetectConfig as RuntimeDetectConfig, LoopAction};
use penny_types::{DetectAlert, EventType, RequestDigest, RequestId, SessionId, Severity};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const SESSION_PAUSED_LOOP_REASON: &str = "session_paused_loop_detected";
const DEFAULT_MAX_SESSIONS: usize = 2048;
const DEFAULT_MAX_RECORDED_EVENTS: usize = 5000;
const DEFAULT_MIN_BURN_RATE_OBSERVATION_SECONDS: u64 = 30;
const DEFAULT_SESSION_STATE_RETENTION_SECONDS: u64 = 3600;

#[derive(Debug, Clone, PartialEq)]
pub struct DetectorConfig {
    pub enabled: bool,
    pub burn_rate_alert_usd_per_hour: f64,
    pub loop_window_seconds: u64,
    pub loop_threshold_similar_requests: u32,
    pub loop_action: LoopAction,
    pub min_burn_rate_observation_seconds: u64,
    pub max_recorded_events: usize,
    pub session_state_retention_seconds: u64,
    pub max_sessions: usize,
}

impl From<&RuntimeDetectConfig> for DetectorConfig {
    fn from(value: &RuntimeDetectConfig) -> Self {
        Self {
            enabled: value.enabled,
            burn_rate_alert_usd_per_hour: value.burn_rate_alert_usd_per_hour,
            loop_window_seconds: value.loop_window_seconds,
            loop_threshold_similar_requests: value.loop_threshold_similar_requests,
            loop_action: value.loop_action.clone(),
            min_burn_rate_observation_seconds: DEFAULT_MIN_BURN_RATE_OBSERVATION_SECONDS,
            max_recorded_events: DEFAULT_MAX_RECORDED_EVENTS,
            session_state_retention_seconds: DEFAULT_SESSION_STATE_RETENTION_SECONDS,
            max_sessions: DEFAULT_MAX_SESSIONS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectEventRecord {
    pub session_id: SessionId,
    pub request_id: Option<RequestId>,
    pub event_type: EventType,
    pub severity: Severity,
    pub detail: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PausedSession {
    pub session_id: SessionId,
    pub reason: String,
    pub paused_at: DateTime<Utc>,
    pub triggered_by: DetectAlert,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionAlert {
    pub session_id: SessionId,
    pub alert: DetectAlert,
    pub triggered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectStatus {
    pub paused_sessions: Vec<PausedSession>,
    pub active_alerts: Vec<SessionAlert>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeedResult {
    pub session_id: SessionId,
    pub alerts: Vec<DetectAlert>,
    pub paused: bool,
    pub pause_reason: Option<String>,
    pub events: Vec<DetectEventRecord>,
}

impl FeedResult {
    fn empty(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            alerts: Vec::new(),
            paused: false,
            pause_reason: None,
            events: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
struct DetectorState {
    windows: HashMap<SessionId, SessionWindow>,
    paused: HashMap<SessionId, PausedSession>,
    active_alerts: HashMap<SessionId, SessionAlert>,
    events: VecDeque<DetectEventRecord>,
}

#[derive(Debug, Clone)]
struct SessionWindow {
    digests: Vec<RequestDigest>,
    last_seen: DateTime<Utc>,
}

#[derive(Debug)]
pub struct DetectEngine {
    config: DetectorConfig,
    state: RwLock<DetectorState>,
}

impl DetectEngine {
    pub fn new(config: DetectorConfig) -> Self {
        Self {
            config,
            state: RwLock::new(DetectorState::default()),
        }
    }

    pub fn from_runtime_config(config: &RuntimeDetectConfig) -> Self {
        Self::new(DetectorConfig::from(config))
    }

    pub fn config(&self) -> &DetectorConfig {
        &self.config
    }

    pub fn feed(
        &self,
        session_id: &str,
        request_id: Option<&str>,
        digest: RequestDigest,
    ) -> FeedResult {
        if !self.config.enabled {
            return FeedResult::empty(session_id);
        }

        let mut state = self.write_state();
        prune_inactive_sessions(
            &mut state,
            digest.timestamp,
            self.config.session_state_retention_seconds,
        );

        let alerts = {
            let window = state
                .windows
                .entry(session_id.to_string())
                .or_insert_with(|| SessionWindow {
                    digests: Vec::new(),
                    last_seen: digest.timestamp,
                });
            window.last_seen = digest.timestamp;
            window.digests.push(digest.clone());
            prune_window(
                &mut window.digests,
                digest.timestamp,
                self.config.loop_window_seconds,
            );
            self.detect_alerts(&window.digests, &digest)
        };
        cap_session_count(&mut state, self.config.max_sessions);
        if alerts.is_empty() {
            state.active_alerts.remove(session_id);
            return FeedResult::empty(session_id);
        }

        let mut events = Vec::new();
        for alert in &alerts {
            let (event_type, severity, detail) = alert_to_event(alert);
            let event = DetectEventRecord {
                session_id: session_id.to_string(),
                request_id: request_id.map(ToOwned::to_owned),
                event_type,
                severity,
                detail,
                created_at: digest.timestamp,
            };
            push_recorded_event(
                &mut state.events,
                event.clone(),
                self.config.max_recorded_events,
            );
            events.push(event);
        }

        state.active_alerts.insert(
            session_id.to_string(),
            SessionAlert {
                session_id: session_id.to_string(),
                alert: alerts[0].clone(),
                triggered_at: digest.timestamp,
            },
        );

        let should_pause = matches!(self.config.loop_action, LoopAction::Pause)
            && alerts.iter().any(|alert| {
                matches!(
                    alert,
                    DetectAlert::ToolLoop { .. } | DetectAlert::ContentLoop { .. }
                )
            });

        let mut paused = false;
        let mut pause_reason = None;
        if should_pause {
            let pause_trigger = alerts
                .iter()
                .find(|alert| {
                    matches!(
                        alert,
                        DetectAlert::ToolLoop { .. } | DetectAlert::ContentLoop { .. }
                    )
                })
                .cloned()
                .expect("loop alert exists when should_pause is true");
            let paused_session = PausedSession {
                session_id: session_id.to_string(),
                reason: SESSION_PAUSED_LOOP_REASON.to_string(),
                paused_at: digest.timestamp,
                triggered_by: pause_trigger.clone(),
            };
            state
                .paused
                .insert(session_id.to_string(), paused_session.clone());
            let pause_event = DetectEventRecord {
                session_id: session_id.to_string(),
                request_id: request_id.map(ToOwned::to_owned),
                event_type: EventType::SessionPaused,
                severity: Severity::Warn,
                detail: json!({
                    "reason": SESSION_PAUSED_LOOP_REASON,
                    "triggered_by": paused_session.triggered_by,
                    "loop_action": "pause",
                }),
                created_at: digest.timestamp,
            };
            push_recorded_event(
                &mut state.events,
                pause_event.clone(),
                self.config.max_recorded_events,
            );
            events.push(pause_event);
            paused = true;
            pause_reason = Some(SESSION_PAUSED_LOOP_REASON.to_string());
        }

        FeedResult {
            session_id: session_id.to_string(),
            alerts,
            paused,
            pause_reason,
            events,
        }
    }

    pub fn is_session_paused(&self, session_id: &str) -> bool {
        self.read_state().paused.contains_key(session_id)
    }

    pub fn paused_reason(&self, session_id: &str) -> Option<String> {
        self.read_state()
            .paused
            .get(session_id)
            .map(|paused| paused.reason.clone())
    }

    pub fn resume_session(
        &self,
        session_id: &str,
        request_id: Option<&str>,
    ) -> Option<DetectEventRecord> {
        let mut state = self.write_state();
        let paused = state.paused.remove(session_id)?;
        state.windows.remove(session_id);
        let event = DetectEventRecord {
            session_id: session_id.to_string(),
            request_id: request_id.map(ToOwned::to_owned),
            event_type: EventType::SessionResumed,
            severity: Severity::Info,
            detail: json!({
                "reason": "manual_resume",
                "previous_reason": paused.reason,
                "paused_at": paused.paused_at,
            }),
            created_at: Utc::now(),
        };
        state.active_alerts.remove(session_id);
        push_recorded_event(
            &mut state.events,
            event.clone(),
            self.config.max_recorded_events,
        );
        Some(event)
    }

    pub fn status(&self) -> DetectStatus {
        let state = self.read_state();
        let mut paused_sessions: Vec<_> = state.paused.values().cloned().collect();
        paused_sessions.sort_by_key(|session| session.paused_at);
        paused_sessions.reverse();

        let mut active_alerts: Vec<_> = state.active_alerts.values().cloned().collect();
        active_alerts.sort_by_key(|alert| alert.triggered_at);
        active_alerts.reverse();

        DetectStatus {
            paused_sessions,
            active_alerts,
        }
    }

    pub fn recorded_events(&self) -> Vec<DetectEventRecord> {
        self.read_state().events.iter().cloned().collect()
    }

    fn detect_alerts(&self, window: &[RequestDigest], current: &RequestDigest) -> Vec<DetectAlert> {
        let mut alerts = Vec::new();

        if let Some(alert) = tool_failure_repetition_alert(
            window,
            current,
            self.config.loop_threshold_similar_requests.max(1),
        ) {
            alerts.push(alert);
        }
        if let Some(alert) = content_similarity_alert(
            window,
            current,
            self.config.loop_threshold_similar_requests.max(1),
        ) {
            alerts.push(alert);
        }
        if let Some(alert) = burn_rate_alert(
            window,
            self.config.burn_rate_alert_usd_per_hour,
            self.config.min_burn_rate_observation_seconds,
        ) {
            alerts.push(alert);
        }

        alerts
    }

    fn read_state(&self) -> RwLockReadGuard<'_, DetectorState> {
        self.state.read().expect("detect state read lock poisoned")
    }

    fn write_state(&self) -> RwLockWriteGuard<'_, DetectorState> {
        self.state
            .write()
            .expect("detect state write lock poisoned")
    }
}

fn prune_window(window: &mut Vec<RequestDigest>, now: DateTime<Utc>, window_seconds: u64) {
    let keep_after = now - Duration::seconds(window_seconds.max(1) as i64);
    window.retain(|entry| entry.timestamp >= keep_after);
}

fn push_recorded_event(
    events: &mut VecDeque<DetectEventRecord>,
    event: DetectEventRecord,
    max: usize,
) {
    if max == 0 {
        events.clear();
        return;
    }

    events.push_back(event);
    while events.len() > max {
        events.pop_front();
    }
}

fn prune_inactive_sessions(state: &mut DetectorState, now: DateTime<Utc>, retention_seconds: u64) {
    let keep_after = now - Duration::seconds(retention_seconds.max(1) as i64);
    state
        .windows
        .retain(|_, window| window.last_seen >= keep_after);

    let mut stale_paused = Vec::new();
    for (session_id, paused) in &state.paused {
        if paused.paused_at < keep_after || !state.windows.contains_key(session_id) {
            stale_paused.push(session_id.clone());
        }
    }
    for session_id in stale_paused {
        state.paused.remove(&session_id);
    }

    let mut stale_alerts = Vec::new();
    for (session_id, alert) in &state.active_alerts {
        if alert.triggered_at < keep_after || !state.windows.contains_key(session_id) {
            stale_alerts.push(session_id.clone());
        }
    }
    for session_id in stale_alerts {
        state.active_alerts.remove(&session_id);
    }
}

fn cap_session_count(state: &mut DetectorState, max_sessions: usize) {
    if max_sessions == 0 {
        state.windows.clear();
        state.paused.clear();
        state.active_alerts.clear();
        return;
    }

    if state.windows.len() <= max_sessions {
        return;
    }

    let overflow = state.windows.len() - max_sessions;
    let mut candidates: Vec<_> = state
        .windows
        .iter()
        .map(|(session_id, window)| (session_id.clone(), window.last_seen))
        .collect();
    candidates.sort_unstable_by_key(|(_, last_seen)| *last_seen);

    for (evict_session_id, _) in candidates.into_iter().take(overflow) {
        state.windows.remove(&evict_session_id);
        state.paused.remove(&evict_session_id);
        state.active_alerts.remove(&evict_session_id);
    }
}

fn tool_failure_repetition_alert(
    window: &[RequestDigest],
    current: &RequestDigest,
    threshold: u32,
) -> Option<DetectAlert> {
    let tool_name = current.tool_name.as_ref()?;
    if current.tool_succeeded {
        return None;
    }

    let failure_count = window
        .iter()
        .filter(|entry| entry.tool_name.as_ref() == Some(tool_name) && !entry.tool_succeeded)
        .count() as u64;
    if failure_count < u64::from(threshold.max(1)) {
        return None;
    }

    Some(DetectAlert::ToolLoop {
        tool_name: tool_name.clone(),
        failure_count,
    })
}

fn content_similarity_alert(
    window: &[RequestDigest],
    current: &RequestDigest,
    threshold: u32,
) -> Option<DetectAlert> {
    let similar_count = window
        .iter()
        .filter(|entry| entry.content_hash == current.content_hash)
        .count() as u64;
    if similar_count < u64::from(threshold.max(1)) {
        return None;
    }

    let window_seconds = window
        .iter()
        .map(|entry| entry.timestamp)
        .min()
        .map(|min_ts| (current.timestamp - min_ts).num_seconds().max(0) as u64)
        .unwrap_or_default();

    Some(DetectAlert::ContentLoop {
        similar_count,
        window_seconds,
    })
}

fn burn_rate_alert(
    window: &[RequestDigest],
    threshold: f64,
    min_observation_seconds: u64,
) -> Option<DetectAlert> {
    if threshold <= 0.0 || window.len() < 2 {
        return None;
    }

    let first = window.first()?.timestamp;
    let last = window.last()?.timestamp;
    let elapsed_seconds = (last - first).num_seconds().max(1) as f64;
    if elapsed_seconds < min_observation_seconds.max(1) as f64 {
        return None;
    }
    let elapsed_hours = elapsed_seconds / 3600.0;
    let total_cost_usd: f64 = window.iter().map(|entry| entry.cost_usd.to_usd()).sum();
    let usd_per_hour = total_cost_usd / elapsed_hours;

    if usd_per_hour <= threshold {
        return None;
    }

    Some(DetectAlert::BurnRate {
        usd_per_hour,
        threshold,
    })
}

fn alert_to_event(alert: &DetectAlert) -> (EventType, Severity, Value) {
    match alert {
        DetectAlert::ToolLoop {
            tool_name,
            failure_count,
        } => (
            EventType::LoopDetected,
            Severity::Warn,
            json!({
                "kind": "tool_failure_repetition",
                "tool_name": tool_name,
                "failure_count": failure_count,
            }),
        ),
        DetectAlert::ContentLoop {
            similar_count,
            window_seconds,
        } => (
            EventType::LoopDetected,
            Severity::Warn,
            json!({
                "kind": "content_similarity",
                "similar_count": similar_count,
                "window_seconds": window_seconds,
            }),
        ),
        DetectAlert::BurnRate {
            usd_per_hour,
            threshold,
        } => (
            EventType::BurnRateAlert,
            Severity::Warn,
            json!({
                "kind": "burn_rate",
                "usd_per_hour": usd_per_hour,
                "threshold": threshold,
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use penny_types::Money;

    fn ts(sec: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_744_000_000 + sec, 0)
            .single()
            .expect("valid timestamp")
    }

    fn digest(
        at_sec: i64,
        cost_usd: f64,
        tool_name: Option<&str>,
        tool_succeeded: bool,
        content_hash: u64,
    ) -> RequestDigest {
        RequestDigest {
            model: "claude-sonnet-4-6".to_string(),
            input_tokens: 100,
            cost_usd: Money::from_usd(cost_usd).expect("money"),
            tool_name: tool_name.map(ToOwned::to_owned),
            tool_succeeded,
            content_hash,
            timestamp: ts(at_sec),
        }
    }

    fn config(loop_action: LoopAction) -> DetectorConfig {
        DetectorConfig {
            enabled: true,
            burn_rate_alert_usd_per_hour: 10_000.0,
            loop_window_seconds: 120,
            loop_threshold_similar_requests: 3,
            loop_action,
            min_burn_rate_observation_seconds: DEFAULT_MIN_BURN_RATE_OBSERVATION_SECONDS,
            max_recorded_events: DEFAULT_MAX_RECORDED_EVENTS,
            session_state_retention_seconds: DEFAULT_SESSION_STATE_RETENTION_SECONDS,
            max_sessions: DEFAULT_MAX_SESSIONS,
        }
    }

    #[test]
    fn tool_failure_repetition_pauses_session_and_records_events() {
        let detector = DetectEngine::new(config(LoopAction::Pause));

        let first = detector.feed(
            "sess-a",
            Some("req-1"),
            digest(0, 0.2, Some("bash"), false, 11),
        );
        assert!(first.alerts.is_empty());

        let second = detector.feed(
            "sess-a",
            Some("req-2"),
            digest(20, 0.2, Some("bash"), false, 12),
        );
        assert!(second.alerts.is_empty());

        let third = detector.feed(
            "sess-a",
            Some("req-3"),
            digest(30, 0.2, Some("bash"), false, 13),
        );
        assert_eq!(third.alerts.len(), 1);
        assert!(matches!(
            third.alerts[0],
            DetectAlert::ToolLoop {
                ref tool_name,
                failure_count: 3
            } if tool_name == "bash"
        ));
        assert!(third.paused);
        assert_eq!(
            third.pause_reason.as_deref(),
            Some(SESSION_PAUSED_LOOP_REASON)
        );
        assert!(detector.is_session_paused("sess-a"));

        let event_types: Vec<_> = third.events.iter().map(|event| &event.event_type).collect();
        assert!(event_types.contains(&&EventType::LoopDetected));
        assert!(event_types.contains(&&EventType::SessionPaused));
    }

    #[test]
    fn content_similarity_alerts_without_pausing_when_action_is_alert() {
        let detector = DetectEngine::new(config(LoopAction::Alert));
        detector.feed("sess-b", Some("req-1"), digest(0, 0.1, None, true, 42));
        detector.feed("sess-b", Some("req-2"), digest(10, 0.1, None, true, 42));
        let result = detector.feed("sess-b", Some("req-3"), digest(20, 0.1, None, true, 42));

        assert!(result.alerts.iter().any(|alert| matches!(
            alert,
            DetectAlert::ContentLoop {
                similar_count: 3,
                ..
            }
        )));
        assert!(!result.paused);
        assert!(!detector.is_session_paused("sess-b"));
    }

    #[test]
    fn burn_rate_alert_uses_configured_threshold() {
        let detector = DetectEngine::new(DetectorConfig {
            burn_rate_alert_usd_per_hour: 10.0,
            loop_threshold_similar_requests: 99,
            min_burn_rate_observation_seconds: 1,
            ..config(LoopAction::Alert)
        });
        detector.feed("sess-c", Some("req-1"), digest(0, 1.0, None, true, 1));
        let result = detector.feed("sess-c", Some("req-2"), digest(10, 1.2, None, true, 2));

        assert!(result
            .alerts
            .iter()
            .any(|alert| matches!(alert, DetectAlert::BurnRate { threshold, .. } if (*threshold - 10.0).abs() < f64::EPSILON)));
    }

    #[test]
    fn resume_session_clears_pause_and_records_resume_event() {
        let detector = DetectEngine::new(config(LoopAction::Pause));
        detector.feed(
            "sess-d",
            Some("req-1"),
            digest(0, 0.2, Some("tool"), false, 11),
        );
        detector.feed(
            "sess-d",
            Some("req-2"),
            digest(1, 0.2, Some("tool"), false, 12),
        );
        detector.feed(
            "sess-d",
            Some("req-3"),
            digest(2, 0.2, Some("tool"), false, 13),
        );
        assert!(detector.is_session_paused("sess-d"));

        let resume = detector
            .resume_session("sess-d", Some("resume-1"))
            .expect("resume event");
        assert_eq!(resume.event_type, EventType::SessionResumed);
        assert!(!detector.is_session_paused("sess-d"));
        let after_resume = detector.feed(
            "sess-d",
            Some("req-4"),
            digest(3, 0.2, Some("tool"), false, 14),
        );
        assert!(
            !after_resume.paused,
            "session should not be immediately re-paused by stale history"
        );

        let status = detector.status();
        assert!(status.paused_sessions.is_empty());
    }

    #[test]
    fn burn_rate_alert_respects_minimum_observation_window() {
        let detector = DetectEngine::new(DetectorConfig {
            burn_rate_alert_usd_per_hour: 10.0,
            loop_threshold_similar_requests: 99,
            min_burn_rate_observation_seconds: 30,
            ..config(LoopAction::Alert)
        });
        detector.feed("sess-f", Some("req-1"), digest(0, 10.0, None, true, 1));
        let result = detector.feed("sess-f", Some("req-2"), digest(10, 10.0, None, true, 2));
        assert!(
            !result
                .alerts
                .iter()
                .any(|alert| matches!(alert, DetectAlert::BurnRate { .. })),
            "burn-rate should not trigger before minimum observation window elapses"
        );
    }

    #[test]
    fn recorded_events_are_capped_to_configured_capacity() {
        let detector = DetectEngine::new(DetectorConfig {
            max_recorded_events: 2,
            loop_threshold_similar_requests: 1,
            min_burn_rate_observation_seconds: 1,
            ..config(LoopAction::Alert)
        });

        detector.feed("sess-g", Some("req-1"), digest(0, 0.1, None, true, 42));
        detector.feed("sess-g", Some("req-2"), digest(1, 0.1, None, true, 42));
        detector.feed("sess-g", Some("req-3"), digest(2, 0.1, None, true, 42));

        let events = detector.recorded_events();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn stale_sessions_are_evicted_from_in_memory_state() {
        let detector = DetectEngine::new(DetectorConfig {
            session_state_retention_seconds: 5,
            max_sessions: 128,
            loop_threshold_similar_requests: 1,
            ..config(LoopAction::Alert)
        });

        detector.feed("sess-old", Some("req-1"), digest(0, 0.1, None, true, 1));
        detector.feed("sess-new", Some("req-2"), digest(20, 0.1, None, true, 2));

        let status = detector.status();
        assert!(status
            .active_alerts
            .iter()
            .all(|alert| alert.session_id != "sess-old"));
        assert!(status
            .active_alerts
            .iter()
            .any(|alert| alert.session_id == "sess-new"));
    }

    #[test]
    fn disabled_detector_does_not_emit_alerts_or_events() {
        let detector = DetectEngine::new(DetectorConfig {
            enabled: false,
            ..config(LoopAction::Pause)
        });
        let result = detector.feed("sess-e", Some("req-1"), digest(0, 5.0, None, false, 99));
        assert!(result.alerts.is_empty());
        assert!(result.events.is_empty());
        assert!(detector.recorded_events().is_empty());
    }
}
