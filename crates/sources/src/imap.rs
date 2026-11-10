//! Mail over IMAP with an app password. The mailbox is
//! opened with EXAMINE and bodies fetched with BODY.PEEK, so reading never marks anything as read.
use crate::{BoxFuture, Item, Source, SourceError, clip};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailAccount {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}
fn default_port() -> u16 {
    993
}
fn default_true() -> bool {
    true
}

impl MailAccount {
    /// Keychain account for this mailbox's app password.
    pub fn secret_account(&self) -> String {
        format!("mail:{}@{}", self.user, self.host)
    }
}

pub type Password = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub struct Imap {
    pub account: MailAccount,
    pub password: Password,
    pub id: String,
    pub label: String,
}

impl Imap {
    pub fn new(account: MailAccount, password: Password) -> Self {
        Self {
            id: format!("mail:{}", account.user),
            label: format!("Mail ({})", account.user),
            account,
            password,
        }
    }
}

#[derive(Debug)]
enum Stream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for Stream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, b: &[u8]) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_write(cx, b),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, b),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

async fn connect(host: &str, port: u16) -> Result<Stream, String> {
    let tcp = TcpStream::connect((host, port)).await.map_err(|e| format!("cannot reach {host}: {e}"))?;
    // Plain IMAP only to the local test server; real mail always goes over TLS.
    if host == "127.0.0.1" {
        return Ok(Stream::Plain(tcp));
    }
    use rustls_platform_verifier::ConfigVerifierExt;
    let cfg = rustls::ClientConfig::with_platform_verifier().map_err(|e| e.to_string())?;
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
        .connect(name, tcp)
        .await
        .map_err(|e| format!("TLS with {host} failed: {e}"))?;
    Ok(Stream::Tls(Box::new(tls)))
}

/// IMAP's SEARCH date: `10-Nov-2026`.
pub fn imap_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default().format("%d-%b-%Y").to_string()
}

/// One message as an item, from its raw bytes.
pub fn parse_message(source: &str, uid: u32, raw: &[u8], unread: bool) -> Option<Item> {
    let m = mail_parser::MessageParser::default().parse(raw)?;
    let who = m
        .from()
        .and_then(|a| a.first())
        .map(|a| match (a.name(), a.address()) {
            (Some(n), Some(e)) => format!("{n} <{e}>"),
            (Some(n), None) => n.to_string(),
            (None, Some(e)) => e.to_string(),
            (None, None) => String::new(),
        })
        .unwrap_or_default();
    let subject = m.subject().unwrap_or("(no subject)").to_string();
    let body = m.body_text(0).map(|b| b.to_string()).unwrap_or_default();
    Some(Item {
        source: source.into(),
        id: uid.to_string(),
        who,
        place: subject.clone(),
        when_ms: m.date().map(|d| d.to_timestamp() * 1000).unwrap_or(0),
        text: clip(&format!("{subject}: {}", body.split_whitespace().collect::<Vec<_>>().join(" ")), 600),
        link: None,
        unread,
    })
}

async fn fetch_unread(acct: &MailAccount, password: &str, source: &str, since_ms: i64) -> Result<Vec<Item>, SourceError> {
    let failed = |e: String| SourceError::Failed(e);
    let stream = connect(&acct.host, acct.port).await.map_err(failed)?;
    let mut client = async_imap::Client::new(stream);
    client
        .read_response()
        .await
        .map_err(|e| failed(e.to_string()))?
        .ok_or_else(|| failed("the server closed the connection".into()))?;
    let mut s = client
        .login(&acct.user, password)
        .await
        .map_err(|(e, _)| SourceError::NotConnected(format!("{} refused the app password ({e})", acct.host)))?;
    s.examine("INBOX").await.map_err(|e| failed(e.to_string()))?;
    let mut uids: Vec<u32> = s
        .uid_search(format!("UNSEEN SINCE {}", imap_date(since_ms)))
        .await
        .map_err(|e| failed(e.to_string()))?
        .into_iter()
        .collect();
    uids.sort_unstable_by(|a, b| b.cmp(a));
    uids.truncate(25);
    let mut out = Vec::new();
    if !uids.is_empty() {
        let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let fetches: Vec<_> = s
            .uid_fetch(set, "(UID BODY.PEEK[])")
            .await
            .map_err(|e| failed(e.to_string()))?
            .try_collect()
            .await
            .map_err(|e| failed(e.to_string()))?;
        for f in &fetches {
            if let (Some(uid), Some(body)) = (f.uid, f.body())
                && let Some(item) = parse_message(source, uid, body, true)
                && item.when_ms >= since_ms
            {
                out.push(item);
            }
        }
    }
    let _ = s.logout().await;
    out.sort_by_key(|i| std::cmp::Reverse(i.when_ms));
    Ok(out)
}

impl Source for Imap {
    fn id(&self) -> &str {
        &self.id
    }
    fn label(&self) -> &str {
        &self.label
    }
    fn fetch(&self, since_ms: i64) -> BoxFuture<'static, Result<Vec<Item>, SourceError>> {
        let (acct, pw, id) = (self.account.clone(), (self.password)(), self.id.clone());
        Box::pin(async move {
            let pw = pw.ok_or_else(|| SourceError::NotConnected(format!("no app password saved for {}", acct.user)))?;
            fetch_unread(&acct, &pw, &id, since_ms).await
        })
    }
}
