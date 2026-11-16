pub(crate) const MAX_STDERR_CHARS: usize = 2048;

pub(crate) fn truncate_tail(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        s.to_string()
    } else {
        s.chars().skip(total - max_chars).collect()
    }
}

pub(crate) fn secs(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_tail_keeps_last_chars() {
        assert_eq!(truncate_tail("abc", 10), "abc");
        assert_eq!(truncate_tail("abcdef", 3), "def");
        assert_eq!(truncate_tail("aé🎬", 2), "é🎬");
    }

    #[test]
    fn secs_has_millisecond_precision() {
        assert_eq!(secs(1500), "1.500");
        assert_eq!(secs(0), "0.000");
        assert_eq!(secs(61_234), "61.234");
    }
}
