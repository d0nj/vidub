//! Batching + document orchestration for [`translate_document`]: adaptive
//! batch sizes, the NIM call with its Riva NMT emergency adapter, the gtx
//! fallback, and the `JoinSet` fan-out over batches (see [`super::run`] for
//! the per-batch breaker/shorten pipeline).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use anyhow::Result;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::segment::{TranscriptDocument, TranscriptSegment};
use crate::transcript::{ProgressCb, ProgressEvent};

use super::fallback::translate_gtx;
use super::model::{self, is_riva, note_model_fallback, Attempt, Breaker, ChainOutcome, Engine};
use super::prompt::{system_prompt, user_prompt, Item, PrevCtx};
use super::run::{prev_ctx, translate_batch};
use super::{
    translate_config, Translation, BATCH_N, BATCH_N_LONG, CONCURRENCY, CPS, LONG_AVG_CHARS,
    MIN_MAX_CHARS, OVERLONG_FACTOR,
};

pub(super) fn max_chars_for(seg: &TranscriptSegment) -> usize {
    let dur_s = seg.end_ms.saturating_sub(seg.start_ms) as f64 / 1000.0;
    ((dur_s * CPS).floor() as usize).max(MIN_MAX_CHARS)
}

/// Adaptive batch size: 3 when mean source chars >250, else 6.
pub(super) fn batch_size(items: &[Item]) -> usize {
    if items.is_empty() {
        return BATCH_N;
    }
    let total: usize = items.iter().map(|it| it.src.chars().count()).sum();
    let avg = total as f64 / items.len() as f64;
    if avg > LONG_AVG_CHARS {
        BATCH_N_LONG
    } else {
        BATCH_N
    }
}

pub(super) fn split_batches(items: &[Item], size: usize) -> Vec<Vec<Item>> {
    items.chunks(size.max(1)).map(|c| c.to_vec()).collect()
}

pub(super) fn is_overlong(vi: &str, max_chars: usize) -> bool {
    (vi.chars().count() as f64) > (max_chars as f64) * OVERLONG_FACTOR
}

/// Merge a shorten pass' replacements into the originals (batch order).
/// A replacement keeps its text whether or not it still exceeds the budget;
/// the `overlong` flag reflects the final length.
pub(super) fn merge_shortened(
    orig: &[(u32, String, usize)],
    shortened: &[(u32, String)],
) -> Vec<Translation> {
    let map: HashMap<u32, &String> = shortened.iter().map(|(id, t)| (*id, t)).collect();
    orig.iter()
        .map(|(id, vi, max)| {
            let text = map
                .get(id)
                .map(|s| (*s).clone())
                .unwrap_or_else(|| vi.clone());
            let overlong = is_overlong(&text, *max);
            Translation {
                id: *id,
                vi_text: text,
                overlong,
            }
        })
        .collect()
}

#[derive(Default)]
pub(super) struct NvidiaBatch {
    pub(super) done: Vec<(u32, String)>,
    pub(super) leftover: Vec<Item>,
    pub(super) auth: bool,
    pub(super) warnings: Vec<String>,
}

/// Translate `items` via the NIM chain. Malformed output: repair re-prompts
/// (2×) → split-half retry → single → leftover (caller routes to gtx).
pub(super) fn translate_items<'a>(
    engine: &'a Engine,
    items: Vec<Item>,
    prev: Option<PrevCtx>,
) -> Pin<Box<dyn Future<Output = NvidiaBatch> + Send + 'a>> {
    Box::pin(async move {
        let mut batch = NvidiaBatch::default();
        if items.is_empty() {
            return batch;
        }
        if engine.auth_failed.load(Ordering::SeqCst) {
            batch.leftover = items;
            batch.auth = true;
            batch
                .warnings
                .push("translate: auth failed — Google gtx fallback".to_string());
            return batch;
        }
        let ids: Vec<u32> = items.iter().map(|it| it.id).collect();
        let user = user_prompt(&items, prev.as_ref());
        match model::call_chain(engine, system_prompt(), &user).await {
            ChainOutcome::Auth => {
                engine.auth_failed.store(true, Ordering::SeqCst);
                batch
                    .warnings
                    .push("translate: 401/403 — Google gtx fallback".to_string());
                batch.leftover = items;
                batch.auth = true;
                batch
            }
            ChainOutcome::Unavailable(msg) => {
                // Last NIM resort before gtx: Riva NMT single-segment calls.
                if engine.riva.is_some() {
                    let mut r = riva_translate_items(engine, items).await;
                    batch.done.append(&mut r.done);
                    batch.warnings.append(&mut r.warnings);
                    if !r.leftover.is_empty() {
                        batch.warnings.push(format!(
                            "translate: NIM unavailable ({msg}), riva partial — gtx fallback"
                        ));
                        batch.leftover = r.leftover;
                    }
                    batch
                } else {
                    batch.warnings.push(format!(
                        "translate: all NIM models unavailable ({msg}) — gtx fallback"
                    ));
                    batch.leftover = items;
                    batch
                }
            }
            ChainOutcome::Content { body, model: used } => {
                note_model_fallback(&mut batch.warnings, engine, &used);
                if let Ok(done) =
                    model::parse_with_repairs(engine, body, &ids, &mut batch.warnings).await
                {
                    batch.done = done;
                } else if items.len() > 1 {
                    batch
                        .warnings
                        .push("translate: malformed output — split-half retry".to_string());
                    let mid = items.len() / 2;
                    let (left, right) = items.split_at(mid);
                    let left = left.to_vec();
                    let right = right.to_vec();
                    let mut a = translate_items(engine, left, prev).await;
                    let mut b = translate_items(engine, right, None).await;
                    batch.done.extend(a.done);
                    batch.leftover.extend(a.leftover);
                    batch.done.extend(b.done);
                    batch.leftover.extend(b.leftover);
                    batch.auth = a.auth || b.auth;
                    batch.warnings.append(&mut a.warnings);
                    batch.warnings.append(&mut b.warnings);
                } else {
                    batch.warnings.push(
                        "translate: single-segment output unusable — gtx fallback".to_string(),
                    );
                    batch.leftover = items;
                }
                batch
            }
        }
    })
}

/// Riva NMT emergency path: one plain call per segment with the fixed
/// `{src}-vi` system prompt. Sequential (emergency only); RPM bucket applies
/// inside `post_once`. Plain-text answers, no JSON involved.
pub(super) async fn riva_translate_items(engine: &Engine, items: Vec<Item>) -> NvidiaBatch {
    let mut batch = NvidiaBatch::default();
    let Some(riva) = engine.riva.clone() else {
        batch.leftover = items;
        return batch;
    };
    let pair = riva_pair(&engine.src_lang);
    let mut used = false;
    let mut iter = items.into_iter();
    while let Some(it) = iter.next() {
        let mut done = false;
        for attempt in 1..=2u32 {
            if attempt > 1 {
                tokio::time::sleep(model::backoff_delay(attempt - 1, None)).await;
            }
            match model::post_once(engine, &riva, &pair, &it.src).await {
                Attempt::Content { body, .. } => {
                    let vi = body.trim().to_string();
                    if !vi.is_empty() {
                        batch.done.push((it.id, vi));
                        used = true;
                        done = true;
                        break;
                    }
                }
                Attempt::Auth => {
                    engine.auth_failed.store(true, Ordering::SeqCst);
                    batch.auth = true;
                    batch
                        .warnings
                        .push("translate: 401/403 — Google gtx fallback".to_string());
                    batch.leftover.push(it);
                    batch.leftover.extend(iter);
                    return batch;
                }
                Attempt::Retryable { msg, retry_after } => {
                    if attempt == 2 {
                        batch
                            .warnings
                            .push(format!("translate: riva failed ({msg})"));
                    } else {
                        tokio::time::sleep(model::backoff_delay(attempt, retry_after)).await;
                    }
                }
                Attempt::ModelError(msg) => {
                    batch
                        .warnings
                        .push(format!("translate: riva model error ({msg})"));
                    break;
                }
            }
        }
        if !done {
            batch.leftover.push(it);
        }
    }
    if used {
        batch
            .warnings
            .push("translate: riva NMT emergency fallback used".to_string());
    }
    batch
}

/// Riva pair tag from a detected language (`en-US` → `en-vi`).
/// Unknown/auto → `en-vi` (most common source; a wrong guess 400s into gtx).
pub(super) fn riva_pair(src_lang: &str) -> String {
    let sub = src_lang
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let src = match sub.as_str() {
        "" | "unknown" | "und" | "multi" => "en",
        s => s,
    };
    format!("{src}-vi")
}

/// Google gtx fallback: `None` results and missing ids keep the source text.
pub(super) async fn apply_gtx(
    client: &reqwest::Client,
    items: &[Item],
    src_lang: &str,
) -> Vec<Translation> {
    let texts: Vec<(u32, String)> = items.iter().map(|it| (it.id, it.src.clone())).collect();
    let got = translate_gtx(client, &texts, src_lang).await;
    let map: HashMap<u32, Option<String>> = got.into_iter().collect();
    items
        .iter()
        .map(|it| match map.get(&it.id) {
            Some(Some(v)) => Translation {
                id: it.id,
                vi_text: v.clone(),
                overlong: false,
            },
            _ => Translation {
                id: it.id,
                vi_text: it.src.clone(),
                overlong: false,
            },
        })
        .collect()
}

pub(crate) struct BatchOutcome {
    pub(crate) translations: Vec<Translation>,
    pub(super) warnings: Vec<String>,
}

/// Translate every segment in `doc` to Vietnamese (provider chain, gtx
/// fallback). Order of the returned vec matches `doc.segments`. Never panics
/// on model output; the chain comes from the settings provider + preset
/// (`translate_config`). `key` is that provider's API key.
pub async fn translate_document(
    client: &reqwest::Client,
    doc: &TranscriptDocument,
    key: &str,
    provider: vidub_config::Provider,
    progress: &ProgressCb,
) -> Result<Vec<Translation>> {
    // Vietnamese source: text already IS the dub script — pass it through
    // instead of round-tripping it through a translator. Report a distinct
    // stage so the UI never claims to be translating.
    if crate::transcript::probe::is_vietnamese(&doc.detected_lang) {
        tracing::info!(
            lang = %doc.detected_lang,
            segments = doc.segments.len(),
            "source is Vietnamese: skipping translation"
        );
        progress(ProgressEvent::Stage("captions ready"));
        return Ok(doc
            .segments
            .iter()
            .map(|s| Translation {
                id: s.id,
                vi_text: s.src_text.clone(),
                overlong: false,
            })
            .collect());
    }

    let cfg = translate_config(provider);
    progress(ProgressEvent::Stage("translating"));
    let mut models = vec![cfg.primary.clone()];
    models.extend(cfg.fallbacks.iter().filter(|m| !is_riva(m)).cloned());
    // Riva NMT exists only on NVIDIA NIM; other providers have no adapter.
    let riva = if provider == vidub_config::Provider::Nvidia {
        std::iter::once(cfg.primary.clone())
            .chain(cfg.fallbacks.clone())
            .find(|m| is_riva(m))
    } else {
        None
    };
    let engine = Arc::new(Engine {
        client: client.clone(),
        key: key.to_string(),
        primary: cfg.primary.clone(),
        models,
        riva,
        buckets: Mutex::new(HashMap::new()),
        breaker: Mutex::new(Breaker::new()),
        auth_failed: AtomicBool::new(false),
        sem: Semaphore::new(CONCURRENCY),
        src_lang: doc.detected_lang.clone(),
        provider,
    });

    // Empty/whitespace segments need no translation.
    let mut by_id: HashMap<u32, Translation> = HashMap::new();
    for s in &doc.segments {
        if s.src_text.trim().is_empty() {
            by_id.insert(
                s.id,
                Translation {
                    id: s.id,
                    vi_text: String::new(),
                    overlong: false,
                },
            );
        }
    }
    let items: Vec<Item> = doc
        .segments
        .iter()
        .filter(|s| !s.src_text.trim().is_empty())
        .map(|s| Item {
            id: s.id,
            src: s.src_text.clone(),
            max_chars: max_chars_for(s),
        })
        .collect();

    if items.is_empty() {
        return Ok(doc
            .segments
            .iter()
            .map(|s| Translation {
                id: s.id,
                vi_text: String::new(),
                overlong: false,
            })
            .collect());
    }

    let batches = split_batches(&items, batch_size(&items));
    let nb = batches.len();
    let slots: Arc<Mutex<Vec<Option<BatchOutcome>>>> =
        Arc::new(Mutex::new((0..nb).map(|_| None).collect()));

    let mut set = JoinSet::<usize>::new();
    let mut next = 0usize;
    while next < nb || !set.is_empty() {
        if set.len() < CONCURRENCY && next < nb {
            let prev = prev_ctx(next, &batches, &slots);
            progress(ProgressEvent::Stage("translating"));
            let eng = Arc::clone(&engine);
            let batch_items = batches[next].clone();
            let slot = Arc::clone(&slots);
            let idx = next;
            set.spawn(async move {
                let outcome = translate_batch(&eng, batch_items, prev).await;
                slot.lock().expect("slots mutex")[idx] = Some(outcome);
                idx
            });
            next += 1;
        } else if let Some(res) = set.join_next().await {
            let _ = res; // on task failure the slot stays None → gtx below
        }
    }

    // Slots still empty (task failed) get the gtx fallback. Collect indices
    // under a short lock; the awaits below must not hold the guard (!Send).
    let missing: Vec<usize> = {
        let guard = slots.lock().expect("slots mutex");
        guard
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.is_none())
            .map(|(i, _)| i)
            .collect()
    };
    for i in missing {
        let gtxed = apply_gtx(&engine.client, &batches[i], &engine.src_lang).await;
        slots.lock().expect("slots mutex")[i] = Some(BatchOutcome {
            translations: gtxed,
            warnings: vec!["translate: batch failed — Google gtx fallback".to_string()],
        });
    }

    // Warnings + assembly in doc order.
    let guard = slots.lock().expect("slots mutex");
    let mut all: Vec<Translation> = Vec::with_capacity(doc.segments.len());
    for o in guard.iter().flatten() {
        for w in &o.warnings {
            progress(ProgressEvent::Warning(w.clone()));
        }
        all.extend(o.translations.iter().cloned());
    }
    for t in all {
        by_id.insert(t.id, t);
    }
    Ok(doc
        .segments
        .iter()
        .map(|s| {
            by_id.get(&s.id).cloned().unwrap_or(Translation {
                id: s.id,
                vi_text: s.src_text.clone(),
                overlong: false,
            })
        })
        .collect())
}
