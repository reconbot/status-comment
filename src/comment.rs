//! The hidden parts of a Status Comment: its Comment Key marker, its state, and
//! the markers around each Step's Output (see ADR 0001).

use std::collections::HashMap;

use crate::state::State;

const KEY_PREFIX: &str = "<!-- status-comment:key ";
const STATE_PREFIX: &str = "<!-- status-comment:state ";
const OUTPUT_PREFIX: &str = "<!-- status-comment:output ";
const OUTPUT_END_PREFIX: &str = "<!-- /status-comment:output ";
const MARKER_SUFFIX: &str = " -->";

/// Percent-encodes everything but `[A-Za-z0-9_.]` so names and keys can never
/// close an HTML comment or contain whitespace.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn key_marker(key: &str) -> String {
    format!("{KEY_PREFIX}{}{MARKER_SUFFIX}", encode(key))
}

pub fn has_key_marker(body: &str, key: &str) -> bool {
    body.contains(&key_marker(key))
}

fn state_marker(state: &State) -> String {
    let json = serde_json::to_string(state).expect("state always serializes");
    // `>` only ever appears inside JSON strings, where `>` is equivalent;
    // escaping it means the JSON can never contain `-->`.
    let json = json.replace('>', "\\u003e");
    format!("{STATE_PREFIX}{json}{MARKER_SUFFIX}")
}

fn parse_state(body: &str) -> Option<State> {
    let start = body.find(STATE_PREFIX)? + STATE_PREFIX.len();
    let len = body[start..].find(MARKER_SUFFIX)?;
    serde_json::from_str(&body[start..start + len]).ok()
}

fn fence_for(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

/// A Step's Output as a fenced code block between markers it can be read back from.
pub fn output_block(name: &str, text: &str) -> String {
    let name = encode(name);
    let fence = fence_for(text);
    let text = text.strip_suffix('\n').unwrap_or(text);
    format!(
        "{OUTPUT_PREFIX}{name}{MARKER_SUFFIX}\n{fence}\n{text}\n{fence}\n{OUTPUT_END_PREFIX}{name}{MARKER_SUFFIX}"
    )
}

/// Reads every Step's Output back out of a rendered body, keyed by Step name.
fn parse_outputs(body: &str) -> HashMap<String, String> {
    let mut outputs = HashMap::new();
    let mut rest = body;
    while let Some(found) = rest.find(OUTPUT_PREFIX) {
        rest = &rest[found + OUTPUT_PREFIX.len()..];
        let Some(name_len) = rest.find(MARKER_SUFFIX) else {
            break;
        };
        let encoded = &rest[..name_len];
        rest = &rest[name_len + MARKER_SUFFIX.len()..];

        let Some(after_newline) = rest.strip_prefix('\n') else {
            continue;
        };
        let Some(fence_len) = after_newline.find('\n') else {
            continue;
        };
        let fence = &after_newline[..fence_len];
        if fence.len() < 3 || !fence.bytes().all(|b| b == b'`') {
            continue;
        }
        let content = &after_newline[fence_len + 1..];
        let end = format!("\n{fence}\n{OUTPUT_END_PREFIX}{encoded}{MARKER_SUFFIX}");
        // An empty Output renders as an empty line between the fences.
        let (text, consumed) = match content.find(&end) {
            Some(i) => (&content[..i], i + end.len()),
            None => continue,
        };
        if let Some(name) = decode(encoded) {
            outputs.entry(name).or_insert_with(|| text.to_string());
        }
        rest = &content[consumed..];
    }
    outputs
}

/// Recovers a Run's state, including each Step's Output, from a Status Comment body.
pub fn load(body: &str) -> Option<State> {
    let mut state = parse_state(body)?;
    let mut outputs = parse_outputs(body);
    for step in &mut state.steps {
        step.output = outputs.remove(&step.name);
    }
    Some(state)
}

/// Appends the hidden key and state markers to a rendered body.
pub fn compose(rendered: &str, state: &State, key: &str) -> String {
    format!(
        "{}\n\n{}\n{}\n",
        rendered.trim_end(),
        state_marker(state),
        key_marker(key)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StepUpdate;
    use crate::status::Status;

    fn state_with_output(name: &str, output: &str) -> State {
        let mut state = State::new("42");
        state.record(
            name,
            StepUpdate {
                status: Some(Status::Success),
                output: Some(output.into()),
                ..StepUpdate::default()
            },
        );
        state
    }

    fn round_trip(state: &State) -> State {
        let rendered: String = state
            .steps
            .iter()
            .filter_map(|s| s.output.as_ref().map(|o| output_block(&s.name, o)))
            .collect::<Vec<_>>()
            .join("\n\n");
        load(&compose(&rendered, state, "wf/job")).expect("state should parse")
    }

    #[test]
    fn encode_decode_round_trip() {
        for s in ["plan", "rules-check", "bar step", "a-->b", "ünï"] {
            assert_eq!(decode(&encode(s)).unwrap(), s);
            assert!(!encode(s).contains("--"));
        }
    }

    #[test]
    fn state_and_output_round_trip() {
        let state = state_with_output("plan", "No changes.\nDone.");
        let loaded = round_trip(&state);
        assert_eq!(loaded, state);
    }

    #[test]
    fn output_with_backticks_and_markers_round_trips() {
        let nasty = "```\ncode\n````\n<!-- /status-comment:output plan -->\nend";
        let state = state_with_output("plan", nasty);
        assert_eq!(
            round_trip(&state).step("plan").unwrap().output.as_deref(),
            Some(nasty)
        );
    }

    #[test]
    fn step_names_that_could_close_a_comment_round_trip() {
        let state = state_with_output("weird --> name", "out");
        let body = compose(&output_block("weird --> name", "out"), &state, "k");
        assert_eq!(body.matches("-->").count(), body.matches("<!--").count());
        assert_eq!(load(&body).unwrap(), state);
    }

    #[test]
    fn json_in_state_cannot_close_the_marker() {
        let mut state = State::new("1");
        state.record(
            "x",
            StepUpdate {
                display_name: Some("a --> b".into()),
                ..StepUpdate::default()
            },
        );
        let body = compose("", &state, "k");
        assert_eq!(
            load(&body).unwrap().step("x").unwrap().display_name,
            "a --> b"
        );
    }

    #[test]
    fn steps_without_rendered_output_have_none() {
        let state = state_with_output("plan", "log");
        let body = compose("no output rendered", &state, "k");
        assert_eq!(load(&body).unwrap().step("plan").unwrap().output, None);
    }

    #[test]
    fn empty_output_round_trips() {
        let state = state_with_output("plan", "");
        assert_eq!(
            round_trip(&state).step("plan").unwrap().output.as_deref(),
            Some("")
        );
    }

    #[test]
    fn trailing_newline_is_dropped() {
        let state = state_with_output("plan", "line\n");
        assert_eq!(
            round_trip(&state).step("plan").unwrap().output.as_deref(),
            Some("line")
        );
    }

    #[test]
    fn key_marker_matches_exact_key() {
        let body = compose("x", &State::new("1"), "wf/plan");
        assert!(has_key_marker(&body, "wf/plan"));
        assert!(!has_key_marker(&body, "wf/pla"));
        assert!(!has_key_marker(&body, "wf/plan-2"));
    }

    #[test]
    fn body_without_state_loads_nothing() {
        assert!(load("just a human comment").is_none());
    }
}
