//! The page as a labelled outline, not a screenshot:
//! indented `[role] "name"` lines from the accessibility tree, with a number
//! on everything the model can click or type into.
use serde_json::Value;
use std::collections::HashMap;

const ACTIONABLE: &[&str] = &[
    "link",
    "button",
    "textbox",
    "searchbox",
    "combobox",
    "checkbox",
    "radio",
    "menuitem",
    "tab",
    "option",
    "switch",
    "slider",
    "spinbutton",
    "listbox",
    "menuitemcheckbox",
    "menuitemradio",
];
/// Wrappers that add a line but no meaning; their children are kept.
const QUIET: &[&str] = &[
    "none",
    "generic",
    "presentation",
    "InlineTextBox",
    "LineBreak",
    "StaticText",
];

fn text(v: &Value, k: &str) -> String {
    v[k]["value"]
        .as_str()
        .map(str::to_string)
        .or_else(|| v[k]["value"].as_f64().map(|n| n.to_string()))
        .unwrap_or_default()
}

/// What a number in the outline points at: the DOM node, and its line (for the secret-field check).
pub type Refs = HashMap<u32, (i64, String)>;

/// `(outline, ref → element)`, at most `max_lines` lines.
pub fn outline(nodes: &[Value], max_lines: usize) -> (String, Refs) {
    let by_id: HashMap<&str, &Value> = nodes
        .iter()
        .filter_map(|n| Some((n["nodeId"].as_str()?, n)))
        .collect();
    let root = nodes
        .iter()
        .find(|n| n["parentId"].is_null())
        .or(nodes.first());
    let mut lines = Vec::new();
    let mut refs = HashMap::new();
    let mut next = 1u32;
    let mut stack: Vec<(&Value, usize)> = root.map(|r| vec![(r, 0)]).unwrap_or_default();
    while let Some((n, depth)) = stack.pop() {
        if lines.len() >= max_lines {
            lines.push("… (more on the page; read again after scrolling or narrowing down)".into());
            break;
        }
        let role = text(n, "role");
        let name = text(n, "name");
        let ignored = n["ignored"].as_bool().unwrap_or(false);
        let shown = !ignored
            && !(QUIET.contains(&role.as_str()) && name.is_empty())
            && role != "StaticText";
        let child_depth = if shown { depth + 1 } else { depth };
        if shown {
            let mut line = "  ".repeat(depth.min(12));
            let numbered = ACTIONABLE
                .contains(&role.as_str())
                .then(|| n["backendDOMNodeId"].as_i64())
                .flatten();
            if numbered.is_some() {
                line.push_str(&format!("[{next}] "));
            }
            line.push_str(&format!("[{role}]"));
            if !name.is_empty() {
                line.push_str(&format!(
                    " \"{}\"",
                    name.chars().take(160).collect::<String>()
                ));
            }
            let value = text(n, "value");
            if !value.is_empty() {
                line.push_str(&format!(
                    " value={}",
                    value.chars().take(80).collect::<String>()
                ));
            }
            if let Some(b) = numbered {
                refs.insert(next, (b, line.trim().to_string()));
                next += 1;
            }
            lines.push(line);
        } else if role == "StaticText" && !name.trim().is_empty() && !ignored {
            lines.push(format!(
                "{}{}",
                "  ".repeat(depth.min(12)),
                name.trim().chars().take(200).collect::<String>()
            ));
        }
        if let Some(kids) = n["childIds"].as_array() {
            for k in kids.iter().rev() {
                if let Some(c) = k.as_str().and_then(|id| by_id.get(id)) {
                    stack.push((c, child_depth));
                }
            }
        }
    }
    (lines.join("\n"), refs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(
        id: &str,
        parent: Option<&str>,
        role: &str,
        name: &str,
        kids: &[&str],
        backend: i64,
    ) -> Value {
        json!({"nodeId": id, "parentId": parent, "ignored": false, "role": {"value": role}, "name": {"value": name},
               "childIds": kids, "backendDOMNodeId": backend})
    }

    #[test]
    fn a_page_becomes_numbered_lines() {
        let nodes = vec![
            node("1", None, "RootWebArea", "Shop", &["2", "3", "6"], 1),
            node("2", Some("1"), "heading", "Basket", &[], 2),
            node("3", Some("1"), "generic", "", &["4", "5"], 3),
            node("4", Some("3"), "textbox", "Search", &[], 40),
            node("5", Some("3"), "button", "Go", &[], 50),
            node("6", Some("1"), "StaticText", "2 items", &[], 6),
        ];
        let (text, refs) = outline(&nodes, 100);
        assert_eq!(
            text,
            "[RootWebArea] \"Shop\"\n  [heading] \"Basket\"\n  [1] [textbox] \"Search\"\n  [2] [button] \"Go\"\n  2 items"
        );
        assert_eq!(refs[&1], (40, "[1] [textbox] \"Search\"".to_string()));
        assert_eq!(refs[&2].0, 50);
    }
}
