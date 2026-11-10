//! Sending mail over SMTP with the account's app password: implicit TLS on 465,
//! STARTTLS on 587. Plain SMTP only to 127.0.0.1 (the test server).
use crate::imap::MailAccount;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// The SMTP host for an IMAP host, for the big three; otherwise the IMAP host with `smtp.` in front.
pub fn smtp_host(imap_host: &str) -> (String, u16) {
    match imap_host {
        "imap.gmail.com" => ("smtp.gmail.com".into(), 465),
        "outlook.office365.com" | "imap-mail.outlook.com" => ("smtp.office365.com".into(), 587),
        "imap.mail.me.com" => ("smtp.mail.me.com".into(), 587),
        h => (
            format!("smtp.{}", h.strip_prefix("imap.").unwrap_or(h)),
            587,
        ),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub body: String,
    /// The Message-ID being answered, for threading.
    pub in_reply_to: Option<String>,
}

fn valid_address(a: &str) -> bool {
    let a = a.trim();
    a.len() <= 254
        && a.split_once('@')
            .is_some_and(|(l, d)| !l.is_empty() && d.contains('.'))
        && !a
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>,;\"".contains(c))
}

/// No header injection: a header value is one line.
fn one_line(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

/// The RFC 5322 message, with lines starting with "." doubled for DATA.
pub fn render(from: &str, m: &Mail, date: &str, id: &str) -> String {
    let mut h = format!(
        "From: {from}\r\nTo: {}\r\nSubject: {}\r\nDate: {date}\r\nMessage-ID: <{id}>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n",
        one_line(&m.to),
        one_line(&m.subject)
    );
    if let Some(r) = &m.in_reply_to {
        let r = one_line(r);
        h.push_str(&format!("In-Reply-To: {r}\r\nReferences: {r}\r\n"));
    }
    let body: String = m
        .body
        .replace("\r\n", "\n")
        .split('\n')
        .map(|l| {
            if l.starts_with('.') {
                format!(".{l}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n");
    format!("{h}\r\n{body}\r\n")
}

async fn reply<R: AsyncRead + Unpin>(r: &mut BufReader<R>) -> Result<(u16, String), String> {
    let mut all = String::new();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).await.map_err(|e| e.to_string())? == 0 {
            return Err("the mail server closed the connection".into());
        }
        all.push_str(&line);
        // "250-..." continues, "250 ..." ends.
        if line.len() < 4 || line.as_bytes()[3] != b'-' {
            let code = line
                .get(..3)
                .and_then(|c| c.parse().ok())
                .ok_or("not an SMTP reply")?;
            return Ok((code, all));
        }
    }
}

async fn expect<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut BufReader<S>,
    cmd: Option<&str>,
    ok: u16,
) -> Result<String, String> {
    if let Some(c) = cmd {
        s.get_mut()
            .write_all(format!("{c}\r\n").as_bytes())
            .await
            .map_err(|e| e.to_string())?;
    }
    let (code, text) = reply(s).await?;
    if code != ok {
        let shown = if cmd.is_some_and(|c| c.starts_with("AUTH")) {
            "AUTH"
        } else {
            cmd.unwrap_or("greeting")
        };
        return Err(format!(
            "the mail server answered {code} to {shown}: {}",
            text.trim()
        ));
    }
    Ok(text)
}

async fn session<S: AsyncRead + AsyncWrite + Unpin>(
    s: S,
    acct: &MailAccount,
    password: &str,
    m: &Mail,
) -> Result<(), String> {
    let mut s = BufReader::new(s);
    expect(&mut s, Some("EHLO markon.local"), 250).await?;
    use base64::Engine;
    let auth =
        base64::engine::general_purpose::STANDARD.encode(format!("\0{}\0{password}", acct.user));
    expect(&mut s, Some(&format!("AUTH PLAIN {auth}")), 235)
        .await
        .map_err(|_| format!("{} refused the app password", acct.user))?;
    expect(&mut s, Some(&format!("MAIL FROM:<{}>", acct.user)), 250).await?;
    expect(&mut s, Some(&format!("RCPT TO:<{}>", m.to.trim())), 250).await?;
    expect(&mut s, Some("DATA"), 354).await?;
    let now = chrono::Utc::now();
    let id = format!(
        "{}.markon@{}",
        now.timestamp_millis(),
        acct.user.split('@').nth(1).unwrap_or("localhost")
    );
    let msg = render(&acct.user, m, &now.to_rfc2822(), &id);
    s.get_mut()
        .write_all(msg.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    expect(&mut s, Some("."), 250).await?;
    let _ = expect(&mut s, Some("QUIT"), 221).await;
    Ok(())
}

/// Send one mail. Callers have already shown the exact text and had a yes.
pub async fn send(
    acct: &MailAccount,
    smtp: (&str, u16),
    password: &str,
    m: &Mail,
) -> Result<(), String> {
    if !valid_address(&m.to) {
        return Err(format!("{} is not one mail address", m.to));
    }
    let (host, port) = smtp;
    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|e| format!("cannot reach {host}: {e}"))?;
    if host == "127.0.0.1" {
        let mut s = BufReader::new(tcp);
        expect(&mut s, None, 220).await?;
        return session(s.into_inner(), acct, password, m).await;
    }
    use rustls_platform_verifier::ConfigVerifierExt;
    let cfg = Arc::new(rustls::ClientConfig::with_platform_verifier().map_err(|e| e.to_string())?);
    let name =
        rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
    let tls = tokio_rustls::TlsConnector::from(cfg);
    if port == 465 {
        let t = tls
            .connect(name, tcp)
            .await
            .map_err(|e| format!("TLS with {host} failed: {e}"))?;
        let mut s = BufReader::new(t);
        expect(&mut s, None, 220).await?;
        return session(s.into_inner(), acct, password, m).await;
    }
    let mut s = BufReader::new(tcp);
    expect(&mut s, None, 220).await?;
    expect(&mut s, Some("EHLO markon.local"), 250).await?;
    expect(&mut s, Some("STARTTLS"), 220).await?;
    let t = tls
        .connect(name, s.into_inner())
        .await
        .map_err(|e| format!("TLS with {host} failed: {e}"))?;
    session(t, acct, password, m).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_cannot_be_injected_and_dots_are_doubled() {
        let m = Mail {
            to: "sara@example.com".into(),
            subject: "Hi\r\nBcc: everyone@example.com".into(),
            body: "line\n.hidden\nend".into(),
            in_reply_to: Some("<a@b>".into()),
        };
        let r = render(
            "me@example.com",
            &m,
            "Tue, 10 Nov 2026 10:00:00 +0000",
            "1@example.com",
        );
        assert!(!r.contains("\r\nBcc:"), "{r}");
        assert!(r.contains("Subject: HiBcc: everyone@example.com"));
        assert!(r.contains("\r\n..hidden\r\n"));
        assert!(r.contains("In-Reply-To: <a@b>"));
        assert!(!valid_address("a@b.com, c@d.com"));
        assert!(valid_address("sara@work.example"));
    }

    #[test]
    fn smtp_hosts_for_the_big_three() {
        assert_eq!(smtp_host("imap.gmail.com"), ("smtp.gmail.com".into(), 465));
        assert_eq!(
            smtp_host("imap.mail.me.com"),
            ("smtp.mail.me.com".into(), 587)
        );
        assert_eq!(
            smtp_host("imap.fastmail.com"),
            ("smtp.fastmail.com".into(), 587)
        );
    }
}
