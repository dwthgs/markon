//! Telegram as the user, on `grammers-client`. The session (datacenter and its auth key) is
//! saved to the keychain after every change and never written to a file.
use crate::{BoxFuture, Item, Source, SourceError, clip};
use grammers_client::session::storages::MemorySession;
use grammers_client::session::types::{DcOption, PeerId, PeerInfo, UpdateState, UpdatesState};
use grammers_client::session::{Session, SessionData};
use grammers_client::{Client, SenderPool, SignInError, tl};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// What survives a restart: the home datacenter and every datacenter's address and auth key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub home_dc: i32,
    pub dc_options: Vec<DcOption>,
}

pub type Store = Arc<dyn Fn(&str) + Send + Sync>;

/// A memory session that writes its keys to the keychain (through `store`) whenever they change.
pub struct KeychainSession {
    inner: MemorySession,
    saved: Mutex<Saved>,
    store: Store,
}

impl KeychainSession {
    pub fn load(json: Option<&str>, store: Store) -> Self {
        let saved: Option<Saved> = json.and_then(|j| serde_json::from_str(j).ok());
        let mut data = SessionData::default();
        if let Some(s) = &saved {
            data.home_dc = s.home_dc;
            for d in &s.dc_options {
                data.dc_options.insert(d.id, d.clone());
            }
        }
        let snapshot = Saved {
            home_dc: data.home_dc,
            dc_options: data.dc_options.values().cloned().collect(),
        };
        Self { inner: MemorySession::from(data), saved: Mutex::new(snapshot), store }
    }

    fn persist(&self) {
        let s = self.saved.lock().expect("session").clone();
        if let Ok(j) = serde_json::to_string(&s) {
            (self.store)(&j);
        }
    }

    pub fn is_signed_in(&self) -> bool {
        let s = self.saved.lock().expect("session");
        s.dc_options.iter().any(|d| d.id == s.home_dc && d.auth_key.is_some())
    }
}

type Fut<'a, T> = grammers_client::session::BoxFuture<'a, Result<T, <MemorySession as Session>::Error>>;

impl Session for KeychainSession {
    type Error = <MemorySession as Session>::Error;

    fn home_dc_id(&self) -> Result<i32, Self::Error> {
        self.inner.home_dc_id()
    }
    fn set_home_dc_id(&self, dc_id: i32) -> Fut<'_, ()> {
        Box::pin(async move {
            self.inner.set_home_dc_id(dc_id).await?;
            self.saved.lock().expect("session").home_dc = dc_id;
            self.persist();
            Ok(())
        })
    }
    fn dc_option(&self, dc_id: i32) -> Result<Option<DcOption>, Self::Error> {
        self.inner.dc_option(dc_id)
    }
    fn set_dc_option(&self, dc_option: &DcOption) -> Fut<'_, ()> {
        let d = dc_option.clone();
        Box::pin(async move {
            self.inner.set_dc_option(&d).await?;
            {
                let mut s = self.saved.lock().expect("session");
                s.dc_options.retain(|x| x.id != d.id);
                s.dc_options.push(d);
            }
            self.persist();
            Ok(())
        })
    }
    fn peer(&self, peer: PeerId) -> Fut<'_, Option<PeerInfo>> {
        self.inner.peer(peer)
    }
    fn cache_peer(&self, peer: PeerInfo) -> Fut<'_, ()> {
        self.inner.cache_peer(peer)
    }
    fn updates_state(&self) -> Fut<'_, UpdatesState> {
        self.inner.updates_state()
    }
    fn set_update_state(&self, update: UpdateState) -> Fut<'_, ()> {
        self.inner.set_update_state(update)
    }
}

/// A signed-in (or signing-in) Telegram account.
pub struct Telegram {
    pub api_id: i32,
    pub api_hash: String,
    session: Arc<KeychainSession>,
    client: Client,
    login: Mutex<Option<grammers_client::client::LoginToken>>,
    password: Mutex<Option<grammers_client::client::PasswordToken>>,
}

impl Telegram {
    /// Connect with the saved session (if any). The network runner lives as long as the app.
    pub fn new(api_id: i32, api_hash: String, saved: Option<&str>, store: Store) -> Arc<Self> {
        let session = Arc::new(KeychainSession::load(saved, store));
        let SenderPool { runner, handle, .. } = SenderPool::new(session.clone(), api_id);
        tokio::spawn(runner.run());
        Arc::new(Self {
            api_id,
            api_hash,
            session,
            client: Client::new(handle),
            login: Mutex::new(None),
            password: Mutex::new(None),
        })
    }

    pub async fn is_authorized(&self) -> bool {
        self.client.is_authorized().await.unwrap_or(false)
    }

    /// Step one: Telegram sends a code to the user's other devices.
    pub async fn request_code(&self, phone: &str) -> Result<(), String> {
        let t = self.client.request_login_code(phone, &self.api_hash).await.map_err(|e| e.to_string())?;
        *self.login.lock().expect("login") = Some(t);
        Ok(())
    }

    /// Step two: the code. Returns true when a two-step password is still needed.
    pub async fn sign_in(&self, code: &str) -> Result<bool, String> {
        let token = self.login.lock().expect("login").take().ok_or("ask for a code first")?;
        match self.client.sign_in(&token, code.trim()).await {
            Ok(_) => Ok(false),
            Err(SignInError::PasswordRequired(p)) => {
                *self.password.lock().expect("password") = Some(p);
                Ok(true)
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Step three, only with two-step verification on.
    pub async fn check_password(&self, password: &str) -> Result<(), String> {
        let p = self.password.lock().expect("password").take().ok_or("no password was asked for")?;
        self.client.check_password(p, password).await.map(|_| ()).map_err(|e| e.to_string())
    }

    pub fn signed_in_locally(&self) -> bool {
        self.session.is_signed_in()
    }

    async fn unread(&self, since_ms: i64) -> Result<Vec<Item>, SourceError> {
        if !self.is_authorized().await {
            return Err(SourceError::NotConnected("sign in to Telegram in settings".into()));
        }
        let mut it = self.client.iter_dialogs();
        let mut out = Vec::new();
        let mut seen = 0;
        while let Some(d) = it.next().await.map_err(|e| SourceError::Failed(e.to_string()))? {
            seen += 1;
            if seen > 40 {
                break;
            }
            let unread = match &d.raw {
                tl::enums::Dialog::Dialog(x) => x.unread_count,
                _ => 0,
            };
            let Some(m) = &d.last_message else { continue };
            let when = m.date().as_millisecond();
            if unread == 0 || when < since_ms || m.outgoing() || m.text().is_empty() {
                continue;
            }
            let chat = d.peer().name().unwrap_or("Telegram").to_string();
            let who = m.sender().and_then(|s| s.name()).unwrap_or(&chat).to_string();
            out.push(Item {
                source: "telegram".into(),
                id: format!("{:?}:{}", d.peer_id(), m.id()),
                who,
                place: if unread > 1 { format!("{chat} ({unread} unread)") } else { chat },
                when_ms: when,
                text: clip(m.text(), 600),
                link: None,
                unread: true,
            });
        }
        Ok(out)
    }
}

/// The source side of a [`Telegram`] account.
pub struct TelegramSource(pub Arc<Telegram>);

impl Source for TelegramSource {
    fn id(&self) -> &str {
        "telegram"
    }
    fn label(&self) -> &str {
        "Telegram"
    }
    fn fetch(&self, since_ms: i64) -> BoxFuture<'static, Result<Vec<Item>, SourceError>> {
        let t = self.0.clone();
        Box::pin(async move { t.unread(since_ms).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_session_survives_a_restart_through_the_keychain_only() {
        let kept = Arc::new(Mutex::new(String::new()));
        let k = kept.clone();
        let s = KeychainSession::load(None, Arc::new(move |j| *k.lock().unwrap() = j.to_string()));
        assert!(!s.is_signed_in());
        let mut dc = s.dc_option(2).unwrap().unwrap();
        dc.auth_key = Some([7u8; 256]);
        s.set_dc_option(&dc).await.unwrap();
        s.set_home_dc_id(2).await.unwrap();
        assert!(s.is_signed_in());
        let json = kept.lock().unwrap().clone();
        let again = KeychainSession::load(Some(&json), Arc::new(|_| {}));
        assert!(again.is_signed_in());
        assert_eq!(again.home_dc_id().unwrap(), 2);
        assert_eq!(again.dc_option(2).unwrap().unwrap().auth_key, Some([7u8; 256]));
    }
}
