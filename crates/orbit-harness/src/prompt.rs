//! The run prompt the CLI adapters (OpenCode, Pi) pass as one argument.
//!
//! The prompt carries the run instruction (`task`, capped by the API) and the untrusted
//! input payload. The payload is not capped upstream: it holds the previous workflow
//! step's whole answer, memory entries and hydrated connector context. So the payload
//! gets whatever room the argument limit leaves after the rest of the prompt, and when
//! it does not fit, its largest strings are shortened (keeping their start and end) with
//! a visible marker, so the run still starts.

use orbit_contracts::ExecutionRequest;
use serde_json::Value;

use crate::stream::MAX_PROMPT_ARG_BYTES;

/// Strings are never shortened below this; past that point the payload is cut as text.
const MIN_KEPT_STRING_BYTES: usize = 256;
/// Bound on shortening passes; each pass shortens the largest string that is left.
const MAX_SHORTENING_PASSES: usize = 64;

/// The run prompt, and what was done to the payload to make it fit.
pub(crate) struct RunPrompt {
    pub text: String,
    /// `Some((kept, original))` payload bytes when the payload was shortened.
    pub payload_shortened: Option<(usize, usize)>,
}

pub(crate) fn build_run_prompt(request: &ExecutionRequest) -> RunPrompt {
    let head = format!(
        "# Run instruction\n\n{}\n\n# Input data (untrusted)\n\nSource: {}\nTrusted: {}\n\n```json\n",
        request.input.task.trim(),
        request.input.provenance.source,
        request.input.provenance.trusted,
    );
    let tail = "\n```\n";
    let budget = MAX_PROMPT_ARG_BYTES.saturating_sub(head.len() + tail.len());
    let (payload, payload_shortened) = fit_payload(&request.input.payload, budget);
    RunPrompt {
        text: format!("{head}{payload}{tail}"),
        payload_shortened,
    }
}

/// Render `payload` as JSON in at most `budget` bytes when possible.
fn fit_payload(payload: &Value, budget: usize) -> (String, Option<(usize, usize)>) {
    let pretty = serde_json::to_string_pretty(payload).unwrap_or_else(|_| "null".into());
    if pretty.len() <= budget {
        return (pretty, None);
    }
    let original = pretty.len();
    let compact = serde_json::to_string(payload).unwrap_or_else(|_| "null".into());
    if compact.len() <= budget {
        return (compact, None);
    }

    // Shorten the largest strings until the compact JSON fits; the result stays valid JSON.
    let mut value = payload.clone();
    let mut length = compact.len();
    for _ in 0..MAX_SHORTENING_PASSES {
        if length <= budget {
            break;
        }
        let Some(longest) = longest_string_mut(&mut value) else {
            break;
        };
        if longest.len() <= MIN_KEPT_STRING_BYTES {
            break;
        }
        let excess = length - budget;
        // Removing N raw bytes removes at least N serialized bytes; the marker costs a few.
        let keep = longest
            .len()
            .saturating_sub(excess + 64)
            .max(MIN_KEPT_STRING_BYTES);
        *longest = shorten_middle(longest, keep);
        length = serde_json::to_string(&value).map_or(usize::MAX, |text| text.len());
    }
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "null".into());
    if text.len() <= budget {
        return (text.clone(), Some((text.len(), original)));
    }
    // Many small values: cut the text itself. The marker says so; the JSON is incomplete.
    let marker = format!("\n… [input data truncated: {original} bytes in total]");
    let mut end = budget.saturating_sub(marker.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = format!("{}{marker}", &text[..end]);
    let kept = cut.len();
    (cut, Some((kept, original)))
}

fn longest_string_mut(value: &mut Value) -> Option<&mut String> {
    match value {
        Value::String(text) => Some(text),
        Value::Array(items) => items
            .iter_mut()
            .filter_map(longest_string_mut)
            .max_by_key(|text| text.len()),
        Value::Object(map) => map
            .values_mut()
            .filter_map(longest_string_mut)
            .max_by_key(|text| text.len()),
        _ => None,
    }
}

/// Keep the start and the end of `text` (an answer's last lines, such as an
/// `OUTCOME:` line, matter as much as its first) and say how much was left out.
fn shorten_middle(text: &str, keep: usize) -> String {
    if text.len() <= keep {
        return text.to_string();
    }
    let mut head_end = keep / 2;
    while head_end > 0 && !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = text.len() - (keep - keep / 2);
    while tail_start < text.len() && !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let left_out = tail_start - head_end;
    format!(
        "{}\n… [{left_out} bytes truncated to fit the prompt limit] …\n{}",
        &text[..head_end],
        &text[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_payload_that_fits_is_unchanged() {
        let payload = json!({"previous": "short answer"});
        let (text, shortened) = fit_payload(&payload, 10_000);
        assert_eq!(text, serde_json::to_string_pretty(&payload).unwrap());
        assert!(shortened.is_none());
    }

    #[test]
    fn the_largest_string_is_shortened_keeping_start_and_end() {
        let answer = format!("START {} OUTCOME: pass", "x".repeat(150_000));
        let payload = json!({"previous": answer, "memory": ["kept entry"], "step": "review"});
        let (text, shortened) = fit_payload(&payload, 90_000);
        assert!(text.len() <= 90_000, "{} bytes", text.len());
        let (kept, original) = shortened.unwrap();
        assert_eq!(kept, text.len());
        assert!(original > 150_000);
        let parsed: Value = serde_json::from_str(&text).expect("still valid JSON");
        let previous = parsed["previous"].as_str().unwrap();
        assert!(previous.starts_with("START "));
        assert!(previous.ends_with("OUTCOME: pass"));
        assert!(previous.contains("bytes truncated to fit the prompt limit"));
        assert_eq!(parsed["memory"][0], "kept entry");
        assert_eq!(parsed["step"], "review");
    }

    #[test]
    fn many_small_values_are_cut_as_text_with_a_marker() {
        let payload = Value::Array((0..50_000).map(|n| json!(n)).collect());
        let (text, shortened) = fit_payload(&payload, 20_000);
        assert!(text.len() <= 20_000);
        assert!(text.contains("[input data truncated:"));
        assert!(shortened.is_some());
    }

    #[test]
    fn shortening_is_utf8_safe() {
        let text = "é".repeat(1_000);
        let short = shorten_middle(&text, 301);
        assert!(short.contains("bytes truncated"));
        assert!(short.starts_with('é') && short.ends_with('é'));
    }
}
