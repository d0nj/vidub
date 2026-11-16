use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Semaphore};

const GTX_BASE: &str = "https://translate.googleapis.com/translate_a/single";
const MAX_CONCURRENCY: usize = 3;
const MIN_INTERVAL: Duration = Duration::from_millis(200);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_DELAY: Duration = Duration::from_secs(2);

pub async fn translate_gtx(
    client: &reqwest::Client,
    texts: &[(u32, String)],
    src_lang: &str,
) -> Vec<(u32, Option<String>)> {
    if texts.is_empty() {
        return Vec::new();
    }
    let sl = gtx_lang(src_lang);
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENCY));
    let gate = Arc::new(Mutex::new(Instant::now()));
    let mut handles = Vec::with_capacity(texts.len());
    for (id, text) in texts {
        let client = client.clone();
        let sl = sl.clone();
        let text = text.clone();
        let semaphore = semaphore.clone();
        let gate = gate.clone();
        let id = *id;
        handles.push(tokio::spawn(async move {
            let out = translate_one(&client, &sl, &text, &semaphore, &gate).await;
            (id, out)
        }));
    }
    let mut out = Vec::with_capacity(handles.len());
    for (idx, handle) in handles.into_iter().enumerate() {
        match handle.await {
            Ok(pair) => out.push(pair),
            Err(err) => {
                let id = texts[idx].0;
                tracing::warn!(task_id = id, error = %err, "gtx: translation task failed");
                out.push((id, None));
            }
        }
    }
    out
}

async fn translate_one(
    client: &reqwest::Client,
    sl: &str,
    text: &str,
    semaphore: &Arc<Semaphore>,
    gate: &Arc<Mutex<Instant>>,
) -> Option<String> {
    for attempt in 0..2 {
        let permit = match semaphore.acquire().await {
            Ok(permit) => permit,
            Err(err) => {
                tracing::warn!(error = %err, "gtx: semaphore closed");
                return None;
            }
        };
        wait_for_slot(gate).await;
        let url = match build_gtx_url(sl, text) {
            Some(url) => url,
            None => {
                tracing::warn!("gtx: failed to build request URL");
                return None;
            }
        };
        let retry = match client.get(url).timeout(REQUEST_TIMEOUT).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    match resp.text().await {
                        Ok(body) => match parse_gtx_body(&body) {
                            Some(translated) => return Some(translated),
                            None => {
                                tracing::warn!(?status, "gtx: unparseable or empty response body");
                                return None;
                            }
                        },
                        Err(err) => {
                            tracing::warn!(error = %err, "gtx: failed to read response body");
                            return None;
                        }
                    }
                } else if status.as_u16() == 429 || status.is_server_error() {
                    tracing::warn!(?status, attempt, "gtx: retryable status");
                    true
                } else {
                    tracing::warn!(?status, "gtx: non-retryable status");
                    return None;
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "gtx: request failed");
                return None;
            }
        };
        drop(permit);
        if retry && attempt == 0 {
            tokio::time::sleep(RETRY_DELAY).await;
            continue;
        }
        return None;
    }
    None
}

async fn wait_for_slot(gate: &Mutex<Instant>) {
    let slot = {
        let mut next = gate.lock().await;
        let now = Instant::now();
        let slot = if *next > now { *next } else { now };
        *next = slot + MIN_INTERVAL;
        slot
    };
    let wait = slot.saturating_duration_since(Instant::now());
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
}

fn gtx_lang(src_lang: &str) -> String {
    let normalized = src_lang.trim().replace('_', "-");
    let lower = normalized.to_ascii_lowercase();
    if lower.is_empty()
        || matches!(
            lower.as_str(),
            "unknown" | "und" | "auto" | "multi" | "none" | "null"
        )
    {
        return "auto".to_string();
    }
    if lower == "zh" || lower.starts_with("zh-") {
        if lower.contains("hant")
            || lower.ends_with("-tw")
            || lower.ends_with("-hk")
            || lower.ends_with("-mo")
        {
            return "zh-TW".to_string();
        }
        return "zh-CN".to_string();
    }
    let primary = lower.split('-').next().unwrap_or("");
    let valid = (primary.len() == 2 || primary.len() == 3)
        && primary.bytes().all(|b| b.is_ascii_alphabetic());
    if valid {
        primary.to_string()
    } else {
        "auto".to_string()
    }
}

fn build_gtx_url(src_lang: &str, text: &str) -> Option<reqwest::Url> {
    let mut url = reqwest::Url::parse(GTX_BASE).ok()?;
    url.query_pairs_mut()
        .append_pair("client", "gtx")
        .append_pair("sl", &gtx_lang(src_lang))
        .append_pair("tl", "vi")
        .append_pair("dt", "t")
        .append_pair("q", text);
    Some(url)
}

fn parse_gtx_body(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    let chunks = value.get(0)?.as_array()?;
    let mut out = String::new();
    for chunk in chunks {
        if let Some(part) = chunk.get(0).and_then(|p| p.as_str()) {
            out.push_str(part);
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_param(url: &reqwest::Url, key: &str) -> Option<String> {
        url.query_pairs()
            .find(|(k, _)| k.as_ref() == key)
            .map(|(_, v)| v.into_owned())
    }

    #[test]
    fn parse_normal() {
        let body = r#"[[["Xin chào","Hello",null,null,10]],null,"en"]"#;
        assert_eq!(parse_gtx_body(body).as_deref(), Some("Xin chào"));
    }

    #[test]
    fn parse_multi_part_concatenated() {
        let body = r#"[[["Phần một ","Part one ",null],["Phần hai","Part two ",null]],null,"en"]"#;
        assert_eq!(parse_gtx_body(body).as_deref(), Some("Phần một Phần hai"));
    }

    #[test]
    fn parse_empty() {
        assert_eq!(parse_gtx_body(""), None);
        assert_eq!(parse_gtx_body("[]"), None);
        assert_eq!(parse_gtx_body("[null]"), None);
        assert_eq!(parse_gtx_body(r#"[[["","Hello",null]]]"#), None);
        assert_eq!(parse_gtx_body(r#"[[[null,"Hello",null]]]"#), None);
    }

    #[test]
    fn parse_error_shape() {
        assert_eq!(parse_gtx_body("<html><body>error</body></html>"), None);
        assert_eq!(parse_gtx_body(r#"{"error":"bad request"}"#), None);
        assert_eq!(parse_gtx_body("null"), None);
    }

    #[test]
    fn lang_mapping() {
        assert_eq!(gtx_lang("en-US"), "en");
        assert_eq!(gtx_lang("en"), "en");
        assert_eq!(gtx_lang("ja"), "ja");
        assert_eq!(gtx_lang("zh-CN"), "zh-CN");
        assert_eq!(gtx_lang("zh-Hans"), "zh-CN");
        assert_eq!(gtx_lang("zh"), "zh-CN");
        assert_eq!(gtx_lang("zh-TW"), "zh-TW");
        assert_eq!(gtx_lang("zh-Hant-TW"), "zh-TW");
        assert_eq!(gtx_lang("pt_BR"), "pt");
        assert_eq!(gtx_lang("unknown"), "auto");
        assert_eq!(gtx_lang("multi"), "auto");
        assert_eq!(gtx_lang("und"), "auto");
        assert_eq!(gtx_lang(""), "auto");
        assert_eq!(gtx_lang("English"), "auto");
    }

    #[test]
    fn url_encoding_vietnamese_and_special_chars() {
        let text = "Xin chào & tạm biệt = 100% + <ok>";
        let url = build_gtx_url("en-US", text).expect("static endpoint must parse");
        assert_eq!(query_param(&url, "client").as_deref(), Some("gtx"));
        assert_eq!(query_param(&url, "sl").as_deref(), Some("en"));
        assert_eq!(query_param(&url, "tl").as_deref(), Some("vi"));
        assert_eq!(query_param(&url, "dt").as_deref(), Some("t"));
        assert_eq!(query_param(&url, "q").as_deref(), Some(text));
        let raw = url.as_str();
        assert!(raw.contains("%C3%A0"), "à must be percent-encoded: {raw}");
        assert!(
            !raw.contains("Xin chào"),
            "raw must not contain unescaped UTF-8: {raw}"
        );
        assert!(raw.contains("%26"), "& must be percent-encoded: {raw}");
    }

    #[tokio::test]
    async fn empty_input_returns_empty_without_network() {
        let client = reqwest::Client::new();
        let out = translate_gtx(&client, &[], "en-US").await;
        assert!(out.is_empty());
    }
}
