use super::args::is_client_retryable;
use super::ProbeError;
use crate::util::{truncate_tail, MAX_STDERR_CHARS};

pub(super) const MSG_FORBIDDEN: &str =
    "YouTube từ chối truy cập (HTTP 403). Video có thể yêu cầu đăng nhập hoặc bị chặn ở khu vực của bạn.";
pub(super) const MSG_JS_MISSING: &str =
    "Thiếu trình chạy JavaScript cho yt-dlp (qjs). Hãy chạy lại bước cài đặt để bổ sung công cụ.";
pub(super) const MSG_PRIVATE: &str = "Video không còn khả dụng (riêng tư hoặc đã bị xóa).";
pub(super) const MSG_LOGIN_AGE: &str = "Video yêu cầu đăng nhập hoặc xác nhận độ tuổi.";
pub(super) const MSG_NETWORK: &str =
    "Không thể kết nối mạng hoặc kết nối quá chậm. Kiểm tra mạng rồi thử lại.";
pub(super) const MSG_DEFAULT: &str = "Không thể xử lý video này. Hãy thử liên kết khác.";

pub fn classify_failure(stderr: &str) -> &'static str {
    let s = stderr.to_lowercase();
    if s.contains("sign in to confirm")
        || s.contains("login required")
        || s.contains("age-gate")
        || s.contains("age gate")
        || s.contains("confirm your age")
    {
        return MSG_LOGIN_AGE;
    }
    if s.contains("private video")
        || s.contains("video unavailable")
        || s.contains("deleted")
        || s.contains("no longer available")
        || s.contains("has been removed")
    {
        return MSG_PRIVATE;
    }
    if is_client_retryable(&s) {
        return MSG_FORBIDDEN;
    }
    if s.contains("javascript")
        || s.contains("js runtime")
        || s.contains("js challenge")
        || s.contains("ejs")
    {
        return MSG_JS_MISSING;
    }
    if s.contains("timeout")
        || s.contains("timed out")
        || s.contains("urlopen error")
        || s.contains("name resolution")
        || s.contains("network is unreachable")
        || s.contains("connection reset")
        || s.contains("connection refused")
    {
        return MSG_NETWORK;
    }
    MSG_DEFAULT
}

pub fn user_message(err: &ProbeError) -> String {
    match err {
        ProbeError::CommandFailed { stderr, .. } => {
            classify_failure(&truncate_tail(stderr, MAX_STDERR_CHARS)).to_string()
        }
        ProbeError::Timeout { .. } => MSG_NETWORK.to_string(),
        ProbeError::MissingBinary(binary) => {
            format!("Thiếu công cụ bắt buộc: {binary}. Hãy chạy lại bước cài đặt để bổ sung.")
        }
        ProbeError::EmptySource
        | ProbeError::Io { .. }
        | ProbeError::InvalidJson(_)
        | ProbeError::AudioNotFound { .. } => MSG_DEFAULT.to_string(),
    }
}
