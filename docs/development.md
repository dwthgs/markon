# Development

How to build Markon from source, run it, test it, and what CI checks. For how the parts fit together,
see [architecture.md](architecture.md).

## Prerequisites

- A Mac with Apple Silicon and macOS 13 or later. Markon builds and runs only on macOS.
- Homebrew build tools. `meson`, `ninja` and `pkgconf` build WebRTC's echo canceller:

  ```bash
  brew install cmake meson ninja pkgconf
  ```

- Rust through [rustup](https://rustup.rs). `rust-toolchain.toml` pins the version and adds `clippy`,
  `rustfmt`, `llvm-tools` and the `aarch64-apple-darwin` target.
- Node 24.16.0 and pnpm 12.4.2. The root `package.json` sets `engines` and `packageManager`, and
  `pnpm-workspace.yaml` sets `engineStrict`, so other versions are refused.

Dependency versions are exact (`saveExact` in pnpm, `=` versions in the Cargo workspace) and both
lockfiles are committed.

## Build and run

```bash
pnpm install
pnpm --filter @markon/desktop tauri dev
```

`tauri dev` starts Vite on port 5173 (`beforeDevCommand` in `tauri.conf.json`) and runs the Rust app
against it. The first build compiles whisper.cpp, llama.cpp and WebRTC's audio library and takes
several minutes.

On first launch Markon downloads the speech files from `models/manifest.json` to `~/.markon/models`
and checks each file's SHA-256. The built-in language model and the embedding model are marked
`on_demand` and are downloaded only from Settings.

Useful while developing:

- `MARKON_HOME=/some/folder` points Markon at another data folder instead of `~/.markon`, so a dev
  build does not touch your real settings, history or models.
- Dev builds print the window's frame rate (`[fps]`, from `report_fps`). Voice, push-to-talk and
  barge-in traces (`[voice]`, `[ptt]`, `[barge]`) go to the terminal in every build.
- `pnpm --filter @markon/desktop dev` runs the interface alone in a browser. Without Tauri every
  `invoke` resolves to `undefined` (`src/lib/tauri.ts`). Add `?e2e` to the URL to load the faked Rust
  side used by the e2e tests.
- `pnpm --filter @markon/voice-core harness` serves the voice core test harness on port 5174.
- `pnpm --filter @markon/site dev` serves the site on port 5180.

The Cargo dev profile keeps only line tables for debug info, so `target/` stays small. Dependencies
and `markon-audio` build at `opt-level = 2`, because unoptimised feature extraction on the playback
thread makes the voice crackle.

## Signing dev builds

macOS ties a keychain "Always Allow" grant to the app's code signature. Each unsigned rebuild gets a
new throwaway signature, so macOS asks for the login password again on the first keychain read.

`.cargo/config.toml` sets `scripts/dev-sign.sh` as the Cargo runner for `aarch64-apple-darwin`. The
script runs before the binary starts. When the binary is `markon-desktop` and its signature is ad hoc,
it signs it with a real certificate and the fixed identifier `com.markon.desktop.dev`:

- the identity comes from `$MARKON_SIGN_IDENTITY`, or else the first "Apple Development" certificate
  that `security find-identity -v -p codesigning` lists,
- with no certificate, the binary runs unsigned as before,
- if signing fails, it prints a warning and runs anyway.

Any other binary (tests, evals) runs unchanged. `MacOs` also keeps each secret in memory after the
first successful read, so a launch reads each secret from the keychain once.

## Checks

All of these must pass before a change is merged:

- `cargo clippy --workspace --all-targets -- -D warnings`: Clippy on every crate, test and binary,
  warnings as errors.
- `cargo test --workspace`: Rust unit tests and the `tests/` acceptance tests of every crate.
- `cargo deny check`: advisories, licences, banned crates and allowed sources (`deny.toml`).
- `pnpm -r typecheck`: TypeScript in the desktop UI, voice core, site and relay.
- `pnpm biome check .`: formatting and lint for TypeScript, JSON and CSS (`biome.json`).
- `pnpm -r test`: Vitest unit tests in every package.
- `pnpm --filter @markon/voice-core e2e`: the voice core in a real browser (pixels and frame time).
- `pnpm --filter @markon/desktop e2e`: the desktop UI in a browser with the Rust side faked.
- `pnpm --filter @markon/relay test`: the relay Worker's handler with a fake upstream and in-memory KV.

The root `package.json` also has `pnpm check`, which runs Biome, typecheck and the unit tests in one
go.

Vitest suites:

- `apps/desktop/src/**/*.test.ts`: card layout (`canvas/layout.test.ts`).
- `packages/voice-core/test`: the engine's state blending and drawing, against a recording canvas
  context.
- `apps/site/src/features.test.ts`: the site's Web Audio feature mapping, which matches
  `crates/audio/src/features.rs`.
- `services/relay/test/relay.test.ts`: registration, tokens, quotas and streaming through the
  relay.

## Tests never call real model APIs

No test needs a key, an account or a network connection.

- **Fake provider.** `crates/provider/src/fake.rs` has `FakeProvider`. Each call to `stream` pops the
  next script of deltas or errors. `FakeProvider::text` turns a reply into word deltas and
  `FakeProvider::tools` scripts tool calls. It records every request so tests can check the prompt.
  The agent, scheduler, sources and evals tests use it.
- **Mock servers.** Route and integration tests bind a server on `127.0.0.1:0` and point the client
  at it: the Anthropic and OpenRouter routes (`crates/provider/tests`), IMAP, GitHub and SMTP
  (`crates/sources/tests`), web fetches and redirects (`crates/tools/tests/acceptance.rs`) and the
  Cartesia voice (`crates/audio/src/cartesia.rs`). MCP tests run an `rmcp` server in process over a
  `tokio::io::duplex` pipe.
- **Fake `claude`.** `crates/provider/tests/claude_code.rs` writes a small shell script named `claude`
  that records its input and prints canned JSON lines.
- **Fixture databases.** `crates/sources/tests/common` builds SQLite files shaped like the Mac's
  Messages, Calendar, Notes and Contacts databases in a temp folder. Tests never open the user's
  files.
- **Fake platform.** `crates/platform/src/fake.rs` has `FakeMac`, a `Platform` that records every
  command as a line ("apps quit Safari") and answers from a script. Permissions are granted per test
  with `FakeMac::with_permissions`. The Mac control and activity tests in `crates/tools/tests` use it
  through `Runtime::with_platform`, so no test touches real apps, settings or permissions.
- **Relay.** The relay tests pass a fake `fetch` and an in-memory KV (`MemoryKV`) to the handler.

`crates/evals` is the one place that talks to a real model, and only when run by hand with
`cargo run -p markon-evals`. Its tests check the scoring with `FakeProvider`.

## End-to-end tests

### Desktop UI

`apps/desktop/e2e` holds Playwright tests for the interface. `playwright.config.ts` starts Vite on
port 5173 and opens the app at `/?e2e` in Chromium at 1440 x 900.

In dev mode, `?e2e` makes `src/main.tsx` load `src/e2e/fakeTauri.ts`, which installs `mockIPC` from
`@tauri-apps/api/mocks`. The fake:

- records every command the UI invokes in `window.__e2e.calls`,
- answers `status` from a default that URL parameters can override (for example `?e2e&onboarded=false`),
- answers other commands from `window.__e2e.responses`, which tests seed with `page.addInitScript`,
- lets tests send Rust-side events with `window.__e2e.emit(event, payload)`.

`e2e/fixtures.ts` holds shared settings payloads and helpers. The specs cover the shell, talking and
text input, the canvas, media cards, history, settings, onboarding and the theme.

### Voice core

`packages/voice-core/e2e` holds Playwright tests against the harness in `packages/voice-core/harness`,
served on port 5174. `perf.spec.ts` checks that the `prime` variant draws an 800 px frame in under 4 ms
(median).

Install the browser once before running either suite:

```bash
pnpm exec playwright install chromium
```

## Repository layout

```
apps/desktop/             the app
  src-tauri/              Rust shell: Core, commands, tray, settings (crate markon-desktop)
  src/                    React interface: HUD, canvas, cards, dialogs
  e2e/                    Playwright tests with the Rust side faked
apps/site/                the project's web page
packages/voice-core/      the animated voice core, a standalone React component
crates/
  agent/                  agent loop, state machine, loop guard, secret scrubbing
  audio/                  microphone, wake phrase, whisper, Kokoro, echo canceller, barge-in
  evals/                  scenario scoring against a real model, run by hand
  hands/                  Chrome over DevTools, JavaScript sandbox
  llm/                    built-in model (llama.cpp) and the embedding model
  memory/                 SQLite: history, facts, cards, index, traces, activity
  platform/               the Platform trait, MacOs, FakeMac
  provider/               model routes and the fake provider
  scheduler/              agent files, cron schedules, reminders, proposals
  sources/                mail, Messages, Calendar, Notes, GitHub, Telegram, MCP, search
  tools/                  tool runtime, permission tiers, cards, MCP client, skills
services/relay/           Cloudflare Worker for a free trial (not deployed)
models/manifest.json      model and voice files: URL, size, SHA-256, licence
scripts/dev-sign.sh       Cargo runner that signs dev builds
```

## CI

`.github/workflows/ci.yml` runs one `check` job on `macos-latest` for pushes to `main` and for pull
requests. It sets up Rust 1.99.0 with `clippy`, `rustfmt` and `llvm-tools`, pnpm 12.4.2 and Node
24.16.0, then runs:

1. `brew install cmake meson ninja pkgconf`
2. `pnpm install --frozen-lockfile`
3. `pnpm exec playwright install chromium`
4. `pnpm biome check .`
5. `pnpm -r typecheck`
6. `pnpm -r test` (this includes the relay tests)
7. `pnpm --filter @markon/voice-core e2e`
8. `pnpm --filter @markon/desktop build`
9. `pnpm --filter @markon/desktop e2e`
10. `cargo install cargo-deny@0.20.2 --locked`
11. `cargo clippy --workspace --all-targets -- -D warnings`
12. `cargo test --workspace`
13. `cargo deny check`

CI runs on macOS only. There are no Windows or Linux jobs.
