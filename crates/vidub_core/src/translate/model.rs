use std::collections::{HashMap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Semaphore;
use vidub_config::Provider;

use super::parse::parse_translations;
use super::prompt::{repair_user_prompt, system_prompt};

pub const ENDPOINT: &str = "https://integrate.api.nvidia.com/v1/chat/completions";
pub const OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";
pub const GEMINI_ENDPOINT_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const GEMINI_KEY_HEADER: &str = "x-goog-api-key";

#[must_use]
pub fn endpoint_for(provider: Provider, model: &str) -> String {
    match provider {
        Provider::Nvidia => ENDPOINT.to_string(),
        Provider::OpenRouter => OPENROUTER_ENDPOINT.to_string(),
        Provider::Gemini => format!("{GEMINI_ENDPOINT_BASE}/{model}:generateContent"),
    }
}

#[must_use]
pub fn attribution_headers(provider: Provider, key: &str) -> Vec<(&'static str, &str)> {
    match provider {
        Provider::Nvidia => Vec::new(),
        Provider::OpenRouter => vec![
            ("HTTP-Referer", "https://github.com/vidub"),
            ("X-Title", "ViDub"),
        ],
        Provider::Gemini => vec![(GEMINI_KEY_HEADER, key)],
    }
}

#[must_use]
pub fn request_body(provider: Provider, model: &str, system: &str, user: &str) -> Value {
    let cap = cap_for(provider, model);
    match provider {
        Provider::Nvidia | Provider::OpenRouter => serde_json::json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "temperature": 0.3,
            "max_tokens": cap,
        }),
        Provider::Gemini => serde_json::json!({
            "contents": [{
                "role": "user",
                "parts": [{"text": format!("{system}\n\n{user}")}]
            }],
            "generationConfig": {
                "temperature": 0.3,
                "maxOutputTokens": cap,
                "responseMimeType": "application/json",
            },
        }),
    }
}

#[must_use]
pub fn response_content(provider: Provider, v: &Value) -> Option<String> {
    match provider {
        Provider::Nvidia | Provider::OpenRouter => v
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::to_string),
        Provider::Gemini => {
            let parts = v.pointer("/candidates/0/content/parts")?.as_array()?;
            let mut out = String::new();
            for p in parts {
                if let Some(t) = p.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out)
            }
        }
    }
}

pub const COMPLETION_TOKEN_CAP: u32 = 1200;
const RPM_LIMIT: f64 = 35.0;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ATTEMPTS: u32 = 3;
const MAX_REPAIRS: u32 = 2;
const BREAKER_FAIL_THRESHOLD: usize = 3;
const BREAKER_WINDOW: Duration = Duration::from_secs(60);
const BREAKER_PROBE_AFTER: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(super) struct RpmBucket {
    tokens: f64,
    last: Instant,
}

impl RpmBucket {
    fn new(now: Instant) -> Self {
        Self {
            tokens: RPM_LIMIT,
            last: now,
        }
    }

    fn wait_dur(&mut self, now: Instant) -> Duration {
        if now > self.last {
            let elapsed = now.duration_since(self.last).as_secs_f64();
            self.tokens = (self.tokens + elapsed * RPM_LIMIT / 60.0).min(RPM_LIMIT);
            self.last = now;
        }
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Duration::ZERO
        } else {
            let need = (1.0 - self.tokens) * 60.0 / RPM_LIMIT;
            Duration::from_secs_f64(need.max(0.01))
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug)]
pub(super) struct Breaker {
    state: BreakerState,
    failures: VecDeque<Instant>,
    opened_at: Option<Instant>,
    probe_in_flight: bool,
}

impl Breaker {
    pub(super) fn new() -> Self {
        Self {
            state: BreakerState::Closed,
            failures: VecDeque::new(),
            opened_at: None,
            probe_in_flight: false,
        }
    }

    pub(super) fn allow(&mut self, now: Instant) -> bool {
        match self.state {
            BreakerState::Closed => true,
            BreakerState::Open => match self.opened_at {
                Some(t) if now.duration_since(t) >= BREAKER_PROBE_AFTER => {
                    self.state = BreakerState::HalfOpen;
                    self.probe_in_flight = false;
                    self.allow(now)
                }
                _ => false,
            },
            BreakerState::HalfOpen => {
                if self.probe_in_flight {
                    false
                } else {
                    self.probe_in_flight = true;
                    true
                }
            }
        }
    }

    pub(super) fn record_success(&mut self) {
        self.state = BreakerState::Closed;
        self.failures.clear();
        self.opened_at = None;
        self.probe_in_flight = false;
    }

    pub(super) fn record_failure(&mut self, now: Instant) {
        match self.state {
            BreakerState::HalfOpen => {
                self.state = BreakerState::Open;
                self.opened_at = Some(now);
                self.probe_in_flight = false;
                self.failures.clear();
            }
            BreakerState::Open => {}
            BreakerState::Closed => {
                self.failures.push_back(now);
                while let Some(f) = self.failures.front() {
                    if now.duration_since(*f) > BREAKER_WINDOW {
                        self.failures.pop_front();
                    } else {
                        break;
                    }
                }
                if self.failures.len() >= BREAKER_FAIL_THRESHOLD {
                    self.state = BreakerState::Open;
                    self.opened_at = Some(now);
                    self.failures.clear();
                }
            }
        }
    }
}

pub(super) enum Attempt {
    Content {
        body: String,
        model: String,
    },
    Auth,
    Retryable {
        msg: String,
        retry_after: Option<Duration>,
    },
    ModelError(String),
}

pub(super) enum ChainOutcome {
    Content { body: String, model: String },
    Auth,
    Unavailable(String),
}

pub(super) struct Engine {
    pub(super) client: reqwest::Client,
    pub(super) key: String,
    pub(super) primary: String,
    pub(super) models: Vec<String>,
    pub(super) riva: Option<String>,
    pub(super) buckets: Mutex<HashMap<String, RpmBucket>>,
    pub(super) breaker: Mutex<Breaker>,
    pub(super) auth_failed: AtomicBool,
    pub(super) sem: Semaphore,
    pub(super) src_lang: String,
    pub(super) provider: Provider,
}

pub(super) fn is_riva(model: &str) -> bool {
    model.contains("riva-translate")
}

pub(super) fn model_cap(model: &str) -> u32 {
    if model.contains("mistral-large-4") {
        6000
    } else if model.contains("gpt-oss") || model.contains("minimax") {
        2000
    } else if model.contains("bolt") {
        800
    } else if model.contains("muse-glimmer") || model.contains("nemotron") {
        1500
    } else if model.contains("riva-translate") {
        300
    } else if model.contains("gemma") {
        800
    } else {
        COMPLETION_TOKEN_CAP
    }
}

fn openrouter_cap(model: &str) -> u32 {
    const OPENROUTER_CAP: u32 = 1000;
    if model.contains("thinking") || model.contains("reasoning") {
        return COMPLETION_TOKEN_CAP;
    }
    if model.contains("gemini") || model.contains("claude") || model.contains("gpt-4o") {
        OPENROUTER_CAP
    } else {
        COMPLETION_TOKEN_CAP
    }
}

fn gemini_cap() -> u32 {
    2000
}

pub(super) fn cap_for(provider: Provider, model: &str) -> u32 {
    match provider {
        Provider::Nvidia => model_cap(model),
        Provider::OpenRouter => openrouter_cap(model),
        Provider::Gemini => gemini_cap(),
    }
}

fn parse_retry_after(h: &str) -> Option<Duration> {
    h.trim().parse::<u64>().ok().map(Duration::from_secs)
}

pub(super) fn backoff_delay(attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(ra) = retry_after {
        return ra.min(MAX_BACKOFF);
    }
    let ms = 500u64.saturating_mul(1u64 << attempt.min(6));
    Duration::from_millis(ms.min(30_000))
}

pub(super) async fn rpm_acquire(engine: &Engine, model: &str) {
    loop {
        let wait = {
            let mut buckets = engine.buckets.lock().expect("bucket mutex");
            let bucket = buckets
                .entry(model.to_string())
                .or_insert_with(|| RpmBucket::new(Instant::now()));
            bucket.wait_dur(Instant::now())
        };
        if wait.is_zero() {
            return;
        }
        tokio::time::sleep(wait).await;
    }
}

pub(super) async fn post_once(engine: &Engine, model: &str, system: &str, user: &str) -> Attempt {
    rpm_acquire(engine, model).await;
    let body = request_body(engine.provider, model, system, user);
    let mut req = engine
        .client
        .post(endpoint_for(engine.provider, model))
        .timeout(HTTP_TIMEOUT);
    if engine.provider != Provider::Gemini {
        req = req.bearer_auth(&engine.key);
    }
    for (name, value) in attribution_headers(engine.provider, &engine.key) {
        req = req.header(name, value);
    }
    let resp = req.json(&body).send().await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            return Attempt::Retryable {
                msg: format!("request error: {e}"),
                retry_after: None,
            }
        }
    };
    let status = resp.status();
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|h| h.to_str().ok())
        .and_then(parse_retry_after);
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Attempt::Auth;
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Attempt::Retryable {
            msg: format!("HTTP {status}"),
            retry_after,
        };
    }
    if !status.is_success() {
        return Attempt::ModelError(format!("HTTP {status}"));
    }
    let text = match resp.text().await {
        Ok(t) => t,
        Err(e) => {
            return Attempt::Retryable {
                msg: format!("read body: {e}"),
                retry_after: None,
            }
        }
    };
    let v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return Attempt::Retryable {
                msg: format!("invalid response JSON: {e}"),
                retry_after: None,
            }
        }
    };
    if let Some(err) = v.get("error") {
        return Attempt::ModelError(format!("model error: {err}"));
    }
    if let Some(feedback) = v
        .pointer("/promptFeedback/blockReason")
        .and_then(Value::as_str)
    {
        return Attempt::ModelError(format!("gemini blocked: {feedback}"));
    }
    match response_content(engine.provider, &v) {
        Some(content) => Attempt::Content {
            body: content,
            model: model.to_string(),
        },
        None => Attempt::ModelError("missing model content".to_string()),
    }
}

pub(super) async fn call_chain(engine: &Engine, system: &str, user: &str) -> ChainOutcome {
    let mut last = String::new();
    for model in &engine.models {
        let mut retry_after = None;
        for attempt in 1..=MAX_ATTEMPTS {
            if attempt > 1 {
                let delay = backoff_delay(attempt - 1, retry_after);
                tokio::time::sleep(delay).await;
                retry_after = None;
            }
            match post_once(engine, model, system, user).await {
                Attempt::Content { body, model: used } => {
                    return ChainOutcome::Content { body, model: used }
                }
                Attempt::Auth => return ChainOutcome::Auth,
                Attempt::Retryable {
                    msg,
                    retry_after: ra,
                } => {
                    last = msg;
                    if attempt < MAX_ATTEMPTS {
                        retry_after = ra;
                    }
                }
                Attempt::ModelError(msg) => {
                    last = msg;
                    break; // next model
                }
            }
        }
    }
    ChainOutcome::Unavailable(last)
}

pub(super) fn note_model_fallback(warnings: &mut Vec<String>, engine: &Engine, used: &str) {
    if used != engine.primary {
        warnings.push(format!("translate: NIM fallback model {used}"));
    }
}

pub(super) async fn parse_with_repairs(
    engine: &Engine,
    first: String,
    expected: &[u32],
    warnings: &mut Vec<String>,
) -> Result<Vec<(u32, String)>, String> {
    let mut raw = first;
    let mut err = parse_translations(&raw, expected).err();
    if err.is_none() {
        return parse_translations(&raw, expected);
    }
    for _ in 0..MAX_REPAIRS {
        warnings.push("translate: repair re-prompt for malformed JSON".to_string());
        let user = repair_user_prompt(&raw, err.as_deref().unwrap_or("unparseable"), expected);
        match call_chain(engine, system_prompt(), &user).await {
            ChainOutcome::Content { body, model: used } => {
                note_model_fallback(warnings, engine, &used);
                raw = body;
                err = parse_translations(&raw, expected).err();
                if err.is_none() {
                    return parse_translations(&raw, expected);
                }
            }
            ChainOutcome::Auth => {
                engine.auth_failed.store(true, Ordering::SeqCst);
                return Err("auth".to_string());
            }
            ChainOutcome::Unavailable(msg) => {
                return Err(format!("repair: all models unavailable ({msg})"));
            }
        }
    }
    Err(err.unwrap_or_else(|| "unparseable model output".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_three_fails_open_probe_halfopen() {
        let t0 = Instant::now();
        let mut br = Breaker::new();
        assert!(br.allow(t0));
        br.record_failure(t0);
        br.record_failure(t0);
        assert!(br.allow(t0), "2 fails still closed");
        br.record_failure(t0);
        assert!(!br.allow(t0), "3rd fail opens");
        assert!(
            !br.allow(t0 + Duration::from_secs(30)),
            "still open at +30s"
        );
        let probe_at = t0 + Duration::from_secs(60);
        assert!(br.allow(probe_at), "probe allowed at +60s");
        assert!(
            !br.allow(probe_at),
            "second probe blocked while probe in flight"
        );
        br.record_failure(probe_at);
        assert!(!br.allow(probe_at + Duration::from_secs(10)), "re-opened");
        let probe2 = probe_at + Duration::from_secs(60);
        assert!(br.allow(probe2), "second probe allowed");
        br.record_success();
        assert!(br.allow(probe2), "closed after probe success");
        br.record_failure(probe2);
        br.record_failure(probe2);
        assert!(br.allow(probe2), "failures were cleared");
    }

    #[test]
    fn rpm_bucket_bursts_35_then_waits() {
        let t0 = Instant::now();
        let mut bucket = RpmBucket::new(t0);
        for _ in 0..35 {
            assert_eq!(bucket.wait_dur(t0), Duration::ZERO);
        }
        assert!(bucket.wait_dur(t0) > Duration::ZERO, "36th waits");
        assert_eq!(
            bucket.wait_dur(t0 + Duration::from_secs(60)),
            Duration::ZERO
        );
    }

    #[test]
    fn model_caps_match_live_budgets() {
        assert_eq!(model_cap("mistralai/mistral-large-4-1t-a52b"), 6000);
        assert_eq!(model_cap("openai/gpt-oss-20b"), 2000);
        assert_eq!(model_cap("minimaxai/minimax-m3.1-flash"), 2000);
        assert_eq!(model_cap("meta/muse-glimmer-30b"), 1500);
        assert_eq!(model_cap("nvidia/nemotron-3-ultra-550b-a55b"), 1500);
        assert_eq!(model_cap("google/gemma-4-31b-it"), 800);
        assert_eq!(model_cap("nvidia/nemotron-3.5-bolt-e4b"), 800);
        assert_eq!(model_cap("nvidia/riva-translate-4b-instruct-v2"), 300);
        assert_eq!(model_cap("some/future-model"), COMPLETION_TOKEN_CAP);
        assert!(is_riva("nvidia/riva-translate-4b-instruct-v2"));
        assert!(!is_riva("google/gemma-4-31b-it"));
    }

    #[test]
    fn endpoints_per_provider() {
        assert_eq!(endpoint_for(Provider::Nvidia, "any/model"), ENDPOINT);
        assert_eq!(
            endpoint_for(Provider::OpenRouter, "any/model"),
            "https://openrouter.ai/api/v1/chat/completions"
        );
        let g = endpoint_for(Provider::Gemini, "gemini-2.5-flash");
        assert_eq!(
            g,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent"
        );
        assert!(g.contains("gemini-2.5-flash"), "{g}");
        assert!(g.ends_with(":generateContent"), "{g}");
    }

    #[test]
    fn headers_per_provider() {
        assert!(attribution_headers(Provider::Nvidia, "nvapi-x").is_empty());
        let or = attribution_headers(Provider::OpenRouter, "sk-or");
        assert_eq!(or.len(), 2, "{or:?}");
        assert!(or.contains(&("HTTP-Referer", "https://github.com/vidub")));
        assert!(or.contains(&("X-Title", "ViDub")));
        assert_eq!(
            attribution_headers(Provider::Gemini, "g-key"),
            vec![("x-goog-api-key", "g-key")]
        );
    }

    #[test]
    fn request_body_openai_shape_for_nvidia_and_openrouter() {
        for p in [Provider::Nvidia, Provider::OpenRouter] {
            let b = request_body(p, "m/one", "SYS", "USR");
            assert_eq!(b["model"], "m/one");
            assert_eq!(b["messages"][0]["role"], "system");
            assert_eq!(b["messages"][0]["content"], "SYS");
            assert_eq!(b["messages"][1]["role"], "user");
            assert_eq!(b["messages"][1]["content"], "USR");
            assert_eq!(b["temperature"], 0.3);
            assert!(b.get("max_tokens").is_some(), "{p:?}: {b}");
            assert!(b.get("contents").is_none());
        }
        assert_eq!(
            request_body(Provider::Nvidia, "nvidia/nemotron-3.5-bolt-e4b", "S", "U")["max_tokens"],
            800
        );
        assert_eq!(
            request_body(Provider::OpenRouter, "google/gemini-2.5-flash", "S", "U")["max_tokens"],
            1000
        );
    }

    #[test]
    fn request_body_gemini_shape() {
        let b = request_body(Provider::Gemini, "gemini-2.5-flash", "SYS", "USR");
        assert!(b.get("model").is_none());
        assert_eq!(b["contents"][0]["role"], "user");
        assert_eq!(b["contents"][0]["parts"][0]["text"], "SYS\n\nUSR");
        assert_eq!(b["generationConfig"]["temperature"], 0.3);
        assert_eq!(b["generationConfig"]["maxOutputTokens"], 2000);
        assert_eq!(
            b["generationConfig"]["responseMimeType"],
            "application/json"
        );
        assert!(b.get("messages").is_none());
        assert_eq!(cap_for(Provider::Gemini, "gemini-2.5-pro"), 2000);
    }

    #[test]
    fn response_content_per_provider() {
        let openai: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"role":"assistant","content":"[{\"id\":1}]"}}]}"#,
        )
        .expect("json");
        for p in [Provider::Nvidia, Provider::OpenRouter] {
            assert_eq!(
                response_content(p, &openai).as_deref(),
                Some("[{\"id\":1}]")
            );
        }

        let gem: Value = serde_json::from_str(
            r#"{"candidates":[{"content":{"role":"model","parts":[
                {"text":"[{\"id\":1,"},{"text":"\"vi\":\"xin chào\"}]"}
            ]}}]}"#,
        )
        .expect("json");
        assert_eq!(
            response_content(Provider::Gemini, &gem).as_deref(),
            Some("[{\"id\":1,\"vi\":\"xin chào\"}]")
        );

        assert!(response_content(Provider::Gemini, &openai).is_none());
        assert!(response_content(Provider::Nvidia, &gem).is_none());
        let empty: Value =
            serde_json::from_str(r#"{"candidates":[{"content":{"parts":[]}}]}"#).expect("json");
        assert!(response_content(Provider::Gemini, &empty).is_none());
    }

    #[test]
    fn openrouter_and_gemini_caps() {
        assert_eq!(
            cap_for(Provider::OpenRouter, "google/gemini-2.5-flash"),
            1000
        );
        assert_eq!(
            cap_for(Provider::OpenRouter, "anthropic/claude-sonnet-4"),
            1000
        );
        assert_eq!(cap_for(Provider::OpenRouter, "openai/gpt-4o-mini"), 1000);
        assert_eq!(
            cap_for(Provider::OpenRouter, "some/thinking-model"),
            COMPLETION_TOKEN_CAP
        );
        assert_eq!(
            cap_for(Provider::OpenRouter, "some/reasoning-model"),
            COMPLETION_TOKEN_CAP
        );
        assert_eq!(
            cap_for(Provider::OpenRouter, "meta-llama/llama-3.3-70b-instruct"),
            COMPLETION_TOKEN_CAP
        );
        assert_eq!(
            cap_for(Provider::Nvidia, "nvidia/riva-translate-4b-instruct-v2"),
            300
        );
        assert_eq!(cap_for(Provider::Nvidia, "google/gemma-4-31b-it"), 800);
    }
}
