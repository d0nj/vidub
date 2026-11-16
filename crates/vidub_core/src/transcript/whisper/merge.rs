//! Merge per-chunk results: offset-correct, overlap-dedup, sort, split
//! window-spanning blobs at sentence boundaries.

use super::{ChunkResult, WhisperSegment};

/// Split overlong segments (>8s) at sentence boundaries, distributing time
/// by character count. Short or boundary-less segments pass through.
fn split_long_segments(segments: Vec<WhisperSegment>) -> Vec<WhisperSegment> {
    const MAX_MS: u64 = 8000;
    let mut out = Vec::with_capacity(segments.len());
    for seg in segments {
        let dur = seg.end_ms.saturating_sub(seg.start_ms);
        let parts = if dur > MAX_MS {
            split_sentences(&seg.src_text)
        } else {
            Vec::new()
        };
        if parts.len() < 2 {
            out.push(seg);
            continue;
        }
        let total: usize = parts.iter().map(|p| p.len()).sum::<usize>().max(1);
        let mut start = seg.start_ms;
        for (i, part) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            let end = if last {
                seg.end_ms
            } else {
                start.saturating_add((dur as f64 * part.len() as f64 / total as f64) as u64)
            };
            out.push(WhisperSegment {
                id: 0,
                start_ms: start,
                end_ms: end.max(start.saturating_add(1)),
                src_text: part.clone(),
                src_lang: seg.src_lang.clone(),
                confidence: seg.confidence,
            });
            start = end;
        }
    }
    out
}

/// Split text after `[.!?]` followed by space/end, keeping delimiters.
fn split_sentences(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if (b == b'.' || b == b'!' || b == b'?')
            && (i + 1 == bytes.len() || bytes[i + 1] == b' ' || bytes[i + 1] == b'\t')
        {
            let piece = text[start..=i].trim().to_string();
            if !piece.is_empty() {
                parts.push(piece);
            }
            start = i + 1;
        }
        i += 1;
    }
    let tail = text[start..].trim().to_string();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}
/// Merge per-chunk results: offset-correct (`+= windowStart`), drop
/// fully-covered overlap duplicates, sort, and assert monotonicity
/// (non-overlapping, strictly ordered). Partial overlaps are resolved by
/// truncating the earlier segment at the later one's start — cross-window
/// duplicates share the sealed boundary, so no unique text is lost.
pub fn merge(chunks: &[ChunkResult]) -> Vec<WhisperSegment> {
    let mut all: Vec<WhisperSegment> = Vec::new();
    for chunk in chunks {
        for seg in &chunk.segments {
            let start_ms = seg.start_ms.saturating_add(chunk.window_start_ms);
            let end_ms = seg.end_ms.saturating_add(chunk.window_start_ms);
            let text = seg.text.trim().to_string();
            if end_ms <= start_ms || text.is_empty() {
                continue;
            }
            all.push(WhisperSegment {
                id: 0,
                start_ms,
                end_ms,
                src_text: text,
                src_lang: chunk.detected_lang.clone(),
                confidence: seg.confidence,
            });
        }
    }

    all.sort_by(|a, b| {
        (a.start_ms, a.end_ms)
            .cmp(&(b.start_ms, b.end_ms))
            .then_with(|| a.src_text.cmp(&b.src_text))
    });

    // Overlap-dedup + clamp into a monotonic, non-overlapping run.
    let mut kept: Vec<WhisperSegment> = Vec::with_capacity(all.len());
    let mut covered_end = 0u64; // max end over kept (original ends)
    for seg in all {
        if seg.end_ms <= covered_end {
            continue; // fully covered by an earlier segment
        }
        // Clamp the previous segment's tail at this one's start.
        if kept
            .last()
            .map(|p| seg.start_ms < p.end_ms)
            .unwrap_or(false)
        {
            let last = kept.last_mut().expect("checked non-empty");
            last.end_ms = seg.start_ms;
            if last.end_ms <= last.start_ms {
                kept.pop();
                covered_end = kept.iter().map(|k| k.end_ms).max().unwrap_or(0);
            }
        }
        covered_end = covered_end.max(seg.end_ms);
        kept.push(seg);
    }

    // Whisper emits window-spanning blobs (no word timings); split them into
    // sentence-sized pieces for translation CPS and TTS.
    let mut kept = split_long_segments(kept);

    for (i, seg) in kept.iter_mut().enumerate() {
        seg.id = i as u32;
    }

    // Monotonicity invariant (assert).
    for w in kept.windows(2) {
        debug_assert!(
            w[0].start_ms < w[1].start_ms && w[0].end_ms <= w[1].start_ms,
            "non-monotonic merge output: {:?} then {:?}",
            w[0],
            w[1]
        );
    }

    kept
}
