//! Native tools: files in approved folders, open an app or URL, run a confirmed command,
//! screenshot a window the user picks, fetch a web page.
use markon_provider::ToolSpec;
use serde_json::{Value, json};

pub fn specs() -> Vec<ToolSpec> {
    let t = |name: &str, description: &str, props: Value, required: &[&str]| ToolSpec {
        name: name.into(),
        description: description.into(),
        input_schema: json!({"type": "object", "properties": props, "required": required, "additionalProperties": false}),
    };
    let path = json!({"path": {"type": "string", "description": "Absolute path"}});
    vec![
        t(
            "list_files",
            "List a folder the user approved.",
            path.clone(),
            &["path"],
        ),
        t(
            "read_file",
            "Read a text file in a folder the user approved.",
            path,
            &["path"],
        ),
        t(
            "write_file",
            "Write a text file in a folder the user approved. The user is asked once per folder.",
            json!({"path": {"type": "string"}, "content": {"type": "string"}}),
            &["path", "content"],
        ),
        t(
            "open_url",
            "Open a web link in the default browser.",
            json!({"url": {"type": "string"}}),
            &["url"],
        ),
        t(
            "open_app",
            "Open an installed Mac app by name.",
            json!({"name": {"type": "string"}}),
            &["name"],
        ),
        t(
            "run_command",
            "Run a shell command. The user sees the exact command and must allow it every time.",
            json!({"command": {"type": "string"}}),
            &["command"],
        ),
        t(
            "screenshot_window",
            "Take a screenshot of one window. The user is asked and then clicks the window to capture.",
            json!({}),
            &[],
        ),
        t(
            "show_image",
            "Show a picture on Markon's screen. Give a `query` to find a fitting picture (searched on \
             Wikimedia Commons), or the `url` of an image you found with web search. Prefer this over \
             opening the browser.",
            json!({"query": {"type": "string"}, "url": {"type": "string"}, "caption": {"type": "string"}}),
            &[],
        ),
        t(
            "play_media",
            "Play audio or video on Markon's screen: a YouTube link, a direct link to an audio or video \
             file, or the absolute path of a local file in a folder the user approved. Use web search to \
             find a YouTube link first. For Spotify use the `spotify` tool instead.",
            json!({"source": {"type": "string"}, "title": {"type": "string"}}),
            &["source"],
        ),
        t(
            "spotify",
            "Control the Spotify app and show what is playing on Markon's screen. `action` is play, \
             pause, next, previous or status. To play something specific, pass its open.spotify.com \
             link or spotify: URI as `uri` (find it with web search).",
            json!({"action": {"type": "string", "enum": ["play", "pause", "next", "previous", "status"]}, "uri": {"type": "string"}}),
            &["action"],
        ),
        t(
            "fetch_page",
            "Fetch a web page and return its readable text.",
            json!({"url": {"type": "string"}}),
            &["url"],
        ),
    ]
}

/// Readable text from HTML: drops scripts, styles and tags, decodes common entities.
pub fn html_to_text(html: &str) -> (String, String) {
    let lower = html.to_lowercase();
    let title = lower
        .find("<title")
        .and_then(|i| lower[i..].find('>').map(|j| i + j + 1))
        .and_then(|s| {
            lower[s..]
                .find("</title>")
                .map(|e| html[s..s + e].trim().to_string())
        })
        .unwrap_or_default();
    let mut out = String::with_capacity(html.len() / 3);
    let mut i = 0;
    let bytes = lower.as_bytes();
    while i < html.len() {
        if bytes[i] == b'<' {
            let skip_to = ["script", "style", "noscript", "svg"]
                .iter()
                .find_map(|tag| {
                    lower[i + 1..].starts_with(tag).then(|| {
                        lower[i..]
                            .find(&format!("</{tag}>"))
                            .map(|e| i + e + tag.len() + 3)
                    })
                });
            let end = match skip_to {
                Some(Some(e)) => e,
                Some(None) => html.len(),
                None => lower[i..]
                    .find('>')
                    .map(|e| i + e + 1)
                    .unwrap_or(html.len()),
            };
            let tag = &lower[i..end.min(html.len())];
            if ["<p", "<br", "<div", "<li", "<h1", "<h2", "<h3", "<tr"]
                .iter()
                .any(|t| tag.starts_with(t))
            {
                out.push('\n');
            } else {
                out.push(' ');
            }
            i = end;
        } else {
            let next = lower[i..].find('<').map(|n| i + n).unwrap_or(html.len());
            out.push_str(&html[i..next]);
            i = next;
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let text = decoded
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (title, text)
}

#[cfg(test)]
mod tests {
    #[test]
    fn extracts_title_and_text() {
        let (t, x) = super::html_to_text(
            "<html><head><title>VR &amp; you</title><style>p{}</style></head><body><script>var a='<p>';</script><h1>Hi</h1><p>One&nbsp;two</p></body></html>",
        );
        assert_eq!(t, "VR &amp; you");
        assert!(x.contains("Hi") && x.contains("One two") && !x.contains("var a"));
    }
}
