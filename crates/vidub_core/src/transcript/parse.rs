//! Caption parsers: SRT / WebVTT (via `subtp`), YouTube JSON3, Bilibili JSON.
//!
//! Every format normalizes into the same [`TranscriptSegment`] shape:
//! sorted by start, zero-length/empty dropped, overlapping soft-merged,
//! `id` sequential in timeline order, `confidence = None`. The track
//! language is stamped from the `src_lang` argument so acquisition stays
//! format-agnostic.

use std::path::Path;

use crate::segment::TranscriptSegment;

/// Canonical format names used as [`TranscriptDocument::caption_format`](crate::segment::TranscriptDocument::caption_format).
pub const FORMAT_SRT: &str = "srt";
pub const FORMAT_VTT: &str = "vtt";
pub const FORMAT_JSON3: &str = "json3";
pub const FORMAT_BILIBILI: &str = "bilibili";

/// Raw `(start_ms, end_ms, text)` triple before normalization.
type Raw = Vec<(u64, u64, String)>;

fn known_format(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "srt" => Some(FORMAT_SRT),
        "vtt" | "webvtt" => Some(FORMAT_VTT),
        "json3" | "json" => Some(FORMAT_JSON3),
        "bilibili" => Some(FORMAT_BILIBILI),
        _ => None,
    }
}

pub fn parse_captions(
    text: &str,
    format: &str,
    src_lang: Option<&str>,
) -> anyhow::Result<Vec<TranscriptSegment>> {
    let fmt = match known_format(format) {
        Some(fmt) => fmt,
        None if format.trim().eq_ignore_ascii_case("auto") => sniff_format(text)?,
        None => anyhow::bail!(
            "unsupported caption format {:?}",
            format.trim().to_ascii_lowercase()
        ),
    };
    let raw = match fmt {
        FORMAT_SRT => parse_srt_raw(text)?,
        FORMAT_VTT => parse_vtt_raw(text)?,
        FORMAT_JSON3 => parse_json3_raw(text)?,
        _ => parse_bilibili_raw(text)?,
    };
    Ok(normalize(raw, src_lang))
}

/// Parse a caption file, inferring the format from its extension
/// (`auto` sniff for unknown extensions). Returns segments plus the
/// canonical format name.
pub fn parse_caption_file(
    path: &Path,
    src_lang: Option<&str>,
) -> anyhow::Result<(Vec<TranscriptSegment>, String)> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
    let fmt = canonical_format(
        path.extension().and_then(|e| e.to_str()).unwrap_or("auto"),
        &text,
    )?;
    let segments = parse_captions(&text, fmt, src_lang)?;
    Ok((segments, fmt.to_string()))
}

/// Parse SRT text into normalized segments.
pub fn parse_srt(text: &str, src_lang: Option<&str>) -> anyhow::Result<Vec<TranscriptSegment>> {
    Ok(normalize(parse_srt_raw(text)?, src_lang))
}

/// Parse WebVTT text into normalized segments (cues only; comments, styles
/// and regions are not speech).
pub fn parse_vtt(text: &str, src_lang: Option<&str>) -> anyhow::Result<Vec<TranscriptSegment>> {
    Ok(normalize(parse_vtt_raw(text)?, src_lang))
}

/// Parse YouTube JSON3 text into normalized segments.
pub fn parse_json3(text: &str, src_lang: Option<&str>) -> anyhow::Result<Vec<TranscriptSegment>> {
    Ok(normalize(parse_json3_raw(text)?, src_lang))
}

/// Parse Bilibili JSON text into normalized segments.
pub fn parse_bilibili(
    text: &str,
    src_lang: Option<&str>,
) -> anyhow::Result<Vec<TranscriptSegment>> {
    Ok(normalize(parse_bilibili_raw(text)?, src_lang))
}

pub fn canonical_format(ext: &str, text: &str) -> anyhow::Result<&'static str> {
    match known_format(ext) {
        Some(fmt) => Ok(fmt),
        None => sniff_format(text),
    }
}

fn parse_srt_raw(text: &str) -> anyhow::Result<Raw> {
    let srt = subtp::srt::SubRip::parse(strip_bom(text))
        .map_err(|e| anyhow::anyhow!("SRT parse: {e}"))?;
    Ok(srt
        .subtitles
        .iter()
        .map(|s| (srt_ms(&s.start), srt_ms(&s.end), s.text.join(" ")))
        .collect())
}

fn srt_ms(ts: &subtp::srt::SrtTimestamp) -> u64 {
    ts.hours as u64 * 3_600_000
        + ts.minutes as u64 * 60_000
        + ts.seconds as u64 * 1_000
        + ts.milliseconds as u64
}

fn parse_vtt_raw(text: &str) -> anyhow::Result<Raw> {
    let vtt = subtp::vtt::WebVtt::parse(strip_bom(text))
        .map_err(|e| anyhow::anyhow!("WebVTT parse: {e}"))?;
    let mut out = Vec::new();
    for block in &vtt.blocks {
        if let subtp::vtt::VttBlock::Que(cue) = block {
            out.push((
                vtt_ms(&cue.timings.start),
                vtt_ms(&cue.timings.end),
                cue.payload.join(" "),
            ));
        }
    }
    Ok(out)
}

fn vtt_ms(ts: &subtp::vtt::VttTimestamp) -> u64 {
    ts.hours as u64 * 3_600_000
        + ts.minutes as u64 * 60_000
        + ts.seconds as u64 * 1_000
        + ts.milliseconds as u64
}

fn parse_json3_raw(text: &str) -> anyhow::Result<Raw> {
    let v: serde_json::Value =
        serde_json::from_str(strip_bom(text)).map_err(|e| anyhow::anyhow!("json3 parse: {e}"))?;
    let events = v
        .get("events")
        .and_then(|e| e.as_array())
        .ok_or_else(|| anyhow::anyhow!("json3: missing events[]"))?;
    let mut out = Vec::new();
    for ev in events {
        let start = ev.get("tStartMs").and_then(|n| n.as_u64()).unwrap_or(0);
        let dur = ev.get("dDurationMs").and_then(|n| n.as_u64()).unwrap_or(0);
        let text: String = ev
            .get("segs")
            .and_then(|s| s.as_array())
            .map(|segs| {
                segs.iter()
                    .filter_map(|s| s.get("utf8").and_then(|u| u.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        out.push((start, start + dur, text));
    }
    Ok(out)
}

fn parse_bilibili_raw(text: &str) -> anyhow::Result<Raw> {
    let v: serde_json::Value = serde_json::from_str(strip_bom(text))
        .map_err(|e| anyhow::anyhow!("bilibili parse: {e}"))?;
    let items: &[serde_json::Value] = if let Some(a) = v.as_array() {
        a
    } else if let Some(a) = v.get("body").and_then(|b| b.as_array()) {
        a
    } else {
        anyhow::bail!("bilibili: expected top-level array or body[]");
    };
    let mut out = Vec::new();
    for it in items {
        let from = it.get("from").and_then(|n| n.as_f64());
        let to = it.get("to").and_then(|n| n.as_f64());
        let content = it.get("content").and_then(|c| c.as_str()).unwrap_or("");
        if let (Some(f), Some(t)) = (from, to) {
            let (start, end) = bilibili_ms(f, t);
            out.push((start, end, content.to_string()));
        }
    }
    Ok(out)
}

/// Bilibili dumps mix seconds (float) and milliseconds: a magnitude below
/// 100_000 is seconds and scales to ms; at/above it is already ms.
fn bilibili_ms(from: f64, to: f64) -> (u64, u64) {
    if from.abs() < 100_000.0 && to.abs() < 100_000.0 {
        ((from * 1000.0).round() as u64, (to * 1000.0).round() as u64)
    } else {
        (from.round() as u64, to.round() as u64)
    }
}

fn sniff_format(text: &str) -> anyhow::Result<&'static str> {
    let t = strip_bom(text).trim_start();
    if t.starts_with("WEBVTT") {
        return Ok(FORMAT_VTT);
    }
    if t.starts_with('{') || t.starts_with('[') {
        let v: serde_json::Value = serde_json::from_str(t).unwrap_or(serde_json::Value::Null);
        if v.get("events").is_some() {
            return Ok(FORMAT_JSON3);
        }
        return Ok(FORMAT_BILIBILI);
    }
    Ok(FORMAT_SRT)
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Strip zero-width junk YouTube embeds in auto/manual tracks (`U+200B`,
/// `U+FEFF`, `U+200E/F`) and collapse runs of whitespace.
fn clean_caption_text(text: &str) -> String {
    let stripped: String = text
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{200E}' | '\u{200F}'
            )
        })
        .collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `normalize` merges overlapping cues by appending text. YouTube often ships
/// the SAME cue twice with identical timings (`vi` manual tracks do this), so
/// appending would duplicate every line — the TTS then reads it twice.
fn is_redundant_merge(prev_text: &str, next_text: &str) -> bool {
    let prev_norm = prev_text.split_whitespace().collect::<Vec<_>>().join(" ");
    let next_norm = next_text.split_whitespace().collect::<Vec<_>>().join(" ");
    !next_norm.is_empty()
        && (prev_norm == next_norm
            || prev_norm.ends_with(&next_norm)
            || prev_norm.contains(&next_norm))
}

/// Drop zero-length/empty, sort by start, soft-merge overlaps, assign ids.
fn normalize(raw: Raw, src_lang: Option<&str>) -> Vec<TranscriptSegment> {
    let mut items: Vec<(u64, u64, String)> = raw
        .into_iter()
        .filter(|(s, e, t)| e > s && !t.trim().is_empty())
        .map(|(s, e, t)| (s, e, clean_caption_text(&t)))
        .filter(|(_, _, t)| !t.is_empty())
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut merged: Vec<(u64, u64, String)> = Vec::with_capacity(items.len());
    for (s, e, t) in items {
        if let Some((_, pe, pt)) = merged.last_mut() {
            if s < *pe {
                *pe = (*pe).max(e);
                if !is_redundant_merge(pt, &t) {
                    tracing::warn!(
                        start_ms = s,
                        prev_end_ms = *pe,
                        "overlapping captions soft-merged"
                    );
                    pt.push(' ');
                    pt.push_str(&t);
                }
                continue;
            }
        }
        merged.push((s, e, t));
    }
    debug_assert!(
        merged
            .windows(2)
            .all(|w| w[0].0 < w[1].0 && w[0].1 <= w[1].0),
        "segments must be monotonic"
    );

    merged
        .into_iter()
        .enumerate()
        .map(|(i, (start_ms, end_ms, src_text))| TranscriptSegment {
            id: i as u32,
            start_ms,
            end_ms,
            src_text,
            src_lang: src_lang.map(str::to_string),
            confidence: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = include_str!("../../tests/fixtures/captions.srt");
    const VTT: &str = include_str!("../../tests/fixtures/captions.vtt");
    const JSON3: &str = include_str!("../../tests/fixtures/captions.json3");
    const BILIBILI: &str = include_str!("../../tests/fixtures/bilibili.json");

    fn parse_expected(segments: &[TranscriptSegment], expected: &[(u64, u64, &str)]) {
        assert_eq!(segments.len(), expected.len(), "count");
        for (seg, (start, end, text)) in segments.iter().zip(expected) {
            assert_eq!((seg.start_ms, seg.end_ms), (*start, *end), "timings");
            assert_eq!(seg.src_text, *text, "text");
            assert_eq!(seg.confidence, None, "captions have no confidence");
        }
        assert_eq!(
            segments.iter().map(|s| s.id).collect::<Vec<_>>(),
            (0..expected.len() as u32).collect::<Vec<_>>(),
            "ids sequential"
        );
        for w in segments.windows(2) {
            assert!(
                w[0].start_ms < w[1].start_ms && w[0].end_ms <= w[1].start_ms,
                "monotonic"
            );
        }
    }

    const EXPECTED: &[(u64, u64, &str)] = &[
        (1_000, 2_500, "Hello world."),
        (3_000, 4_000, "Multi line cue here."),
        (70_250, 72_000, "Third cue."),
    ];

    #[test]
    fn parse_srt_fixture() {
        let segs = parse_srt(SRT, Some("en")).unwrap();
        parse_expected(&segs, EXPECTED);
        assert!(segs.iter().all(|s| s.src_lang.as_deref() == Some("en")));
    }

    #[test]
    fn parse_vtt_fixture() {
        let segs = parse_vtt(VTT, None).unwrap();
        parse_expected(&segs, EXPECTED);
    }

    #[test]
    fn parse_json3_fixture() {
        let segs = parse_json3(JSON3, Some("en")).unwrap();
        parse_expected(
            &segs,
            &[
                (1_000, 2_500, "Hello world."),
                (3_000, 4_000, "Second cue."),
                (10_000, 12_000, "Third cue."),
            ],
        );
    }

    #[test]
    fn parse_bilibili_fixture_seconds() {
        let segs = parse_bilibili(BILIBILI, Some("zh")).unwrap();
        parse_expected(
            &segs,
            &[
                (1_000, 2_500, "Hello world."),
                (3_000, 4_000, "Second cue."),
                (70_250, 72_000, "Third cue."),
            ],
        );
        assert!(segs.iter().all(|s| s.src_lang.as_deref() == Some("zh")));
    }

    #[test]
    fn parse_bilibili_milliseconds_sniff() {
        let segs = parse_bilibili(
            r#"{"body":[{"from":125000,"to":131500,"content":"ms cue"}]}"#,
            None,
        )
        .unwrap();
        parse_expected(&segs, &[(125_000, 131_500, "ms cue")]);
    }

    #[test]
    fn parse_bilibili_flat_array_root() {
        let segs = parse_bilibili(r#"[{"from":1.5,"to":3.0,"content":"flat"}]"#, None).unwrap();
        parse_expected(&segs, &[(1_500, 3_000, "flat")]);
    }

    #[test]
    fn parse_json3_out_of_order_sorted_and_segless_dropped() {
        let segs = parse_json3(
            r#"{"events":[{"tStartMs":3000,"dDurationMs":1000,"segs":[{"utf8":"b"}]},{"tStartMs":1000,"dDurationMs":1500,"segs":[{"utf8":"a"}]}]}"#,
            None,
        )
        .unwrap();
        parse_expected(&segs, &[(1_000, 2_500, "a"), (3_000, 4_000, "b")]);

        let empty = parse_json3(
            r#"{"events":[{"tStartMs":1000,"dDurationMs":0,"segs":[{"utf8":"x"}]}]}"#,
            None,
        )
        .unwrap();
        assert!(empty.is_empty(), "zero-length dropped");
    }

    #[test]
    fn parse_overlap_soft_merged() {
        let srt = "1\n00:00:01,000 --> 00:00:03,000\na\n\n2\n00:00:02,000 --> 00:00:04,000\nb\n";
        let segs = parse_srt(srt, None).unwrap();
        parse_expected(&segs, &[(1_000, 4_000, "a b")]);
    }

    #[test]
    fn parse_auto_sniff_dispatches() {
        assert_eq!(parse_captions(VTT, "auto", None).unwrap().len(), 3);
        assert_eq!(parse_captions(JSON3, "auto", None).unwrap().len(), 3);
        assert_eq!(parse_captions(BILIBILI, "auto", None).unwrap().len(), 3);
    }

    #[test]
    fn parse_unknown_format_errors() {
        assert!(parse_captions(SRT, "ass", None).is_err());
    }

    #[test]
    fn parse_via_file_infers_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("caps.json3");
        std::fs::write(&path, JSON3).unwrap();
        let (segs, fmt) = parse_caption_file(&path, Some("en")).unwrap();
        assert_eq!(fmt, FORMAT_JSON3);
        assert_eq!(segs.len(), 3);
        assert!(segs.iter().all(|s| s.src_lang.as_deref() == Some("en")));
    }
}
