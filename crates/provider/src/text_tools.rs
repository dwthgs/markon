//! Tool calling in plain text, for models reached without a tools API (the built-in model, Claude
//! Code): tools described in `<tools>`, calls written as `<tool_call>` JSON (the Qwen shape).
use crate::ToolSpec;
use serde_json::{Value, json};

/// The tool section appended to the system prompt.
pub fn tools_prompt(tools: &[ToolSpec]) -> String {
    let mut s = String::from(
        "\n\n# Tools\n\nYou may call one or more functions to assist with the user query.\n\n\
         You are provided with function signatures within <tools></tools> XML tags:\n<tools>",
    );
    for t in tools {
        s.push('\n');
        s.push_str(
            &json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.input_schema}})
                .to_string(),
        );
    }
    s.push_str(
        "\n</tools>\n\nFor each function call, return a json object with function name and arguments \
         within <tool_call></tool_call> XML tags:\n<tool_call>\n{\"name\": <function-name>, \"arguments\": \
         <args-json-object>}\n</tool_call>",
    );
    s
}

/// Where a tool call can start: the `<tool_call>` JSON we ask for, and Claude's own `<invoke>` markup,
/// which Claude models write out of habit even when asked for the JSON form.
const OPENERS: &[&str] = &["<tool_call>", "<invoke", "<function_calls>"];

/// Split what the model wrote into spoken text and tool calls.
pub fn parse_calls(out: &str) -> (String, Vec<(String, Value)>) {
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut rest = out;
    while let Some((start, open)) = OPENERS
        .iter()
        .filter_map(|o| rest.find(o).map(|i| (i, *o)))
        .min()
    {
        text.push_str(&rest[..start]);
        let after = &rest[start + open.len()..];
        let close = match open {
            "<tool_call>" => "</tool_call>",
            "<invoke" => "</invoke>",
            _ => "", // a `<function_calls>` wrapper: drop the tag, keep reading inside it
        };
        if close.is_empty() {
            rest = after;
            continue;
        }
        let end = after.find(close).unwrap_or(after.len());
        let body = &after[..end];
        if open == "<invoke" {
            if let Some(call) = parse_invoke(body) {
                calls.push(call);
            }
        } else {
            // Every JSON object in the tag, up to anything that is not one (a stray brace).
            let found = calls.len();
            for v in serde_json::Deserializer::from_str(body).into_iter::<Value>() {
                let Ok(v) = v else { break };
                let Some(name) = v["name"].as_str() else {
                    continue;
                };
                let args = match (&v["arguments"], &v["parameters"]) {
                    (Value::String(s), _) => serde_json::from_str(s).unwrap_or(json!({})),
                    (Value::Null, Value::Null) => json!({}),
                    (Value::Null, p) => p.clone(),
                    (a, _) => a.clone(),
                };
                calls.push((name.to_string(), args));
            }
            if calls.len() == found {
                eprintln!("[tools] unreadable tool call: {}", body.trim());
            }
        }
        rest = after.get(end + close.len()..).unwrap_or("");
    }
    text.push_str(rest);
    let text = text.replace("</function_calls>", "");
    (text.trim().to_string(), calls)
}

/// ` name="fetch_page"> <parameter name="url">https://x</parameter> ` (the text after `<invoke`).
/// A value that reads as JSON (a number, a list) is kept as JSON, anything else as a string.
fn parse_invoke(body: &str) -> Option<(String, Value)> {
    let attr = |s: &str| -> Option<(String, usize)> {
        let i = s.find("name=\"")? + 6;
        let j = s[i..].find('"')? + i;
        let gt = s[j..].find('>')? + j + 1;
        Some((s[i..j].to_string(), gt))
    };
    let (name, mut at) = attr(body)?;
    let mut args = serde_json::Map::new();
    while let Some(p) = body[at..].find("<parameter") {
        let p = at + p + "<parameter".len();
        let Some((key, v0)) = attr(&body[p..]) else {
            break;
        };
        let v0 = p + v0;
        let v1 = body[v0..]
            .find("</parameter>")
            .map_or(body.len(), |e| v0 + e);
        let raw = body[v0..v1].trim();
        let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.into()));
        args.insert(key, value);
        at = (v1 + "</parameter>".len()).min(body.len());
    }
    Some((name, Value::Object(args)))
}

/// How much of `buf` is safe to speak now: everything except a tail that could be the start of a
/// tool call, and nothing once one has begun.
pub fn speakable(buf: &str) -> usize {
    if let Some(i) = OPENERS.iter().filter_map(|o| buf.find(o)).min() {
        return i;
    }
    OPENERS
        .iter()
        .flat_map(|tag| (1..tag.len()).filter(|n| buf.ends_with(&tag[..*n])))
        .max()
        .map(|n| buf.len() - n)
        .unwrap_or(buf.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claudes_own_invoke_markup_is_a_call_and_never_spoken() {
        let out = "Checking. <function_calls> <invoke name=\"what_did_i_miss\"> </invoke> \
                   <invoke name=\"fetch_page\"> <parameter name=\"url\">https://wttr.in/?format=3</parameter> \
                   <parameter name=\"max\">5</parameter> </invoke> </function_calls>";
        let (text, calls) = parse_calls(out);
        assert_eq!(text, "Checking.");
        assert_eq!(
            calls,
            [
                ("what_did_i_miss".to_string(), json!({})),
                (
                    "fetch_page".to_string(),
                    json!({"url": "https://wttr.in/?format=3", "max": 5})
                ),
            ]
        );
        assert_eq!(speakable("Checking. <inv"), 10);
        assert_eq!(speakable("Checking. <invoke name="), 10);
        assert_eq!(speakable("Checking. <function_calls>"), 10);
        assert_eq!(speakable("1 < 2"), 5);
    }

    /// Seen on the Claude Code route: a call that did not parse ended the turn with nothing said.
    #[test]
    fn loosely_written_tool_calls_still_run() {
        let url = || json!({"url": "https://x"});
        // Two calls in one tag.
        let (text, calls) = parse_calls(
            "<tool_call>\n{\"name\": \"fetch_page\", \"arguments\": {\"url\": \"https://x\"}}\n\
             {\"name\": \"what_did_i_miss\", \"arguments\": {}}\n</tool_call>",
        );
        assert_eq!(text, "");
        assert_eq!(
            calls,
            [
                ("fetch_page".to_string(), url()),
                ("what_did_i_miss".to_string(), json!({}))
            ]
        );
        // A stray closing brace, and `parameters` for `arguments`.
        for body in [
            "{\"name\": \"fetch_page\", \"arguments\": {\"url\": \"https://x\"}}}",
            "{\"name\": \"fetch_page\", \"parameters\": {\"url\": \"https://x\"}}",
        ] {
            let (_, calls) = parse_calls(&format!("<tool_call>{body}</tool_call>"));
            assert_eq!(calls, [("fetch_page".to_string(), url())], "{body}");
        }
    }
}
