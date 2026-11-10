//! Sign-in for hosted MCP servers: OAuth 2.1 with PKCE through rmcp's `auth` feature,
//! a one-time listener on 127.0.0.1 for the redirect, tokens in the keychain only.
use markon_platform::Platform;
use rmcp::transport::auth::OAuthState;
use rmcp::transport::{AuthError, AuthorizationRequest, CredentialStore, StoredCredentials};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The redirect Markon listens on. Register exactly this URL with the service (Slack needs it).
pub const REDIRECT_PORT: u16 = 53682;
pub fn redirect_uri() -> String {
    format!("http://127.0.0.1:{REDIRECT_PORT}/callback")
}
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OAuthConfig {
    /// A registered app's client id. Without one, Markon registers itself if the server allows it.
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// Keychain account for a server's tokens.
pub fn secret_account(server: &str) -> String {
    format!("mcp-oauth:{server}")
}

/// OAuth credentials kept in the macOS keychain, never in a file.
pub struct KeychainStore {
    pub platform: Arc<dyn Platform>,
    pub account: String,
}

fn store_err(e: impl std::fmt::Display) -> AuthError {
    AuthError::InternalError(format!("keychain: {e}"))
}

#[async_trait::async_trait]
impl CredentialStore for KeychainStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        match self.platform.secret_get(&self.account).map_err(store_err)? {
            Some(s) => Ok(serde_json::from_str(&s).ok()),
            None => Ok(None),
        }
    }
    async fn save(&self, c: StoredCredentials) -> Result<(), AuthError> {
        let s = serde_json::to_string(&c).map_err(store_err)?;
        self.platform
            .secret_set(&self.account, &s)
            .map_err(store_err)
    }
    async fn clear(&self) -> Result<(), AuthError> {
        self.platform
            .secret_delete(&self.account)
            .map_err(store_err)
    }
}

/// `code` and `state` from the redirect's request line, `GET /callback?code=..&state=.. HTTP/1.1`.
pub fn parse_callback(request: &str) -> Option<(String, String)> {
    let target = request.lines().next()?.split_whitespace().nth(1)?;
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    if url.path() != "/callback" {
        return None;
    }
    let get = |k: &str| {
        url.query_pairs()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.to_string())
    };
    Some((get("code")?, get("state")?))
}

/// Wait for one redirect on the fixed port and answer the browser with a short page.
async fn wait_for_redirect() -> Result<(String, String), String> {
    let l = tokio::net::TcpListener::bind(("127.0.0.1", REDIRECT_PORT))
        .await
        .map_err(|e| format!("port {REDIRECT_PORT} is busy ({e})"))?;
    let accept = async {
        loop {
            let (mut c, _) = l.accept().await.map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; 8192];
            let n = c.read(&mut buf).await.map_err(|e| e.to_string())?;
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let found = parse_callback(&req);
            let body = if found.is_some() {
                "Signed in. You can close this tab and go back to Markon."
            } else {
                "Markon is waiting for a sign-in."
            };
            let _ = c
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await;
            if let Some(f) = found {
                return Ok::<_, String>(f);
            }
        }
    };
    tokio::time::timeout(SIGN_IN_TIMEOUT, accept)
        .await
        .map_err(|_| "sign-in was not finished within five minutes".to_string())?
}

/// Run the whole sign-in: discover, open the browser, wait for the redirect, store the tokens.
pub async fn sign_in(
    server: &str,
    url: &str,
    cfg: &OAuthConfig,
    platform: Arc<dyn Platform>,
) -> Result<(), String> {
    let mut st = OAuthState::new(url, None)
        .await
        .map_err(|e| e.to_string())?;
    if let OAuthState::Unauthorized(m) = &mut st {
        m.set_credential_store(KeychainStore {
            platform: platform.clone(),
            account: secret_account(server),
        });
    }
    let mut req = AuthorizationRequest::new(redirect_uri())
        .with_client_name("Markon")
        .with_scopes(cfg.scopes.clone());
    if let Some(id) = &cfg.client_id {
        req = req.with_preregistered_client(id.clone());
    }
    st.start_authorization(req)
        .await
        .map_err(|e| e.to_string())?;
    let auth_url = st
        .get_authorization_url()
        .await
        .map_err(|e| e.to_string())?;
    let waiting = tokio::spawn(wait_for_redirect());
    platform.open_url(&auth_url).map_err(|e| e.to_string())?;
    let (code, state) = waiting.await.map_err(|e| e.to_string())??;
    st.handle_callback(&code, &state)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_redirect() {
        assert_eq!(
            parse_callback(
                "GET /callback?code=abc%2F1&state=xyz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
            ),
            Some(("abc/1".into(), "xyz".into()))
        );
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1\r\n"), None);
        assert_eq!(
            parse_callback("GET /callback?error=denied HTTP/1.1\r\n"),
            None
        );
    }
}
