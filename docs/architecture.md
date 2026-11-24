# Architecture

Markon is a macOS voice assistant built on Tauri 2. A Rust core does the listening, thinking, acting
and speaking. A React interface draws the animated voice core in the centre of a frameless window and
shows the work as cards around it. This page describes what the app is made of and how one spoken
request moves through it.

## The big picture

```
 microphone                                                   speakers
     |                                                            ^
     v                                                            |
 +---------------------- crates/audio ------------------------------+
 | wake listener -> push-to-talk / wake session -> whisper.cpp      |
 | echo canceller + Silero VAD (barge-in)        Kokoro or Cartesia |
 +------------------------------------------------------------------+
     | transcript                                    ^ sentences
     v                                               |
 +---------------- apps/desktop/src-tauri (Core) ----+--------------+
 | voice commands, "remember that" -> local reply    |              |
 | everything else -> crates/agent (Agent::submit)   |              |
 +---------------------------------------------------+--------------+
     |                         ^ deltas                  | events
     v                         |                         v
 crates/provider  <- model route (Anthropic, Claude Code,    React UI (apps/desktop/src)
                     OpenRouter, local server, built-in)    voice core + cards + dialogs
     |
     v tool calls
 tool stack: hands -> memory -> search -> actions -> sources -> skills -> reminders -> tools
     |            (each layer adds tools; crates/tools checks the permission tier)
     v
 crates/platform (MacOs): keychain, AppleScript, Shortcuts, screenshots, Accessibility
```

State lives in `~/.markon` and in the macOS keychain. See [Where state lives](#where-state-lives).

## The Tauri shell and the React interface

The Rust side of the app is `apps/desktop/src-tauri`. `src/lib.rs` builds the Tauri app:

- registers the global push-to-talk shortcut (`tauri-plugin-global-shortcut`, `Alt+Space` by default).
  Press and release call `Core::ptt_pressed` and `Core::ptt_released`.
- builds the app menu (Settings, Quit, and the standard Edit items so copy and paste work).
- starts `Core` in `setup` and stores it as managed state (`CoreState = Arc<Core>`).
- builds the menu-bar icon (`tray.rs`).
- exposes the commands the UI calls with `invoke`: `submit_text`, `cancel`, `answer_permission`,
  `history_list`, `open_conversation`, the settings commands in `setup.rs` and the source commands in
  `sources.rs`.

The window is configured in `tauri.conf.json`: frameless (`decorations: false`), maximized, dark
background, macOS 13 minimum. The CSP allows only local assets and a few media hosts. The asset
protocol scope starts empty. `Core` allows one local file at a time when a picture or media card
needs it (`allow_media` in `core.rs`).

The Rust core talks to the UI with Tauri events:

| Event                   | Sent from           | Carries                                       |
| ----------------------- | ------------------- | --------------------------------------------- |
| `markon://agent`        | `core.rs`           | `AgentEvent` (state, text deltas, tool calls) |
| `markon://audio`        | `core.rs`           | `AudioEvent` (transcripts, playback, errors)  |
| `markon://features`     | `core.rs`           | loudness, pitch and 48 bands for the core     |
| `markon://status`       | `core.rs`           | `Status` (state, models ready, route, hotkey) |
| `markon://card`         | `workspace.rs`      | one `Card` to add or update                   |
| `markon://cards-load`   | `workspace.rs`      | the cards of a conversation from History      |
| `markon://cards-remove` | `workspace.rs`      | card ids to drop                              |
| `markon://permission`   | `workspace.rs`      | a question for the permission dialog          |
| `markon://command`      | `voice_commands.rs` | a spoken window or canvas command             |
| `markon://menu`         | `lib.rs`            | a menu item id                                |

The interface is `apps/desktop/src`, built with Vite, React 19, TypeScript and Tailwind 4:

- `App.tsx` lays out the voice core, the cards, the HUD and the dialogs.
- `lib/tauri.ts` wraps `invoke`. Outside Tauri (a plain browser) every call resolves to `undefined`.
- `lib/core.ts` listens to the status, agent and audio events and types their payloads.
- `canvas/` holds the cards: `useCards.ts` keeps the list from `markon://card` events, `layout.ts`
  places them in one or two columns around the core, `CardView.tsx` and `MediaBody.tsx` render them.
- `hud/` holds the controls, text input, settings panel, onboarding, history, permission and quit
  dialogs, and the microphone meter.
- `components/thegridcn` holds the HUD frame component and its theme.

The UI only renders. Cards are built in Rust (`crates/tools/src/cards.rs`) and the UI draws whatever
arrives.

## The voice core package

`packages/voice-core` is a standalone React component with no app dependencies. The desktop app and
the site in `apps/site` both use it.

- `src/engine.ts`: `CoreEngine`. A seeded random source, a fixed 60 Hz step, a state blend between
  `idle`, `listening`, `thinking` and `speaking`, and live features (`rms`, `pitch`, 48 `bands`)
  pushed from outside.
- `src/draw.ts`: canvas drawing for three variants, `prime`, `shell` and `minimal`.
- `src/speech.ts`: `FakeSpeaker`, a synthetic voice that drives the core when no live audio arrives.
- `src/VoiceCore.tsx`: the React component. A `FeatureFeed` ref pushes features straight into the
  running engine without React renders.

In the app, `crates/audio/src/features.rs` computes loudness, pitch and 48 log-spaced bands 30 times a
second, for both the microphone (`Who::User`) and Markon's voice (`Who::Markon`). The scaling matches a
Web Audio analyser, so the core looks the same in the app and on the web.

## Crates

The Cargo workspace is listed in the root `Cargo.toml`. Every crate inherits the workspace lints,
which deny `unsafe_code` and turn on all default Clippy lints.

### `crates/agent`

The agent loop and the state machine.

- `Agent`: holds the history, the model route (`set_provider` takes effect on the next turn), the
  prompt sections and the tool runtime. `submit(UserTurn)` runs one turn.
- `AgentState`: `Idle`, `Listening`, `Thinking`, `Speaking`. `set_state` refuses illegal moves.
- `UserTurn`: text, source (`Voice`, `Text`, `Schedule`), an optional tool allow-list and round
  limit for scheduled agents.
- `AgentEvent`: what the loop reports (`StateChanged`, `TextDelta`, `ToolCallStarted`,
  `ToolCallFinished`, `TurnFinished`, `TurnCancelled`, `Error`). Serialised as `{type, data}`.
- `ToolRuntime` trait: `specs()` and `call(ToolCall)`. Implemented by the tool stack.
- `PromptSection` trait: a piece of the system prompt rendered per turn (persona, recalled facts,
  skill catalog).
- `AgentConfig`: 12 tool rounds, a 90 s turn limit, a 45 s stall limit on the stream, a 60 s limit per
  round of tool calls, 32 KB of tool output per call, 3 tries with back-off, history summarised past
  120,000 characters.
- `loop_guard.rs`: the same tool with the same input more than three times in a turn is refused.
- `scrub.rs`: removes API keys, tokens, private keys and connection strings from every tool result
  before it reaches the model or the UI.
- `progress.rs`: one short spoken progress line when a turn with tools runs past 4 s, at most one
  every 10 s and three per turn.

History is committed only when a turn finishes. A cancelled turn leaves nothing behind, except the
sentences already spoken when the user talked over the reply.

### `crates/audio`

The audio pipeline. `service.rs` holds `Audio`, the running service. cpal streams are not `Send` on
macOS, so each stream lives on its own thread.

- `stt.rs`: `SttEngine` trait and whisper.cpp through `whisper-rs`. Partial text comes from re-running
  whisper on the growing clip.
- `tts.rs`: `TtsEngine` trait and Kokoro-82M on ONNX Runtime (`ort`). `Voice` is `Markon`
  (`bm_daniel`) or `Markan` (`bf_isabella`).
- `g2p.rs`: text to Kokoro phonemes from the misaki dictionaries, suffix rules and number words.
- `cartesia.rs`: the optional hosted voice. A failed request speaks nothing.
- `wake.rs`: the wake phrase ("Hey Markon", "Hey Markan", "Hey Mark", "Mark"). An energy detector cuts
  short bursts from the always-open microphone, whisper transcribes each burst on the Mac, and the
  text is matched. Nothing is stored or sent.
- `vad.rs`: energy-based end-of-speech detection.
- `echo.rs` and `barge.rs`: talking over Markon. The echo canceller removes Markon's voice from the
  microphone. `barge.rs` runs Silero VAD with a noise floor learned while Markon is silent.
- `sentences.rs`: `SentenceQueue` splits a streamed reply so each sentence is spoken when complete.
- `resample.rs`: microphone audio to 16 kHz for whisper, Kokoro's 24 kHz to the output device.
- `manifest.rs`: `Manifest` reads `models/manifest.json`, downloads missing files and checks size and
  SHA-256. A file that does not match is never used. Files marked `on_demand` (the built-in model and
  the embedding model) are fetched only from Settings.
- `AudioEvent`: transcripts, `SpeechStarted` / `SpeechEnded`, `Features`, `PlaybackStarted` /
  `PlaybackEnded`, `BargeIn`, `Wake { rest }`, model and device errors.

### `crates/provider`

One trait, several model routes.

- `ModelProvider` trait: `route()`, `stream(ChatRequest) -> DeltaStream`, `models()`,
  `default_model()`, `validate()`.
- `ChatDelta`: the one shape every route streams: `Text`, `ToolCall`, `Block` (a finished content
  block kept verbatim), `Usage`, `Stop(StopReason)`.
- `Message`: role plus content blocks in the Anthropic Messages shape. History stays in this shape
  and routes convert at the edge.
- `ProviderError`: `NoKey`, `Auth`, `RateLimited`, `Overloaded`, `QuotaExhausted`,
  `ModelUnavailable`, `Http`, `Network`, `Parse`. Rate limits and overload are retried by the agent.
- `KeySource`: a closure that reads the key per request. The app reads the keychain. Tests pass a
  fixed value.

Routes:

| Module           | Route id     | What it talks to                                                     |
| ---------------- | ------------ | -------------------------------------------------------------------- |
| `anthropic.rs`   | `anthropic`  | Messages API over SSE with the user's key (`AnthropicDirect`)        |
| `claude_code.rs` | `claudecode` | `claude -p` from the installed Claude Code CLI, tools sent as text   |
| `openrouter.rs`  | `openrouter` | OpenAI-compatible chat completions over SSE                          |
| `local.rs`       | `local`      | Ollama or LM Studio on this Mac, OpenAI chat shape                   |
| `relay.rs`       | `relay`      | the free-trial Worker in `services/relay`, Messages API with a token |
| `fake.rs`        | `fake`       | scripted replies for tests, no network                               |

The built-in model lives in `crates/llm`. `text_tools.rs` describes tools in `<tools>` and parses
`<tool_call>` JSON for routes without a tools API (Claude Code and the built-in model). `sse.rs` is a
small Server-Sent Events parser.

On first launch, if Claude Code is installed and the route is still the Anthropic default, `Core`
switches the route to Claude Code once (`claude_code_switched` in settings).

### `crates/tools`

The tool runtime: native tools and MCP servers behind one list, run under permission tiers, each call
shown as a card.

- `Runtime`: holds `Permissions`, the MCP servers, the card sink, the cache folder and the platform.
  `Shared(Arc<Runtime>)` implements `ToolRuntime`.
- `permissions.rs`: `Tier` (`Read`, `Write`, `Execute`, `Send`), `Decision` (`Allow`, `Deny`,
  `Always`), the `Prompter` trait that asks the user, and the `ApprovalStore` trait for remembered
  "always yes" answers on sends. Read works only inside the folders the user allowed. Write asks once
  per folder. Execute and Send ask every time with the exact command or text.
- `guard.rs`: web fetches never reach private or local addresses (checked again on redirects), and
  secret files (`.env`, keys) are never read.
- `native.rs`: `list_files`, `read_file`, `write_file`, `open_url`, `open_app`, `run_command`,
  `fetch_page`, `show_image`, `play_media`, `close_cards`, `pause_media`, `spotify`.
- `mac.rs`: Shortcuts, system controls, apps and windows, other apps through the Accessibility API,
  and `look_at_screen`. Every OS call goes through `Platform`.
- `media.rs`: pictures, audio, video and Spotify cards.
- `mcp.rs`: MCP client on `rmcp`. Servers come from `~/.markon/mcp.json` in Claude Desktop's shape.
  HTTP servers always work. Stdio servers work only when their command is already installed.
- `oauth.rs`: OAuth 2.1 with PKCE for hosted MCP servers, a one-time listener on 127.0.0.1, tokens in
  the keychain.
- `skills.rs`: skills as folders with a `SKILL.md` (front matter `name`, `description`). The catalog
  goes in the prompt (`CatalogSection`) and `load_skill` reads one. Bundled skills ship in
  `crates/tools/skills`.
- `activity.rs`: `WithActivity` wraps the whole tool stack and writes every call that changed
  something to the activity log, with what undo needs when there is one.
- `cards.rs`: `Card` (`id`, `turn_id`, `kind`, `title`, `status`, `body`), `CardKind`, `CardStatus`
  and the `CardSink` callback.

### `crates/platform`

Everything OS-specific sits behind the `Platform` trait in `src/lib.rs`. `MacOs` is the only
implementation. Another OS is a new implementation of the same trait.

The trait covers:

- the data folder (`data_dir`: `~/.markon`, or `$MARKON_HOME` when set),
- keychain secrets (`secret_get`, `secret_set`, `secret_delete`) under the service
  `com.markon.desktop`, with account names in `platform::accounts`,
- opening URLs, apps and System Settings panes, and checking permissions,
- sending an iMessage and adding a calendar event,
- Spotify, Shortcuts, system controls, apps and windows, the Accessibility tree, screenshots
  (`control.rs`), all run with values passed as arguments, never pasted into script text,
- whether the user is in a call or sharing the screen (`in_call_or_sharing`).

`MacOs` reads each secret from the keychain once per launch and keeps it in memory, because every
keychain read by a newly built binary can make macOS ask for the password.

`voice_mic.rs` opens the microphone through Apple's Voice Processing I/O unit, which removes what the
Mac is playing from the microphone signal. `crates/audio/src/wake.rs` uses it on macOS and falls back
to WebRTC's echo canceller when it is not available.

`fake.rs` holds `FakeMac`, a stand-in used by tests. See [docs/development.md](development.md).

Some code still reaches macOS directly outside this crate: `crates/sources/src/mac.rs` opens the Mac's
own SQLite databases (paths come from `platform::mac_db`), `setup.rs` runs `osascript` for the folder
picker, and `crates/hands` drives Chrome.

### `crates/memory`

One SQLite file (`~/.markon/markon.db`) in WAL mode, with migrations in `crates/memory/migrations`
applied in order and tracked with `PRAGMA user_version`.

- `Memory`: conversations, turns, cards, facts, preferences and usage. `recall` searches facts with
  FTS5. `export` dumps everything as JSON and `wipe` clears it.
- `Source`: where a conversation came from (voice, text, schedule).
- `FactOrigin`: facts come only from what the user said or from confirmed tool results.
- `index.rs`: chunks of indexed files and source items, keyword search with FTS5 and optional vector
  search, merged with reciprocal rank fusion. People and their identities across sources.
- `traces.rs`: per-turn timings (speech to text, first word, first audio, each tool, tokens). Kept on
  this Mac only.
- `activity.rs`: the activity log and undo marks.

Tables: `conversations`, `turns`, `tool_calls`, `cards`, `facts`, `preferences`, `schedules`, `usage`,
`chunks`, `people`, `identities`, `traces`, `activity`, the FTS5 tables `facts_fts`, `turns_fts` and
`chunks_fts`, and `reminders` (created by the scheduler).

### `crates/scheduler`

Agents that run on a cron line without a prompt.

- `Scheduler`: schedules live in the memory database, so they survive restarts. A run missed while
  the Mac slept happens once on wake.
- `files.rs`: one TOML file per agent in `~/.markon/agents` (`AgentFile`: `name`, `prompt`, `tools`,
  `max_rounds`, `schedule`, `enabled`). Files are read on every tick, so a new file works without a
  rebuild. Three agents ship in `crates/scheduler/agents`: morning briefing, meeting prep and
  overnight helper.
- `AgentSpec`: the prompt and the only tools the agent may use.
- `Delivery`: during a call or screen share a run posts a card instead of speaking.
- `reminders.rs`: `remind_me` and the `reminders` table.
- `proposals.rs`: improved prompts and skill descriptions are written beside the originals as
  `<file>.proposed`, shown as a diff, and replace the original only when the user accepts.

`Core::schedule_loop` in `core.rs` ticks every 30 s: it syncs agent files, runs due agents, delivers
reminders, and every five minutes runs the watch rules and meeting prep. Every ten minutes it indexes
the allowed folders.

### `crates/sources`

What happened in the user's accounts, in one shape. Reading is read-only.

- `Source` trait and `Item` (`source`, `id`, `who`, `place`, `when_ms`, `text`, `link`, `unread`).
  `collect` asks every source at once and cuts off slow ones.
- Sources: `mac.rs` (Messages, Calendar, Notes, Contacts databases), `imap.rs` (mail with an app
  password, never marks anything read), `github.rs` (notifications), `telegram.rs` (as the user, on
  `grammers-client`, session in the keychain), `mcp.rs` (Slack or Notion through a connected MCP
  server).
- `tool.rs`: `what_did_i_miss`, one Source card per source. Sources marked private are summarised by a
  local model, or left out when none is set up, before a cloud model sees them (`Private`).
- `act.rs`: sending and replying per source, archiving, adding an event and drafts. Every action posts
  a card with the exact text, then asks on the Send tier. `smtp.rs` sends mail.
- `search.rs`: indexing of allowed folders and source items, `search_my_stuff`, `who_is` and
  `link_person`.
- `watch.rs`: the watch rules. An item matters when it is from a watched person or has a watch word,
  and it is new.

### `crates/llm`

The built-in model. Qwen2.5 1.5B Instruct loaded with llama.cpp (`llama-cpp-2`), no server. The file
is pinned in the manifest. `BuiltIn` implements `ModelProvider` with Qwen's own chat and tool format.
Experimental.

`embed.rs` holds `Embedder`: all-MiniLM-L6-v2 (int8 ONNX) on `ort` with a WordPiece tokenizer, for
search by meaning. Without it, search is keyword only.

### `crates/hands`

Drives the user's Chrome and runs small programs, each step asked first and shown on a card.

- `Driver` trait: `open`, `tree`, `click`, `type_text`.
- `chrome.rs`: `Chrome` drives Chrome over the DevTools protocol with `chromiumoxide`, in its own
  profile at `~/.markon/browser`. The window is visible.
- `outline.rs`: the page as an indented outline of `[role] "name"` lines from the accessibility tree,
  with numbers on everything clickable.
- `sandbox.rs`: JavaScript in QuickJS (`rquickjs`) with no files and no network, capped at 32 MB and
  3 s.
- `WithHands`: adds the browser tools (only when Chrome is installed) and the sandbox tool.

### `crates/evals`

Saved scenarios (`scenarios.json`) replayed through the real agent loop with stand-in tools, each
scored on the tool called, the words said and the time taken. Run by hand only:

```bash
cargo run -p markon-evals
```

It uses the Anthropic key from the keychain, or `MARKON_EVAL_LOCAL=<model>` for an Ollama server, and
costs real tokens on a hosted route. The test suite checks only the scoring.

## App modules

The modules in `apps/desktop/src-tauri/src` wire the crates together.

- `core.rs`: `Core` owns the platform, settings, `Audio`, `Agent`, tool `Runtime`, `Memory`,
  `Scheduler`, sources and skills. It builds the tool stack, picks the model route
  (`provider_for`), turns agent events into speech and audio events into turns, persists finished
  turns and cards, and runs the schedule loop. It also downloads and loads the speech models on a
  background thread (`prepare_models`).
- `workspace.rs`: the tool runtime with `HudPrompter` (permission questions as dialogs, answered with
  a click or, for Send only, by voice; unanswered questions are denied after 120 s), `TurnCards`
  (the per-turn Agent card and the final Result card), and MCP startup.
- `ptt.rs`: `Ptt`, push-to-talk on one key. Hold to talk, or tap and talk until a pause. After a reply
  a 6 s follow-up window listens without the wake phrase. `is_stop` recognises "stop", "cancel",
  "never mind", "shut up", "hold on" and similar, said right after the wake phrase.
- `settings.rs`: `Settings`, saved to `~/.markon/settings.json` with an atomic write. `Route`
  (`anthropic`, `openrouter`, `relay`, `local`, `builtin`, `claudecode`) and `VoiceEngine`
  (`kokoro`, `cartesia`). A corrupt file is kept aside as `settings.json.bad`.
- `setup.rs`: onboarding and settings commands: read and patch settings, store keys, validate a
  route, preview a voice, pick a folder, download the optional models, export memory, wipe
  everything, and accept or reject skills and proposals.
- `sources.rs`: which sources are switched on, their keychain secrets, Full Disk Access, Telegram
  sign-in and MCP sign-in. Builds the `Source` list and the accounts used for sending.
- `tray.rs`: the menu-bar icon, one pre-rendered still per agent state.
- `remember.rs`: `RecallSection` puts recent and relevant facts in the prompt, `WithMemory` adds the
  `remember` tool, and `parse` handles "remember that ..." and "forget that" locally.
- `voice_commands.rs`: window and canvas commands (hide, quit, settings, tidy, clear, standby, bigger
  and more) handled without a model call. Phrases live in `voice_commands.json`. The same words are
  passed to whisper as a vocabulary hint.

## How one spoken request travels

1. **Microphone.** Once the speech models load, `Audio::start_wake` keeps the microphone open for the
   wake phrase (`wake.rs`). Audio is checked on the Mac and not stored.
2. **Wake or push-to-talk.** The hotkey calls `Core::ptt_pressed` / `ptt_released`, and `Ptt` decides
   to listen, finish or abandon. "Hey Markon" emits `AudioEvent::Wake { rest }`. If `rest` already
   holds a request, it is submitted at once. Otherwise Markon says "Yes?" and listens when that ends.
   `Core::start_listening` sets the state to `Listening` and starts recording.
3. **Speech to text.** On a pause or key release, `Core::finish_listening` calls
   `Audio::stop_listening`, which runs whisper on the clip. After a barge-in, a transcript of nothing,
   a filler word or Markon's own words counts as a false stop and is dropped.
4. **Local handling first.** `Core::submit` checks, in order: an open permission question (a spoken
   yes or no answers it), a voice command, and "remember that" / "forget that". These get a local reply
   with no model call.
5. **Agent loop.** `Agent::submit` renders the prompt sections (persona, recalled facts, skill
   catalog), sets `Thinking`, and streams the model through the chosen `ModelProvider`. Text deltas
   are emitted as they arrive. Rate limits and overload are retried. A model that is no longer
   available falls back to the route's default.
6. **Tools and approvals.** When the model stops for tool use, `run_tools` starts every call in
   parallel through the tool stack. Each layer adds its own tools and passes the rest down:
   `WithActivity` (log) → `WithHands` (browser, sandbox) → `WithMemory` (`remember`) → `WithSearch` →
   `WithActions` (send, reply) → `WithSources` (`what_did_i_miss`) → `WithSkills` → `WithReminders` →
   `Shared` (native, Mac and MCP tools). Calls that need permission go to `Permissions`, which asks
   through `HudPrompter`: the UI shows a dialog and `answer_permission` returns the decision. Results
   are scrubbed for secrets, capped, and sent back to the model for the next round. A tool result that
   reads outside content (web pages, files, mail, MCP) marks the conversation, and later sends always
   ask.
7. **Cards on the canvas.** Tools post `Card`s through the card sink. `Core` emits each one as
   `markon://card`, saves it with the conversation, and tracks it per turn so a cancelled turn removes
   its cards. `TurnCards` keeps one Agent card per turn listing its steps.
8. **Spoken reply.** Each `TextDelta` goes into `SentenceQueue`. Every complete sentence is handed to
   `Audio::speak` (Kokoro on the Mac, or Cartesia when chosen). Playback start sets `Speaking`.
   Features from the voice drive the core. When playback ends and the turn is done, a short follow-up
   window listens again without the wake phrase.
9. **Interruptions.** If the user talks over the reply, `barge.rs` emits `BargeIn`. `Core` cancels
   speech and the turn, keeps only the sentences that played in the history, and starts listening.
   The next turn starts with a note that the user interrupted.
10. **Persisting.** On `TurnFinished`, `Core` writes the user and assistant turns, any facts the model
    saved with `remember`, token usage and a timing trace to the database.

Typed input follows the same path from step 4 (`submit_text`). Scheduled agents enter at step 5 with
their own tool allow-list (`Core::run_agent`).

## Where state lives

Everything is under `~/.markon` (`Platform::data_dir`; `$MARKON_HOME` overrides it):

| Path            | What it holds                                                                  |
| --------------- | ------------------------------------------------------------------------------ |
| `settings.json` | `Settings`: voice, route, model, hotkey, allowed folders, sources, watch rules |
| `markon.db`     | SQLite (WAL): history, cards, facts, index, schedules, reminders, traces, log  |
| `models/`       | speech, voice, VAD and optional model files from `models/manifest.json`        |
| `agents/`       | scheduled agents as TOML files                                                 |
| `skills/`       | user skills, one folder with a `SKILL.md` each                                 |
| `cache/`        | screenshots and images shown on cards                                          |
| `mcp.json`      | MCP servers in Claude Desktop's shape                                          |
| `browser/`      | the Chrome profile Markon drives                                               |

The database is not encrypted. "Wipe" in settings clears the database, the model and voice keys,
the cache and the settings. Model files stay.

Secrets live only in the macOS keychain under the service `com.markon.desktop`: the Anthropic and
OpenRouter keys, the relay token, the Cartesia key, the GitHub token, mail app passwords, Telegram's
API hash and session, and MCP OAuth tokens. Account names are in `crates/platform/src/lib.rs`
(`accounts`) and `apps/desktop/src-tauri/src/sources.rs`.

## Other parts of the repository

- `apps/site`: the project's web page, a Vite app that draws the voice core from
  `@markon/voice-core`.
- `services/relay`: a Cloudflare Worker for a free trial. A device registers, gets a short-lived JWT,
  and its `/v1/messages` requests are proxied to Anthropic with a per-device daily quota in KV. It is
  not deployed.
- `models/manifest.json`: every downloadable file with URL, size, SHA-256 and licence.
