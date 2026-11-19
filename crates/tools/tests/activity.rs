//! Every action that changed something is recorded, "what did you
//! just do?" is answered from the log, file writes and window moves can be undone, the rest says so.
use markon_agent::{ToolCall, ToolRuntime};
use markon_memory::Memory;
use markon_platform::fake::FakeMac;
use markon_tools::activity::WithActivity;
use markon_tools::permissions::{Decision, Permissions, Prompter, Request};
use markon_tools::{BoxFuture, Runtime, Shared};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct Yes(Mutex<usize>);
impl Prompter for Yes {
    fn ask(&self, _: Request) -> BoxFuture<'static, Decision> {
        *self.0.lock().unwrap() += 1;
        Box::pin(async { Decision::Allow })
    }
}

struct Rig {
    tools: WithActivity<Shared>,
    mac: Arc<FakeMac>,
    memory: Arc<Memory>,
    dir: PathBuf,
}

fn rig(name: &str) -> Rig {
    let dir = std::env::temp_dir().join(format!("markon-activity-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let mac = Arc::new(FakeMac::default());
    let rt = Arc::new(
        Runtime::new(
            Permissions::new(vec![dir.clone()], Arc::new(Yes(Mutex::new(0)))),
            Arc::new(|_| {}),
        )
        .with_platform(mac.clone()),
    );
    let memory = Arc::new(Memory::open_in_memory().unwrap());
    Rig {
        tools: WithActivity {
            inner: Shared(rt.clone()),
            rt,
            memory: memory.clone(),
        },
        mac,
        memory,
        dir,
    }
}

async fn run(r: &Rig, id: &str, name: &str, input: Value) -> Result<String, String> {
    r.tools
        .call(ToolCall {
            id: id.into(),
            name: name.into(),
            input,
            turn_id: 1,
            tainted: false,
        })
        .await
}

#[tokio::test]
async fn what_changed_is_logged_and_reads_are_not() {
    let r = rig("log");
    let f = r.dir.join("notes.md");
    run(&r, "1", "write_file", json!({"path": f, "content": "one"}))
        .await
        .unwrap();
    run(&r, "2", "list_files", json!({"path": r.dir}))
        .await
        .unwrap();
    run(&r, "3", "system_control", json!({"action": "status"}))
        .await
        .unwrap();
    run(&r, "4", "system_control", json!({"action": "mute"}))
        .await
        .unwrap();
    let log = r.memory.actions(10).unwrap();
    let tools: Vec<&str> = log.iter().map(|a| a.tool.as_str()).collect();
    assert_eq!(
        tools,
        ["system_control", "write_file"],
        "newest first, reads left out"
    );
    assert_eq!(log[1].target, f.display().to_string());

    let said = run(&r, "5", "recent_activity", json!({})).await.unwrap();
    assert!(
        said.contains("notes.md") && said.contains("can be undone"),
        "{said}"
    );
    assert!(
        said.contains("mute") && said.contains("cannot be undone"),
        "{said}"
    );
}

#[tokio::test]
async fn a_file_write_is_undone_to_its_previous_text() {
    let r = rig("file");
    let f = r.dir.join("plan.txt");
    run(
        &r,
        "1",
        "write_file",
        json!({"path": f, "content": "first"}),
    )
    .await
    .unwrap();
    run(
        &r,
        "2",
        "write_file",
        json!({"path": f, "content": "second"}),
    )
    .await
    .unwrap();
    let said = run(&r, "3", "undo_last", json!({})).await.unwrap();
    assert!(said.contains("plan.txt"), "{said}");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "first");
    // The file did not exist before the first write: undoing that removes it.
    run(&r, "4", "undo_last", json!({})).await.unwrap();
    assert!(!f.exists());
    let err = run(&r, "5", "undo_last", json!({})).await.unwrap_err();
    assert!(err.contains("nothing"), "{err}");
}

#[tokio::test]
async fn a_window_move_is_undone_and_a_shortcut_says_it_cannot_be() {
    let r = rig("window");
    r.mac.answer("apps window", Ok("Safari\t100,100,800,600"));
    r.mac.answer("apps screen", Ok("0,33,1512,949"));
    run(&r, "1", "arrange_window", json!({"layout": "right"}))
        .await
        .unwrap();
    run(&r, "2", "undo_last", json!({})).await.unwrap();
    assert_eq!(
        r.mac.did().last().unwrap(),
        "apps set_window Safari 100,100,800,600"
    );

    run(&r, "3", "run_shortcut", json!({"name": "Log water"}))
        .await
        .unwrap();
    let err = run(&r, "4", "undo_last", json!({})).await.unwrap_err();
    assert!(
        err.contains("cannot be undone") && err.contains("Log water"),
        "{err}"
    );
}

#[test]
fn the_log_tools_are_offered() {
    let r = rig("specs");
    let names: Vec<String> = r.tools.specs().into_iter().map(|s| s.name).collect();
    assert!(names.contains(&"recent_activity".to_string()));
    assert!(names.contains(&"undo_last".to_string()));
}
