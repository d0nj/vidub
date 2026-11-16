use crate::util::secs;

pub const HARD_CUT_TEMPO: f32 = 1.50;
pub const BORROW_MARGIN_MS: u64 = 40;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaceAction {
    Pad,
    Fit { tempo: f32 },
    Chained { a: f32, b: f32 },
    BorrowGap { new_end_ms: u64 },
    ShortenRetry,
    HardCut,
}

pub fn pace_action(r: f32, window_ms: u64, gap_ms: u64) -> PaceAction {
    if window_ms == 0 || r == f32::INFINITY {
        return PaceAction::HardCut;
    }
    if !r.is_finite() || r <= 0.0 {
        return PaceAction::Pad;
    }
    if r < 0.70 {
        return PaceAction::Pad;
    }
    if r < 1.25 {
        return PaceAction::Fit { tempo: r };
    }
    if r < 1.35 {
        let overrun_ms = (((r - 1.0) * window_ms as f32).round().max(0.0)) as u64;
        let borrow = borrowed_ms(overrun_ms, gap_ms);
        if borrow > 0 {
            return PaceAction::BorrowGap {
                new_end_ms: window_ms + borrow,
            };
        }
        let s = r.sqrt();
        return PaceAction::Chained { a: s, b: s };
    }
    if r <= 1.50 {
        return PaceAction::ShortenRetry;
    }
    PaceAction::HardCut
}

fn borrowed_ms(overrun_ms: u64, gap_ms: u64) -> u64 {
    overrun_ms.min(gap_ms.saturating_sub(BORROW_MARGIN_MS))
}

pub fn atempos_for(r: f32) -> Vec<f32> {
    if !r.is_finite() {
        return vec![1.0];
    }
    let r = r.clamp(0.5, 100.0);
    if r <= 2.0 {
        return vec![r];
    }
    let n = r.log2().ceil() as usize;
    vec![r.powf(1.0 / n as f32); n]
}

pub fn atempo_filter(r: f32) -> String {
    atempos_for(r)
        .iter()
        .map(|f| format!("atempo={f:.3}"))
        .collect::<Vec<_>>()
        .join(",")
}

pub fn tile_filter(window_ms: u64) -> String {
    format!("apad=whole_dur={s},atrim=0:{s}", s = secs(window_ms))
}

pub fn fade_filter(window_ms: u64) -> Option<String> {
    if window_ms < 50 {
        return None;
    }
    Some(format!("afade=t=out:st={}:d=0.050", secs(window_ms - 50)))
}

pub fn pace_filter(r_eff: f32, action: &PaceAction, eff_window_ms: u64) -> String {
    let tile = tile_filter(eff_window_ms);
    match *action {
        PaceAction::Pad => tile,
        PaceAction::Fit { tempo } => format!("atempo={tempo:.3},{tile}"),
        PaceAction::Chained { a, b } => format!("atempo={a:.3},atempo={b:.3},{tile}"),
        PaceAction::BorrowGap { .. } | PaceAction::ShortenRetry => {
            format!("{},{}", atempo_filter(r_eff), tile)
        }
        PaceAction::HardCut => {
            let mut chain = format!("atempo={HARD_CUT_TEMPO:.3}");
            if let Some(fade) = fade_filter(eff_window_ms) {
                chain.push(',');
                chain.push_str(&fade);
            }
            format!("{chain},{tile}")
        }
    }
}
