//! Model output parsing: fence stripping, JSON object extraction, the
//! `{translations:[{id,vi_text}]}` shape plus the flat `{"<id>":"<text>"}`
//! dict shape, with exact id-set validation.

use std::collections::HashSet;

use serde_json::Value;

/// Strip a ```json / plain ``` fence, if present.
fn strip_fences(raw: &str) -> &str {
    let t = raw.trim();
    if !t.starts_with("```") {
        return t;
    }
    let after = match t.find('\n') {
        Some(i) => &t[i + 1..],
        None => &t[3..],
    };
    let after = after.trim_end();
    after.strip_suffix("```").unwrap_or(after).trim_end()
}

/// First `{` through last `}` (after fence stripping).
fn extract_json_object(raw: &str) -> Option<&str> {
    let s = strip_fences(raw);
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end < start {
        return None;
    }
    Some(&s[start..=end])
}

/// Parse `{translations:[{id,vi_text}]}` and validate the id set equals
/// `expected` exactly, entries unique and non-empty. Also accepts the flat
/// dict shape `{"<id>":"<text>"}` (observed live: bolt-e4b batches).
pub(crate) fn parse_translations(
    raw: &str,
    expected: &[u32],
) -> Result<Vec<(u32, String)>, String> {
    let json = extract_json_object(raw).ok_or("no JSON object found")?;
    let v: Value = serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
    let entries: Vec<(u32, String)> =
        if let Some(arr) = v.get("translations").and_then(Value::as_array) {
            arr.iter()
                .map(|item| {
                    let id = item
                        .get("id")
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or("entry missing/invalid `id`")?;
                    let text = item
                        .get("vi_text")
                        .and_then(Value::as_str)
                        .ok_or("entry missing `vi_text`")?;
                    Ok((id, text.to_string()))
                })
                .collect::<Result<Vec<_>, String>>()?
        } else if let Some(obj) = v.as_object() {
            obj.iter()
                .map(|(k, val)| {
                    let id: u32 = k.parse().map_err(|_| format!("non-numeric key `{k}`"))?;
                    let text = val
                        .as_str()
                        .ok_or_else(|| format!("non-string value for id {id}"))?;
                    Ok((id, text.to_string()))
                })
                .collect::<Result<Vec<_>, String>>()?
        } else {
            return Err("missing `translations` array".to_string());
        };
    if entries.is_empty() {
        return Err("no entries".to_string());
    }
    let mut out = Vec::with_capacity(entries.len());
    let mut seen = HashSet::new();
    for (id, text) in entries {
        if !seen.insert(id) {
            return Err(format!("duplicate id {id}"));
        }
        if text.trim().is_empty() {
            return Err(format!("empty vi_text for id {id}"));
        }
        out.push((id, text));
    }
    let mut got: Vec<u32> = out.iter().map(|(id, _)| *id).collect();
    got.sort_unstable();
    let mut exp = expected.to_vec();
    exp.sort_unstable();
    if got != exp {
        return Err(format!("id mismatch: expected {exp:?}, got {got:?}"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_strip_and_brace_extract() {
        let fenced = "```json\n{\"translations\":[{\"id\":0,\"vi_text\":\"xin chao\"}]}\n```";
        assert_eq!(
            extract_json_object(fenced),
            Some("{\"translations\":[{\"id\":0,\"vi_text\":\"xin chao\"}]}")
        );
        let prose = "Here is the result:\n{\"translations\":[]}\nDone.";
        assert_eq!(extract_json_object(prose), Some("{\"translations\":[]}"));
        assert_eq!(extract_json_object("no json here"), None);
        assert_eq!(extract_json_object("}{"), None);
        let bare = "```\n{\"translations\":[]}\n```";
        assert_eq!(extract_json_object(bare), Some("{\"translations\":[]}"));
    }

    #[test]
    fn response_parse_validates_id_set_and_nonempty() {
        let ok =
            r#"{"translations":[{"id":0,"vi_text":"xin chao"},{"id":1,"vi_text":"tam biet"}]}"#;
        assert_eq!(
            parse_translations(ok, &[0, 1]),
            Ok(vec![
                (0, "xin chao".to_string()),
                (1, "tam biet".to_string())
            ])
        );
        // Missing id.
        assert!(parse_translations(ok, &[0, 2]).is_err());
        // Extra id.
        assert!(parse_translations(ok, &[0]).is_err());
        // Empty vi_text.
        let empty = r#"{"translations":[{"id":0,"vi_text":" "}]}"#;
        assert!(parse_translations(empty, &[0]).is_err());
        // Duplicate id.
        let dup = r#"{"translations":[{"id":0,"vi_text":"a"},{"id":0,"vi_text":"b"}]}"#;
        assert!(parse_translations(dup, &[0]).is_err());
        // Not JSON.
        assert!(parse_translations("hello", &[0]).is_err());
    }

    #[test]
    fn parse_accepts_flat_dict_shape() {
        // Observed live: batch responses often come as {"<id>":"<text>"}.
        let raw = r#"{"0": "Rồi, đây trước voi", "1": "vòi rất dài"}"#;
        let got = parse_translations(raw, &[0, 1]).expect("dict shape");
        assert_eq!(
            got,
            vec![
                (0, "Rồi, đây trước voi".to_string()),
                (1, "vòi rất dài".to_string())
            ]
        );
        // Id-set validation still applies to dict shape.
        assert!(parse_translations(raw, &[0, 2]).is_err());
        assert!(parse_translations(r#"{"x": "oops"}"#, &[0]).is_err());
    }
}
