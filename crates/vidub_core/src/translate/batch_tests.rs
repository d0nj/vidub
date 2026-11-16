use crate::segment::TranscriptSegment;

use super::batch::{
    batch_size, is_overlong, max_chars_for, merge_shortened, riva_pair, split_batches,
};
use super::prompt::Item;

fn seg(id: u32, start_ms: u64, end_ms: u64, src: &str) -> TranscriptSegment {
    TranscriptSegment {
        id,
        start_ms,
        end_ms,
        src_text: src.to_string(),
        src_lang: None,
        confidence: None,
    }
}

fn item(id: u32, src: &str, max_chars: usize) -> Item {
    Item {
        id,
        src: src.to_string(),
        max_chars,
    }
}

#[test]
fn max_chars_is_14cps_floored_at_12() {
    assert_eq!(max_chars_for(&seg(0, 0, 2_000, "hello")), 28);
    assert_eq!(max_chars_for(&seg(0, 0, 100, "hi")), 12);
    assert_eq!(max_chars_for(&seg(0, 0, 10_000, "hi")), 140);
    assert_eq!(max_chars_for(&seg(0, 5_000, 5_000, "hi")), 12);
}

#[test]
fn batch_size_adapts_to_mean_length() {
    let short: Vec<Item> = (0..13).map(|i| item(i, "hi there", 28)).collect();
    assert_eq!(batch_size(&short), 6);
    let long_src = "x".repeat(300);
    let long: Vec<Item> = (0..5).map(|i| item(i, &long_src, 98)).collect();
    assert_eq!(batch_size(&long), 3);
}

#[test]
fn batches_split_by_size() {
    let items: Vec<Item> = (0..13).map(|i| item(i, "hi", 28)).collect();
    let batches = split_batches(&items, 6);
    assert_eq!(batches.len(), 3);
    assert_eq!(batches[0].len(), 6);
    assert_eq!(batches[1].len(), 6);
    assert_eq!(batches[2].len(), 1);
    assert_eq!(batches[2][0].id, 12);
    let ids: Vec<u32> = batches.iter().flatten().map(|it| it.id).collect();
    assert_eq!(ids, (0..13).collect::<Vec<_>>());
}

#[test]
fn overlong_flag_is_len_above_110pct() {
    assert!(!is_overlong(&"x".repeat(20), 20));
    assert!(!is_overlong(&"x".repeat(22), 20)); // 22 == 20×1.10, not above
    assert!(is_overlong(&"x".repeat(23), 20));
    assert!(is_overlong("xin chào bạn", 8)); // 12 chars > 8.8
}

#[test]
fn merge_shortened_prefers_replacement_and_reflags() {
    let orig = vec![
        (0, "a".repeat(40), 20),
        (1, "b".repeat(10), 20),
        (2, "c".repeat(30), 20),
    ];
    let shortened = vec![(0, "ok".to_string()), (1, "ok2".to_string())];
    let merged = merge_shortened(&orig, &shortened);
    assert_eq!(merged[0].vi_text, "ok");
    assert!(!merged[0].overlong);
    assert_eq!(merged[1].vi_text, "ok2");
    assert!(!merged[1].overlong);
    assert_eq!(merged[2].vi_text, "c".repeat(30));
    assert!(merged[2].overlong);
    let still = vec![(0, "d".repeat(30))];
    let merged2 = merge_shortened(&orig, &still);
    assert_eq!(merged2[0].vi_text, "d".repeat(30));
    assert!(merged2[0].overlong);
}

#[test]
fn riva_pair_tags() {
    assert_eq!(riva_pair("en"), "en-vi");
    assert_eq!(riva_pair("en-US"), "en-vi");
    assert_eq!(riva_pair("ja"), "ja-vi");
    assert_eq!(riva_pair("unknown"), "en-vi");
    assert_eq!(riva_pair("multi"), "en-vi");
    assert_eq!(riva_pair(""), "en-vi");
}
