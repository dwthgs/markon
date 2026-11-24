# Models

Markon talks to one language model at a time, through a route you pick in Settings. Every route streams
the same internal deltas, so the agent loop does not care which one is in use. Speech, voice, meaning search
and the built-in model are local files pinned by hash.

| Part                              | Code                                                                               |
| --------------------------------- | ---------------------------------------------------------------------------------- |
| The route trait and shared types  | `crates/provider/src/lib.rs`                                                       |
| Anthropic                         | `crates/provider/src/anthropic.rs`                                                 |
| Claude Code                       | `crates/provider/src/claude_code.rs`                                               |
| OpenRouter                        | `crates/provider/src/openrouter.rs`, `crates/provider/data/openrouter-models.json` |
| Local servers                     | `crates/provider/src/local.rs`                                                     |
| Plain-text tool calling           | `crates/provider/src/text_tools.rs`                                                |
| Built-in model and meaning search | `crates/llm/src/lib.rs`, `crates/llm/src/embed.rs`                                 |
| Choosing a route, private routing | `apps/desktop/src-tauri/src/core.rs` (`provider_for`, `private_route`)             |
| Settings and keys                 | `apps/desktop/src-tauri/src/settings.rs`, `apps/desktop/src-tauri/src/setup.rs`    |
| Model files                       | `models/manifest.json`, `crates/audio/src/manifest.rs`                             |

The agent loop that uses these routes is described in [agent-and-tools.md](agent-and-tools.md).

## How a route works

Each route implements `ModelProvider`:

- `stream` sends a request and returns a stream of deltas: text, tool calls, finished content blocks, token
  usage and a stop reason.
- `models` lists the models the route offers. `default_model` names the one used when none is chosen.
- `validate` checks the key or connection and reports a plan or balance when the service exposes one.

Inside Markon the history is kept in the Anthropic Messages shape. Routes that speak another format convert
it on the way out. A route change takes effect on the next turn, with no restart.

Errors are normalised. A rejected key is `Auth`, 429 is a rate limit, 529 and 503 are overload, and 404 means
the model is not available. The agent retries rate limits and overload, and falls back to the route's default
model when the chosen one is gone.

## Routes

| Route        | Setting      | Key               | Tools                             | Default model               |
| ------------ | ------------ | ----------------- | --------------------------------- | --------------------------- |
| Anthropic    | `anthropic`  | Anthropic API key | Native, plus web search and fetch | `claude-opus-5-5`           |
| Claude Code  | `claudecode` | None              | Plain text                        | `sonnet`                    |
| OpenRouter   | `openrouter` | OpenRouter key    | Native                            | `anthropic/claude-opus-5.5` |
| Local server | `local`      | None              | Native                            | Picked by the Mac's memory  |
| Built-in     | `builtin`    | None              | Plain text                        | `qwen2.5-1.5b-instruct`     |

The route is `settings.route` in `~/.markon/settings.json`. `settings.model` picks a model on that route.
When it is empty, the route's default is used.

### Anthropic

`AnthropicDirect` calls the Messages API at `api.anthropic.com` with your key, streamed over server-sent
events.

- Models offered: Claude Opus 5.5 (default), Claude Sonnet 5.5, Claude Haiku 5.5.
- Anthropic's own web search and web fetch tools are added to every request, each with at most 5 uses per
  request. Anthropic runs them on its side. This is the only route with web search.
- When a server-side search loop pauses, the agent sends the turn again to resume it, within the turn's
  round and time limits.
- Thinking blocks are kept within a turn, so a tool loop sends them back unchanged.
- Requests ask for Anthropic's server-side fallback (`"fallbacks": "default"`).
- Screenshots from `look_at_screen` are sent as images. This is the route that sees pictures.
- `validate` lists the account's models. Anthropic does not expose plan or balance.

### Claude Code

`ClaudeCode` runs the Claude Code CLI that is already installed and signed in on the Mac. Replies are billed
to your Claude plan, and Markon needs no API key.

- Markon looks for `claude` on `PATH`, then in `~/.local/bin`, `~/.claude/local`, `/opt/homebrew/bin` and
  `/usr/local/bin`. An app opened from the Finder does not get the shell's `PATH`, hence the fixed list.
- Each round runs `claude -p` with streamed JSON output. Claude Code's own tools, settings files, MCP
  servers, slash commands and session history are all switched off.
- The process gets a clean environment: only `HOME`, `USER`, `LOGNAME`, `LANG`, `TMPDIR` and a short `PATH`.
  Inherited `ANTHROPIC_*` or `CLAUDE_*` variables could switch it to a key or another host. It runs in the
  temp folder, so no project `CLAUDE.md` is picked up.
- The conversation is sent as tagged `<user>` and `<assistant>` turns. Markon's tools travel as text (see
  [Plain-text tool calling](#plain-text-tool-calling)).
- Models: `sonnet` (default), `opus`, `haiku`.
- `validate` runs `claude auth status` and shows your Claude plan.
- On first launch, if Claude Code is found and the route is still Anthropic, Markon switches the route to
  Claude Code once. A route you pick later is kept.

### OpenRouter

`OpenRouter` calls OpenRouter's OpenAI-compatible chat completions API with your key.

- Only the models in `crates/provider/data/openrouter-models.json` are offered:

  | Model id                              | Name              |
  | ------------------------------------- | ----------------- |
  | `anthropic/claude-opus-5.5` (default) | Claude Opus 5.5   |
  | `anthropic/claude-sonnet-5.5`         | Claude Sonnet 5.5 |
  | `anthropic/claude-haiku-5.5`          | Claude Haiku 5.5  |
  | `openai/gpt-5.6-sol`                  | GPT-5.6 Sol       |
  | `google/gemini-3.8-flash`             | Gemini 3.8 Flash  |
  | `mistralai/mistral-large-4-0`         | Mistral Large 4   |

- History is converted to OpenAI chat messages (`to_openai`). Thinking blocks are dropped.
- Tools are sent as OpenAI function definitions. Streamed tool-call fragments are joined before they run.
- `validate` reads the key's details and shows free tier or paid, and the money left or used.

### Local servers

`LocalServer` talks to a model server you already run on the Mac, such as Ollama or LM Studio, in the OpenAI
chat shape. This route is experimental.

- The default address is Ollama's, `http://127.0.0.1:11434`. LM Studio's is `http://127.0.0.1:1234`. Set
  it in `settings.local_url`.
- The HTTP read timeout is 180 s, because the first request can take a while as the model loads. The agent
  loop still gives up on a round after 45 s with no output.
- `validate` checks that the server answers and that the model is on it. If it is not, it says to run
  `ollama pull <model>`.

When no model is chosen (`settings.local_model`), Markon picks one that fits the Mac's memory. These are the
Ollama tags it knows, all of which call tools:

| Tag            | Download | Memory needed |
| -------------- | -------- | ------------- |
| `qwen3.5:0.8b` | 1.0 GB   | 4 GB          |
| `qwen3.5:2b`   | 2.7 GB   | 6 GB          |
| `qwen3.5:4b`   | 3.4 GB   | 8 GB          |
| `qwen3.5:9b`   | 6.6 GB   | 12 GB         |
| `qwen3.5:27b`  | 17 GB    | 24 GB         |
| `qwen3.5:35b`  | 24 GB    | 32 GB         |
| `qwen3.5:122b` | 81 GB    | 96 GB         |

The default is the second-largest model that fits, which leaves room for everything else. A 32 GB Mac gets
`qwen3.5:27b` and an 8 GB Mac gets `qwen3.5:2b`. Settings shows the Mac's memory and the models that fit.

### Built-in model

The built-in model runs inside Markon with llama.cpp (`llama-cpp-2` with Metal). There is no server. This
route is experimental.

- The model is Qwen2.5 1.5B Instruct, quantised to Q4_K_M, a 1.1 GB GGUF file.
- It is not downloaded at first launch. You download it from Settings, with progress in the status line.
- The first time it is needed in a launch, the file is checked against its pinned SHA-256 and loaded once
  in the background. Until then the route says it is loading or what is missing.
- All layers run on the GPU. The context is 8,192 tokens. A conversation longer than that is refused with a
  clear error.
- Sampling: top-k 20, top-p 0.8, temperature 0.7.
- One reply is generated at a time.
- Prompts use Qwen2.5's ChatML format. Tools travel as text.

## Plain-text tool calling

Claude Code and the built-in model have no tools API. For them, `text_tools.rs` describes the tools in the
system prompt and reads tool calls back out of the reply, in the Qwen tool-call format:

- Tools are listed as JSON function signatures inside `<tools></tools>`.
- The model writes each call as JSON with `name` and `arguments` inside `<tool_call></tool_call>`.
- Results go back inside `<tool_response></tool_response>`.

The parser (`parse_calls`) is forgiving:

- several calls in one tag, a stray closing brace, and `parameters` in place of `arguments` all work
- Claude's own `<invoke>` and `<function_calls>` markup is read as calls too
- on the Claude Code route, the same call written twice in one reply runs once.

Text is streamed to the voice as it arrives, but `speakable` holds back anything that could be the start of
a tool call. Markup is never read aloud.

## Private routing

In Settings, Sources, each source can be marked private. When `what_did_i_miss` reports a private source,
its text is not sent to the model as it is (`crates/sources/src/tool.rs`, `for_cloud`):

1. If the built-in model has been downloaded, it summarises the private items on the Mac.
2. Otherwise, if the route is a local server or a local model is set, that server summarises them.
3. Otherwise the source is left out, and the model is told how many new items were kept private.

The summary is at most four short lines: who wrote and what each needs from you. Phone numbers, addresses,
codes and anything that looks like a password are left out. If the local model does not answer, the source
is left out.

The full items still show on the Source cards, which stay on the Mac. They are also added to the search
index, so they stay searchable: `search_my_stuff` treats hits from private sources the same way, summarised
on the Mac or left out, and only the other results go to the model as they are (`for_cloud` in
`crates/sources/src/search.rs`).

"Screen" can be marked private the same way. Screenshots then stay on their card and are never sent to the
model.

## Keys

API keys and tokens live only in the macOS keychain, under the service `com.markon.desktop`
(`crates/platform/src/lib.rs`, through the `keyring` crate).

| Account                                 | Holds                            |
| --------------------------------------- | -------------------------------- |
| `anthropic-api-key`                     | Anthropic API key                |
| `openrouter-api-key`                    | OpenRouter key                   |
| `cartesia-api-key`                      | Cartesia voice key               |
| `github-token`                          | GitHub source token              |
| `telegram-api-hash`, `telegram-session` | Telegram source                  |
| `mcp-oauth:<server>`                    | Sign-in tokens for an MCP server |

- Keys are entered in Settings (`set_secret`). A value that is empty, longer than 512 characters, or
  contains whitespace is refused.
- A route reads its key from the keychain on every request. Keys are never logged, and request URLs are
  kept out of error text.
- `settings.json` holds no keys or tokens. A test checks this.
- Settings shows whether each key is set, never the key itself.

## Meaning search

`search_my_stuff` searches your approved folders and what your sources returned before, by keyword and by
meaning. The meaning part uses all-MiniLM-L6-v2 (`crates/llm/src/embed.rs`):

- an int8 ONNX file of 23 MB, run with `ort`, the same runtime as the Kokoro voice
- a BERT uncased WordPiece tokenizer, at most 256 tokens per text
- mean pooling over the attention mask, scaled to unit length.

The model and its vocabulary are downloaded on demand from Settings. Without them, search is keyword only.

Indexing runs every ten minutes over the approved folders and reads only files that changed. Text is cut into
chunks of up to 1,600 characters with 200 characters of overlap. A query runs a full-text search (SQLite
FTS5, ranked by BM25) and a cosine similarity search over the chunk vectors. The two rankings are merged by
reciprocal rank fusion with k = 60 (`crates/memory/src/index.rs`).

## Model files

Markon ships no model files. Every file it downloads is listed in `models/manifest.json` with its URL, size,
SHA-256 and licence. The manifest is compiled into the app, so it cannot be swapped next to the models.

| Id                                   | Use                        | Size             | Downloaded      |
| ------------------------------------ | -------------------------- | ---------------- | --------------- |
| `whisper-base.en`                    | Speech to text             | 148 MB           | At first launch |
| `kokoro-v1`                          | Text to speech             | 86 MB            | At first launch |
| `voice-markon`, `voice-markan`       | The two voices             | 0.5 MB each      | At first launch |
| `g2p-gb-gold`, `g2p-gb-silver`       | Pronunciation dictionaries | 2.8 and 3.7 MB   | At first launch |
| `silero-vad`                         | Detecting speech           | 2.3 MB           | At first launch |
| `qwen2.5-1.5b-instruct`              | Built-in language model    | 1.1 GB           | On demand       |
| `minilm-l6-v2`, `minilm-l6-v2-vocab` | Meaning search             | 23 MB and 0.2 MB | On demand       |

Files live in `~/.markon/models`. The check in `crates/audio/src/manifest.rs` works like this:

- A file is accepted only when its size and SHA-256 both match the manifest.
- A download is written to a `.part` file, hashed as it arrives, and moved into place only if size and hash
  match. A file that does not match is deleted and refused.
- At start, every file that is not on demand is checked. On-demand files are checked when they are loaded.
- Downloads come from `huggingface.co` and `raw.githubusercontent.com`, and only when a pinned file is
  missing.
