//! Acting with approval: send and reply per source, archive, add an event, and drafts.
//! Every action posts a card with the exact text first, then asks on the Send tier.
use crate::imap::MailAccount;
use crate::smtp::{self, Mail};
use crate::telegram::Telegram;
use markon_agent::{ToolCall, ToolRuntime};
use markon_memory::Memory;
use markon_provider::{BoxFuture, ToolSpec};
use markon_tools::cards::{Card, CardKind, CardSink, CardStatus};
use markon_tools::mcp::McpServer;
use markon_tools::permissions::Permissions;
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};

/// What leaves the Mac. The app uses Messages and Calendar; tests record.
pub trait Outbox: Send + Sync {
    fn imessage(&self, to: &str, text: &str) -> Result<(), String>;
    fn event(&self, title: &str, start: &str, minutes: u32) -> Result<(), String>;
}

pub type MailPassword = Arc<dyn Fn(&MailAccount) -> Option<String> + Send + Sync>;

/// Accounts the send tools can use, rebuilt with the sources.
#[derive(Clone, Default)]
pub struct Accounts {
    pub mail: Vec<MailAccount>,
    pub mail_password: Option<MailPassword>,
    /// Slack's MCP server and the name of its send tool.
    pub slack: Option<(Arc<McpServer>, String)>,
    pub telegram: Option<Arc<Telegram>>,
    pub imessage: bool,
    pub calendar: bool,
}

pub struct WithActions<T> {
    pub inner: T,
    pub accounts: Arc<RwLock<Accounts>>,
    pub outbox: Arc<dyn Outbox>,
    pub perms: Arc<Permissions>,
    pub cards: CardSink,
    pub memory: Arc<Memory>,
}

pub const SENDS: &[&str] = &["send_imessage", "send_mail", "send_slack", "send_telegram"];
const DRAFTS: &str = "drafts";

fn t(name: &str, description: &str, props: Value, required: &[&str]) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        input_schema: json!({"type": "object", "properties": props, "required": required, "additionalProperties": false}),
    }
}

impl<T> WithActions<T> {
    fn specs_now(&self) -> Vec<ToolSpec> {
        let a = self.accounts.read().expect("accounts").clone();
        let ask = " The user sees the exact text and must say yes every time.";
        let mut v = vec![
            t(
                "draft_reply",
                "Write a reply for the user to look at later, without sending it. Shown as a Draft card.",
                json!({"to": {"type": "string"}, "via": {"type": "string", "enum": ["imessage", "mail", "slack", "telegram"]}, "text": {"type": "string"}, "subject": {"type": "string"}}),
                &["to", "via", "text"],
            ),
            t(
                "list_drafts",
                "The drafts written earlier (for example overnight), newest first.",
                json!({}),
                &[],
            ),
        ];
        if a.imessage {
            v.push(t(
                "send_imessage",
                &format!("Send an iMessage. `to` is a phone number or Apple ID address.{ask}"),
                json!({"to": {"type": "string"}, "text": {"type": "string"}}),
                &["to", "text"],
            ));
        }
        if !a.mail.is_empty() {
            v.push(t("send_mail", &format!("Send or reply to an email from the user's account. Give `in_reply_to` (the Message-ID) when replying. `from` picks the account when there are several.{ask}"),
                json!({"to": {"type": "string"}, "subject": {"type": "string"}, "text": {"type": "string"}, "in_reply_to": {"type": "string"}, "from": {"type": "string"}}), &["to", "subject", "text"]));
            v.push(t("archive_mail", &format!("Archive one email (move it out of the inbox) by its id from what_did_i_miss.{ask}"),
                json!({"id": {"type": "string"}, "from": {"type": "string"}}), &["id"]));
        }
        if a.slack.is_some() {
            v.push(t("send_slack", &format!("Send a Slack message to a channel or person.{ask}"),
                json!({"to": {"type": "string", "description": "channel name or id, or a person"}, "text": {"type": "string"}}), &["to", "text"]));
        }
        if a.telegram.is_some() {
            v.push(t(
                "send_telegram",
                &format!("Send a Telegram message to a chat by its name.{ask}"),
                json!({"to": {"type": "string"}, "text": {"type": "string"}}),
                &["to", "text"],
            ));
        }
        if a.calendar {
            v.push(t("add_event", &format!("Add an event to the user's calendar. `start` is local \"YYYY-MM-DD HH:MM\".{ask}"),
                json!({"title": {"type": "string"}, "start": {"type": "string"}, "minutes": {"type": "integer"}}), &["title", "start"]));
        }
        v
    }
}

fn arg(c: &ToolCall, k: &str) -> String {
    c.input[k].as_str().unwrap_or("").trim().to_string()
}

struct Plan {
    /// Who or what, for the card title and "always yes for".
    to: String,
    /// The exact action, shown on the card and in the question.
    detail: String,
    /// May "always yes" ever apply (sends only, never archive or calendar changes).
    always: bool,
}

fn plan(c: &ToolCall) -> Result<Plan, String> {
    let need = |k: &str| {
        let v = arg(c, k);
        if v.is_empty() {
            Err(format!("`{k}` is missing"))
        } else {
            Ok(v)
        }
    };
    Ok(match c.name.as_str() {
        "send_imessage" => {
            let (to, text) = (need("to")?, need("text")?);
            Plan {
                detail: format!("iMessage to {to}:\n\n{text}"),
                to,
                always: true,
            }
        }
        "send_mail" => {
            let (to, subject, text) = (need("to")?, need("subject")?, need("text")?);
            Plan {
                detail: format!("Email to {to}\nSubject: {subject}\n\n{text}"),
                to,
                always: true,
            }
        }
        "send_slack" => {
            let (to, text) = (need("to")?, need("text")?);
            Plan {
                detail: format!("Slack message to {to}:\n\n{text}"),
                to,
                always: true,
            }
        }
        "send_telegram" => {
            let (to, text) = (need("to")?, need("text")?);
            Plan {
                detail: format!("Telegram message to {to}:\n\n{text}"),
                to,
                always: true,
            }
        }
        "archive_mail" => {
            let id = need("id")?;
            Plan {
                detail: format!("Archive email {id}"),
                to: format!("email {id}"),
                always: false,
            }
        }
        "add_event" => {
            let (title, start) = (need("title")?, need("start")?);
            let mins = c.input["minutes"].as_u64().unwrap_or(30);
            Plan {
                detail: format!("Add \"{title}\" to your calendar at {start} for {mins} minutes"),
                to: title,
                always: false,
            }
        }
        other => return Err(format!("unknown action {other}")),
    })
}

fn pick_account(a: &Accounts, from: &str) -> Result<MailAccount, String> {
    a.mail
        .iter()
        .find(|m| from.is_empty() || m.user.eq_ignore_ascii_case(from))
        .cloned()
        .ok_or_else(|| format!("no mail account {from}"))
}

async fn perform(c: &ToolCall, a: &Accounts, outbox: &dyn Outbox) -> Result<String, String> {
    match c.name.as_str() {
        "send_imessage" => outbox
            .imessage(&arg(c, "to"), &arg(c, "text"))
            .map(|_| "Sent.".into()),
        "add_event" => outbox
            .event(
                &arg(c, "title"),
                &arg(c, "start"),
                c.input["minutes"].as_u64().unwrap_or(30) as u32,
            )
            .map(|_| "Added to the calendar.".into()),
        "send_mail" => {
            let acct = pick_account(a, &arg(c, "from"))?;
            let pw = a
                .mail_password
                .as_ref()
                .and_then(|f| f(&acct))
                .ok_or("no app password saved")?;
            let (host, port) = smtp::smtp_host(&acct.host);
            let (host, port) = if acct.host == "127.0.0.1" {
                (acct.host.clone(), acct.port)
            } else {
                (host, port)
            };
            let reply = arg(c, "in_reply_to");
            let mail = Mail {
                to: arg(c, "to"),
                subject: arg(c, "subject"),
                body: arg(c, "text"),
                in_reply_to: (!reply.is_empty()).then_some(reply),
            };
            smtp::send(&acct, (&host, port), &pw, &mail)
                .await
                .map(|_| "Sent.".into())
        }
        "archive_mail" => {
            let acct = pick_account(a, &arg(c, "from"))?;
            let pw = a
                .mail_password
                .as_ref()
                .and_then(|f| f(&acct))
                .ok_or("no app password saved")?;
            let uid: u32 = arg(c, "id")
                .parse()
                .map_err(|_| "that is not a mail id from what_did_i_miss")?;
            crate::imap::archive(&acct, &pw, uid)
                .await
                .map(|_| "Archived.".into())
        }
        "send_slack" => {
            let (server, tool) = a.slack.clone().ok_or("Slack is not connected")?;
            server
                .call(
                    &tool,
                    json!({"channel_id": arg(c, "to"), "message": arg(c, "text")}),
                )
                .await
                .map(|_| "Sent.".into())
        }
        "send_telegram" => {
            let tg = a.telegram.clone().ok_or("Telegram is not signed in")?;
            tg.send(&arg(c, "to"), &arg(c, "text"))
                .await
                .map(|_| "Sent.".into())
        }
        other => Err(format!("unknown action {other}")),
    }
}

impl<T: ToolRuntime> ToolRuntime for WithActions<T> {
    fn specs(&self) -> Vec<ToolSpec> {
        let mut v = self.inner.specs();
        v.extend(self.specs_now());
        v
    }

    fn call(&self, c: ToolCall) -> BoxFuture<'static, Result<String, String>> {
        let known = self.specs_now();
        if !known.iter().any(|s| s.name == c.name) {
            return self.inner.call(c);
        }
        let (accounts, outbox, perms, cards, memory) = (
            self.accounts.read().expect("accounts").clone(),
            self.outbox.clone(),
            self.perms.clone(),
            self.cards.clone(),
            self.memory.clone(),
        );
        Box::pin(async move {
            if c.name == "list_drafts" {
                let d = memory
                    .preference(DRAFTS)
                    .ok()
                    .flatten()
                    .unwrap_or(json!([]));
                return Ok(format!(
                    "<drafts>{d}</drafts>\nDrafts are text you wrote earlier; sending one still asks the user."
                ));
            }
            if c.name == "draft_reply" {
                let draft = json!({"to": arg(&c, "to"), "via": arg(&c, "via"), "subject": arg(&c, "subject"), "text": arg(&c, "text"), "at_ms": crate::now_ms()});
                let mut all = memory
                    .preference(DRAFTS)
                    .ok()
                    .flatten()
                    .and_then(|v| v.as_array().cloned())
                    .unwrap_or_default();
                all.insert(0, draft.clone());
                all.truncate(30);
                let _ = memory.set_preference(DRAFTS, &Value::Array(all));
                cards(
                    Card::new(
                        format!("draft-{}-{}", c.turn_id, c.id),
                        c.turn_id,
                        CardKind::Note,
                        format!("Draft to {}", arg(&c, "to")),
                    )
                    .with(
                        CardStatus::Done,
                        json!({"text": arg(&c, "text"), "draft": draft}),
                    ),
                );
                return Ok("Saved as a draft. Nothing was sent.".into());
            }
            let p = plan(&c)?;
            let card = Card::new(
                format!("act-{}-{}", c.turn_id, c.id),
                c.turn_id,
                CardKind::App,
                p.detail.lines().next().unwrap_or("").to_string(),
            );
            cards(card.clone().with(
                CardStatus::Running,
                json!({"preview": p.detail, "action": "Waiting for your yes"}),
            ));
            if let Err(e) = perms
                .send(&c.name, &p.to, &p.detail, c.tainted, p.always)
                .await
            {
                cards(card.with(
                    CardStatus::Blocked,
                    json!({"preview": p.detail, "reason": "Not sent: you said no."}),
                ));
                return Err(e);
            }
            match perform(&c, &accounts, &*outbox).await {
                Ok(done) => {
                    cards(card.with(
                        CardStatus::Done,
                        json!({"preview": p.detail, "action": done}),
                    ));
                    Ok(done)
                }
                Err(e) => {
                    cards(card.with(CardStatus::Error, json!({"preview": p.detail, "reason": e})));
                    Err(e)
                }
            }
        })
    }
}
