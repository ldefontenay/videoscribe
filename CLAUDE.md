# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# Video Scribe

A small Windows app that produces an **audio-described soundtrack** for a YouTube video or a local video
file: the original audio, ducked, with AI-written descriptions spoken into the gaps between speech.
Two use cases, and the second one drives the design: YouTube, and family videos that have no captions.

It is a deliberate narrowing of one feature out of [SonarPad](https://github.com/Ambro86/Sonarpad),
whose audio-description module was researched in detail before this plan was written. Same spirit as
StableSound and Photo Scribe: one small Win32 app that does one thing without sight.

**`PLAN.md` is the source of truth** for decisions, findings and milestones. Read it at the start of a
session. Its §3 is the actual product; everything else is plumbing. New people: `ORIENTATION.md` is the
plain-language tour.

Where to look in `PLAN.md`: §2 is how SonarPad does it (research), §4 maps that onto Windows built-ins,
§5 holds the simplifications and the rules behind them, §6 is the proposed architecture, §7 the settled
decisions, §8 the milestones, §9 the open risks. Claims carry a status marker: `[proven]` (read in
SonarPad's shipping source), `[claimed]` (in SonarPad's docs, not checked in code), `[untested]` (our
proposal, needs a spike). Keep using them, and never upgrade a marker without having read the source.

## Where things stand

Pre-release, public since 2 November 2026 so that a collaborator can join. Milestones 0 to 10 are built
and mostly tested by the project's owner; milestone 11 (languages and voices) is built and under test;
categories (folders inside the output folder, as in NoteFetch) come next. See `PLAN.md` §8.

This repository started with a fresh history. The first 96 commits, the development diary `log.md` and
the manual test files stay in the owner's private repository, because they hold details of family videos.
"Log entry N" in `PLAN.md` and in code comments points into that diary: the decision it led to is always
in `PLAN.md` §7, so read that rather than asking for the entry.

The spikes and the walking skeleton live in `spike/`, which builds on its own, outside the app's workspace.
The Silero model the app embeds is committed in `models/`.

Sibling projects by the same author are the reference for house style and for code to reuse:
[StableSound](https://github.com/ldefontenay/stablesound), [NoteFetch](https://github.com/ldefontenay/notefetch)
and Photo Scribe (not public yet; much of `net.rs`, `claude.rs`, `people.rs` and `faces.rs` came from it).
When they sit next door (`..\stablesound`, `..\PhotoScribe`), read their code freely; never modify them
from a Video Scribe session.

## Who you are working with

Two or more people direct this project, each with their own Claude Code sessions. If it is not clear
who you are working with, ask once at the start of the session.

- **Laurent, the owner**, is blind and uses JAWS. He has the final say on anything a user hears or
  touches. For him: findings, plans and questions go in `log.md`, newest entry at the top, each question
  followed by an `ANSWER:` line; terminal replies stay short and point to the file. Manual tests go in
  `test-milestone-N.txt`: plain text, one step at a time, each followed by `ANSWER:`, plus an
  "ANYTHING ODD:" line in each section. Never put manual steps in chat.
- **Collaborators** may be sighted or not, and are not necessarily programmers ("vibe coders": they
  direct Claude in plain language). Explain in plain words, say what you changed and why, and say
  plainly what is untested. A collaborator may keep their own `log.md` the same way, if they like it.
- `log.md` and `test-milestone-*.txt` are **personal and git-ignored**: each person's diary stays on
  their own computer. What is shared goes through GitHub: an **issue** to propose or discuss, a **pull
  request** for a change, with a "How to test" section in plain steps that Laurent's sessions can turn
  into his test file. When a decision is settled, it goes into `PLAN.md` in the same pull request.

## Accessibility rules for any UI work

- Load the `accessible-windows-apps` skill before any UI work, and run its probe before asking anyone to
  test. If the skill is not installed, follow `PLAN.md` §2.11 and §5.10, and say that the probe was not run.
- Do not grey out a control to express a constraint. See PLAN.md §5.10: every control stays enabled and
  announces what it just did to the others.

## Non-negotiable constraints

1. Accessibility is the product. Everything must be usable without sight — including **reviewing the
   result**, which a blind user cannot do by scrubbing a waveform (PLAN.md milestone 8).
2. Real Win32 controls from an rc.exe dialog template. No drawn-UI toolkit (egui, iced, Slint, Tauri, Electron).
3. Keyboard first. Every state change the user causes is audible or announced.
4. Small. Prefer Windows built-ins (Media Foundation, WinHTTP including its WebSockets, SAPI 5, CNG, WIC,
   the property system, Credential Manager) to crates. **A shipped binary needs a capability argument, not a
   convenience one** (PLAN.md §5.8): FFmpeg does not ship, because Media Foundation decodes and encodes.
5. **Never modify the user's original video or audio.** Output goes to new files, always.
6. Nothing starts a second process except `yt-dlp.exe` and, for one service, `claude`. There is no Python
   worker and no downloaded analysis exe — avoiding those is most of why this app exists (PLAN.md §5.1).
   A second command-line describer of the same kind (such as OpenAI's `codex`, see `ORIENTATION.md`)
   would be an agreed exception: propose it in an issue first.
7. **Family video is personal data.** The VAD runs locally so the audio never leaves the machine; only the
   frames or clips a gap needs are uploaded. Show what will be sent before the job starts. Never send
   folder or file names unless the user asks (Photo Scribe's `send_names`).
8. The people catalogue (`%APPDATA%\Video Scribe\people.txt`, in Photo Scribe's exact format, optionally
   Photo Scribe's own file instead — PLAN.md §5.7) is the most sensitive file this app writes. Plain text
   so it can be read and corrected by hand, atomic writes, never sent anywhere wholesale. Video Scribe
   must not depend on Photo Scribe being installed. During development, never open a person's real
   `people.txt`, `faces.bin` or `%LOCALAPPDATA%\Video Scribe\people`: point `VIDEOSCRIBE_PEOPLE_DIR`
   and `VIDEOSCRIBE_PHOTOSCRIBE_DIR` at scratch folders instead.

## Test material and keys

- Test videos go in `corpus/`, which git ignores. Family copies there are never uploaded without asking
  their owner first, and never committed. YouTube test videos are public ones (Blender's open films are
  the usual choice: Sprite Fright, Sintel, Charge).
- **No real names, file names or places from anyone's family videos in code, tests, docs or commit
  messages.** This repository is public. Use made-up names (Sam, Alex, Ruth, Ben) in tests.
- API keys are read, never written, from Windows Credential Manager: `Video Scribe:OpenRouter` (or
  Photo Scribe's `Photo Scribe:OpenRouter`). Describing costs real money on OpenRouter: say what a test
  run will cost before running it, and keep runs short.
- Launch the app with `VIDEOSCRIBE_SILENT=1` when testing it yourself: playing then writes silence.
  Unattended runs that muted playback instead once left Windows' volume for the app at nought, which
  Windows remembers, and the owner then heard nothing.

## Commands

The spikes are a separate crate: run them with `cargo run --release --manifest-path spike\Cargo.toml -- <args>`.
The app:

```
cargo build / cargo run / cargo build --release
cargo test <name>          # a single test
cargo fmt
cargo clippy -- -D warnings
cargo test
```

Run `cargo fmt`, `cargo clippy -- -D warnings` and `cargo test` after every change.

## Session hygiene

- Use a branch for each piece of work (`m12-categories`, `codex-provider`) and a pull request into
  `main`. Docs-only changes by the owner can go straight to `main`.
- Commit in small steps. When a decision is settled, update `PLAN.md` in the same commit.
- Never commit secrets (API keys live in Credential Manager), test media, build output, `log.md` or test
  files.
- Never point the app at someone's real video folders when testing: use copies in a scratch folder.
- Ask before pushing. Say plainly what is untested.

## Provenance and licences

Worth keeping straight, because three upstreams are involved:

- **Omni Describer** (`audioses/omni-describer`, MIT) — SonarPad's analysis started from it. Reusable with
  attribution: its unified JSON prompt shape, verbosity word counts and 2-words-a-second rule. It has no
  speech detection or scheduling.
- **SonarPad** (`Ambro86/Sonarpad`) — **all rights reserved, source-available non-commercial.** Read it for
  ideas and facts; never copy its code, prompt text or tests. The gap-aware prompt, scheduling, ducking
  and Edge TTS code PLAN.md describes are SonarPad's, not Omni's (PLAN.md §2.1).
- **Silero VAD** (MIT) — the voice-activity model. Record the pinned revision, as SonarPad does for pyannote.
- **tract** (MIT OR Apache-2.0) — the preferred pure-Rust ONNX runtime, which may let us ship one file.
- **yt-dlp** (Unlicense) — downloaded at runtime, not shipped.
- pyannote's CC BY 4.0 weights are **not** used here. We use Silero instead; see PLAN.md §4.1.
