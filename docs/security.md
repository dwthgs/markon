# Security

Markon can read your files, run commands, control apps and send messages as you. This page describes what
stops it from doing that without you, and what still gets through.

Code:

- `crates/tools/src/permissions.rs`: tiers, approvals, the audit log.
- `crates/tools/src/guard.rs`: secret files, the private-address guard, password fields.
- `crates/agent/src/scrub.rs`: secret scrubbing of tool output.
- `crates/agent/src/lib.rs`: outside-content tracking (`reads_outside`) and tool allow-lists.
- `crates/platform/src/lib.rs`: the keychain, URL and app name checks, AppleScript arguments.
- `crates/hands/src/sandbox.rs`: the JavaScript sandbox.
- `apps/desktop/src-tauri/src/workspace.rs`: the approval dialog and spoken answers.
- `apps/desktop/src-tauri/src/settings.rs`: `~/.markon/settings.json`.

## Risk tiers

Every tool that touches something outside Markon goes through one of four tiers (`Tier`):

| Tier        | Covers                                        | Asks                                  |
| ----------- | --------------------------------------------- | ------------------------------------- |
| **Read**    | `list_files`, `read_file`                     | Never. Approved folders only.         |
| **Write**   | `write_file`, undoing a file write            | Once per approved folder, per launch. |
| **Execute** | Commands, MCP tools, app and browser control, | Every time, with the exact command or |
|             | the JavaScript sandbox, some system settings  | step.                                 |
| **Send**    | Send, reply, archive, add an event            | Every time, with the exact text.      |

What each Execute ask shows:

- `run_command`: the exact shell command. It runs with `/bin/sh -c` and is stopped after 60 seconds.
- MCP tools: the server, the tool and its arguments. The server's own read-only hints are not trusted, so
  every MCP tool call asks.
- App control (clicking, typing and pressing keys in another app, quitting an app, running a Shortcut):
  each step. After a no, the rest of that turn's steps are refused without asking.
- Browser control: every step except reading the page.
- System settings: turning Wi-Fi on or off, sleep and lock. Volume, mute and brightness do not ask.
- Loading an unreviewed skill and saving a new skill (see [automation.md](automation.md#trust-levels)).
- Looking at the screen, once the conversation has read outside content.

## Approvals

The approval dialog shows on the HUD (`workspace.rs`).

- An unanswered question becomes a no after 120 seconds.
- A Write or Execute denied once is refused again without asking: the same folder, or the exact same
  command, until the app restarts.
- A spoken no answers any question. A spoken yes answers only a Send. Writes and Execute steps need a
  click, because the microphone also hears the room, the TV and Markon's own voice.

**Always yes.** For `send_imessage`, `send_mail`, `send_slack` and `send_telegram` the dialog can offer
"always yes" for one recipient. Archive and calendar changes never offer it. A remembered yes:

- is stored in the memory database and can be cleared in Settings (`forget_approvals`),
- is not offered and not used once the conversation has read outside content (see
  [Outside content](#outside-content)).

**Audit log.** Every decision (allowed or refused, and why) is kept in memory for the running session
(`Permissions::audit`). It is not written to disk.

## Approved folders and secret files

You choose approved folders in Settings. Only absolute paths to existing folders are kept.

- Paths are resolved before the check: `.` and `..` are removed and symlinks are followed, so a link
  inside an approved folder cannot point the check outside it.
- Reading or writing outside every approved folder is refused and logged.
- Taking a folder off the list also removes it from the search index.

### Secret files

`read_file` and the indexer refuse secret files, even inside an approved folder (`is_secret_file` in
`guard.rs`). The check looks at the path, every symlink on the way to the target, and the resolved target.

- Names: `.env`, `.env.*`, `credentials`, `credentials.*`, `id_rsa`, `id_ed25519` and the other SSH key
  names, `.netrc`, `.npmrc`, `.pypirc`, `.pgpass`, `.htpasswd`, `.git-credentials`, `known_hosts`,
  `authorized_keys`, `password.txt` and similar.
- Extensions: `.pem`, `.key`, `.p12`, `.pfx`, `.jks`, `.keychain`, `.keychain-db`, `.kdbx`, `.secrets`.
- Anything inside `.ssh`, `.gnupg`, `.aws`, `.kube`, `.docker`, `Keychains` or `.password-store`.

Writing a file keeps the previous text for undo, except for secret, binary or large files.

### Password and card fields

Markon never types into a field that looks like a password, passcode, card number, CVC, security code or
one-time code, or types any text with 13 or more digits (`looks_secret`). This covers the browser and
other apps. Secure text fields in other apps are refused as well. The model is told to ask you to type it.

## Web fetches

`fetch_page` never reaches your Mac or its network (`guard.rs`):

- Only `http` and `https` links are fetched.
- Hosts `localhost`, `*.localhost`, `*.local` and cloud metadata names are refused.
- Literal IP addresses are checked before any connection: loopback, private, link-local, carrier-grade
  NAT, multicast, broadcast, documentation and reserved ranges, and the IPv6 equivalents, including
  IPv4-mapped and NAT64 addresses.
- DNS answers go through `PublicOnly`, which drops every private address. A name that resolves only to
  private addresses is refused. Because every connection resolves through it, this also covers
  redirects and DNS rebinding.
- At most five redirects, each checked the same way and only to web links.

`open_url` opens only `http`, `https` and `mailto` links, never `file:` or custom schemes
(`is_safe_url`). `open_app` takes plain app names only, no paths or flags (`is_safe_app_name`).

## Secret scrubbing

Every tool result, success or error, passes through `scrub` before it reaches the model or the screen.
Anything that looks like a secret becomes `[secret removed]`:

- Anthropic, OpenRouter and OpenAI-style keys, AWS access key ids, GitHub, GitLab and Slack tokens,
  Stripe keys, Google API keys.
- JSON Web Tokens.
- Database URLs with a password (`postgres://`, `mysql://`, `mongodb://`, `redis://`, `amqp://`).
- PEM private key blocks.
- `password=`, `api_key=`, `access_token=`, `client_secret=` and similar assignments.

## Keys in the keychain only

Every API key, token and password is stored in the macOS keychain under the service `com.markon.desktop`
(`KEYCHAIN_SERVICE`). `settings.json` holds no secrets, and a test checks that it has no key, token or
secret field.

| Account                                   | Holds                               |
| ----------------------------------------- | ----------------------------------- |
| `anthropic-api-key`, `openrouter-api-key` | Model keys                          |
| `cartesia-api-key`                        | The online voice                    |
| `relay-token`                             | The free-trial relay                |
| `github-token`                            | GitHub notifications                |
| `mail:<user>@<host>`                      | Mail app passwords                  |
| `telegram-api-hash`, `telegram-session`   | Telegram                            |
| One per MCP server                        | OAuth tokens for hosted MCP servers |

Secrets are read once per launch and kept in memory. The Telegram session and MCP tokens are written to
the keychain directly and never to a file. OAuth sign-in uses PKCE with a one-time listener on
`127.0.0.1` for the redirect.

## What leaves the Mac

Markon has no telemetry, analytics, crash reporting or update checks.

- **Microphone audio:** transcribed on your Mac, not stored, not uploaded. Once the speech models load,
  the microphone stays open to hear the wake phrase, and that audio is checked on your Mac too.
- **What you said, tool results, screenshots you ask for:** the model route you chose: Anthropic,
  OpenRouter, Claude Code, or a local model.
- **Reply text:** stays local with the Kokoro voice. Sent to Cartesia only if you pick the online voice.
- **Mail, messages, calendar, notes:** read locally. Private sources are summarised by a local model or
  left out before a cloud model sees anything.
- **API keys and tokens:** macOS keychain only (service `com.markon.desktop`).
- **History, memory, cards, traces:** a plain SQLite database in `~/.markon`. It is not encrypted.

Other connections:

- Model files from `huggingface.co` and `raw.githubusercontent.com`, only when a pinned file is missing.
  Each file's SHA-256 is checked and a mismatch is refused.
- MCP servers you list in `~/.markon/mcp.json`, and the accounts you connect as sources.
- Web pages the model fetches.

Settings lists these endpoints with the host each one uses (`settings_get` in `setup.rs`).

The default model is a cloud one, so Markon is not offline by default. For a local-only setup choose a
local server or the built-in model and keep the Kokoro voice.

## Outside content

Text from web pages, files, messages, mail, MCP servers, other apps' windows, Shortcut output and the
screen is someone else's words. Markon treats it as data:

- **Marked as data.** Each tool wraps it in tags (`<page>`, `<source>`, `<results>`, `<app>`) followed by
  a line saying it is data, not instructions.
- **Taints the conversation.** When a tool in `reads_outside` runs, the conversation is marked. From then
  on, a remembered "always yes" no longer applies and every send asks, and looking at the screen asks
  too. The mark is set before the tool runs, so a cancelled read still counts. Starting a new
  conversation clears it.
- **Private sources.** Sources you mark private reach a cloud model only as a local summary, or not at
  all (see [sources-and-memory.md](sources-and-memory.md#private-sources)).
- **Limited agents.** Scheduled agents see only the tools in their file. The bundled overnight helper has
  no send tools.

## The sandbox

`run_js` runs JavaScript in QuickJS (`rquickjs`) inside Markon (`crates/hands/src/sandbox.rs`):

- No files, no network, no `require`, `process` or `fetch`. Only `console.log` output and the last value
  come back.
- Memory is capped at 32 MB, the stack at 1 MB and run time at 3 seconds. An endless loop or a memory
  bomb stops with an error.
- Output is cut at 16 KB.
- Each program still asks first, on the Execute tier, showing the code.

`run_command` is not sandboxed. It runs as you, which is why it asks every time.

## Known gaps

- Fetching a web page or opening a URL does not ask. Text injected by a web page or an email could make
  the model request an attacker's URL, and the request itself can carry data.
- Approving a folder for writing covers dotfiles in it.
- "Stop" stops running tools, but a Mac control action (AppleScript) that has already started still
  finishes.
- The memory database is not encrypted.
- Items from private sources are added to the search index, and `search_my_stuff` results go to the
  model you chose, private or not.

Treat approved folders and connected sources accordingly.
