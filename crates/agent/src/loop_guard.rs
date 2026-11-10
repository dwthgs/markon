//! Stops repeated identical tool calls within a turn: the same
//! tool with the same input more than three times is refused, after one warning.
use serde_json::Value;
use std::collections::HashMap;

pub const MAX_IDENTICAL: u32 = 3;

#[derive(Default)]
pub struct LoopGuard {
    counts: HashMap<(String, String), u32>,
    warned: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Run,
    /// Run it, and tell the model it is going in circles.
    Warn(String),
    Refuse(String),
}

impl LoopGuard {
    pub fn check(&mut self, name: &str, input: &Value) -> Verdict {
        let n = self.counts.entry((name.to_string(), input.to_string())).or_insert(0);
        *n += 1;
        if *n <= MAX_IDENTICAL {
            return Verdict::Run;
        }
        let msg = format!(
            "{name} was already called {} times with exactly this input. It will not give a different answer: \
             use what you have, or try something else.",
            *n - 1
        );
        // Their warn-before-block: the first time over the limit still runs.
        if !self.warned {
            self.warned = true;
            return Verdict::Warn(msg);
        }
        Verdict::Refuse(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_fifth_identical_call_is_refused() {
        let mut g = LoopGuard::default();
        let i = json!({"q": "weather"});
        for _ in 0..3 {
            assert_eq!(g.check("search", &i), Verdict::Run);
        }
        assert!(matches!(g.check("search", &i), Verdict::Warn(_)));
        assert!(matches!(g.check("search", &i), Verdict::Refuse(_)));
        assert_eq!(g.check("search", &json!({"q": "news"})), Verdict::Run, "a different input is fine");
    }
}
