//! Language/track selection: primary-subtag matching, Vietnamese detection,
//! and the manual-before-auto tiered caption pick.

use serde_json::Value;

use super::{CaptionTrack, ProbeError, ProbeInfo};
use crate::transcript::probe::args::is_excluded_lang;

/// Primary subtag (`"en-US"` → `"en"`, `"zh_Hans"` → `"zh"`), lowercased.
fn primary_subtag(lang: &str) -> String {
    lang.split(['-', '_'])
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase()
}

fn is_english(lang: &str) -> bool {
    primary_subtag(lang) == "en"
}

/// True for any Vietnamese track tag (`vi`, `vi-VN`, `vi_HN`, `vi-orig`).
pub fn is_vietnamese(lang: &str) -> bool {
    primary_subtag(lang) == "vi"
}

/// A manual Vietnamese caption track, if the video has one: its text is
/// already the target language, so it needs no translation.
pub fn vietnamese_manual_track(info: &ProbeInfo) -> Option<&CaptionTrack> {
    best_among(
        info.manual
            .iter()
            .filter(|track| is_vietnamese(&track.lang) && is_usable_text_ext(&track.ext)),
    )
}

/// Captions that may be used as-is without translation. Machine translations
/// (`*-orig`, `srv*`, `ttml`) re-translate badly, so only real text tracks
/// (`srt`/`vtt`) qualify.
fn is_usable_text_ext(ext: &str) -> bool {
    matches!(ext, "srt" | "vtt")
}

fn same_primary(a: &str, b: &str) -> bool {
    primary_subtag(a) == primary_subtag(b)
}

/// Format preference within a tier: `srt` > `vtt` > `json3` > anything else.
fn ext_rank(ext: &str) -> u8 {
    match ext.to_lowercase().as_str() {
        "srt" => 0,
        "vtt" => 1,
        "json3" => 2,
        _ => 3,
    }
}

/// Pick the best track among `candidates` (deterministic total order:
/// ext rank, then lang, ext, url — so `select` never depends on JSON key order).
fn best_among<'a>(candidates: impl Iterator<Item = &'a CaptionTrack>) -> Option<&'a CaptionTrack> {
    candidates.min_by(|a, b| {
        ext_rank(&a.ext)
            .cmp(&ext_rank(&b.ext))
            .then_with(|| a.lang.cmp(&b.lang))
            .then_with(|| a.ext.cmp(&b.ext))
            .then_with(|| a.url.cmp(&b.url))
    })
}

/// Tiered fallback: source language > `en*` > any (skipping excluded langs).
fn select_tier<'a>(tracks: &'a [CaptionTrack], src_lang: Option<&str>) -> Option<&'a CaptionTrack> {
    let candidates = tracks.iter().filter(|t| !is_excluded_lang(&t.lang));
    // Collect a snapshot (borrowed) so the three passes share one allocation.
    let pool: Vec<&CaptionTrack> = candidates.collect();
    if pool.is_empty() {
        return None;
    }
    if let Some(src) = src_lang.filter(|s| !s.trim().is_empty()) {
        if let Some(best) = best_among(pool.iter().copied().filter(|t| same_primary(&t.lang, src)))
        {
            return Some(best);
        }
    }
    if let Some(best) = best_among(pool.iter().copied().filter(|t| is_english(&t.lang))) {
        return Some(best);
    }
    best_among(pool.into_iter())
}

/// Choose the preferred caption: manual tier first, then automatic.
/// Within a tier: source language > `en*` > any. NEVER `danmaku`/`live_chat`.
/// `None` means "no captions — caller falls back to Whisper".
pub fn select(info: &ProbeInfo) -> Option<&CaptionTrack> {
    select_tier(&info.manual, info.src_lang.as_deref())
        .or_else(|| select_tier(&info.auto_caps, info.src_lang.as_deref()))
}

/// Collect `(lang, ext, url)` tracks from one `-J` subtitle map
/// (`lang -> [{ext, url}]` array shape, or `lang -> {ext, url}` object shape
/// as used by `requested_subtitles`). Excluded langs are dropped here AND in
/// `select` (defence in depth).
fn collect_tracks(map: &serde_json::Map<String, Value>, out: &mut Vec<CaptionTrack>) {
    for (lang, formats) in map {
        if is_excluded_lang(lang) {
            continue;
        }
        let push = |entry: &Value, out: &mut Vec<CaptionTrack>| {
            let ext = entry
                .get("ext")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let url = entry
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if !ext.is_empty() && !url.is_empty() {
                out.push(CaptionTrack {
                    lang: lang.clone(),
                    ext: ext.to_lowercase(),
                    url: url.to_string(),
                });
            }
        };
        match formats {
            Value::Array(entries) => {
                for entry in entries {
                    push(entry, out);
                }
            }
            Value::Object(_) => push(formats, out),
            _ => {}
        }
    }
}

/// Parse `yt-dlp -J` stdout into [`ProbeInfo`]. Pure (no IO).
pub fn parse_probe_json(json: &str) -> Result<ProbeInfo, ProbeError> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| ProbeError::InvalidJson(e.to_string()))?;
    let title = v
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let src_lang = v
        .get("original_language")
        .and_then(Value::as_str)
        .or_else(|| v.get("language").and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let mut manual = Vec::new();
    if let Some(Value::Object(m)) = v.get("subtitles") {
        collect_tracks(m, &mut manual);
    }
    // `requested_subtitles` merges into manual (dedup) so a probe that only
    // reports requested langs still yields tracks.
    if let Some(Value::Object(m)) = v.get("requested_subtitles") {
        let mut extra = Vec::new();
        collect_tracks(m, &mut extra);
        for track in extra {
            if !manual.contains(&track) {
                manual.push(track);
            }
        }
    }
    let mut auto_caps = Vec::new();
    if let Some(Value::Object(m)) = v.get("automatic_captions") {
        collect_tracks(m, &mut auto_caps);
    }
    Ok(ProbeInfo {
        title,
        src_lang,
        manual,
        auto_caps,
    })
}
