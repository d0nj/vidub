//! Whisper tests: synthetic chunks/responses only (no network, no ffmpeg).

use std::path::PathBuf;
use std::time::Duration;

use super::client::{jitter_fraction, response_to_chunk};
use super::nvidia::riva::asr::{
    RecognizeResponse, SpeechRecognitionAlternative, SpeechRecognitionResult, WordInfo,
};
use super::*;

fn chunk_result(index: usize, start: u64, words: &[(&str, u64, u64)]) -> ChunkResult {
    ChunkResult {
        index,
        window_start_ms: start,
        segments: words
            .iter()
            .map(|(w, s, e)| RawSeg {
                start_ms: *s,
                end_ms: *e,
                text: w.to_string(),
                confidence: None,
            })
            .collect(),
        detected_lang: None,
    }
}
#[test]
fn window_plan_steps_by_28s() {
    assert_eq!(windows(0), vec![]);
    assert_eq!(windows(28_000), vec![(0, 0)]);
    assert_eq!(windows(29_000), vec![(0, 0), (1, 28_000)]);
    assert_eq!(windows(30_000), vec![(0, 0), (1, 28_000)]);
    assert_eq!(windows(60_000), vec![(0, 0), (1, 28_000), (2, 56_000)]);
    assert_eq!(
        windows(90_000),
        vec![(0, 0), (1, 28_000), (2, 56_000), (3, 84_000)]
    );
}

#[test]
fn merge_offsets_dedups_and_keeps_monotonic() {
    // "again" spans the window boundary (global 27_500..28_300): window 0
    // sees it as 27_500..28_300, window 1 (start 28_000) as 0..300 ->
    // same absolute span 28_000..28_300, fully covered => dropped.
    let c0 = chunk_result(0, 0, &[("hello", 0, 400), ("again", 27_500, 28_300)]);
    let c1 = chunk_result(1, 28_000, &[("again", 0, 300), ("next", 500, 900)]);
    let merged = merge(&[c0, c1]);
    let texts: Vec<&str> = merged.iter().map(|s| s.src_text.as_str()).collect();
    assert_eq!(texts, vec!["hello", "again", "next"]);
    let again = &merged[1];
    assert_eq!((again.start_ms, again.end_ms), (27_500, 28_300));
    assert_eq!((merged[2].start_ms, merged[2].end_ms), (28_500, 28_900));
    for w in merged.windows(2) {
        assert!(w[0].end_ms <= w[1].start_ms);
    }
    assert_eq!(
        merged.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

#[test]
fn merge_drops_fully_covered_and_empty() {
    let c0 = chunk_result(
        0,
        0,
        &[
            ("a", 0, 1_000),
            ("b", 100, 200),
            ("", 300, 400),
            ("c", 500, 400),
        ],
    );
    let merged = merge(&[c0]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].src_text, "a");
}

#[test]
fn merge_sorts_out_of_order_chunks() {
    let c0 = chunk_result(0, 0, &[("first", 0, 100)]);
    let c1 = chunk_result(1, 28_000, &[("second", 100, 300)]);
    let merged = merge(&[c1, c0]);
    let texts: Vec<&str> = merged.iter().map(|s| s.src_text.as_str()).collect();
    assert_eq!(texts, vec!["first", "second"]);
}

#[test]
fn merge_splits_window_blob_at_sentence_boundaries() {
    let c0 = chunk_result(
        0,
        0,
        &[("First sentence here. Second one follows.", 0, 30_000)],
    );
    let merged = merge(&[c0]);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].src_text, "First sentence here.");
    assert_eq!(merged[1].src_text, "Second one follows.");
    assert_eq!(merged[0].start_ms, 0);
    assert_eq!(merged[1].end_ms, 30_000);
    assert_eq!(merged[0].end_ms, merged[1].start_ms);
    assert!(merged[0].end_ms > merged[0].start_ms);
    assert_eq!(merged.iter().map(|s| s.id).collect::<Vec<_>>(), vec![0, 1]);
}

#[test]
fn backoff_grows_caps_and_honors_retry_after() {
    let seed = 42;
    // base=500*2^1=1000ms, jitter ±25% -> [750, 1250].
    let d1 = backoff_delay(1, None, seed);
    assert!(d1 >= Duration::from_millis(700) && d1 <= Duration::from_millis(1300));
    // base=500*2^4=8000ms, jitter ±25% -> [6000, 10000] (< cap).
    let d4 = backoff_delay(4, None, seed);
    assert!(d4 >= Duration::from_secs(6) && d4 <= Duration::from_secs(10));
    // capped at 30s base, jitter ±25% -> [22500, 37500].
    let d6 = backoff_delay(6, None, seed);
    assert!(d6 >= Duration::from_millis(22_500) && d6 <= Duration::from_millis(37_500));
    let after = Duration::from_secs(20);
    assert!(backoff_delay(0, Some(after), seed) >= after);
}

#[test]
fn jitter_stays_bounded() {
    for seed in 0..100 {
        let j = jitter_fraction(seed);
        assert!((-0.25..=0.25).contains(&j), "seed {seed}: {j}");
    }
}

#[test]
fn persist_roundtrip_and_resume_skip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let original = chunk_result(3, 84_000, &[("hi", 0, 100)]);
    let path = chunk_path(dir.path(), 3);
    assert_eq!(load_chunk(&path), None);
    store_chunk(&path, &original).expect("store");
    assert_eq!(load_chunk(&path), Some(original));
    assert!(!path.with_extension("json.tmp").exists());
}

#[test]
fn wav_header_parses_synthetic_pcm() {
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&u32::MAX.to_le_bytes()); // size ignored
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&16_000u32.to_le_bytes()); // sample rate
    wav.extend_from_slice(&32_000u32.to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits/sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(30_000u32 * 32).to_le_bytes()); // 30 s of PCM
    wav.extend_from_slice(&vec![0u8; 30_000 * 32]);
    let (params, total_ms) = parse_wav_header(&wav).expect("parse");
    assert_eq!(params.sample_rate_hz, 16_000);
    assert_eq!(params.channels, 1);
    assert_eq!(total_ms, 30_000);
}

#[test]
fn wav_header_rejects_non_pcm() {
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&0u32.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&0u32.to_le_bytes());
    assert!(parse_wav_header(&wav).is_err());
}

#[test]
fn function_id_parse_finds_whisper_table() {
    let toml = r#"
[whisper]
function_id = "b702f636-f60c-4a3d-a6f4-f3568c13bd7d"

[translate]
primary = "mistralai/mistral-large-4-1t-a52b"
"#;
    assert_eq!(
        parse_function_id(toml).as_deref(),
        Some("b702f636-f60c-4a3d-a6f4-f3568c13bd7d")
    );
    assert_eq!(parse_function_id("[other]\nfunction_id=\"x\"\n"), None);
}

#[test]
fn response_maps_word_timings() {
    let win = ChunkWindow {
        index: 0,
        start_ms: 28_000,
        path: PathBuf::from("chunk-0000.wav"),
    };
    let resp = RecognizeResponse {
        results: vec![SpeechRecognitionResult {
            alternatives: vec![SpeechRecognitionAlternative {
                transcript: "hello world".into(),
                confidence: 0.9,
                language_code: vec!["en-US".into()],
                words: vec![
                    WordInfo {
                        start_time: 100,
                        end_time: 300,
                        word: "hello".into(),
                        confidence: 0.8,
                        ..Default::default()
                    },
                    WordInfo {
                        start_time: 400,
                        end_time: 600,
                        word: "world".into(),
                        confidence: 0.0,
                        ..Default::default()
                    },
                ],
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let out = response_to_chunk(&win, &resp);
    assert_eq!(out.segments.len(), 2);
    assert_eq!(out.segments[0].confidence, Some(0.8));
    // Zero word confidence falls back to the alternative's.
    assert_eq!(out.segments[1].confidence, Some(0.9));
    assert_eq!(out.detected_lang.as_deref(), Some("en-US"));
    let merged = merge(&[out]);
    assert_eq!(merged[0].start_ms, 28_100);
    assert_eq!(merged[0].src_lang.as_deref(), Some("en-US"));
}
