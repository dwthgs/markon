//! The user's own Chrome, driven over the DevTools protocol with `chromiumoxide` (no Node, no
//! Playwright). Chrome refuses remote control of the default profile, so Markon uses its own profile in
//! `~/.markon/browser`: the user signs in to sites there once. The window is visible.
use crate::{BoxFuture, Driver};
use chromiumoxide::cdp::browser_protocol::accessibility::GetFullAxTreeParams;
use chromiumoxide::cdp::browser_protocol::dom::{BackendNodeId, ResolveNodeParams};
use chromiumoxide::cdp::js_protocol::runtime::{CallArgument, CallFunctionOnParams};
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures_util::StreamExt;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct Chrome {
    pub executable: PathBuf,
    pub profile: PathBuf,
    /// Headless for the local test only.
    pub headless: bool,
    state: Mutex<Option<(Browser, Page)>>,
}

impl Chrome {
    pub fn new(executable: PathBuf, profile: PathBuf) -> Self {
        Self {
            executable,
            profile,
            headless: false,
            state: Mutex::new(None),
        }
    }

    async fn page(&self) -> Result<Page, String> {
        let mut st = self.state.lock().await;
        if let Some((_, p)) = st.as_ref() {
            return Ok(p.clone());
        }
        let mut cfg = BrowserConfig::builder()
            .chrome_executable(&self.executable)
            .user_data_dir(&self.profile);
        if !self.headless {
            cfg = cfg.with_head();
        }
        let (browser, mut handler) = Browser::launch(cfg.build()?)
            .await
            .map_err(|e| format!("Chrome did not start: {e}"))?;
        tokio::spawn(async move { while handler.next().await.is_some() {} });
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| e.to_string())?;
        *st = Some((browser, page.clone()));
        Ok(page)
    }

    async fn call_on(&self, backend: i64, function: &str, args: Vec<Value>) -> Result<(), String> {
        let page = self.page().await?;
        let node = page
            .execute(
                ResolveNodeParams::builder()
                    .backend_node_id(BackendNodeId::new(backend))
                    .build(),
            )
            .await
            .map_err(|e| format!("that element is gone; read the page again ({e})"))?;
        let object = node
            .result
            .object
            .object_id
            .clone()
            .ok_or("that element cannot be used")?;
        let mut p = CallFunctionOnParams::new(function);
        p.object_id = Some(object);
        p.arguments = Some(
            args.into_iter()
                .map(|v| CallArgument {
                    value: Some(v),
                    ..Default::default()
                })
                .collect(),
        );
        p.await_promise = Some(true);
        page.execute(p).await.map(|_| ()).map_err(|e| e.to_string())
    }
}

impl Driver for Chrome {
    fn open(self: Arc<Self>, url: String) -> BoxFuture<'static, Result<String, String>> {
        Box::pin(async move {
            let page = self.page().await?;
            page.goto(url).await.map_err(|e| e.to_string())?;
            Ok(page.get_title().await.ok().flatten().unwrap_or_default())
        })
    }

    fn tree(self: Arc<Self>) -> BoxFuture<'static, Result<(String, Vec<Value>), String>> {
        Box::pin(async move {
            let page = self.page().await?;
            let url = page.url().await.ok().flatten().unwrap_or_default();
            let r = page
                .execute(GetFullAxTreeParams::default())
                .await
                .map_err(|e| e.to_string())?;
            let nodes = r
                .result
                .nodes
                .iter()
                .filter_map(|n| serde_json::to_value(n).ok())
                .collect();
            Ok((url, nodes))
        })
    }

    fn click(self: Arc<Self>, backend: i64) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            self.call_on(
                backend,
                "function(){ this.scrollIntoView({block: 'center'}); this.click(); }",
                vec![],
            )
            .await
        })
    }

    fn type_text(
        self: Arc<Self>,
        backend: i64,
        text: String,
    ) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            self.call_on(
                backend,
                "function(v){ this.focus(); this.value = v; this.dispatchEvent(new Event('input', {bubbles: true})); \
                 this.dispatchEvent(new Event('change', {bubbles: true})); }",
                vec![Value::String(text)],
            )
            .await
        })
    }
}
