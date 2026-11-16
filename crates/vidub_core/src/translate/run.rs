//! Per-batch NIM pipeline: breaker gate → NV translate → overlong shorten
//! pass → gtx for leftovers.

use std::collections::HashMap;
use std::sync::{atomic::Ordering, Mutex};
use std::time::Instant;

use super::batch::{apply_gtx, is_overlong, merge_shortened, translate_items, BatchOutcome};
use super::model::{self, ChainOutcome, Engine};
use super::parse::parse_translations;
use super::prompt::{shorten_user_prompt, system_prompt, Item, PrevCtx};
use super::{Translation, MIN_MAX_CHARS};

/// Full per-batch pipeline: breaker gate → NV translate → overlong shorten
/// pass → gtx for leftovers. Result is in batch item order.
pub(super) async fn translate_batch(
    engine: &Engine,
    items: Vec<Item>,
    prev: Option<PrevCtx>,
) -> BatchOutcome {
    let _permit = engine.sem.acquire().await.expect("semaphore open");
    let mut warnings = Vec::new();

    let allowed = engine
        .breaker
        .lock()
        .expect("breaker mutex")
        .allow(Instant::now());
    if !allowed || engine.auth_failed.load(Ordering::SeqCst) {
        if !allowed {
            warnings.push("translate: circuit breaker OPEN — Google gtx fallback".to_string());
        }
        let translations = apply_gtx(&engine.client, &items, &engine.src_lang).await;
        return BatchOutcome {
            translations,
            warnings,
        };
    }

    let nb = translate_items(engine, items.clone(), prev).await;
    warnings.extend(nb.warnings.clone());

    let ok = nb.leftover.is_empty() && !nb.auth;
    {
        let mut br = engine.breaker.lock().expect("breaker mutex");
        if ok {
            br.record_success();
        } else {
            br.record_failure(Instant::now());
        }
    }

    // Overlong shorten pass (2nd call, overlong items only).
    let mut translations = shorten_overlong(engine, &items, nb.done).await;
    if !nb.leftover.is_empty() {
        warnings.push(format!(
            "translate: {} segment(s) via Google gtx fallback",
            nb.leftover.len()
        ));
        let gtxed = apply_gtx(&engine.client, &nb.leftover, &engine.src_lang).await;
        let mut map: HashMap<u32, Translation> =
            translations.into_iter().map(|t| (t.id, t)).collect();
        for t in gtxed {
            map.insert(t.id, t);
        }
        translations = items
            .iter()
            .filter_map(|it| map.get(&it.id).cloned())
            .collect();
    }
    BatchOutcome {
        translations,
        warnings,
    }
}

/// Flag and (once) re-translate overlong VI texts, keeping the best attempt.
async fn shorten_overlong(
    engine: &Engine,
    items: &[Item],
    done: Vec<(u32, String)>,
) -> Vec<Translation> {
    let max: HashMap<u32, usize> = items.iter().map(|it| (it.id, it.max_chars)).collect();
    let orig: Vec<(u32, String, usize)> = done
        .iter()
        .map(|(id, vi)| (*id, vi.clone(), *max.get(id).unwrap_or(&MIN_MAX_CHARS)))
        .collect();
    let overlong: Vec<(u32, String, usize)> = orig
        .iter()
        .filter(|(_, vi, m)| is_overlong(vi, *m))
        .cloned()
        .collect();
    if overlong.is_empty() {
        return orig
            .iter()
            .map(|(id, vi, m)| Translation {
                id: *id,
                vi_text: vi.clone(),
                overlong: is_overlong(vi, *m),
            })
            .collect();
    }
    let ids: Vec<u32> = overlong.iter().map(|(id, _, _)| *id).collect();
    let user = shorten_user_prompt(&overlong);
    match model::call_chain(engine, system_prompt(), &user).await {
        ChainOutcome::Content { body, .. } => match parse_translations(&body, &ids) {
            Ok(shortened) => merge_shortened(&orig, &shortened),
            Err(_) => merge_shortened(&orig, &[]),
        },
        _ => merge_shortened(&orig, &[]),
    }
}

pub(super) fn prev_ctx(
    idx: usize,
    batches: &[Vec<Item>],
    slots: &Mutex<Vec<Option<BatchOutcome>>>,
) -> Option<PrevCtx> {
    if idx == 0 {
        return None;
    }
    let prev_batch = batches.get(idx - 1)?;
    let last = prev_batch.last()?;
    let vi = slots
        .lock()
        .expect("slots mutex")
        .get(idx - 1)
        .and_then(|s| s.as_ref())
        .and_then(|o| o.translations.last())
        .map(|t| t.vi_text.clone());
    Some(PrevCtx {
        src: last.src.clone(),
        vi,
    })
}
