---

name: markon-setup
description: How to connect Markon's sources (Messages, Calendar, Notes, mail, GitHub, Slack, Telegram) and what each one needs. Use when the user asks how to set something up.
---

Answer in one or two spoken sentences and point to the exact place in Settings.

- Messages, Calendar and Notes: Settings, Sources, switch them on. They need Full Disk Access for
  Markon: the button next to them opens the right page in System Settings.
- Mail: Settings, Sources, Mail. The server (imap.gmail.com, outlook.office365.com, imap.mail.me.com),
  the address, and an app password made in the mail account's security settings, not the normal password.
- GitHub: switch on GitHub and paste a personal access token with the notifications scope.
- Slack or Notion: add the service's MCP server to ~/.markon/mcp.json with an "oauth" entry, then press
  "Sign in" in Settings.
- Telegram: make an app at my.telegram.org, paste the api_id and api_hash, then the phone number and the
  code Telegram sends.

Markon only reads these. Anything it sends, it asks first, every time.
