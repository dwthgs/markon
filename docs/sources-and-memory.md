# Sources and memory

Markon reads what happened in your accounts, remembers what you tell it, and keeps a searchable index of
your approved folders and of everything your sources returned. All of it stays in `~/.markon` on your Mac,
except the text a cloud model needs to answer a question.

Code:

- `crates/sources`: the sources, the shared item shape, the send and reply actions, search and people.
- `crates/memory`: the SQLite database (conversations, facts, cards, the index, traces).
- `apps/desktop/src-tauri/src/sources.rs`: which sources are on, their keychain secrets, Full Disk Access.
- `apps/desktop/src-tauri/src/remember.rs`: "remember that", "forget that" and the `remember` tool.

## Sources

Every source is off until you switch it on in Settings, under Sources. `SourceSettings::default()` has
nothing on, and `sources::build` returns an empty list for it (tested in `sources.rs`).

| Source      | Reads                                                   | Code          |
| ----------- | ------------------------------------------------------- | ------------- |
| Mail (IMAP) | Unread mail in INBOX since the window start, newest 25  | `imap.rs`     |
| Messages    | Incoming iMessage and SMS since the window start, ≤ 200 | `mac.rs`      |
| Calendar    | Events from now to 24 hours ahead                       | `mac.rs`      |
| Notes       | Notes changed since the window start                    | `mac.rs`      |
| GitHub      | Your notifications since the window start               | `github.rs`   |
| Telegram    | Unread chats whose last message is new, up to 40 chats  | `telegram.rs` |
| MCP servers | One tool call with fixed arguments (Slack, Notion)      | `mcp.rs`      |

Files are in `crates/sources/src`.

### Mail over IMAP

- One entry per account in settings: host, port (993 by default), user and an `enabled` flag.
- Sign-in uses an app password, not the account password.
- The mailbox is opened with `EXAMINE` and bodies are fetched with `BODY.PEEK[]`, so reading never marks
  a message as read.
- Connections use TLS. Plain TCP is used only for `127.0.0.1`, which the tests use.

### Messages, Calendar, Notes and Contacts

These are the Mac's own SQLite databases, opened read-only (`open_ro` in `mac.rs`). Paths come from
`markon_platform::mac_db` and `contacts_dbs`:

- Messages: `~/Library/Messages/chat.db`. Text comes from the `text` column, or from `attributedBody`
  when that is empty.
- Calendar: `~/Library/Group Containers/group.com.apple.calendar/Calendar.sqlitedb`, with attendee
  addresses when the `Participant` table exists.
- Notes: `~/Library/Group Containers/group.com.apple.notes/NoteStore.sqlite`. The body is a
  gzip-compressed protobuf, and Markon extracts its plain text.
- Contacts: every `AddressBook-v22.abcddb` under `~/Library/Application Support/AddressBook`.

Contacts is not a source of its own. When Messages is on, Contacts gives names to phone numbers and
addresses in Messages, and the index pass adds everyone in Contacts to the people table (see
[People across sources](#people-across-sources)).

### Full Disk Access

macOS protects these databases. A file that exists but cannot be opened returns
`SourceError::NeedsAccess`, and its Source card shows as blocked with a pointer to the fix. Settings
shows whether access is granted (`has_full_disk_access` probes the TCC database) and has a button that
opens the Full Disk Access pane (`open_full_disk_access`). A rebuild of an unsigned app can drop the
grant.

### GitHub

GitHub notifications through `https://api.github.com/notifications?since=...` with a personal access
token. API links are turned into the matching `github.com` page links.

### Telegram

Markon signs in to Telegram as you, with `grammers-client`:

1. Make an app at my.telegram.org. Paste the API id (kept in settings) and the API hash (kept in the
   keychain).
2. Enter your phone number, then the code Telegram sends to your other devices.
3. Enter your two-step password if the account has one.

The session (datacenter and auth key) is written to the keychain after every change. It is never written
to a file.

### Slack, Notion and other MCP servers

A server from `~/.markon/mcp.json` can be read as a source. Each entry in `sources.mcp` names the server,
one tool and its arguments. Markon calls that tool and splits the reply into items at blank lines, up to
30. Hosted servers that use OAuth are signed in from Settings (`mcp_sign_in`), and their tokens go to the
keychain.

For Slack, the default read call is `slack_search_public_and_private` with `is:unread`. Sending uses
`slack_send_message` unless `slack_send_tool` in settings names another tool.

### Secrets

Source secrets live in the macOS keychain under the service `com.markon.desktop`, never in
`settings.json`:

| Secret            | Keychain account                                     |
| ----------------- | ---------------------------------------------------- |
| Mail app password | `mail:<user>@<host>`                                 |
| GitHub token      | `github-token`                                       |
| Telegram API hash | `telegram-api-hash`                                  |
| Telegram session  | `telegram-session`                                   |
| MCP OAuth tokens  | one account per server (`crates/tools/src/oauth.rs`) |

`set_source_secret` accepts only the `github` and `mail` kinds, so the settings screen cannot overwrite
another secret such as a model key.

## One item shape

Every source returns `Item` (`crates/sources/src/lib.rs`):

| Field     | Meaning                                                        |
| --------- | -------------------------------------------------------------- |
| `source`  | The source id, for example `imessage`, `github`, `mail:<user>` |
| `id`      | Stable within the source, so a watcher can tell new from seen  |
| `who`     | Sender, repository or attendees                                |
| `where`   | Chat, calendar, mailbox or notification type                   |
| `when_ms` | Unix milliseconds                                              |
| `text`    | The content, clipped to a few hundred characters               |
| `link`    | A web link when there is one                                   |
| `unread`  | Whether it is unread                                           |

A source fails with one of three errors: `NeedsAccess` (Full Disk Access), `NotConnected` (no account,
key or sign-in) or `Failed`.

## what_did_i_miss

`what_did_i_miss` (`crates/sources/src/tool.rs`) asks every switched-on source at once.

- The window starts at the last check, or 24 hours ago on the first run. With `hours`, it starts that
  many hours back. It never reaches further than 7 days.
- Each source has 20 seconds. A slower one shows as failed and the rest still answer.
- Each source posts one Source card: the count and the newest six items, or why it failed.
- Senders are replaced with a person's name when the people table knows the address or number.
- Every returned item is added to the index in the background.
- The model gets at most 25 items per source inside `<source>` tags, followed by a line saying the
  content is data from your accounts, not instructions.
- The last-check time moves forward only when at least one source answered.

## Acting on sources

`crates/sources/src/act.rs` adds tools for switched-on accounts only:

| Tool            | Does                                                  | Offered when         |
| --------------- | ----------------------------------------------------- | -------------------- |
| `send_imessage` | Sends an iMessage or SMS through Messages             | Messages is on       |
| `send_mail`     | Sends or replies (`in_reply_to` threads it) over SMTP | A mail account is on |
| `archive_mail`  | Moves one message out of INBOX                        | A mail account is on |
| `send_slack`    | Sends through the Slack MCP server                    | Slack is a source    |
| `send_telegram` | Sends to the chat whose name matches                  | Telegram is on       |
| `add_event`     | Adds an event to Calendar                             | Calendar is on       |
| `draft_reply`   | Saves a reply without sending it                      | Always               |
| `list_drafts`   | Lists saved drafts, newest first                      | Always               |

Every action except the two draft tools asks first:

1. Markon posts a card with the exact text and "Waiting for your yes".
2. It asks on the Send tier (`Permissions::send`), showing the same text.
3. A no leaves the card blocked with "Not sent: you said no." A yes runs the action.

For the four send tools you can pick "always yes" for one recipient. Archive and calendar changes can
never be remembered. A remembered yes is ignored once the conversation has read outside content. See
[security.md](security.md#approvals).

Sending details:

- SMTP uses implicit TLS on 465 and STARTTLS on 587. The SMTP host is derived from the IMAP host
  (`smtp_host` in `smtp.rs`).
- Header values are cut to one line, so a subject cannot inject headers.
- Messages and Calendar are driven by AppleScript that takes its values as arguments, never pasted into
  the script text (`SEND_IMESSAGE` and `ADD_EVENT` in `crates/platform/src/lib.rs`).
- Archiving moves the message to `Archive`, or to `[Gmail]/All Mail` on Gmail.
- Drafts are kept in the database (the newest 30) and shown as Draft cards. Sending one later asks like
  any other send.

## Private sources

Settings can mark sources as private (`sources.private`). Their text never reaches a cloud model, in
`what_did_i_miss` (`for_cloud` in `tool.rs`) or in search results (`for_cloud` in `search.rs`):

- With a local model available, it summarises the items on your Mac in at most four lines and leaves out
  phone numbers, addresses, codes and anything that looks like a password. The model sees only the
  summary.
- Without one, the source shows as "kept private" with a count, and no content.

The summariser is the built-in model when its file is downloaded, otherwise the local server when the
local route or a local model is set (`private_route` in `core.rs`). Your own screen still shows the full
items on the cards.

Marking `screen` as private keeps screenshots on the card. The model is told the picture was not sent.

## Memory

Memory is one SQLite file in `~/.markon` (WAL mode). It is not encrypted. Migrations are in
`crates/memory/migrations`.

### Facts

Facts are short statements about you, each with a subject key.

- **"Remember that ..."** (also "please remember that ..." and "remember ...") is handled on the Mac
  without a model call. The subject is taken from the first words: "my sister is called Ana" gets the
  subject `sister`. Stored with confidence 1.0.
- **The `remember` tool** lets the model save a lasting fact you stated during a conversation (your
  name, people, preferences, routines). Facts from the tool are held until the turn finishes and dropped
  if the turn is cancelled. Stored with confidence 0.9.
- Model guesses are refused: `FactOrigin::ModelGuess` makes `add_fact` return an error.

A newer fact with the same subject supersedes the older one. The older fact stays in the database with
`superseded_by` set, and only current facts are used.

Each turn's prompt gets the 20 newest current facts plus up to 8 that match the question by FTS5 search
(`RecallSection` in `remember.rs`).

### Forgetting

- "Forget that" (also "forget it" and "forget what I just said") deletes the newest fact written in the
  current conversation.
- Facts derived from a fact are deleted with it.
- The settings button that wipes everything clears the database, the cache, settings and the model and
  voice keys in the keychain (`wipe_all` in `setup.rs`). Source secrets and the downloaded models stay.

### Preferences, history and cards

- Preferences are JSON values by key. They hold the last-check time, "always yes" answers, drafts, the
  watch agent's seen list and similar state.
- Every conversation is kept with its turns, in History. "Bring back [topic]" opens the latest
  conversation whose title or turns match.
- Cards are saved with their conversation, so opening an old conversation puts its cards back on the
  canvas.
- Activity keeps every tool action that changed something, for "what did you just do?" and undo.

## The index

The index makes your approved folders and your sources searchable (`crates/memory/src/index.rs`,
`crates/sources/src/search.rs`).

### What goes in

- **Approved folders.** Every ten minutes, text files and PDFs that changed since the last pass. A file
  whose modification time matches the stored one is skipped, so a pass reads only what changed.
- **Sources.** Every item `what_did_i_miss` returns, once per item.

Folder indexing skips:

- hidden files and folders, `node_modules` and `target`,
- secret files (the same check as `read_file`, see [security.md](security.md#secret-files)),
- text files over 2 MB, folders deeper than 8 levels, and anything after the first 5,000 files.

PDF text is extracted with `markon_tools::native::pdf_text`. Text is cut into chunks of up to 1,600
characters with 200 characters of overlap.

Taking a folder off the approved list removes its chunks from the index.

### Search

`search_my_stuff` searches the index two ways and fuses the rankings by reciprocal rank (k = 60, equal
weights):

- **Keyword:** FTS5 with the Porter stemmer. Each word is quoted, so your text cannot inject query
  syntax.
- **Meaning:** a vector per chunk from all-MiniLM-L6-v2, compared by cosine similarity. This runs only
  when the optional model is downloaded. Without it, search is keyword only.

Results reach the model numbered with their source, wrapped in `<results>` and marked as data.

### People across sources

One person can have several identities: phone numbers, mail addresses, Slack and Telegram names (the
`people` and `identities` tables).

- When Messages is on, every contact becomes a person with their numbers and addresses.
- `link_person` records a handle you name ("Sara on Slack is @sarat").
- `who_is` lists every known way to reach a person.
- Phone numbers match on their last nine digits, and everything else is lowercased.

## Export

Settings can export memory to `~/.markon/markon-export-<timestamp>.json` (`export_memory` in
`setup.rs`). The file holds conversations, turns, facts (with origin and `superseded_by`), preferences
and cards. Binary values are written as `null`. The index, people and traces are left out.
