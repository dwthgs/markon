# Voice: how Markon listens and speaks

Markon hears you through the Mac's microphone, turns speech into text with whisper.cpp, and answers
with the Kokoro voice. Speech recognition and the default voice run on the Mac. You can talk over a
reply to stop it.

Where the code lives:

| Area                                           | Code                                                   |
| ---------------------------------------------- | ------------------------------------------------------ |
| Audio service: listening sessions, playback    | `crates/audio/src/service.rs`                          |
| Wake phrase and the always-open microphone     | `crates/audio/src/wake.rs`                             |
| End of speech                                  | `crates/audio/src/vad.rs`                              |
| Speech to text                                 | `crates/audio/src/stt.rs`                              |
| Kokoro voice, first chunk                      | `crates/audio/src/tts.rs`                              |
| Pronunciation                                  | `crates/audio/src/g2p.rs`                              |
| Sentence splitting                             | `crates/audio/src/sentences.rs`                        |
| Cartesia online voice                          | `crates/audio/src/cartesia.rs`                         |
| Echo cancellation (WebRTC)                     | `crates/audio/src/echo.rs`                             |
| Talking over Markon (Silero)                   | `crates/audio/src/barge.rs`                            |
| Apple voice processing microphone              | `crates/platform/src/voice_mic.rs`                     |
| Features for the voice core                    | `crates/audio/src/features.rs`                         |
| Model pinning                                  | `crates/audio/src/manifest.rs`, `models/manifest.json` |
| Push-to-talk and stop words                    | `apps/desktop/src-tauri/src/ptt.rs`                    |
| Wiring: wake, follow-up, barge-in, transcripts | `apps/desktop/src-tauri/src/core.rs`                   |
| Local voice commands                           | `apps/desktop/src-tauri/src/voice_commands.rs`         |
| Command phrases                                | `apps/desktop/src-tauri/src/voice_commands.json`       |

## Listening

There are three ways into a listening session: the push-to-talk key, the wake phrase, and the
follow-up window after a reply.

### Push-to-talk

The default key is Alt+Space. It is a global shortcut and can be changed in Settings. If the new
shortcut cannot be registered, the old one stays (`register_hotkey` in `lib.rs`).

One key does two things (`ptt.rs`):

- **Hold.** A press of 350 ms or longer is a hold. Markon records until you release the key. Pauses
  while you hold do not end the turn.
- **Tap.** A press shorter than 350 ms latches hands-free listening. The turn ends at the first pause
  after speech, or when you tap again. If no speech starts within 8 seconds, the session is dropped.

Pressing the key while Markon speaks or thinks cancels the reply and starts listening. If a key
session ends with no words, Markon shows "I didn't hear any words" with a hint about hold and tap.

### Wake phrase

Once the speech models are loaded, a background thread keeps the microphone open and listens for the
wake phrase (`wake.rs`). There is no separate wake-word model:

1. An energy detector cuts short bursts of sound. A burst ends after 400 ms of silence. 300 ms of
   audio before the burst is kept, so the start of "hey" is not lost.
2. Bursts of 200 ms to 8 seconds of sound go to whisper. Shorter ones are clicks, longer ones are
   conversation.
3. The transcript is matched against the wake phrase. The audio is not stored or sent anywhere.

`match_wake` accepts:

- a greeting (`hey`, `hi`, `hay`, `a`, `okay`, `ok`, `hello`) followed by a name,
- a name with no greeting ("Markon, set a timer"),
- the names `markon`, `markan` and whisper's usual spellings of them (`marcon`, `marcan`, `markin`,
  `marken`, `markham`, `markhan`, `markun`),
- `mark`, `mak`, `mack` or `mac` only after a greeting ("Hey Mark", "Hey Mac"). "Mark on" also counts,
  as whisper sometimes splits the name,
- a bare `Mark` only when it is the whole utterance or is followed by punctuation ("Mark, what's the
  time?"), so "Mark said he's coming" does not wake Markon.

The name must be the first or second word. Anything after it is kept as the request.

Unbroken sound such as music or a film has no pauses to cut at. After 8 seconds of it, whisper gets
3-second windows that overlap by 1 second, so the phrase is never split across two windows. In this
mode the greeting and name may appear anywhere in the window (`match_wake_within`), and only the wake
is used. The request is heard afresh.

What happens after a wake (`AudioEvent::Wake` in `core.rs`):

- A stop phrase ("Hey Markon, stop") stops Markon.
- Two or more words, or a local voice command, said in the same breath are sent at once.
- With nothing more said and Markon idle, it answers "Yes?" and opens the microphone when that word
  has played. Phrases of up to 12 characters are cached as audio, so "Yes?" is synthesised once.
- While a turn is running and Markon is not speaking (thinking, or between sentences), real words
  reach it without the wake phrase (`set_attentive`). Whisper's guesses for noise ("you", "thank you",
  "um", "okay" and similar) do not count. A single stray word while Markon works is ignored.

Markon's own voice never wakes it. The wake detector is reset while Markon speaks, and for 400 ms
after it stops.

### Follow-up listening

When a reply finishes playing, Markon waits 300 ms for the room's echo to die away, then listens
hands-free for up to 6 seconds without the key or the wake phrase. If something in that window turns
out to hold no words (a cough, the tail of Markon's own voice), it keeps listening for the time left,
as long as at least a second remains. A key press ends the window and takes over.

There is no follow-up window after a reply that started media playing ("Playing ..."). The wake phrase
opens the microphone then.

### End of speech

A listening session ends on a pause, measured by an energy detector (`vad.rs`):

- A 10 ms chunk counts as speech when its RMS is above 0.01 and above three times the learned noise
  level. The noise level follows the quiet chunks.
- The turn ends after 1.2 seconds of silence.
- It waits 2 seconds when the partial transcript so far does not end a clause. `ends_clause` needs a
  final `.`, `!` or `?`, not `...`, and a last word that is not a hesitation such as "um", "so",
  "and", "the" or "because".

Partial transcripts come from re-running whisper on the growing clip about every 900 ms, when at least
250 ms of new audio has arrived. They are shown live and feed the clause check.

## Speech to text

Markon uses whisper.cpp with the English `base.en` model (`stt.rs`):

- Audio is downmixed to mono and resampled from the microphone's rate to 16 kHz.
- Clips under a second are padded with silence, as whisper needs at least one second.
- 4 threads, greedy decoding, English, one segment, no context carried between calls.
- An initial prompt biases whisper toward the names ("Markon, Markan.") and the first phrase of each
  local voice command, so "activate Eclipse" is not heard as "activate the clips".
- Non-speech markers such as `[BLANK_AUDIO]` or `(laughs)` are removed.

What is sent onward is the transcript text only, never audio. `core.rs` routes it in this order:

1. If a permission question is on screen, a spoken answer settles it. Anything else denies it, except
   a local voice command, which leaves the question open.
2. A local voice command is handled in the app with no model call.
3. Everything else goes to the agent as a voice turn, and from there to the chosen model route.

## Speaking

### Voices

Two Kokoro-82M voices ship with Markon (`tts.rs`). The assistant's name follows the voice:

| Name   | Kokoro voice file        | Cartesia voice |
| ------ | ------------------------ | -------------- |
| Markon | `voices/bm_daniel.bin`   | Archie         |
| Markan | `voices/bf_isabella.bin` | Gemma          |

Kokoro runs through ONNX Runtime on the CPU with 4 threads, using the `q8f16` model at 24 kHz. Each
call takes at most 500 phoneme tokens. Longer text is split at punctuation, then at spaces. The model
synthesises "Ready." once at load, so the first reply does not pay for graph set-up.

Switching voices re-checks the voice file against the manifest before use.

### Pronunciation

Text becomes Kokoro phonemes in `g2p.rs`, without espeak-ng:

- **Dictionaries.** misaki's British dictionaries: `gb_gold.json` over `gb_silver.json`. Markon and
  Markan have their own entries.
- **Numbers and symbols.** Whole numbers become British words ("one hundred and five"). Decimals are
  read digit by digit after "point". `%`, `&`, `+` and `@` become words.
- **Suffixes.** Plural and third-person `-s`, past `-ed` and `-ing` are built from the stem's
  pronunciation.
- **Acronyms.** Words of two to five capitals, and words with no vowels ("BBC", "mph"), are read
  letter by letter.
- **Unknown words** are sounded out from the fewest dictionary pieces, with letter sounds for what is
  left ("Addis" as "add" + "is").

Phonemes are mapped to Kokoro's token ids. Unknown symbols are skipped.

### Sentences and the first chunk

The model's reply streams in as text. `SentenceQueue` (`sentences.rs`) cuts it at `.`, `!` or `?`
followed by a space, and each sentence is queued for speech as soon as it is complete. One reply
speaks at most 40 sentences. The unfinished tail is spoken when the reply ends.

The first sentence of a reply gets special handling (`split_first_chunk`). If it has more than five
words and a comma within the first six, the words up to that comma are spoken on their own ("Good
morning," then "Dev. You have three meetings."). The first audio starts sooner. A sentence with no
early comma is not split, to avoid a pause mid-phrase.

While a turn works, Markon may say short progress lines. They stay quiet while you talk, while media
starts, and while Markon is already speaking.

### Cartesia online voice

Settings can switch speech to Cartesia's hosted Sonic model (`cartesia.rs`):

- The API key is read from the macOS keychain. Without a key Markon stays on Kokoro and says so.
- Reply text is sent to Cartesia. Audio comes back as raw 32-bit float PCM at 24 kHz.
- Each request times out after 15 seconds. A failed request speaks nothing and reports an error.
  There is no fallback to Kokoro for that sentence.
- Kokoro stays loaded, so switching back takes effect from the next sentence.

### Playback

Playback runs on its own thread (`speaker_thread` in `service.rs`):

- The default output device opens with the first sentence and closes once the queue has drained.
  The next reply follows a device change, such as plugging in headphones.
- Samples are resampled to the device rate and written to every channel.
- Each sentence's end position in the stream is recorded, so a cut knows which sentences finished.
- `cancel_speech` clears the queue at once. Sentences still being synthesised are dropped when they
  finish.

## Talking over Markon

You can interrupt a reply by speaking. Three parts make this work.

### Echo cancellation

The microphone also hears Markon's own voice, which must not count as you.

- **Apple voice processing** (`voice_mic.rs`) is tried first. It opens the microphone through macOS's
  Voice Processing I/O unit, the echo canceller FaceTime uses. It also removes the distortion of the
  built-in speakers at high volume. On macOS 14 and later, Markon sets the unit to duck other audio
  as little as possible, because the default turns everything else down, Markon's voice included.
- **WebRTC AEC3** (`echo.rs`) is the fallback when Apple's unit cannot open. It runs on 10 ms frames
  with a high-pass filter, fed with what the speakers just played.

The WebRTC canceller is fed the microphone between replies too. Without it, the canceller loses its
alignment and the start of the next reply comes through almost untouched (`echo.rs` tests).

While Apple's unit is open, a second plain microphone stream would be turned far down by macOS. Listening
sessions therefore record from the wake listener's echo-cancelled stream (the `Tap` in `wake.rs`).

If neither canceller is available, talking over Markon is off.

### Voice detection

`barge.rs` decides whether what is left after echo cancellation is a person. It uses Silero VAD
through ONNX Runtime on 32 ms windows of 16 kHz audio and a four-state machine: quiet, starting,
speaking, stopping.

- A window counts as speech when Silero's confidence is at least 0.7 and the smoothed level is above
  the gate.
- 200 ms of speech confirms it. 200 ms without speech ends it.
- The model's recurrent state is reset every 2 seconds while quiet, after 5 low-confidence windows.

### Noise floor, gate and grace window

- **Noise floor.** Learned only while Markon is silent: the 90th percentile of the last 100 quiet
  windows, never under 0.002 RMS. A window joins only when it is under the trigger, so talking never
  raises the floor.
- **Trigger.** Three times the floor, kept between 0.006 and 0.015 RMS.
- **Grace window.** For 500 ms after Markon's voice starts, following more than about a second of
  silence, nothing counts as speech. Pauses between sentences are shorter, so they cannot chain grace
  windows and hide an interruption. During the grace window Markon measures how much of its own
  output comes back as echo.
- **Gate.** The trigger, raised to 1.5 times the expected echo of what is playing now. The played
  level fades over about 150 ms, like the echo's tail. The bar is high on a loud word and drops back
  between words, where people cut in.

When the detector reaches "starting", Markon's voice drops to a quarter of its volume. At "speaking",
playback stops in the same tick and does not resume. While Markon speaks, the listener checks the
microphone every 10 ms instead of every 30 ms.

### What happens to your words and the reply

- The last 1.5 seconds of speech before the stop are handed to the new listening session, so your
  first words are part of the request. The listener keeps collecting for up to 3 seconds until the
  session opens.
- If the transcript holds no real words, or matches a continuous stretch of what Markon was just
  saying (`is_own_words`), it was Markon's echo. Nothing is sent and the reply does not resume.
  "Stop", "wait", "no", "cancel", "hold on", "enough", "quiet" and "never mind" said alone always
  count as you.
- The conversation history keeps only the sentences that finished playing. The cut sentence and the
  unplayed rest are dropped. Tool calls keep their results.
- Your next message, if sent within 2 minutes, carries a note that you interrupted the previous reply.

If the Silero model fails its checksum or does not load, talking over Markon is off. Markon still
listens and speaks.

## Media goes silent while Markon listens

While Markon listens, thinks or speaks (any state but idle), media cards on the canvas go silent
(`MediaBody.tsx`):

- Audio and video cards are set to volume 0, then restored.
- YouTube embeds get volume 0 through the player API, then 100.
- Spotify's volume is not Markon's to change, so Spotify is paused and resumed afterwards.

This keeps the microphone hearing only you.

## Voice commands

Some phrases are handled in the app with no model call.

### Stop

`is_stop` (`ptt.rs`) recognises "stop", "stop it", "stop that", "cancel", "cancel that", "never
mind", "enough", "quiet", "be quiet", "shut up", "hold on" and "wait". Every sentence of the
utterance must be a stop phrase: "Stop. Stop it!" stops, "Stop. Play the next one." is a request.
Stop phrases work after the wake phrase ("Hey Markon, stop"). Stopping cancels speech and the running
reply.

### Window and canvas commands

`voice_commands.json` lists the phrases:

| Command    | Example phrases                   | Effect                              |
| ---------- | --------------------------------- | ----------------------------------- |
| `hide`     | "hide", "go away"                 | hides the window                    |
| `quit`     | "quit", "quit Markon", "exit"     | asks for confirmation, then quits   |
| `settings` | "settings", "open settings"       | opens Settings                      |
| `tidy`     | "tidy up"                         | groups cards by turn, newest first  |
| `clear`    | "clear the screen", "clear view"  | closes all cards                    |
| `standby`  | "standby", "stand by"             | stops and returns to idle           |
| `bigger`   | "make it bigger"                  | enlarges the focused or newest card |
| `theme`    | "activate Eclipse", "normal mode" | switches the theme                  |

Two more are recognised by prefix: "bring back ..." reopens a past conversation on that topic, and
"focus on ..." brings the card whose title matches to the front.

### How phrases match

`parse` in `voice_commands.rs`:

1. Lowercases the text and drops punctuation.
2. Strips leading words such as "hey Markon", "okay", "please", "now" or "stop", and trailing ones such
   as "please" or "thanks".
3. Looks for an exact phrase. Then it tries a close match, then strips one leading verb ("go to",
   "switch to", "activate", "turn on" and others) and tries again.
4. A close match allows one wrong letter per five, ignoring spaces and hyphens, and at most one word
   more than the phrase. When two commands are equally close, nothing matches.
5. A repeated command counts when every sentence asks for the same thing ("Stand by. Go to stand
   by.").

**Quit is strict.** It never matches by closeness, so "Quiet", "Quits" or "Edit" do not quit. The app
then asks for confirmation before it quits.

A sentence that only contains a command word is not a command: "How do I quit smoking?" or "Make the
font in this document bigger" go to the model.

## Features for the voice core

The animated voice core moves with the sound (`features.rs`). Thirty times a second Markon computes:

- **Loudness**, normalised to 0..1.
- **Pitch** in Hz, by autocorrelation between 70 and 350 Hz, 0 when unvoiced or quiet.
- **48 band levels** from 90 Hz to 7 kHz, log spaced, 0..1.

The analysis window is at least 64 ms, rounded up to a power of two, with a Blackman window. The band
scaling matches a Web Audio analyser, so the core looks the same in the app and on the web. A sound
appears in a published frame within one hop, about 33 ms.

Frames are tagged with who is talking: `user` during a listening session, `markon` while a reply
plays. They reach the UI as `markon://features` events and drive `packages/voice-core`.

## Model files and pinning

`models/manifest.json` lists every model file with its download URL, size, SHA-256 and licence. The
manifest is compiled into the app, so it cannot be swapped next to the models.

| ID                | File                     | Used for            |
| ----------------- | ------------------------ | ------------------- |
| `whisper-base.en` | `whisper-base.en.bin`    | speech to text      |
| `kokoro-v1`       | `kokoro-v1-q8f16.onnx`   | the voice           |
| `voice-markon`    | `voices/bm_daniel.bin`   | the Markon voice    |
| `voice-markan`    | `voices/bf_isabella.bin` | the Markan voice    |
| `g2p-gb-gold`     | `g2p/gb_gold.json`       | pronunciation       |
| `g2p-gb-silver`   | `g2p/gb_silver.json`     | pronunciation       |
| `silero-vad`      | `vad/silero_vad.onnx`    | talking over Markon |

Entries marked `on_demand` (the built-in language model and the search model) are downloaded only when
you ask for them.

At start (`prepare_models` in `core.rs`, `manifest.rs`):

1. Missing or wrong files are downloaded to `~/.markon/models`. Each download goes to a `.part` file
   and is moved into place only if its size and SHA-256 match.
2. Every file is checked again before it is loaded.
3. A mismatch is reported (`ModelMismatch`) and the file is never loaded.

There is no fallback voice. If whisper, Kokoro, the chosen voice or a dictionary fails its check,
voice is off and Markon says why in the status line. Replies still show as text. The Silero model is
the one exception: a bad file only turns off talking over Markon.

## Measured latency

Measured on a MacBook Pro with an Apple M1 Pro (8 performance and 2 efficiency cores), 32 GB, CPU
only, release build. Slower Macs and 8 GB Macs are not measured.

Reproduce with the pinned models and a 16 kHz mono WAV:

```bash
cargo run --release -p markon-audio --example latency -- ~/.markon/models <clip.wav>
```

| What                                                               | Measured      |
| ------------------------------------------------------------------ | ------------- |
| SHA-256 check of all pinned files                                  | 0.77 s        |
| Loading whisper, Kokoro and a voice                                | 2.3 s         |
| Final transcript of a 5.64 s utterance (48 kHz to 16 kHz, whisper) | 350 to 371 ms |
| First audio after the first complete sentence, output device cold  | 387 ms        |
| First audio, output device already open                            | 179 ms        |
| `cancel_speech` returning                                          | 1 to 2 µs     |

`cancel_speech` clears the queue. The device goes silent at its next callback, one CoreAudio buffer
of about 10 ms. That last step is not measured at the speaker.

The Kokoro warm-up at load brings the cold first reply from 482 ms to 387 ms.

With the built-in language model (Qwen2.5 1.5B Instruct, Q4_K_M, llama.cpp with Metal) on the same
Mac, the first spoken text of a request that needs no tool arrives in 426 ms median, 433 ms at most,
over twelve one-sentence requests. Reproduce with:

```bash
MARKON_LLM_MODEL=/path/to/qwen2.5-1.5b-instruct-q4_k_m.gguf \
  cargo run --release -p markon-llm --example measure
```
