//! Small programs in a sandbox: JavaScript in QuickJS (`rquickjs`). No files, no
//! network, no clock beyond `Date`: only `console.log` comes out. Memory and time are capped, so an
//! endless loop or a memory bomb stops with an error.
use rquickjs::{Context, Ctx, Function, Object, Runtime, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const MEMORY: usize = 32 * 1024 * 1024;
pub const TIME: Duration = Duration::from_secs(3);
const MAX_OUTPUT: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Ran {
    /// `console.log` lines, then the value of the last expression.
    pub output: String,
}

fn show<'js>(ctx: &Ctx<'js>, v: Value<'js>) -> String {
    if v.is_undefined() {
        return String::new();
    }
    if let Some(s) = v.as_string().and_then(|s| s.to_string().ok()) {
        return s;
    }
    // Numbers, objects and arrays as JSON, which is what a person would expect to read.
    ctx.json_stringify(v)
        .ok()
        .flatten()
        .and_then(|s| s.to_string().ok())
        .unwrap_or_else(|| "(a value that cannot be shown)".into())
}

/// Run `code` and return what it printed and its final value.
pub fn run_js(code: &str) -> Result<Ran, String> {
    let rt = Runtime::new().map_err(|e| e.to_string())?;
    rt.set_memory_limit(MEMORY);
    rt.set_max_stack_size(1024 * 1024);
    let started = Instant::now();
    rt.set_interrupt_handler(Some(Box::new(move || started.elapsed() > TIME)));
    let ctx = Context::full(&rt).map_err(|e| e.to_string())?;
    let logs: Arc<Mutex<String>> = Arc::default();
    ctx.with(|ctx| {
        let l = logs.clone();
        let log = Function::new(
            ctx.clone(),
            move |args: rquickjs::function::Rest<Value<'_>>| {
                let mut out = l.lock().expect("logs");
                if out.len() < MAX_OUTPUT {
                    let line: Vec<String> = args
                        .0
                        .into_iter()
                        .map(|a| {
                            let c = a.ctx().clone();
                            show(&c, a)
                        })
                        .collect();
                    out.push_str(&line.join(" "));
                    out.push('\n');
                }
            },
        )
        .map_err(|e| e.to_string())?;
        let console = Object::new(ctx.clone()).map_err(|e| e.to_string())?;
        console.set("log", log).map_err(|e| e.to_string())?;
        ctx.globals()
            .set("console", console)
            .map_err(|e| e.to_string())?;
        let result: Result<Value<'_>, _> = ctx.eval(code);
        match result {
            Ok(v) => {
                let last = show(&ctx, v);
                let mut out = logs.lock().expect("logs").clone();
                out.push_str(&last);
                out.truncate(MAX_OUTPUT);
                Ok(Ran {
                    output: out.trim_end().to_string(),
                })
            }
            Err(e) => {
                let detail = ctx.catch();
                let msg = if started.elapsed() > TIME {
                    format!(
                        "the program ran longer than {} s and was stopped",
                        TIME.as_secs()
                    )
                } else if let Some(ex) = detail.as_exception() {
                    ex.message().unwrap_or_else(|| e.to_string())
                } else {
                    let s = show(&ctx, detail);
                    if s.is_empty() { e.to_string() } else { s }
                };
                Err(msg)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_code_and_shows_logs_and_the_result() {
        let r = run_js("console.log('sum', [1,2,3].reduce((a,b)=>a+b)); ({total: 6})").unwrap();
        assert_eq!(r.output, "sum 6\n{\"total\":6}");
    }

    #[test]
    fn an_endless_loop_is_stopped() {
        let t = Instant::now();
        let e = run_js("while (true) {}").unwrap_err();
        assert!(e.contains("longer than"), "{e}");
        assert!(t.elapsed() < TIME + Duration::from_secs(2));
    }

    #[test]
    fn a_memory_bomb_is_stopped() {
        let e = run_js("let a = []; while (true) { a.push('x'.repeat(1e6)); }").unwrap_err();
        assert!(!e.is_empty());
    }

    #[test]
    fn there_is_no_way_out_to_files_or_the_network() {
        for probe in [
            "typeof require",
            "typeof process",
            "typeof fetch",
            "typeof std",
            "typeof os",
            "typeof XMLHttpRequest",
        ] {
            assert_eq!(run_js(probe).unwrap().output, "undefined", "{probe}");
        }
        assert!(
            run_js("throw new Error('nope')")
                .unwrap_err()
                .contains("nope")
        );
    }
}
