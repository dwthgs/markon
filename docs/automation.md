# Automation

Markon runs work without a prompt: agents on a schedule, reminders, meeting prep and the watch agent.
Skills teach it how to do a task. Both are plain files in `~/.markon`, read again on every use, so a new or
edited file works without a rebuild or restart.

Code:

- `crates/scheduler`: agent files, schedules, reminders, meeting prep timing, proposals.
- `crates/scheduler/agents`: the bundled agent files.
- `crates/tools/src/skills.rs` and `crates/tools/skills`: skills and the bundled ones.
- `crates/sources/src/watch.rs`: the watch agent's rules.
- `crates/memory/src/traces.rs`: traces.
- `crates/evals`: scenario replay.
- `schedule_loop` in `apps/desktop/src-tauri/src/core.rs`: the loop that runs all of it.

## The loop

`schedule_loop` runs every 30 seconds:

| Every      | Does                                                                        |
| ---------- | --------------------------------------------------------------------------- |
| 30 seconds | Reads the agent files, syncs schedules, runs due agents, delivers reminders |
| 5 minutes  | Runs the watch agent and meeting prep                                       |
| 10 minutes | Indexes approved folders and learns people from Contacts                    |

## Agents

An agent is one TOML file in `~/.markon/agents` (`crates/scheduler/src/files.rs`):

```toml
name = "Morning briefing"
schedule = "30 7 * * *"
tools = ["what_did_i_miss", "fetch_page", "remind_me"]
max_rounds = 6
enabled = true
prompt = """
Give me my morning briefing out loud...
"""
```

| Field        | Required | Meaning                                                                |
| ------------ | -------- | ---------------------------------------------------------------------- |
| `name`       | yes      | Shown in History and Settings. Also the schedule's key.                |
| `prompt`     | yes      | What the agent is asked, as if you said it.                            |
| `tools`      | no       | The only tools it may call. Empty or missing means none.               |
| `max_rounds` | no       | Tool rounds before it must answer, 1 to 30.                            |
| `schedule`   | no       | A cron line. Without one the agent runs only when something starts it. |
| `enabled`    | no       | Defaults to `true`.                                                    |

Unknown fields are refused. A file that does not parse (bad TOML, empty name or prompt, bad cron line,
`max_rounds` out of range) is skipped, and Settings lists it with the error.

### Schedules

Schedules live in the `schedules` table of the memory database, so they survive restarts.

- On every tick the table is synced with the files: a new file adds a schedule, a changed one updates
  it, and a deleted file removes its schedule.
- A due schedule is moved to its next time after now before it runs. Runs missed while the Mac was
  asleep collapse into one run on wake.
- Cron lines are parsed with `croner`, in local time.

### How an agent runs

`run_agent` in `core.rs`:

- Each run is its own conversation in History, titled with the agent's name.
- The tool list is an allow-list. Every other tool is hidden from the model, and a call to a name not
  on the list is refused.
- Tools still ask as usual. An agent with a send tool still needs your yes for every send.
- In a call or while sharing the screen (Zoom, FaceTime, Teams, Webex, Slack huddles), the run is
  muted and its result shows as cards only.

### Bundled agents

The bundled files are written to `~/.markon/agents` once. A marker file (`.bundled`) remembers what was
written, so a bundled file you delete stays deleted. Edit them freely.

**Morning briefing** (`morning-briefing.toml`): runs at 07:30. It calls `what_did_i_miss` once and fetches
the weather from `wttr.in`, then speaks calendar, messages, work and weather, one or two sentences each,
skipping empty sections. You can answer "more", "skip" or "remind me about this at three".

**Overnight helper** (`overnight-helper.toml`): off by default (`enabled = false`). At 03:00 it reads the
last 12 hours, sorts them into "needs me today", "can wait" and "no action", writes drafts for short
obvious replies with `draft_reply`, and sets 08:30 reminders for anything due today. Its tools are
`what_did_i_miss`, `draft_reply` and `remind_me`, so it cannot send anything. In the morning, "send the
draft to Sara" asks you as usual.

**Meeting prep** (`meeting-prep.toml`): see [Meeting prep](#meeting-prep).

## Reminders

`remind_me` (`crates/scheduler/src/reminders.rs`) stores a reminder in the database.

- `when` takes `15:00`, `3pm`, `3:30 pm`, `in 20 minutes`, `in 2 hours`, `in a day`,
  `2026-11-11 09:00` or `2026-11-11T09:00`.
- A bare time that has already passed today means tomorrow.
- When due, it is marked done and delivered once: always as a card, and spoken when Markon is idle and
  you are not in a call.

## Meeting prep

About ten minutes before a calendar event with other people in it, Markon runs the "Meeting prep" agent
file once for that event.

- Needs Calendar switched on and the file present with `enabled = true`.
- Checked every five minutes, for events starting in 5 to 11 minutes that have attendees.
- `{title}`, `{start}` and `{attendees}` in the prompt are filled from the event.
- Prepared events are remembered (the newest 200), so an event is never prepared twice.
- The bundled prompt looks back three days with `what_did_i_miss` and answers in three spoken sentences:
  who the people are, the last thing each sent, and any note that mentions the meeting.

## The watch agent

The watch agent speaks up when something important arrives. It is off by default and is configured in
Settings (`watch` in `settings.json`):

| Field     | Default                       |
| --------- | ----------------------------- |
| `enabled` | `false`                       |
| `people`  | empty                         |
| `words`   | `urgent`, `asap`, `emergency` |

Every five minutes it asks the switched-on sources for the last hour. No model is called
(`crates/sources/src/watch.rs`):

- An item matters when its sender contains a watched name, or its text contains a watch word as whole
  words ("urgently" does not match "urgent").
- Each item is keyed by source and id. A key already seen is skipped, so each item is announced once.
  The newest 500 keys are kept.
- At most three items per pass. Each gets a card and one sentence: "Sara Tesfaye just wrote: ...".

## Skills

A skill is a folder in `~/.markon/skills` with a `SKILL.md` file: YAML front matter, then instructions.

```markdown
---

name: weekly-report
description: "Write the weekly report. Use when the user asks for the weekly report."
---

1. Gather ...
2. Write ...
```

Rules (`from_text` in `skills.rs`):

- `name`: lowercase letters, digits and single hyphens, at most 64 characters. The folder must have the
  same name.
- `description`: required, at most 1,024 characters. It says when to use the skill.
- The front matter parser reads top-level `key: value` lines and `>` or `|` blocks.
- A folder whose skill has the name of a bundled skill is refused.

The skill catalog (name, trust and description of each skill) is in every turn's prompt. The model calls
`load_skill` to read one, and gets its instructions plus the names of any other files in the folder.

### Trust levels

| Trust        | Means                                                                  |
| ------------ | ---------------------------------------------------------------------- |
| `bundled`    | Ships inside Markon.                                                   |
| `yours`      | You approved this exact text.                                          |
| `unreviewed` | Dropped into the folder and never approved, or changed after approval. |

Approval writes the SHA-256 of the approved text to `.markon-approved` beside `SKILL.md`. Any edit to the
file changes the hash, so the skill falls back to unreviewed.

Loading an unreviewed skill asks you first, on the Execute tier. The catalog tells the model to follow
unreviewed skills only where they fit the request, and never to send, delete or run anything because of
one without asking. You can approve a skill as it is from Settings.

### Bundled skills

- `deep-research`: splits a question into two to four smaller ones, searches your own data first
  (`search_my_stuff`, `who_is`), then the web, cross-checks sources and answers in at most five sentences
  with numbered citations.
- `markon-setup`: how to connect each source and what it needs.

### "Learn this"

When you say "learn this" or "remember how to do this", the model calls `save_skill` with a name, a
description and the steps. You see the exact `SKILL.md` text and must approve it (Execute tier). The
skill is saved as yours.

## Proposals

Settings has a button that asks the model to improve your agent and skill files
(`suggest_improvements` in `setup.rs`, `crates/scheduler/src/proposals.rs`).

1. Notes come from the last 50 traces: turns that failed, took over 8 seconds or had a failed tool,
   up to 20 of them.
2. The model gets each file and the notes, for up to five files, and returns the whole file with small
   wording changes.
3. An unchanged answer is dropped. An agent file that would not parse is never proposed.
4. The new text is written beside the original as `<file>.proposed`. The original does not change.

Settings lists each proposal as a line diff. Then:

- **Approve** copies the original to `<file>.before` and moves the proposal into its place. An approved
  `SKILL.md` is marked as yours.
- **Reject** deletes the proposal.

Only files inside `~/.markon/agents` and `~/.markon/skills` can be approved or rejected this way.

## Traces

Every finished turn writes one row to the `traces` table (`crates/memory/src/traces.rs`). Traces never
leave the Mac.

| Field            | Meaning                                         |
| ---------------- | ----------------------------------------------- |
| `stt_ms`         | From the end of speech to the transcript        |
| `first_text_ms`  | From the request to the model's first word      |
| `first_audio_ms` | From the request to the first sound             |
| `total_ms`       | The whole turn                                  |
| `tokens_in/out`  | Tokens used                                     |
| `tools`          | Each tool's name, time and whether it succeeded |
| `ok`             | Whether the turn finished without an error      |

The first 200 characters of what you said are kept with the provider and model. The newest 2,000 traces
are kept. Settings shows the latest 15. Tool calls run in parallel and are timed in order, so a fast call
behind a slow one can show the slow one's time.

## Evals

Evals replay saved scenarios through the real agent loop against a real model, and score them
(`crates/evals`). They are a manual command and never part of the test suite:

```bash
cargo run -p markon-evals                    # the bundled scenarios.json
cargo run -p markon-evals -- my-scenarios.json
MARKON_EVAL_LOCAL=qwen2.5 cargo run -p markon-evals   # a model on a local Ollama server
```

Without `MARKON_EVAL_LOCAL` it uses the Anthropic key from the keychain and costs real tokens.

A scenario:

| Field         | Meaning                                       |
| ------------- | --------------------------------------------- |
| `name`        | Shown in the report                           |
| `say`         | What the user says                            |
| `tool`        | The tool that should be called first, or none |
| `contains`    | Words the answer must contain (any case)      |
| `never`       | Words it must not contain                     |
| `tool_result` | What the stand-in tool returns                |
| `limit_ms`    | Time limit, 8,000 by default                  |

Tools are stand-ins: each returns the scenario's `tool_result`. The model is offered short descriptions
of the main tools and the real descriptions of the Mac tools. Each scenario prints PASS or FAIL with its
time to first text, total time, the tools called and what went wrong. The bundled scenarios cover
catching up, reminders, document search, music, plain questions, speed, Mac control (shortcuts, volume,
Focus, apps, windows, the screen) and a message that tries to give the model orders.
