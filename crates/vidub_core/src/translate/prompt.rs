//! Prompt text for the NIM translation chain: the fixed system prompt plus
//! the translate/shorten/repair user prompts.

/// One batch item: segment `id`, source text, char budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) id: u32,
    pub(crate) src: String,
    pub(crate) max_chars: usize,
}

/// Prev-1 carryover context: previous segment's source + its translation
/// (translation present only when that batch already completed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrevCtx {
    pub(crate) src: String,
    pub(crate) vi: Option<String>,
}

pub(crate) fn system_prompt() -> &'static str {
    "You are a professional Vietnamese dub translator. Translate the lines \
    below into natural spoken Vietnamese (casual and concise, as spoken on \
    screen — not formal written Vietnamese). Translate the meaning, not \
    literally.\n\
    Interjections and short exclamations (OMG, oh my god, holy crap, wow, \
    whoa, damn, no way, let's go, oh, ah, ugh, yikes, bro, dude, wait) must \
    stay interjections: use the Vietnamese exclamation a Vietnamese speaker \
    would actually shout in that moment (e.g. OMG → \"Trời ơi!\", holy crap → \
    \"Chết tiệt!\", let's go → \"Đi thôi!\", wow → \"Wow!\"), never a \
    descriptive sentence, never a literal word-by-word rendering, and never \
    something unrelated to the emotion expressed.\n\
    Keep proper names, numbers and on-screen game/show terminology as-is. \
    Only drop true verbal fillers (uh, um, uhh); do NOT drop meaning-bearing \
    words.\n\
    Hard constraint: each translation must fit within its max_chars character \
    budget — shorten or paraphrase to fit if needed, without losing meaning \
    and without inventing content. Return ONLY a JSON object: \
    {\"translations\":[{\"id\":0,\"vi_text\":\"...\"}]} with exactly one \
    entry per id given, no extra keys, no markdown, no commentary."
}

pub(crate) fn user_prompt(items: &[Item], prev: Option<&PrevCtx>) -> String {
    let mut out = String::new();
    if let Some(p) = prev {
        out.push_str("Previous line (context): ");
        out.push_str(&p.src);
        out.push_str(" -> ");
        out.push_str(p.vi.as_deref().unwrap_or("(untranslated)"));
        out.push('\n');
    }
    out.push_str(&format!(
        "Translate these {} lines to Vietnamese. Format: [id] (<= max chars) source\n",
        items.len()
    ));
    for it in items {
        out.push_str(&format!(
            "[{}] (<= {} chars) {}\n",
            it.id, it.max_chars, it.src
        ));
    }
    out.push_str("Return only the JSON object.");
    out
}

pub(crate) fn shorten_user_prompt(overlong: &[(u32, String, usize)]) -> String {
    let mut out = format!(
        "These {} translations exceed their character budgets. Shorten each \
        to its limit, keeping the meaning:\n",
        overlong.len()
    );
    for (id, vi, max) in overlong {
        out.push_str(&format!("[{}] (<= {} chars) {}\n", id, max, vi));
    }
    out.push_str("Return only the JSON object.");
    out
}

pub(crate) fn repair_user_prompt(raw: &str, err: &str, expected: &[u32]) -> String {
    let ids: Vec<String> = expected.iter().map(|id| id.to_string()).collect();
    format!(
        "Your previous reply was invalid: {err}\nPrevious reply:\n{raw}\n\
        Reply with ONLY the JSON object \
        {{\"translations\":[{{\"id\":0,\"vi_text\":\"...\"}}]}} containing \
        exactly the ids [{}], all non-empty.",
        ids.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u32, src: &str, max_chars: usize) -> Item {
        Item {
            id,
            src: src.to_string(),
            max_chars,
        }
    }

    /// Exclamations were translated into unrelated sentences because the
    /// prompt invited "paraphrase" with no interjection rule.
    #[test]
    fn system_prompt_guards_interjections() {
        let p = system_prompt();
        for needle in [
            "Interjections",
            "never something unrelated",
            "Trời ơi!",
            "without inventing content",
        ] {
            assert!(p.contains(needle), "system prompt lost: {needle}");
        }
        assert!(
            !p.contains("Drop filler words (uh, um, like, you know)"),
            "the old filler rule invited dropping meaning-bearing words"
        );
    }

    #[test]
    fn user_prompt_echoes_ids_budgets_and_carryover() {
        let items = vec![item(3, "hello world", 28), item(4, "bye", 12)];
        let prev = PrevCtx {
            src: "good morning".to_string(),
            vi: Some("chào buổi sáng".to_string()),
        };
        let p = user_prompt(&items, Some(&prev));
        assert!(p.contains("[3] (<= 28 chars) hello world"));
        assert!(p.contains("[4] (<= 12 chars) bye"));
        assert!(p.contains("good morning -> chào buổi sáng"));
        assert!(p.contains("Return only the JSON object"));
    }
}
