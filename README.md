# Video Scribe

Audio description for video, on Windows, for screen reader users.

> **Pre-release.** It works end to end and is used every day by its author, but it has no installer or
> release yet, and things still change from one week to the next. New here? Read
> [ORIENTATION.md](ORIENTATION.md).

Give it a YouTube link or a video file. It works out where the gaps between speech are, asks an AI what is
happening on screen during those gaps, speaks the answers in a voice you choose, and mixes them into the
original soundtrack with the film turned down underneath. What comes out is an audio file you can listen
to, or a described video file you can share. Your original video is never changed.

It is built for two things in particular:

- **YouTube** and other video sites, where captions often exist and give the speech timings for free.
- **Family videos with no captions**, where a voice-activity model finds the speech on your own computer,
  and where the app learns who people are, so "a woman lifts a child onto a swing" becomes "Mum lifts
  Sophie onto the swing".

One small Win32 program, keyboard first, usable without sight — the same shape as
[StableSound](https://github.com/ldefontenay/stablesound) and [NoteFetch](https://github.com/ldefontenay/notefetch).

## What it needs

- Windows 10 or 11, 64-bit.
- For describing, one of:
  - an [OpenRouter](https://openrouter.ai) key, paid per use (about US$0.08 for an hour of video with the
    default model), stored in Windows Credential Manager as `Video Scribe:OpenRouter`; or
  - [Claude Code](https://claude.com/code) installed and signed in with a Claude subscription.
- Voices come from Microsoft Edge's online voices, with Windows' own voices as a fallback. Web videos are
  downloaded by [yt-dlp](https://github.com/yt-dlp/yt-dlp), which Video Scribe fetches the first time.

F1 in the program opens the help.

## Building

Rust stable with the MSVC toolchain, and the Windows SDK (for rc.exe).

```
cargo build --release
```

The program is `target\release\videoscribe.exe`. `PLAN.md` holds the design, the research behind it,
the decisions and the milestones.

## Credit

The design owes a great deal to two projects that did this first:

- [SonarPad](https://github.com/Ambro86/Sonarpad) by Ambrogio Riili, whose audio-description module was
  studied in detail. `PLAN.md` §2 is a write-up of how it works. No code of it is used.
- [Omni Describer](https://github.com/audioses/omni-describer) (MIT), which SonarPad's analysis half
  started from, and whose prompt shape and description lengths Video Scribe's prompt builds on.

Also [Silero VAD](https://github.com/snakers4/silero-vad) (MIT) for voice activity detection,
[tract](https://github.com/sonos/tract) (MIT OR Apache-2.0) for running it, OpenCV's YuNet and SFace
face models, downloaded when first needed, and [yt-dlp](https://github.com/yt-dlp/yt-dlp) for the downloads.

## Licence

MIT. See [LICENSE](LICENSE).
