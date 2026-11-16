pub mod batch;
pub mod fallback;
pub mod model;
pub mod parse;
pub mod prompt;
pub mod run;

pub use batch::translate_document;
pub use model::{
    attribution_headers, endpoint_for, request_body, response_content, COMPLETION_TOKEN_CAP,
    ENDPOINT, GEMINI_ENDPOINT_BASE, OPENROUTER_ENDPOINT,
};

pub(super) const CPS: f64 = 14.0;
pub(super) const MIN_MAX_CHARS: usize = 12;
pub const BATCH_N: usize = 6;
pub(super) const BATCH_N_LONG: usize = 3;
pub(super) const LONG_AVG_CHARS: f64 = 250.0;
pub(super) const OVERLONG_FACTOR: f64 = 1.10;
pub(super) const CONCURRENCY: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translation {
    pub id: u32,
    pub vi_text: String,
    pub overlong: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslateConfig {
    pub primary: String,
    pub fallbacks: Vec<String>,
}

pub fn translate_config(provider: vidub_config::Provider) -> TranslateConfig {
    let (primary, fallbacks) =
        vidub_config::chain_for_provider(provider, vidub_config::load().preset);
    TranslateConfig { primary, fallbacks }
}

#[cfg(test)]
mod batch_tests;
