use crate::sync::pace::*;
use crate::sync::tiling::*;
use crate::util::secs;

#[test]
fn pace_boundaries() {
    assert_eq!(pace_action(0.69, 1000, 0), PaceAction::Pad);
    assert_eq!(pace_action(0.70, 1000, 0), PaceAction::Fit { tempo: 0.70 });
    assert_eq!(pace_action(0.75, 1000, 0), PaceAction::Fit { tempo: 0.75 });
    assert_eq!(pace_action(0.80, 1000, 0), PaceAction::Fit { tempo: 0.80 });
    assert_eq!(pace_action(1.0, 1000, 0), PaceAction::Fit { tempo: 1.0 });
    assert_eq!(pace_action(1.24, 1000, 0), PaceAction::Fit { tempo: 1.24 });
    assert_eq!(
        pace_action(1.25, 1000, 2000),
        PaceAction::BorrowGap { new_end_ms: 1250 }
    );
    assert_eq!(pace_action(1.35, 1000, 2000), PaceAction::ShortenRetry);
    assert_eq!(pace_action(1.50, 1000, 2000), PaceAction::ShortenRetry);
    assert_eq!(pace_action(1.51, 1000, 2000), PaceAction::HardCut);
    assert_eq!(pace_action(3.0, 1000, 2000), PaceAction::HardCut);
    let s = 1.30f32.sqrt();
    assert_eq!(
        pace_action(1.30, 1000, 0),
        PaceAction::Chained { a: s, b: s }
    );
}

#[test]
fn pace_borrow_margin() {
    assert_eq!(
        pace_action(1.30, 1000, 2000),
        PaceAction::BorrowGap { new_end_ms: 1300 }
    );
    assert_eq!(
        pace_action(1.30, 1000, 100),
        PaceAction::BorrowGap { new_end_ms: 1060 }
    );
    let s = 1.30f32.sqrt();
    assert_eq!(
        pace_action(1.30, 1000, 40),
        PaceAction::Chained { a: s, b: s }
    );
    assert_eq!(
        pace_action(1.30, 1000, 30),
        PaceAction::Chained { a: s, b: s }
    );
    assert_eq!(
        pace_action(1.30, 1000, 41),
        PaceAction::BorrowGap { new_end_ms: 1001 }
    );
    assert_eq!(
        pace_action(1.34, 1000, 50),
        PaceAction::BorrowGap { new_end_ms: 1010 }
    );
}

#[test]
fn pace_defensive() {
    assert_eq!(pace_action(f32::NAN, 1000, 0), PaceAction::Pad);
    assert_eq!(pace_action(-1.0, 1000, 0), PaceAction::Pad);
    assert_eq!(pace_action(0.0, 1000, 0), PaceAction::Pad);
    assert_eq!(pace_action(f32::INFINITY, 1000, 0), PaceAction::HardCut);
    assert_eq!(pace_action(1.0, 0, 0), PaceAction::HardCut);
}

#[test]
fn atempos_never_passes_over_two_to_one_instance() {
    for r in [0.4f32, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 4.5, 8.0, 100.0] {
        let chain = atempos_for(r);
        assert!(
            chain.iter().all(|f| *f >= 0.5 && *f <= 2.0),
            "r={r}: {chain:?}"
        );
        let product: f32 = chain.iter().product();
        let clamped = r.clamp(0.5, 100.0);
        assert!((product - clamped).abs() < 1e-3, "r={r}: product {product}");
    }
    assert_eq!(atempos_for(1.0), vec![1.0]);
    assert_eq!(atempos_for(0.3), vec![0.5]); // clamped to atempo floor
    assert_eq!(atempos_for(f32::NAN), vec![1.0]);
}

#[test]
fn filter_strings() {
    assert_eq!(secs(1500), "1.500");
    assert_eq!(secs(0), "0.000");
    assert_eq!(tile_filter(1500), "apad=whole_dur=1.500,atrim=0:1.500");
    assert_eq!(atempo_filter(1.25), "atempo=1.250");
    assert_eq!(
        pace_filter(1.0, &PaceAction::Pad, 1500),
        "apad=whole_dur=1.500,atrim=0:1.500"
    );
    assert_eq!(
        pace_filter(1.25, &PaceAction::Fit { tempo: 1.25 }, 1500),
        "atempo=1.250,apad=whole_dur=1.500,atrim=0:1.500"
    );
    let s = 1.30f32.sqrt();
    assert_eq!(
        pace_filter(1.30, &PaceAction::Chained { a: s, b: s }, 1000),
        "atempo=1.140,atempo=1.140,apad=whole_dur=1.000,atrim=0:1.000"
    );
    assert_eq!(
        pace_filter(1.0, &PaceAction::BorrowGap { new_end_ms: 1300 }, 1300),
        "atempo=1.000,apad=whole_dur=1.300,atrim=0:1.300"
    );
    assert_eq!(
        pace_filter(1.40, &PaceAction::ShortenRetry, 1000),
        "atempo=1.400,apad=whole_dur=1.000,atrim=0:1.000"
    );
    assert_eq!(
        pace_filter(2.8, &PaceAction::HardCut, 1500),
        "atempo=1.500,afade=t=out:st=1.450:d=0.050,apad=whole_dur=1.500,atrim=0:1.500"
    );
    assert_eq!(
        pace_filter(2.8, &PaceAction::HardCut, 30),
        "atempo=1.500,apad=whole_dur=0.030,atrim=0:0.030"
    );
    assert_eq!(
        fade_filter(1500).as_deref(),
        Some("afade=t=out:st=1.450:d=0.050")
    );
    assert_eq!(fade_filter(30), None);
}

#[test]
fn samples_math_is_integer() {
    assert_eq!(ms_to_samples(1500, 22050), 33075);
    assert_eq!(ms_to_samples(1000, 16000), 16000);
    assert_eq!(ms_to_samples(1, 22050), 22); // floor, no float drift
    assert_eq!(ms_to_samples(0, 22050), 0);
}

#[test]
fn tile_exact_pads_truncates_and_keeps() {
    assert_eq!(tile_exact(vec![1, 2, 3], 5), vec![1, 2, 3, 0, 0]);
    assert_eq!(tile_exact(vec![1, 2, 3, 4], 2), vec![1, 2]);
    assert_eq!(tile_exact(vec![7, 8], 2), vec![7, 8]);
}

#[test]
fn concat_gap_fill_is_silent_on_synthetic_pieces() {
    let a = [1i16; 100];
    let b = [2i16; 100];
    let out = mix_timeline(&[(0, &a[..]), (400, &b[..])], 500);
    assert_eq!(out.len(), 500);
    assert_eq!(&out[0..100], &a[..]);
    assert_eq!(&out[100..400], &[0i16; 300][..]); // gap silence
    assert_eq!(&out[400..500], &b[..]);
}

#[test]
fn concat_overlap_later_wins_and_out_of_range_dropped() {
    let a = [1i16; 10];
    let b = [2i16; 10];
    let c = [3i16; 10];
    let out = mix_timeline(&[(0, &a[..]), (5, &b[..]), (100, &c[..])], 20);
    assert_eq!(&out[0..5], &[1i16; 5][..]);
    assert_eq!(&out[5..15], &[2i16; 10][..]);
    assert_eq!(&out[15..20], &[0i16; 5][..]); // untouched tail
}

#[test]
fn wav_roundtrip_mono_i16() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.wav");
    let samples: Vec<i16> = (0..4410).map(|i| (i as i16) % 100).collect();
    write_wav(&path, 44100, &samples).unwrap();
    let (rate, got) = read_wav_i16(&path).unwrap();
    assert_eq!(rate, 44100);
    assert_eq!(got, samples);
}

#[test]
fn wav_read_downmixes_stereo_by_mean() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stereo.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for (l, r) in [(100i16, 200i16), (-50, 50), (10, 30)] {
        writer.write_sample(l).unwrap();
        writer.write_sample(r).unwrap();
    }
    writer.finalize().unwrap();
    let (rate, got) = read_wav_i16(&path).unwrap();
    assert_eq!(rate, 16000);
    assert_eq!(got, vec![150, 0, 20]);
}

#[test]
fn wav_read_rejects_float() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("float.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    writer.write_sample(0.5f32).unwrap();
    writer.finalize().unwrap();
    assert!(read_wav_i16(&path).is_err());
}
