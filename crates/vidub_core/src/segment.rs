use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub id: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub src_text: String,
    pub src_lang: Option<String>,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TranscriptSource {
    CaptionManual,
    CaptionAuto,
    Whisper,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptDocument {
    pub source: TranscriptSource,
    pub detected_lang: String,
    pub segments: Vec<TranscriptSegment>,
    pub caption_format: Option<String>,
    #[serde(default)]
    pub title: String,
}
