# Welcome to Video Scribe

A guide for a new collaborator: what the app is, how to get it running, and how we work. It is written
for someone who builds software by talking to an AI (Claude Code, Codex and the like) rather than by
writing every line, because that is how this app was made too. You don't need to know Rust to help.

Video Scribe is **pre-release**. It works end to end and Laurent, its author, uses it, but there is no
installer yet and things still change from week to week.

---

## Contents

1. What it does, in one minute
2. Who it's for, and the rules that follow
3. How a video becomes a described video
4. Where things stand
5. Getting it running on your computer
6. A first tour of the app
7. OpenRouter: what it is, and how to get started
8. Claude Code as the describer
9. Could Codex describe instead? Yes, probably
10. How we work together
11. Experimenting with prompts and models
12. Ideas, if any appeal
13. A map of the code
14. Words you will meet
15. Handy things to say to your AI

---

## 1. What it does, in one minute

Blind and partially sighted people miss whatever a video shows without saying it: who walked in, what
the sign says, why everyone laughed. **Audio description** fills that in: a voice describes what is on
screen in the pauses between the dialogue. TV and cinema do it with professional writers. Almost nobody
does it for YouTube, and nobody does it for your own family videos.

Video Scribe does it with AI. You give it a YouTube link or a video file. It:

1. finds where people are speaking, and so where the **pauses** are;
2. sends a few still pictures from each pause to an AI model, which writes a short description;
3. speaks each description in a natural voice, fitted into its pause;
4. mixes the voice into the film's own sound, with the film turned down a little underneath;
5. saves the result as a new audio file (MP3) or video file (MP4). The original is never touched.

Then you can **listen** to it, **review** every description and change any of them, and **name the
people** in the video ("the bearded man" is Sam), so the names are spoken next time. In family videos it
can even recognise faces it has been told about before, all on your own computer.

## 2. Who it's for, and the rules that follow

Laurent is blind and uses the JAWS screen reader. Video Scribe is built for people like him first. That
shapes everything, and these are the rules that can't bend:

- **Everything works from the keyboard**, and every change is announced out loud. If a sighted person
  can do it with the mouse, a blind person must be able to do it with keys.
- **Real Windows controls.** The windows are plain, old-fashioned Windows dialogs (the kind made from a
  `.rc` file), because screen readers understand those perfectly. No web views, no fancy drawn
  interfaces. They look plain on purpose.
- **Nothing is ever greyed out.** A greyed-out button is invisible to a screen reader user, who just hears
  nothing and doesn't know why. Instead, every control stays usable and says what it did.
- **Every label makes sense on its own.** A screen reader reads one control at a time, so "Enable it" is
  useless; "Know people by their faces in family videos" is right.
- **Family video is private.** The speech-finding runs on the computer, so the sound never leaves it. Only
  the pictures a pause needs are sent, the app says exactly what will be sent and asks first, and file
  names are not sent unless the user asks.
- **Small and self-contained.** One program file, no installer, no Python, no FFmpeg. Windows' own
  built-in pieces do the work (Media Foundation for video and sound, Edge's online voices for speech).

Laurent has the final say on anything a user hears or touches. If you aren't sure whether a change fits,
ask (see section 10).

## 3. How a video becomes a described video

The steps, with the file in `src/` that does each one, so you can point your AI at the right place:

| Step | What happens | Where |
|---|---|---|
| Get the video | A web address is downloaded with yt-dlp, a file is used as it is | `ytdlp.rs`, `addvideo.rs` |
| Find the speech | Captions if the site has them; otherwise Silero, a small speech-detection model, runs on the computer | `captions.rs`, `vad.rs`, `speech.rs` |
| Find the pauses | Gaps between speech long enough for a few words become "places" | `speech.rs`, `report.rs` |
| Pick pictures | Two or three stills from each place; for family videos, faces are tagged ("Person 3") locally | `frames.rs`, `faces.rs` |
| Describe | The pictures and a long, carefully tuned prompt go to the AI, several requests at once | `describe.rs`, `openrouter.rs`, `claude.rs`, `prompts/` |
| Put in names | Labels like "the bearded man" become the names you gave, on the computer | `people.rs`, `known.rs` |
| Speak | Microsoft Edge's online voices, or Windows' own voices as a fallback | `edge.rs`, `sapi.rs`, `speak.rs` |
| Mix | The voice goes into the film's sound, film turned down under it | `mix.rs`, `mux.rs` |
| Review and listen | Lists every description; you can play, change, or rewrite each one | `review.rs`, `player.rs` |

Everything is saved as it goes (a "checkpoint"), so describing the same video again with only the voice
changed costs nothing and sends nothing.

## 4. Where things stand

Done (milestones 0 to 10 in `PLAN.md`): the whole pipeline above; YouTube and other sites; family videos;
OpenRouter and Claude Code as describers; review and listening; people and faces; presets; described
video output.

Just built and being tested (milestone 11): the Language and voice window, with over 300 voices in 100+
languages, and descriptions written in French, Afrikaans and others.

Next on Laurent's list: **categories** (sorting the video list into folders).

## 5. Getting it running on your computer

You need **Windows 10 or 11**. Video Scribe is Windows-only, because it is built on Windows' own parts.

### Install the tools (once)

1. **Git**, from git-scm.com, if you don't have it.
2. **Rust**, from rustup.rs. Run the installer and accept the defaults.
3. **Visual Studio Build Tools**, from visualstudio.microsoft.com/downloads (scroll to "Tools for Visual
   Studio"). In the installer, tick **"Desktop development with C++"**, which includes the Windows SDK.
   It's large (several GB). Rust needs it to make Windows programs.

Or skip the list: open Claude Code (or Codex) in an empty folder and say *"Install what I need to build
a Rust Windows app with the MSVC toolchain and the Windows SDK"*, and let it walk you through.

### Get the code and build it

You work on your own copy of the project on GitHub, called a **fork**: you can change anything in it
freely, and offer changes back when you like (section 10).

1. Make a GitHub account if you don't have one.
2. Open https://github.com/ldefontenay/videoscribe and press **Fork** (top right), then **Create fork**.
   You now have `github.com/<your-name>/videoscribe`.
3. On your computer:

```
git clone https://github.com/<your-name>/videoscribe.git
cd videoscribe
git remote add upstream https://github.com/ldefontenay/videoscribe.git
cargo build --release
```

(`upstream` is Laurent's copy, so you can pull in his latest work: ask your AI to "update my fork from
upstream".)

The first build takes a few minutes (it compiles the speech-detection engine). The program is then
`target\release\videoscribe.exe`. Run it. F1 opens the full help.

### If you'd like to hear it the way Laurent does

Install **NVDA**, a free screen reader (nvaccess.org). Turn it on, close your eyes, and try the app with
the keyboard only. Ten minutes of this teaches more about the rules in section 2 than any document.
(Insert+Q quits NVDA.)

## 6. A first tour of the app

The main window is the **list of your videos**. Then:

- **Ctrl+N, Add a video.** Paste a YouTube address (try a Blender open film such as *Sprite Fright*),
  press Enter. It finds the speech and pauses, then tells you exactly what it will send, to which
  model, and what it should cost, and asks. Say Describe.
- **Enter on a video** plays the described sound, in the Listen window (arrows move 2 seconds).
- **Ctrl+R, Review the descriptions**: every place, what's said there, and you can change it.
- **Ctrl+P, People in this video**: name the people the descriptions mention.
- **Alt+O, the Options menu**: the describing service and model, style (short, standard, detailed),
  language and voice (Ctrl+Shift+V), speed, output (audio or video), presets.

Results go in `Videos\Video Scribe`, one folder per video. Settings are in
`%APPDATA%\Video Scribe\settings.txt`.

## 7. OpenRouter: what it is, and how to get started

**What it is.** OpenRouter (openrouter.ai) is a single shop front for hundreds of AI models from many
companies: Google, OpenAI, Anthropic, Z.ai, Meta and others. You make one account, put some credit in,
and get one **API key**: a long secret password that lets a program use those models. You pay per use,
by the amount of text and pictures sent, usually fractions of a cent per request. There's no
subscription.

**Why Video Scribe uses it.** One key reaches every model, so we could test several and switch with a
setting. And OpenRouter can be told to use only providers that **don't keep or train on** what they're
sent, which matters for family videos.

**What it costs here.** The default model, **GLM 5.3 Flash**, won the blind listening tests and costs
about **US$0.08 for an hour of video**. A 10-minute YouTube video costs a cent or two. Gemini 3.5
Flash-Lite, which watches short clips with their sound instead of stills, is about US$0.25 an hour.
Video Scribe shows the estimate before anything is sent. US$5 of credit will last a very long time.

**Getting started:**

1. Go to **openrouter.ai** and sign up (a Google account or email is fine).
2. Open **Settings, Credits** and add some credit: US$5 is plenty to start.
3. Open **Settings, Privacy**. Video Scribe already asks only for providers that don't keep or train on
   data, for each request. You can also switch off training and logging there for the whole account.
4. Open **Keys**, then **Create Key**. Give it a name ("Video Scribe"), and optionally a credit limit
   (say US$2) so a bug can never cost much. **Copy the key now**: it starts with `sk-or-v1-` and is
   shown only once.
5. Store it in Windows, where Video Scribe looks for it (never in a file, never in the code):
   - Press the Windows key, type **Credential Manager**, open it.
   - Choose **Windows Credentials**, then **Add a generic credential**.
   - Internet or network address: `Video Scribe:OpenRouter` (exactly, with the colon).
   - User name: anything, such as your name.
   - Password: paste the key.
   - OK.
6. In Video Scribe: **Options, Model provider, OpenRouter**. Add a video, and describe.

**Keep the key secret.** Never paste it into a chat with an AI, a GitHub issue, or a file in the
repository. If it ever leaks, delete it on the Keys page and make a new one: that's all it takes.

## 8. Claude Code as the describer

The other option in **Options, Model provider** is **Claude Code**. If you have a Claude subscription
(Pro or Max) and Claude Code installed and signed in, Video Scribe runs `claude` in the background, hands
it the pictures and the prompt, and reads back the descriptions. It uses your plan's allowance, so there's
no extra bill.

It is careful about it: it **refuses** to run if Claude Code is signed in with an API key rather than a
subscription, so it can never run up a bill by accident. And it starts `claude` with no tools, no plugins,
an empty folder and no saved session, so the describing can't do anything else on your computer. All of
that is in `src/claude.rs`, about 340 lines with its tests.

In Laurent's tests Claude Sonnet was the best at **people** (keeping track of who is who) and was best
on the film; GLM stays the default because it's faster and very cheap.

## 9. Could Codex describe instead? Yes, probably

If you use **OpenAI's Codex** with a ChatGPT subscription, Video Scribe could use it exactly the way it
uses Claude Code: run it in the background on your subscription, so describing costs nothing extra.

**Why it should work.** Codex has a command-line mode for programs to use, `codex exec`, and it has
everything `claude.rs` relies on:

- pictures can be attached (`--image`);
- the model can be chosen (`--model`);
- it can be locked down so it can't change anything (`--sandbox read-only`), run outside a project
  folder (`--skip-git-repo-check`), and save no session (`--ephemeral`);
- it can write just its final answer to a file (`--output-last-message`), or report as JSON (`--json`);
- `codex login status` says whether you're signed in, and how.

**What the work would be.** A new `src/codex.rs` alongside `claude.rs`, plus a third choice under Model
provider and a few lines of help. Something like 200 to 300 lines, most of it a copy of `claude.rs`'s
shape. The same care applies: it must **refuse to run on an API key** (only on a ChatGPT sign-in), so
nobody gets an unexpected bill, and the pictures go to OpenAI under that account's privacy settings, which
the "what will be sent" question must say.

**What's not known yet.** Nobody has tried it. Check:

- how the prompt (about 400 lines) is best given to it;
- whether its answers come back in the strict JSON form Video Scribe expects;
- how fast it is with a dozen pictures at a time;
- what happens when the plan's limit is reached (Video Scribe should stop cleanly and carry on later, as
  it does for Claude).

It also needs a small change to the project's rules (`CLAUDE.md`, rule 6: only `yt-dlp` and `claude` may
be started), so open an issue first, if it appeals.

## 10. How we work together

### The files that matter

- **`PLAN.md`**: the source of truth. Every decision, with the reason and the test that settled it
  (section 7 lists them, numbered). Long, but searchable: if you wonder "why is it like this?", the
  answer is almost always in there.
- **`CLAUDE.md`**: standing instructions that Claude Code reads at the start of every session. It
  tells your AI the rules, so you don't have to. (Codex reads `AGENTS.md`: if you use Codex, ask it to
  read `CLAUDE.md` first, or copy it to an `AGENTS.md`.)
- **`ORIENTATION.md`**: this guide.
- **`src/help.html`**: the user's help page (F1 in the app). If you change what something does, change
  the help too.
- **`log.md` and `test-milestone-N.txt`**: personal notes, **not shared** (git ignores them). Laurent's
  AI writes him a diary of findings and questions in `log.md`, and test scripts in plain text that he
  works through with JAWS. You can keep your own the same way if you like. "Log entry 37" in a comment
  points into his diary, which holds details of family videos and stays private. The decision it led to
  is always in `PLAN.md`.

### The way of working

1. **Talk first.** For anything bigger than a small fix, open a GitHub **issue** saying what you'd like
   to do and why. A quick yes saves a wasted afternoon.
2. **Work on a branch in your fork**, not directly on `main`: `git checkout -b music-lessons`. (Or
   tell your AI "make a branch for this".) Experiments that go nowhere can simply stay on their branch.
3. **Commit small, and often**, with messages that say what changed for the user.
4. **Before you share**, have your AI run the three checks: `cargo fmt`, `cargo clippy -- -D warnings`
   and `cargo test`. All must pass.
5. **Push the branch to your fork and open a pull request** into Laurent's `main` (GitHub offers a
   "Compare & pull request" button after you push). Include a **"How to test"** section in plain steps ("Press
   Ctrl+N, paste this address, press Enter: you should hear..."). Laurent's AI turns that into a test
   script he can follow with JAWS. Say plainly what you did **not** test.
6. Laurent tests and merges, or says what's off.

### Things never to commit (the repository is public)

- **Videos or sound files**, ever, especially family ones. Put test videos in `corpus/`, which git
  ignores.
- **API keys** or passwords of any kind.
- **Real names** of people from anyone's videos, in code, tests or commit messages. Tests use made-up
  names (Sam, Alex, Ruth, Ben).
- Build output (`target/`), which git already ignores.

### On cost

Describing on OpenRouter costs real (small) money. Before letting your AI run real describing tests,
ask it what they'll cost. Short public videos are cheap; a feature film is still only cents with GLM,
but it adds up over many runs.

## 11. Experimenting with prompts and models

This is where most of the quality comes from, and you need no Rust for it.

### The prompt

The instructions sent to the AI with every request are in `prompts/describe.v10.txt`: about 400 lines
of plain English, with comments (lines starting `#`) saying why each rule is there. They grew from
the published guidance for professional audio description and from Laurent's listening tests. Words in
curly brackets, such as `{style}` or `{known_people}`, are filled in by the app.

To try a change:

1. Copy it to `prompts/describe.v11.txt` and edit the copy. Keep the old one: results made with it can
   still be explained.
2. Point the app at it: line 23 of `src/describe.rs` names the file. Rebuild (`cargo build --release`).
3. **Make sure the video is described afresh.** Video Scribe saves every description it has paid for, and
   deliberately reuses them when only the prompt changed. To describe a video again from scratch, delete
   its file in `%LOCALAPPDATA%\Video Scribe\checkpoints` (the one named after the video), or change the
   model or style. Otherwise you will hear the old descriptions and think your change did nothing.
4. Compare by ear in Listen (Ctrl+L), and line by line in Review (Ctrl+R). The text file saved beside
   each result, "name - described.txt", lists every description with its time, which is handy for
   comparing two versions side by side or with your AI.

### The model

The Options menu offers three OpenRouter models, but **any OpenRouter model that accepts pictures** can be
tried. Close Video Scribe, open `%APPDATA%\Video Scribe\settings.txt` in Notepad, and change the line
`openrouter model = z-ai/glm-5.3-flash` to another model's id from openrouter.ai/models; copy the exact id
from the model's page, and check its price and that it takes pictures. The cost estimate
is only known for the three listed models; for others, OpenRouter's Activity page shows what each run
cost. Laurent's findings so far (`PLAN.md` §7, decisions 19, 23 and 28): GLM was best overall and fastest,
Claude best at keeping track of people, Gemini the only one that hears the sound.

### Music lessons: an interesting case

Laurent mentioned you're interested in descriptions of music lessons. That would be a genuinely new use,
and it would test the app in ways nobody has yet:

- **The pauses are different.** Video Scribe finds pauses in *speech*. In a lesson, the gaps are often
  where the teacher *plays*, and that's the part a learner most needs to hear, not talk over. The option
  **"When a description does not fit: pause the film"** may suit lessons better than speaking over.
- **What matters on screen is different.** Hand position, fingering, which string or key, the bow, a
  diagram or sheet music, the chord shape. A film describer is told to describe people and action;
  a lesson might want a prompt of its own, or a "lesson" style alongside short, standard and detailed.
- **Captions help.** Many lesson videos have the teacher's words captioned, which the app uses for timing
  and passes to the AI, so it knows what the teacher is talking about.
- **Models may differ a lot here.** Reading a fretboard or a keyboard from a still picture is hard.
  Gemini, which watches short clips, might do better than models that only see stills. Nobody knows yet.

A few lessons, described with two or three models and a prompt tuned for them, and a note of what was
good and bad, would be valuable on its own, even with no code changed.

## 12. Ideas, if any appeal

No obligation and no order: just things that are open, in case one catches your interest.

- Music lessons (above): a prompt or a style for them, and findings on which model reads hands and
  instruments best.
- Codex as a third describer (section 9).
- A friendlier message when a job finishes (on the release checklist, `PLAN.md` §10).
- German speaks more slowly than English or French, so its descriptions need fewer words a second; and
  the cost estimate is a little low for French.
- Anything that confused you. You'll see the app with fresh eyes, and an issue saying "this puzzled me"
  is useful.

Laurent's next piece is categories (folders in the list of videos), so check with him before changing the
main window, so you don't both change it at once.

## 13. A map of the code

All of it is in `src/`, about 19,000 lines of Rust, most of it plain and well commented:

- `main.rs`, `ui.rs`: the main window, menus and wiring.
- `videoscribe.rc` (at the root): the layout of every window. Change a window here.
- `addvideo.rs`, `voicebox.rs`, `peopleui.rs`, `review.rs`, `listen.rs`: the other windows.
- `job.rs`: runs one describing job, step by step, off the main thread.
- `describe.rs`: builds the requests and reads the answers. `prompts/describe.v10.txt` is the prompt in
  use; the older versions are kept so old results can be explained.
- `openrouter.rs`, `claude.rs`: the two describers. `net.rs`: web requests, through Windows' own WinHTTP.
- `config.rs`, `choices.rs`, `presets.rs`: settings and the Options menu.
- `spike/`: the early experiments that proved each piece would work. Separate, and safe to ignore.

## 14. Words you will meet

- **Audio description (AD)**: spoken narration of what's on screen, in the pauses.
- **Place**: one pause long enough for a description.
- **VAD (voice activity detection)**: finding where speech is. We use **Silero**, a small model that
  runs inside the app.
- **Ducking**: turning the film down while the description speaks.
- **Captions**: the timed text of the dialogue; when a video has them, they give the speech timings.
- **yt-dlp**: the open-source tool that downloads web videos. Video Scribe fetches it the first time.
- **Edge voices / TTS (text-to-speech)**: Microsoft's natural online voices, the ones in the Edge
  browser's Read Aloud. **SAPI** is Windows' older built-in voices, the fallback.
- **Media Foundation**: the part of Windows that reads and writes video and sound.
- **Win32 dialog, `.rc` file, rc.exe**: the classic way of making Windows windows; screen readers
  understand them best.
- **JAWS, NVDA, Narrator**: screen readers. JAWS is the most used commercial one, NVDA the free one,
  Narrator is built into Windows.
- **Credential Manager**: Windows' own safe place for passwords and keys.
- **Checkpoint**: what was already described, saved, so nothing is paid for twice.
- **Spike**: a small experiment to check that something will work before building it.
- **`[proven]`, `[untested]`**: markers in `PLAN.md` saying whether a claim has actually been checked.

## 15. Handy things to say to your AI

- *"Read CLAUDE.md, ORIENTATION.md and PLAN.md section 7, then tell me in plain words how describing a
  video works."*
- *"Build the app and run the tests. Tell me what passed."*
- *"I'd like to [idea]. Check PLAN.md for anything already decided about it, and tell me what it would
  touch before changing anything."*
- *"Make a branch, make the change, run fmt, clippy and the tests, and write me a pull request
  description with a 'How to test' section for a screen reader user."*
- *"What would this test run cost on OpenRouter?"*
- *"Copy the describe prompt to a new version, and change it so that, for a music lesson, it describes
  the hands and the instrument. Then build the app with it."*
- *"Compare these two described.txt files and tell me which descriptions changed and how."*
- *"Check this change against the accessibility rules in CLAUDE.md: labels, announcements, nothing
  greyed out."*

Welcome aboard. Questions are always welcome, as issues or straight to Laurent.
