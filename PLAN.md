# Video Scribe — plan

Working name only — the folder, the crate and this title all rename together, and nothing in the plan
depends on the name. Drafted 2026-10-23, revised the same day after a critical read (see `log.md` entry 1).
Revised 2026-10-25 with the answers to entries 1 and 2; milestone 0 under way on the development machine.

Status markers follow the house convention: `[proven]` = read in SonarPad's shipping source;
`[claimed]` = stated in SonarPad's own docs but not verified in code; `[untested]` = my proposal, needs a spike.

---

## 1. What it is

A Windows-only app that takes a YouTube URL or a local video file and produces an **audio-described
soundtrack**: the original audio, ducked, with AI-written descriptions spoken into the gaps between
dialogue. JAWS-first, in the StableSound / Photo Scribe mould.

Two use cases, settled 2026-10-23, and they pull in different directions:

1. **YouTube.** Captions usually exist, so the dialogue timings come free; content is scripted and
   well-shot; files are downloaded, so we control the format.
2. **Family video with no captions.** No subtitle track ever, so a local VAD is mandatory; no script and
   often poor audio; and the people in it have names we want the app to learn and remember across videos.

The second one is the harder and the more valuable, and §5.7 is the feature it exists for.

**The premise this whole design rests on, stated so it can be tested early.** Every part of the pipeline
assumes descriptions go into gaps between speech, and therefore that useful gaps exist. That is true of
film, which is where SonarPad's design comes from. It may not be true of a birthday party, where people
talk continuously and a VAD will mark nearly everything as protected — producing a described video with
almost no descriptions in it, silently, by working exactly as designed.

Spike 0(a) exists to find out, and it is the gate on the whole plan rather than a detail of calibration.
If family video turns out to have too few gaps, the answer is not more tuning; it is a user setting for
what to do when gaps are scarce (§5.11), and spike 0(a)'s numbers decide its default.

It does **only** that. No editor, no ebooks, no player, no podcasts, no radio, no OCR — all the things
that make SonarPad a 70-module, 60-crate program.

---

## 2. How SonarPad does it

Source read: `Ambro86/Sonarpad` at HEAD (v0.9.18, Rust, edition 2024), plus `Ambro86/Sonarpad-Tools`.
The feature lives in `src/audio_description.rs` (357 KB), `src/app_windows/audio_description_*.rs`,
`src/tools/audio_description_bridge.rs`, `src/ffmpeg_export.rs`, and `bridge/`.
Its own design doc is `AUDIO_DESCRIPTION_INTEGRATION.md` (33 KB, Italian) — useful, but behind the code
in places (see §2.8).

### 2.1 Provenance — this is not original to SonarPad

`[proven]` The analysis half **starts from** **Omni Describer** (`audioses/omni-describer`, Python, MIT,
© 2026 Draqoken), vendored under `bridge/audio_description_runtime/` with its MIT licence kept intact.

**Corrected 2026-10-25 (log entry 8): SonarPad rewrote most of it, and SonarPad is not MIT.** Upstream
Omni's `audio_describer.py` is 30 KB; SonarPad's copy is 173 KB. `[proven]` by reading both: upstream Omni
has **no** speech detection, no dialogue-free windows, no intensive slots, no long-silence partitions, no
pause anchors, no visual-evidence timestamps and no language-correction pass, and its TTS module has no
scheduling or ducking — it asks Gemini to avoid dialogue by itself, under a "2 words per second" rule, in
ten languages. Everything this plan admired in §2.6–§2.10 is SonarPad's own work, and SonarPad's `LICENSE`
reads **"Copyright (c) 2025 Ambrogio Riili. All rights reserved. Source-available non-commercial
license."** So:

- **From Omni (MIT, with attribution):** the unified JSON shape (glossary plus timed descriptions in one
  response), the three verbosity levels and their word counts, the 2-words-a-second rule, the
  content-type focus prompts.
- **From SonarPad: ideas and facts only, never code or prompt text.** We may learn that synthesising
  before scheduling works, that −12 dB ducking with a pre-duck sounds right, or how Edge TTS's protocol
  behaves — those are facts and methods — but Video Scribe's code, prompts and tests are written fresh.
  This is a practical rule rather than a legal opinion; asking SonarPad's author is entry 8, Q12.

`[proven]` The speech model is **pyannote speaker-diarization-community-1**, segmentation weights only,
exported from PyTorch to ONNX, bundled as a file: **CC BY 4.0**, revision `3533c8cf…`, SHA-256 `6575e57e…`.
Attribution is required if reused; the bundled `MODEL_INFO.txt` is explicit that it is not endorsed by pyannote.

### 2.2 Two-process split

`[proven]` A deliberate split, and the single most important structural decision:

| | Python worker (`audio_description_bridge.exe`) | Rust host (SonarPad) |
|---|---|---|
| Does | pyannote VAD, Gemini description generation | download, decode, chunk, TTS, timing, duck, mix, encode |
| Never does | TTS, audio playback, mixing, export, calling `ffmpeg.exe` | vision AI, speech detection |

The worker is a PyInstaller one-file exe, **not** shipped in the installer. It is fetched on first use from
a GitHub release and cached:

```
%APPDATA%\Sonarpad\tools\audio_description_bridge_v11.exe
```

`[proven]` Two candidate URLs are tried (tag `0.7` then `v0.7`) on `Ambro86/Sonarpad-Tools`; the download is
rejected unless it is a valid PE and ≥ 5,000,000 bytes. Debug builds prefer a local `dll\audio_description_bridge.exe`.
Old cache versions (`_v1`…`_v10`) are pruned once per run.

Stated reasons for the split: ONNX Runtime upgrades must not disturb the Faster-Whisper worker used for
transcription; an audio-description crash must not kill dictation; the worker can be updated without
shipping a new SonarPad.

### 2.3 Worker protocol

`[proven]` Request is written to a temp JSON file; the worker is invoked as
`audio_description_bridge.exe --request <path>` with `CREATE_NO_WINDOW`.

Line-oriented on **stdout**, everything else to stderr (merged into SonarPad's log):

```
STATUS:{"stage":"pyannote_analyzing","message":""}
PROGRESS:42
CHECKPOINT:{"completed_chunks":3,"total_chunks":9,"descriptions":[…],"character_glossary":[…]}
QUOTA:{"model":"…","error":"…"}
OVERLOAD:{"model":"…","error":"…"}
RESULT:{…}
```

**stdin** carries one JSON decision per QUOTA/OVERLOAD event: `{"action":"switch","model":"…"}`,
`{"action":"wait"}` or `{"action":"stop"}`. The worker blocks on the same chunk and keeps everything in
memory — pyannote intervals, glossary, prior descriptions, uploaded files — so a model switch costs nothing.

`[proven]` Request fields: `input_path`, `duration_sec`, `chunks[]` (each `{path,start_sec,end_sec}`),
`audio_wav_path`, `gemini_api_key` **or** the hosted-service triple
(`sonarpad_ai_service_url` / `sonarpad_ai_access_code` starting `sp_` / `sonarpad_ai_device_id`),
`ai_access_mode` (`personal` | `sonarpad`), `gemini_model`, `language` (BCP 47), `verbosity`
(`short|standard|detailed`), `allow_extended_pauses`, `recognize_characters`, `recognize_screen_text`,
`allow_nonverbal_vocals`, `initial_character_glossary[]`, `resume{completed_chunks,descriptions,character_glossary}`.
Chunk timelines are validated as contiguous to within 2 s. Note the mutual exclusion: personal-key mode must
not receive service credentials and vice versa.

Progress identifiers are **stable IDs, not English prose** (`pyannote_done`, `gemini_chunk`,
`language_correction`, `gemini_uploading`, `gemini_repair`…) so the host can localise them into 17 languages.
Gemini's own English log lines are regex-mapped onto those IDs in `_gemini_status()`.

### 2.4 The YouTube download

`[proven]` Plain **yt-dlp.exe as a child process**. There is no in-process extractor.

- Binary: `yt-dlp.exe`, found on PATH or downloaded from
  `https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe`, with a prompt offering
  "download a local copy" vs "keep the system one".
- Base args: `--no-playlist --socket-timeout <n> --no-warnings --newline --progress-template <template>`.
- For YouTube URLs only, `--extractor-args` pins the standard player client. The comment in
  `configure_ytdlp_standard_youtube_client()` says why: letting yt-dlp pick `android_vr` yields signed media
  URLs that 403 even for public videos.
- Progress is machine-readable, not scraped. The template emits
  `downloaded_bytes:total_bytes:total_bytes_estimate:speed:eta:elapsed:format_id:ext:height:protocol:vcodec:acodec:fragment_index:fragment_count:status`
  behind a fixed prefix, parsed line by line.
- Format selection, and this is the bit that matters:
  - **Auto + video wanted** → `-f "best[vcodec!=?none][acodec!=?none]"` — a **single already-muxed stream**,
    so no external `ffmpeg.exe` is needed to merge, and nothing audio-only slips through.
    The `!=?` form tolerates extractors that omit codec metadata.
  - **MP4 requested** → `--merge-output-format mp4` plus
    `-f "bestvideo[ext=mp4]+bestaudio[ext=m4a]/best[ext=mp4]/best"` (quality variants clamp `height<=720` / `<=480`).
  - **Audio-only paths** (transcription, streaming) → `bestaudio/best`.
  - Explicitly *not* preferring 720p for video: a lower rendition can be served much more slowly than the best one.
- `[proven]` A v16 bug fix that is worth stealing outright: "Auto" used to mean `bestaudio/best` always, so a
  file could be named `.webm` and contain no video, and the describer rightly refused it because Gemini had no
  pictures. The fix is the context split above, plus an FFmpeg-level check that accepts WebM/Matroska which only
  expose width and height once the decoder opens — provided the track is not merely an attached cover image.
- Saved to `media_save_folder` (default `Documents\Sonarpad\Media`) with no Save-As dialog; collisions become
  `Title (2).webm`, `Title (3).webm`. Playback continues during the download, which stays cancellable.
- Local video files are never downloaded or re-encoded; the path goes straight in.

### 2.5 Media preparation — host side, FFmpeg libraries, never `ffmpeg.exe`

`[proven]` SonarPad embeds FFmpeg **DLLs** in the exe (`avcodec-62`, `avformat-62`, `avutil-60`,
`swresample-6`, `swscale-9`, `libmp3lame`, `opus`, from vcpkg) and extracts them to `%APPDATA%\sonarpad\`
on first run, alongside BASS, pdfium, libcurl, zlib, a CA bundle and `nvdaControllerClient64.dll`.
Bindings are bindgen-generated into `ffmpeg_dyn.rs`. All media work goes through those libraries in-process.

Preparation steps:

1. Read duration and stream layout through the FFmpeg libraries.
2. Decode the soundtrack to **mono 16 kHz PCM WAV** for pyannote (`PYANNOTE_SAMPLE_RATE = 16_000`).
   There is a recovery path, `recover_pyannote_wav_with_ffmpeg_libraries()`, for awkward sources.
3. Cut **180-second video chunks** for Gemini (`GEMINI_CHUNK_SECONDS = 180`). A file already under 180 s is
   passed through as-is with no copy.
4. Chunk sizing: `GEMINI_INLINE_TARGET_CHUNK_BYTES = 40 MB`, hard ceiling `GEMINI_MAX_CHUNK_BYTES = 2 GB`,
   `GEMINI_MIN_SEGMENT_SECONDS = 30`, retry limit 5. A compatibility mode halves it all
   (15 MB target, 10 s floor, 6 retries) for when Gemini rejects the media.
5. Chunk-timeline construction has three fallbacks
   (`build_gemini_chunk_timeline`, `…_with_repeated_small_start_offset`, `…_with_segment_schedule_fallback`),
   which tells you how much real-world trouble lives here.

### 2.6 Speech detection — where descriptions are allowed to go

`[proven]` pyannote is the **only** thing that decides protected intervals. FFmpeg silence detection is
explicitly not used, and the doc says so.

- Window 10.0 s, hop 1.0 s, output frame step **0.016875 s**, 589 frames per window, 16 kHz mono.
- ONNX Runtime, `CPUExecutionProvider`, `intra_op_num_threads = min(4, cpu_count)`, batched windows.
- Per frame: `argmax` over the powerset classes → multi-label mapping → **count of active speakers**.
  No embeddings, no clustering, no diarization — voice activity only.
- Overlapping windows are aggregated back onto the timeline; runs of non-zero speaker count become
  protected intervals; `merge_intervals()` applies padding.
- A softmax "voice confidence" is computed but is **diagnostic only** — the code comments are emphatic that it
  never influences whether a frame is protected.
- There is an opt-in non-verbal path (`candidate_nonverbal_voice_intervals`) for screams, laughter and the like,
  which Gemini then verifies.

The free gaps are formatted as text and handed to Gemini in the prompt
(`format_intervals_for_prompt`, capped at 12,000 chars), together with "intensive slots" — gaps over 3 s that
*should* get a description — so the model knows where it may speak.

`[proven]` **SonarPad Mobile (iPhone and Android, `Ambro86/Sonarpad-Mobile`, Dart), version 0.5.2,
read 2026-10-27 (log entry 24):** pauses under 0.5 s are ignored. Those of 0.5 to 1.5 s are optional
"anchors", offered to Gemini from 1 s, for important information only, describing the scene in the
4 s after the pause. From 1.5 s a pause is mandatory (it was 3 s until 24 October 2026, lowered "to
produce more descriptions"), split into parts of up to 15 s, at 2 words a second. A sentence that does
not fit is synthesised again at +10%, then +15%, and measured.

### 2.7 Gemini description generation

`[proven]` Google `google-genai` SDK, blocking `generate_content` (not streaming), with a background
heartbeat thread purely so the UI keeps moving during multi-minute video calls.

- Default model `gemini-3.5-flash-lite`, stored separately from SonarPad's general Gemini model
  (`audio_description_gemini_model`); the API key is shared with the rest of the app's AI features.
  A "Refresh model list" button queries the API for models supporting `generateContent`.
- `temperature = 0.3`, `response_mime_type = "application/json"`, safety settings forced to
  `BLOCK_NONE`/`OFF` where the SDK allows (service-side non-disableable blocks still apply),
  `frame_rate_for_ai = 0`.
- Video delivery: **inline bytes preferred** for small chunks (`Part.from_bytes`, ~100 MB API ceiling,
  40 MB target) because the Files API "often returns processing failures"; larger chunks go through
  `client.files.upload` + poll until state `ACTIVE`, with inline as the fallback after a terminal `FAILED`.
  `types.VideoMetadata(start_offset, end_offset)` does time-slicing without re-cutting the file.
- **Character glossary** for cross-chunk continuity: Gemini returns `character_glossary[]` with id, name and a
  physical description; later chunks receive it so "the woman in the red coat" becomes a name and stays that
  name. Capped at 96 characters, descriptions at 2,000 chars, merged by token-coverage heuristics. Turn the
  option off and the model returns an empty glossary and uses generic references.
- **Language control**: the UI language becomes a BCP 47 code with an explicit table entry for all 17 languages;
  the JSON example shown to the model is itself written in the target language to stop English leaking back in.
  After each response a language detector checks the descriptions — and each glossary physical description
  separately — and sends only the wrong-language items back for correction, preserving ids, names, order and
  timestamps. Unreliable detection keeps the original rather than failing the job.
- **Retry policy**: unlimited 5-second retries for timeouts, network errors, 5xx, transient overload, transient
  `PROHIBITED_CONTENT` and transient file-processing failures, with an 8-minute per-request ceiling. Genuine
  `429`/`RESOURCE_EXHAUSTED` escalates to the interactive QUOTA protocol of §2.3. Repeated high-demand 503s
  raise OVERLOAD. Malformed JSON gets a repair pass.
- Returned to the host: **timestamps, text, pyannote intervals and metadata only**. No audio.

### 2.8 TTS, timing, ducking and export — all Rust

This is where SonarPad earns its keep, and where the published doc is stale: it says −15 dB with 150 ms fades;
the code says otherwise. Trust the constants.

`[proven]` From `src/audio_description.rs`:

```
MAX_SHIFT_SEC                     = 5.0    // how far a description may slide from its scene
MIN_EXTENDED_ANCHOR_SEC           = 1.0
AUDIO_DESCRIPTION_DUCKING_DB      = -12.0
AUDIO_DESCRIPTION_FADE_MS         = 280
AUDIO_DESCRIPTION_PRE_DUCK_MS     = 180   // duck starts before the voice
AUDIO_DESCRIPTION_RELEASE_MS      = 600   // and recovers slowly after
AUDIO_DESCRIPTION_BITRATE_KBPS    = 192
EDGE_TRAILING_MIN_REMOVE_MS       = 60    // Edge TTS tail-silence trim
EDGE_TRAILING_KEEP_MS             = 30
EDGE_TRAILING_WINDOW_MS           = 60
EDGE_TRAILING_SEEK_MS             = 5
```

The order of operations is the clever part — **synthesise first, then schedule**:

1. Synthesise every description for real, with the engine and voice the user chose, reusing the app's existing
   `tts_engine.rs` — Edge TTS (the full protocol is in §2.9), SAPI 5, SAPI 4 or Google. Batched and
   parallelised, with retry logic for SAPI 5 returning empty audio.
2. For Edge output, trim trailing silence in PCM: threshold = max(−55 dBFS, mean − 35 dB), 60 ms window,
   5 ms step, keep 30 ms, and only trim at all if there is ≥ 60 ms to remove.
3. Measure the **real duration** of each rendered clip. No words-per-second estimate.
4. `schedule_synthesized_descriptions()`:
   - Invert the protected intervals into free gaps (`speech_free_intervals`).
   - **Mandatory** descriptions (one representative per intensive slot, chosen by fewest words and closeness
     to the slot centre) are placed first and restricted to their own slot; optional ones fill in after.
   - `choose_slot()` finds a gap where the clip fits, within ±5 s of the scene, minimising displacement from
     the desired start.
   - Placed clips **reserve** their span (`subtract_reserved_intervals`), so two descriptions never overlap.
   - If nothing fits and extended pauses are enabled, `choose_pause_anchor()` picks a ≥ 1 s anchor and the
     film is **paused** there to make room. If they are disabled, the description is **dropped** — not
     squeezed, not overlapped.
5. `ffmpeg_export.rs` decodes the original audio, applies ducking with smooth fades
   (`audio_description_duck_gain`, `audio_description_smooth_fade`, pre-duck and release as above), inserts
   extended pauses, writes a temp PCM WAV, then encodes to MP3 at 192 kbps through the FFmpeg libraries.
   `ffmpeg.exe` is never spawned.
6. `[proven]` A video output path also exists: mix to MP3, then mux alongside the copied video stream, with an
   **MKV fallback** when MP4 muxing fails. It refuses when any extended pause is present, since pausing the
   audio would desynchronise a copied video stream — a neat illustration of why audio-only output is the
   easier product.

### 2.9 Edge TTS, in protocol detail

Planned for 1.0, so here is the whole protocol as SonarPad implements it in `src/tts_engine.rs`.
`[proven]` throughout, read from source. SonarPad does this with tokio + tokio-tungstenite + rustls;
§4.4 explains why we will not need any of them.

**Two routes, tried in order.** The constant comments say Edge 155 is the primary Read Aloud endpoint and
the older one is the fallback when the modern connection fails.

| | Modern | Legacy |
|---|---|---|
| Host | `wss://api.msedgeservices.com/tts/cognitiveservices/websocket/v1` | `wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1` |
| Token param | `Ocp-Apim-Subscription-Key` | `TrustedClientToken` |
| `Sec-MS-GEC-Version` | `1-155.0.4283.45` | `1-132.0.2917.39` |
| User-Agent | Chrome/155 … Edg/155.0.0.0 | Chrome/132 … Edg/132.0.0.0 |

**Query string:** `?{token_param}={TRUSTED_CLIENT_TOKEN}&ConnectionId={request_id}&Sec-MS-GEC={gec}&Sec-MS-GEC-Version={version}`

**Handshake headers:** `Pragma: no-cache`, `Cache-Control: no-cache`,
`Origin: chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold`, the Edge `User-Agent` above,
`Accept-Encoding: gzip, deflate, br`, `Accept-Language: en-US,en;q=0.9`.

**The `Sec-MS-GEC` token**, which is the part that trips people up — a rolling 5-minute signature:

```
ticks = (unix_seconds + 11_644_473_600)      // Windows FILETIME epoch
ticks = (ticks - ticks % 300) * 10_000_000   // floor to 5 min, convert to 100 ns units
gec   = uppercase_hex(SHA256(ticks_as_decimal_string + TRUSTED_CLIENT_TOKEN))
```

**Superseded by the service, 2026-10-25 (log entry 12):** both endpoints now refuse this token (the
modern one answers 200 without upgrading, the legacy one 403). The working client `edge-tts` 7.3.1
sends a "v2" token, `<16 hex>-<64 hex>`, with `Sec-MS-GEC-Version: 2-143.0.3650.75`. The first part is a
checksum built on a deliberately altered SHA-256. Whether to follow it is Q13; until then Edge TTS is
not a route.

SonarPad's source comments are explicit that authentication parameters must never be logged and captured
browser credentials must never be reused. Worth honouring both.

**Then two text frames, in order:**

1. `Path:speech.config` — `X-Timestamp`, `Content-Type: application/json; charset=utf-8`, body
   `{"context":{"synthesis":{"audio":{"metadataoptions":{"sentenceBoundaryEnabled":"false","wordBoundaryEnabled":"false"},"outputFormat":"audio-24khz-48kbitrate-mono-mp3"}}}}`.
   Boundary metadata is switched off, which is right for us — we want audio and a duration, not word events.
2. `Path:ssml` — `X-RequestId:{id}`, `Content-Type: application/ssml+xml`, `X-Timestamp:{…}Z`, then the SSML.

**SSML**: language is the first two segments of the voice id (`it-IT-IsabellaNeural` → `it-IT`);
rate as `+N%`, pitch as `+NHz`, volume as `(volume - 100)` rendered `+N%`.

**Audio comes back as binary frames**: a 2-byte big-endian header length, then a text header
(`Path:audio\r\nContent-Type:audio/mpeg`), then MP3 bytes. Completion is a **text** frame containing
`Path:turn.end`.

`[proven]` **The failure rule matters more than anything else here, and there are four tests pinning it:**
a close, an EOF, or a stream end *without* `turn.end` must **discard the buffered MP3 and retry** —
never return it as success. `edge_close_before_turn_end_discards_partial_mp3`,
`edge_eof_before_turn_end_retries_instead_of_succeeding`. For audio description this is not a nicety:
a silently truncated clip would be measured short, scheduled into a gap it does not fit, and the film
would talk over itself. Any reimplementation must keep this rule and test it — with our own tests of the
same behaviour, not SonarPad's (§2.1).

Output is 24 kHz mono MP3, so it must be decoded to PCM and resampled to the mix rate before scheduling —
and the tail-silence trim of §2.8 step 2 exists precisely because Edge pads the end of every clip.

### 2.10 The editable project file

`[proven]` Optional, off by default, and deliberately written **after** a successful export:
`FilmName_audiodescritto.sonarpad-ad.json` beside the MP3.

- `descriptions[]` holds only what actually made it into the MP3: spoken text, Gemini's proposed time, the real
  position on the original timeline, start and end **in the MP3's own timeline** (extended pauses shift
  everything after them, and the file records where you really hear it), measured TTS duration, extended-pause
  flag, ducking interval.
- `excluded_descriptions[]` holds the dropped ones with the reason.
- MP3 and JSON are written as temporaries and swapped together, with rollback, so a project can never describe
  a different MP3 than the one next to it.
- Re-editing synthesises only the edited line, measures it, and accepts the change **only if it still fits**;
  otherwise the old text stands. Re-export reuses the saved pyannote intervals and spends no Gemini tokens.
- There is also a mid-job partial checkpoint (`sonarpad-audio-description-partial`, v1) plus a resume window,
  so a long film survives a crash or a cancel.

### 2.11 The UI, in accessibility terms

`[proven]` Worth copying: during generation the window **hides every configuration control**, leaving only
the progress bar, the current message and Cancel, so NVDA/JAWS cannot wander into the file, model and voice
fields mid-job. Controls come back at the end. On completion, OK opens the MP3 in the player and Escape
returns focus to the create button. `Ctrl+Shift+I` starts the job from whatever is playing; the shortcut is
chosen so the player's `Ctrl+I` (announce time) requires Shift and Alt to be *up* and therefore cannot swallow it.

---

## 3. What a good audio description says

This section exists because the first draft of this plan did not have one, and that was the worst thing
wrong with it. Nine sections of plumbing and one table row for the prompt — when the prompt *is* the
product. Everything else here is a delivery mechanism for a few hundred words of text per video, and if
those words are bad then a flawless pipeline has produced nothing worth listening to.

### 3.1 Read the existing guidance before writing a prompt

Audio description is a forty-year-old craft with published standards. We should not invent our own.

- **Ofcom / ITC, *Guidance on Standards for Audio Description*** — the British broadcast standard, and the
  most practical thing written on when to speak and when to shut up.
- **WCAG 2.1 SC 1.2.3 and 1.2.5** — the accessibility requirements, which define what "described" means
  formally, if we ever make a claim about compliance.
- **Omni Describer's prompts** (upstream, MIT) — the base shape: unified JSON, verbosity word counts,
  2 words a second, content-type focus. SonarPad's much richer version (17 languages, gap-aware slots,
  language correction) is **not** reusable text (§2.1), though its ideas informed ours.
- **DCMP Description Key** — present tense, active voice, third person; objective; "do not describe every
  pause, allow mood and background sound to come through"; for montages, summarise first, then detail;
  when time is short, sentence fragments and "describe over the least consequential audio".

`[proven]` Done 2026-10-25 as spike 0(c): the first prompt is `spike/prompts/describe.v0.txt`, written for
this project, and the scoring sheet is `spike/prompts/rubric.txt` (log entry 8). Two deliberate departures
from SonarPad: **silence is allowed** — an empty description beats a repeated or invented one, per DCMP,
where SonarPad's intensive mode demands one per slot — and **montages are summarised in their first part**.

### 3.2 The rules that matter most

From the craft, and each one is a line in the prompt:

1. **Present tense, third person, no narrator.** "Sophie climbs onto the swing", never "we see Sophie
   climb onto the swing" or "the camera shows".
2. **Describe only what is visible.** Not what it means, not what someone feels, unless the face or body
   shows it. "She frowns" is description; "she is disappointed" is interpretation.
3. **Never say what the soundtrack already says.** If a door slams audibly, do not describe a door
   slamming. This is the single most common failure, and it is the one an AI with the audio muted cannot
   detect for itself — so the prompt has to forbid it explicitly.
4. **Never repeat what the dialogue conveys.** If someone says "look at that red car", the red car does not
   need describing.
5. **Names once established, descriptions before that.** "A woman in a navy coat" until we know she is Mum;
   "Mum" forever after. This is what §5.7's catalogue is for.
6. **Fit the gap.** The prompt must be told how many words it has room for, computed from the gap length,
   not left to guess. A description that does not fit is a description that gets dropped (§2.8 step 4).
7. **Priority under pressure.** When there is not room for everything: who is present, then where, then
   what changed. Appearance and scenery last.
8. **No hedging and no meta-commentary.** "appears to be", "possibly", "in this scene", "the video shows" —
   all banned in the prompt, all things models do by default.

### 3.3 Family video needs different rules from film

This is the part no existing guidance covers, because broadcast AD is written for broadcast.

- **Who is present matters more than what happens.** A birthday video is mostly people standing about. The
  valuable description is "Mum, Dad and Grandma are round the table; Sophie is on Dad's lap", not a
  blow-by-blow of the cake being cut.
- **The person holding the camera is usually talking** and is usually not on screen. The model will not
  know that. Tell it.
- **Faces and expressions carry the content**, far more than in film. Who is laughing, who is looking at
  whom.
- **Date and place are context the file already knows.** Photo Scribe already reads DateTaken and camera
  from the property system (its PLAN.md §4) and feeds them to the model; the same should happen here, and
  the same `send_names` restraint should apply (§5.9).
- **Continuity across videos, not just within one.** In film the glossary lasts 90 minutes. Here it should
  last twenty years, which is why the catalogue is a shared file and not a per-job scratchpad.

### 3.4 How we will know whether it is any good

A bake-off with no rubric is just a preference. So define the test before running it: play the described
audio to someone who cannot see the picture, and ask whether they can say

1. who is present,
2. where it is happening,
3. what changed since the last description.

Three questions, scored per clip, same clips across all three services. That is the number that decides the
default service — not which one writes the nicest prose, which is what a casual reading of the output will
reward.

`[untested]` The prompt itself should live in its own file, be version-stamped in the output sidecar, and
have a small golden set of clips so a prompt change can be shown to be an improvement rather than assumed
to be one. Photo Scribe already keeps its style prompts in `prompts.rs`; same idea, one level more formal.

## 4. What this means for a small, portable Windows app

SonarPad's answer costs: ~17 embedded DLLs (FFmpeg ×5, BASS ×8, pdfium, libcurl, zlib, NVDA client), a
downloaded 5 MB+ PyInstaller worker carrying ONNX Runtime and the Google SDK, an NSIS/WiX installer, and
60-odd crates including tokio, reqwest, curl, rodio, cpal and symphonia. That is the opposite of
`photoscribe/CLAUDE.md` constraint 4 — one exe, no installer, no runtime dependency, Windows built-ins first.

The useful distinction is between SonarPad's **behaviour**, almost all of which we want, and its
**transport**, most of which we do not:

| SonarPad pillar | Us |
|---|---|
| yt-dlp as a child process | copy exactly (§4.5) |
| Gemini for description | copy, reimplemented over WinHTTP, joined by two image services (§4.2) |
| Edge TTS for voice | copy the protocol exactly, over WinHTTP WebSockets instead of tokio (§4.4) |
| Synthesise → measure → schedule → duck | same method, our own code; it is SonarPad's, not Omni's, so ideas only (§2.1) |
| pyannote in a Python worker for timing | replace with Silero VAD in-process, plus caption cues (§4.1) |
| FFmpeg DLLs for decode/mix/encode | replace with Media Foundation (§4.3) |

The last two are the only places we genuinely diverge, and they are the two that carry all the weight —
the Python worker and ~17 DLLs. Everything else is the same program with a different delivery van.

### 4.1 Dialogue timing: Silero VAD in-process, plus captions when they exist

Decided, once the use case became YouTube **and family video with no captions**: ship a VAD.
Family videos have no subtitle track and never will, so a captions-only 1.0 would not do the job it is for.

**Not pyannote.** Shipping pyannote means SonarPad's whole Python worker, and it is the wrong model for the
job anyway — it counts speakers, which we never need. Instead, **Silero VAD**, in-process, in Rust:

| | pyannote Community-1 (SonarPad) | Silero VAD |
|---|---|---|
| Model | `model.onnx`, exported segmentation weights | `silero_vad_16k_op15.onnx`, **1,289,603 bytes** |
| Licence | CC BY 4.0, attribution required | **MIT** |
| Inference | 10 s windows, 1 s hop — 10× redundant compute | sequential 512-sample (32 ms) frames at 16 kHz |
| Output | powerset argmax → active-speaker count per frame | one speech probability per frame |
| Post-processing | aggregate overlapping windows onto a 0.016875 s frame grid | hysteresis: threshold, min speech, min silence, pad |
| Host | Python worker exe, downloaded on first use | in-process, in Rust (see below) |

The post-processing row is the one that matters. pyannote's overlapping-window aggregation is the fiddliest
code in SonarPad's worker, and it exists to support speaker counting that we throw away. Silero gives one
probability per frame and a small state machine turns it into intervals. That is an afternoon, not a module.

`[proven]` Spike 0(b), 2026-10-25 (log entry 4). Pinned: Silero VAD **v6.2.3**, commit
`5cd7945676eb32225748052e2e6a0580e4686a08`, `LICENSE` is MIT (the README badge's CC BY-NC is wrong).
Defaults from `get_speech_timestamps` at that commit: threshold 0.5, negative threshold 0.35
(threshold − 0.15), min speech 250 ms, min silence 100 ms, pad 30 ms, 512-sample windows with 64 samples of
context. Those defaults are tuned for cutting speech up for transcription; for audio description the gap
we care about is a second or more, so min silence is ours to raise.

**Which file:** `tract-onnx` 0.21 **cannot** run `silero_vad_16k_op15.onnx` — nor `silero_vad.onnx`,
`silero_vad_half.onnx` or even `silero_vad_op18_ifless.onnx` — because each contains an `If` node whose
branches disagree on output shape, which tract refuses to type. It **does** run
`silero_vad_16k_sequence.onnx` (1,246,165 bytes, SHA-256 `9ccdacc4…38a85`, full hash in `spike/models/README.txt`), which is 16 kHz only,
takes `[N, 576]` windows at once and carries the LSTM's `h` and `c` explicitly. Batched 256 windows at a
time it runs at **134× real time** — a two-hour video in under a minute — and on a synthetic test with
known word times its speech starts land within 50 ms of the synthesiser's. So: pure Rust, one file, and
§5.8's rule is not invoked. Real, noisy audio is spike 0(a)'s job.

**And captions as a second provider, not a fallback.** `yt-dlp --write-auto-subs` / `--write-subs` gives cue
timings for free. The first draft said to take the **union** of captions and VAD and called it "strictly
safer". That was sloppy: the union protects more, so it is safer against talking over dialogue and *worse*
for coverage — and given the gap-density worry in §1, fewer gaps is itself a failure mode. So the merge
strategy is a spike question, not a decision:

- **union** — never speak over dialogue, fewest descriptions;
- **captions only, where captions exist** — trusts the transcript, lets the VAD cover the rest;
- **intersection** — only protect where both agree, most descriptions, highest risk.

Spike 0(a) compares all three on a video that has real captions, and §9 records that getting this wrong is
how the app ends up quietly saying nothing. For family video the VAD runs alone and the question does not
arise.

`[proven]` YouTube half of spike 0(a), 2026-10-25 (log entry 5). On *Sprite Fright* (hand-made captions) the
VAD and the captions agree on only ~two-thirds of each other's speech, and the disagreements are of two
kinds: **Silero misses shouted dialogue under loud music and effects** (the whole 5:30–7:21 action scene,
~−12 dB mean, scores 0% even at threshold 0.2, and ONNX Runtime agrees with tract window for window), and
**Silero hears end-credit singing** that has no cue. Each source catches the other's misses, so the
recommendation is **union** on YouTube: protected time 38% → 50%, still 20 gaps of ≥ 3 s in 10.5 min.
Interview content (*Making of CHARGE*) is 90% speech with ~0.5 usable gaps a minute whichever strategy is
used — the scarce-gaps setting (§5.11) is a necessity. Pure music (*Spring*) is 0% protected.

**Superseded for YouTube by "captions lead"** (log entry 15). The first listening test found the union far
too cautious: on Sprite Fright 1:00–4:00 it left 8 gaps (45 s), where the user's reference description
speaks 22 times. Silero calls laughs, shouts, effects and music speech, and the cue padding adds more.
Captions lead: each cue is cut down to the VAD speech inside it; a cue the VAD barely hears (under 30%) is
kept whole; VAD speech outside every cue is dropped. With gaps from 1.0 s and long silences split into
parts of at most 5 s, the same window gives 23 gaps (78 s), in nearly the same places as the reference.
`[untested]` whether this ever puts a description over uncaptioned speech; that is the second listening
test's question. Without captions (family video) the VAD still runs alone.

`[proven]` Family half, 2026-10-25 (log entry 6). A 2.8-minute phone video is 61% speech with ~11 gaps of
≥ 1 s a minute but only 2 of ≥ 3 s in all — family gaps exist and are **short**, mostly 1–2.5 s, room for
two or three words; a 30-second WhatsApp clip is similar; a 5-minute wedding highlights reel is 20% speech
with gaps of 75 s and 142 s. **The premise holds**; the default for family video is "pause, with short
descriptions" (§5.11). **Long gaps must be split** into ~6–10 s stretches, at scene changes where they can
be found, each described in turn — one description per gap would leave a 142-second montage nearly silent.
`[proven]` by the user's listening check (`test-milestone-0.txt`, log entry 7): every gap in the phone and
WhatsApp videos is real silence. In the wedding reel Silero **misses quiet speech as music fades** — ~13 s
of vows at about −24 dB scored ≈ 0 — and is 2 s late at another music-to-speech change. Not stereo phase
(L, R and mono agree). +12 dB of gain finds the vows to within a second of where the user heard them but
also marks music as speech; dynamic normalisation recovers part of it at no cost elsewhere. So the VAD's
weakness, in film and family video alike, is **speech under or just after music**. Milestone 3 tries:
normalise before the VAD; treat the seconds after music fades as unsafe; let Gemini (which hears the
audio) mark speech onsets; and milestone 8's review remains the backstop.

`[proven]` Milestone 3 tried the first two in Rust, on the wedding reel and the phone video (log entry 18),
and ships neither. A dynaudnorm-like normaliser (half-second frames, 20 dB at most, held to the quietest
gain within ±7.5 s, then smoothed) finds the vows 1.5 s sooner (1:44.2 against 1:45.7) and costs the phone
video 2 of its 31 places. Its look-around is why it cannot do better: the gain stays low for seconds after
loud music, which is exactly where the vows are. A second VAD pass at +12 dB, trusted only where the
original is quiet, fails because **the music before the vows is no louder than the vows** (−21 to −26 dBFS
RMS each second from 0:55 to 1:47), so no level separates them. "Unsafe after music" needs a music
detector we do not have. That leaves Gemini's ear (milestone 4) and the review (milestone 8). The report
says the detector can miss quiet speech straight after loud music.

`[proven]` **The real cause was the model's memory, not the level** (2026-10-31, log entry 33, after the
user heard place 17 run into the vows). Silero's LSTM state was carried through the whole film. Started
afresh at 1:28, the same model scores the vows at 0.9 and more from 1:34.8; carried from 0:00 through
the choir, it scores them near 0 until 1:45.7. Started from 0:30 or 1:00 it lands somewhere between: it
drifts. Fix: **start each 256-window batch (8.2 s) afresh, after a half-second run-up** from the sound
before it, and keep only the batch's own windows. Against Sprite Fright's hand-made captions it is better
both ways: captioned speech heard 68% → 75%, detected speech inside a caption 65% → 75%, places 91 → 97.
The phone video is unchanged (61% speech); the wedding goes from 21% to 29% speech and 52 to 50 places,
the vows now protected from 1:34.8. Taking the maximum of both runs, and run-ups of 1 and 2 s, were
tried and were no better. The speech-map cache version was bumped.

Both sources produce the same thing — a list of intervals — so they sit behind one interface (§5.3).

**The cost may be nothing at all — try pure Rust first.** The first draft of this plan reached straight for
`ort`, accepted `onnxruntime.dll` beside the exe, and then wrote a governance rule (§5.8) to justify the
concession. That was the wrong order. There are pure-Rust routes, and they should be tried first:

| Route | Ships | Risk |
|---|---|---|
| **`tract-onnx`** (preferred) | nothing — pure Rust, runs the official ONNX graph unchanged | `[untested]` that it supports every operator Silero uses |
| `silero-vad-pure` | nothing — hand-written STFT, Conv1d and LSTM, no inference framework | a single-purpose crate reimplementing the model; less general, less proven |
| `ort` (fallback) | `onnxruntime.dll`, ~12 MB | the known-good path, but costs the one-file goal |

`[proven]` `tract-onnx` is MIT OR Apache-2.0, from Sonos, around 3,100 stars, and was pushed the day this
was written. Its ONNX operator set includes **LSTM and STFT**, which are the two Silero needs that a
minimal runtime usually lacks. That makes it a genuine candidate rather than a hope — and if it works,
**we ship one file after all** and §5.8's rule never has to be invoked.

Two things to know about the `ort` fallback if we end up there. Its **default features include
`download-binaries`**, which fetches ONNX Runtime at build time — unacceptable for a reproducible build, so
it would need `default-features = false` with `load-dynamic` and the DLL shipped deliberately. And `ort` 2.0
is still a **release candidate** (`2.0.0-rc.15`), which is a real thing to accept knowingly rather than
discover later.

`[untested]` Windows 11 24H2 does ship a system-wide ONNX Runtime through Windows ML, but it arrives via
the Windows App SDK and its bootstrapper, which pulls packaged-app deployment into a portable exe. If
`tract` works this question never needs asking. The older in-OS `Windows.AI.MachineLearning` is no help
regardless: our model is opset 15 and legacy WinML tops out well below that.

### 4.2 Three description services behind one interface

Decided: **Gemini, Claude Code, and OpenRouter all three**, with Gemini first because we know from
SonarPad that it works.

The complication is that they do not take the same input. Gemini ingests long video with timestamps
natively; Claude and the OpenRouter models take **images**. So the abstraction cannot be "send the video to
the service" — it has to be "describe this moment, from whatever evidence this service can accept".

But notice what §2.6 makes possible: **descriptions only ever go in the gaps.** You never need to understand
the whole film, only the moments you are allowed to speak over. SonarPad uploads every second anyway. That is
what lets the image-only services compete at all, and it makes them very much cheaper.

```rust
enum Evidence {
    /// A cut video segment, with the gap's offsets inside it. Gemini.
    Clip { path: PathBuf, start_sec: f64, end_sec: f64 },
    /// Frames sampled across the gap, as JPEGs. Claude Code, OpenRouter.
    Frames(Vec<Vec<u8>>),
}

trait Describer {
    fn accepts(&self) -> EvidenceKind;        // the host prepares only what is wanted
    fn describe(&self, ev: &Evidence, ctx: &Context, stop: &AtomicBool)
        -> Result<Described, Failure>;        // text + glossary additions
}
```

`Context` carries what §2.7 shows matters: the running character glossary, the previous few descriptions,
the target language, the verbosity, and how long the gap is — because the model should be told how many
words it has room for (§3.2 rule 6).

**But do not write the trait first.** The two evidence kinds are not one abstraction with three
implementations; they are two genuinely different pipelines — chunking, upload and ACTIVE-polling for
Gemini, frame extraction and a multi-image request for the others — that happen to share a prompt and a
glossary. A trait guessed in advance of the second implementation is a trait shaped by imagination.

So: build Gemini's path concretely at milestone 4, write Claude's at milestone 6, and **extract** the trait
once both exist and their real shape is visible. What should be factored out from the start is the part that
is genuinely shared and is also the valuable part — the prompt, the glossary, and the word budget (§3).
The sketch above is a prediction, not a specification.

| Service | Evidence | Credential | Cost | Notes |
|---|---|---|---|---|
| **Gemini** | `Clip` (180 s chunks, as SonarPad) | API key, Credential Manager | per-token, free tier exists | Best quality: sees motion, holds continuity. `[untested]` Files-API-or-inline upload must be reimplemented over WinHTTP. |
| **Claude Code** | `Frames` | none — Pro subscription via `claude -p` | nothing beyond the plan | Reuses Photo Scribe's `describe.rs` nearly as-is, including the `authMethod: claude.ai` refusal and the stripped environment. Usage limits are the risk (§9). |
| **OpenRouter** | `Frames` | API key, Credential Manager | per-token, cheap models available | Reuses Photo Scribe's existing OpenRouter path and its no-data-collection provider filter. Lets a model be swapped without new code. |

**As built (milestone 4, log entry 21):** Gemini goes through OpenRouter, not its own API, and gets
one clip per pause, not 180 s chunks: a chunk would send the speech too, which constraint 7 forbids.
Clips are inline base64 MP4s (640 px, 2 fps, mono AAC), four places to a request (eight until log entry 29); no Files API was
needed. The direct API and its free tier are still for YouTube only, later.

Frame sampling needs a decision of its own, which spike 0(h) should settle: how many frames per gap, and
from where. Starting point: three — just after the gap opens, the middle, and just before it closes — so the
model can at least see that something changed. One frame is cheapest and will miss movement entirely.

`[untested]` **Batch several gaps into one request** for the frame services — say ten gaps of three frames
each, with each gap's times and word budget. It cuts a two-hour video from ~480 `claude -p` calls to ~48,
which is the difference between fitting in a Pro usage window or not (log entry 3); it cuts the repeated
prompt cost on OpenRouter and Ollama; and it lets the model see consecutive moments together, which is
some of what Gemini gets from watching video. Spike 0(h) compares one-gap and ten-gap requests.

The glossary is the piece that makes image-only services viable across a whole film: carry it forward as
text, exactly as §2.7 describes, so character identities survive from gap to gap even though each request
sees only a few stills.

### 4.2b Why Gemini first, and what that costs

Gemini is the known-good path, so it goes in first and becomes the quality bar the other two are measured
against. It is also the one piece of genuinely new network code: §2.7's upload dance (inline bytes under
~40 MB, Files API with ACTIVE polling above that, inline again as the fallback after a terminal `FAILED`)
has to be written over WinHTTP instead of the Python SDK. That includes the retry and quota policy, which is
not optional — a 90-minute film will hit `RESOURCE_EXHAUSTED` on a free tier. SonarPad's interactive answer
(§2.3) is a good model: hold the chunk, ask the user, offer a different model, keep the glossary in memory.

### 4.3 Decode, mix and encode without FFmpeg

`[untested]` **Media Foundation** is in Windows and already exposed by the `windows` crate
(`Win32_Media_MediaFoundation`, which SonarPad also pulls in). The Source Reader decodes MP4/H.264/AAC to PCM
and grabs frames; the Sink Writer encodes AAC or MP3. The mixing itself — ducking, pre-duck, fades, release,
inserting pauses — is plain `f32` arithmetic over sample buffers, which is literally what
`ffmpeg_export.rs` does by hand anyway. SonarPad already has `mf_source.rs` and `mf_encoder.rs`, so the
approach is not exotic.

The catch: MF does not reliably decode **WebM/VP9/Opus**, and that is yt-dlp's "Auto" favourite.
The fix follows from §2.4 — ask yt-dlp for MP4 (`--merge-output-format mp4`, or
`best[ext=mp4][vcodec!=?none][acodec!=?none]` for a single muxed stream), and MF covers the rest.
Needs a spike on real YouTube downloads before it is a decision.

### 4.4 Edge TTS with no new crates — the WinHTTP WebSocket route

Decided: **Edge TTS is in 1.0**, as SonarPad has it, with SAPI 5 alongside it as the offline fallback.
It is the right call — Edge's neural voices are the difference between a described film you can sit through
and one you cannot.

SonarPad pays for Edge with tokio, tokio-tungstenite and rustls. `[proven]` **We do not have to.**
WinHTTP has spoken WebSocket since Windows 8, and the bindings are in the `windows` crate under the
`Win32_Networking_WinHttp` feature that Photo Scribe **already enables** — I checked
`microsoft/windows-rs` directly and all of these are generated:

```
WINHTTP_OPTION_UPGRADE_TO_WEB_SOCKET   WinHttpWebSocketCompleteUpgrade
WinHttpWebSocketSend                   WinHttpWebSocketReceive
WinHttpWebSocketShutdown               WinHttpWebSocketClose
WinHttpWebSocketQueryCloseStatus
```

So the whole of §2.9 becomes an extension of Photo Scribe's existing `net.rs`: set the upgrade option on the
request before sending it, complete the upgrade, send two UTF-8 text frames, read binary frames until the
text frame carrying `Path:turn.end`. Synchronous, on the worker thread, exactly like every other request the
app makes. No async runtime, no TLS crate, no WebSocket crate.

The two remaining pieces are also already in Windows:

- **SHA-256** for the `Sec-MS-GEC` token — CNG (`BCryptOpenAlgorithmProvider` / `BCryptHashData`) under
  `Win32_Security_Cryptography`, so no `sha2` and no `hex` crate.
- **MP3 → PCM** for the 24 kHz mono MP3 Edge returns — Media Foundation's built-in MP3 decoder, the same
  Source Reader already needed in §4.3. SonarPad uses rodio for this; we do not need to.

`[untested]` Both of those, and the upgrade handshake itself, want a spike. The handshake is the risky one:
WinHTTP must be allowed to send the custom `Origin` and `User-Agent` headers of §2.9, and it must not
mangle the query string. If WinHTTP turns out to refuse the handshake, the fallback is a WebSocket crate
over WinHTTP's plain TLS socket, and only as a last resort tungstenite + rustls.

**SAPI 5** stays in as a second engine, through COM (`Win32_Media_Speech`), rendering to a stream rather
than the speakers so the scheduler gets an exact duration. It earns its place three times over: it works
offline, it works when Edge changes its endpoint again (note §2.9 has already needed two), and a
screen-reader user may genuinely prefer a familiar voice at a familiar rate to a slower natural one.
Windows 11's OneCore voices are SAPI5-visible and sound respectable.

Both engines sit behind one trait with one hard requirement — return PCM plus its true measured duration:

```rust
trait Voice {
    fn speak(&self, text: &str, stop: &AtomicBool) -> Result<Pcm, Failure>;
}
```

and the Edge implementation applies the tail-silence trim of §2.8 step 2 before returning, because that trim
exists specifically to undo Edge's end-padding.

### 4.5 yt-dlp

Copy SonarPad's shape: look on PATH, else offer to download `yt-dlp.exe` from the official latest-release
URL into `%LOCALAPPDATA%\VideoScribe\tools` (verifying it against the release's `SHA2-256SUMS`), parse
`--progress-template` rather than scraping. It is a downloaded tool rather than a shipped dependency, and it
needs frequent updating regardless.

`[proven]` Built at milestone 3 (`src/ytdlp.rs`, log entry 18), with these details settled:

- The folder is `%LOCALAPPDATA%\Video Scribe\`, with a space like the settings folder: `tools\` for
  yt-dlp, `downloads\` for what it fetches.
- Our own copy is updated with `yt-dlp -U` when it is over 14 days old.
- yt-dlp runs in a **job object** that kills it on Stop and when Video Scribe ends. The PyInstaller exe
  starts a second Python process, which killing the first would leave downloading.
- `-o subtitle:%(id)s.%(ext)s` names the captions `<id>.<lang>.vtt`.
- A run that fails with a 403 is retried twice, resuming where it was.
- Milestone 3 downloads only the sound and the captions. The picture comes with milestone 4, which
  needs frames.

**But not SonarPad's format choice — that has stopped working.** `[proven]` 2026-10-25, yt-dlp 2026.10.21
(log entry 5): without a JavaScript runtime (Deno by default), YouTube offers **no muxed format at all**,
only separate video and audio, with occasional transient 403s; with one (Node, via `--js-runtimes node`)
it offers exactly one muxed format, 360p. SonarPad's `best[vcodec!=?none][acodec!=?none]` therefore finds
nothing, and `bv+ba` needs `ffmpeg.exe` to merge. So:

- Download **video and audio as two files and never merge**: `bv*[vcodec^=avc1][height<=720]` and
  `ba[ext=m4a]`, both of which Media Foundation decodes. The audio is what the VAD and the mixer need
  anyway; the video is only needed for frames or Gemini clips; and a described video is a Media Foundation
  remux of our mixed audio with the untouched H.264 stream (milestone 9), which never needed yt-dlp's
  merge.
- Retry 403s a few times before failing.
- Do not ship or download Deno. If YouTube eventually refuses even separate streams without a JS runtime,
  that is a decision for then, against §5.8's rule.
- `--write-subs --write-auto-subs --sub-langs "<lang>,<lang>-orig" --sub-format vtt` gets both caption
  kinds. Auto captions roll and carry per-word `<00:00:05.040>` tags, which give better speech timing than
  their cue boundaries; the spike's `vtt.rs` parses both.

The development machine happens to have FFmpeg (winget) on PATH, and yt-dlp silently uses it. The spike and
the app must pass `--ffmpeg-location` pointing nowhere, or test with it hidden, so that a user's machine
without FFmpeg is what gets tested.

---

## 5. Keeping the architecture simple

SonarPad's audio description is awkward for reasons that are mostly **traceable to one decision** — pyannote
is Python, so there had to be a second process, and once there was a second process everything had to be
marshalled across it. Replacing pyannote with an in-process VAD does not just save megabytes; it lets us
delete most of the scaffolding. The useful question at this stage is not what to build but what never to build.

### 5.1 One process. This is the big one.

Everything in this list exists in SonarPad **only** because the analysis half is Python, and none of it
needs to exist for us:

- the PyInstaller worker exe, its `.spec`, and its build script
- download-on-first-use from a GitHub release, two candidate URLs, PE validation, the ≥ 5 MB size check
- the `_v1`…`_v11` cache-version naming and the pruning of old caches
- the temp-file request handoff and `--request <path>`
- the `STATUS` / `PROGRESS` / `CHECKPOINT` / `QUOTA` / `OVERLOAD` / `RESULT` stdout protocol and its parser
- the stdin decision channel for quota answers
- the stable-status-ID translation layer, which exists because the worker must not emit English prose
- `terminate_bridge_process_tree`, stderr tail capture, and the UTF-8/ANSI decode guessing

That is a large fraction of SonarPad's complexity in this feature, and it buys us nothing. One process, one
worker thread, one job. Cancellation becomes an `AtomicBool` instead of killing a process tree.

### 5.2 One source-resolution step

SonarPad threads the YouTube case through its player's live context, which is why §2.4 has so many branches.
We do not have a player. So:

```rust
fn resolve_source(input: &Input, …) -> Result<PathBuf, Failure>  // URL → yt-dlp → file; file → file
```

Everything after this point is identical for both use cases, and the YouTube-specific knowledge stays inside
`ytdlp.rs`. Family videos skip it entirely — no download, no re-encode, no temp copy.

### 5.3 One kind of speech map, two providers

Captions and the VAD both answer the same question, so they return the same type and nothing downstream
knows which ran:

```rust
struct Interval { start_sec: f64, end_sec: f64 }

trait SpeechSource { fn intervals(&self, media: &Media) -> Result<Vec<Interval>, Failure>; }
// CaptionCues (yt-dlp subs, embedded track, sidecar .srt) | SileroVad
```

Merge (by the strategy spike 0(a) chooses, §4.1), then invert once to get the gaps. One merge function,
one inversion, used by everything.

`[proven]` Built at milestone 3 **without the trait**. Both providers return plain `(start, end)` spans
(`src/captions.rs`, `src/vad.rs`), and `src/speech.rs` merges them, lets the captions lead, inverts and
cuts. Two concrete providers with one obvious merge did not need an interface: §4.2's rule is to extract a
trait from working code, not from a guess. Captions come from yt-dlp, or from a `.srt` or `.vtt` beside a
local video (`party.srt`, `party.en.vtt`). An embedded subtitle track is not read yet.

### 5.4 Chunk only when you must

SonarPad has **three** chunk-timeline construction fallbacks, because a 90-minute film must be cut into 180 s
pieces and the arithmetic kept exact. Family videos are minutes long and go to the service whole — SonarPad
already passes anything under 180 s through untouched. So make chunking conditional on the file exceeding the
service's limit, and most jobs never enter that code path at all. Keep it out of the main flow, not threaded
through it.

### 5.5 Serial, not parallel

Drop `synthesize_description_tasks_parallel`, `run_description_batches` and the SAPI 5 empty-output batch
recovery. For a few dozen descriptions, one at a time is fast enough and is Photo Scribe's proven shape:
one background thread, results posted to the window with `PostMessageW`, the window owning all state.
Parallelism here buys seconds and costs a class of bug that is miserable to diagnose.

**Partly superseded (log entry 31).** The user asked whether describing could go faster. Speech has
been fetched over 6 connections since log entry 29, and description requests now go 4 at once, on by
default with an Options choice to go one at a time. The window still owns all state and one job
thread posts to it; the job thread starts scoped workers for the requests and joins them. The first
request goes alone; each later one is told only the labels found so far, not the earlier text, and
one text-only request afterwards makes sure each person has one label. Wedding copy: 1 min 45 s;
Sprite Fright: 6 min 34 s one at a time, 2 min 26 s four at once; same cost.

### 5.6 A checkpoint, but not SonarPad's resume UI

The first draft dropped the checkpoint entirely, on the grounds that family videos are minutes long and
cheap to redo. **That was wrong, and it is worth saying why**: a two-hour wedding video is a family video,
and so is a feature film the user happens to own. "Family video" bounds the content, not the duration. The
justification was elegance, not need, which is the wrong reason to remove a safety net.

So the shape is:

- **Keep** the `.json` sidecar — cheap, and it is what lets you fix a word without paying to re-describe.
- **Keep** a minimal mid-job checkpoint: append the descriptions and glossary after each chunk, which is a
  few lines and costs one file write per chunk. If a 40-minute job dies at minute 38, nothing is lost.
- **Drop** SonarPad's resume *window* and its `AudioDescriptionResumeSettings` machinery. On finding a
  checkpoint, offer one plain choice — carry on, or start again — and nothing more.

The cost of the checkpoint is trivial; the cost of not having it is a user watching 38 minutes of work
evaporate. Keep it.

### 5.7 The people catalogue, promoted to a headline feature

`[proven]` SonarPad already has this and it is nearly invisible in its docs: named catalogues of characters
saved as JSON in a `Catalogs` folder, loaded back in as `initial_character_glossary` (capped at 96 people,
descriptions merged by token-coverage so they accumulate rather than overwrite).

For family video this is the single most valuable thing in the whole design. Describe one video, confirm that
the woman in the blue coat is Mum, and **every later video gets her name** — across years of footage, with no
re-identification. It turns "a woman lifts a child onto a swing" into "Mum lifts Sophie onto the swing",
which is the entire difference between a usable family archive and a stranger's home movies.

It is also the same problem Photo Scribe already has a spike for (`faces-spike/`, PLAN.md §5c).

**Superseded 2026-10-25: Photo Scribe got there first, and its format wins.** The first draft proposed a
new neutral folder, `%APPDATA%\Scribe\People\*.txt`, with `=== Name ===` headings. Photo Scribe has since
built its catalogue (its `people.rs`, log entry 7), and `[proven]` from its source:

- `%APPDATA%\Photo Scribe\people.txt` — an intro paragraph, then one `## 3: Mum` heading per person (a
  stable number, then the name) and free-text notes under it. Safe to edit in Notepad while Photo Scribe is
  closed. The number is the identity, so a rename keeps the person — the first draft's "the name *is* the
  identity" is not what shipped, and the number is better.
- `faces.bin` beside it — face fingerprints from two ONNX face models run locally through `tract`, each
  linked to a person number. Biometric data; never leaves the laptop.
- The notes are relationships and context ("Sam's brother"), sent with every photo the person is in.
  They are **not** physical descriptions, because Photo Scribe identifies people by face, not by
  appearance text.

**Settled (log entry 2, Q7): read Photo Scribe's catalogue, and keep the two apps independent.** The
user's condition is that the apps ship separately and the synergy is optional. So:

1. Video Scribe has its own `%APPDATA%\Video Scribe\people.txt` **in exactly Photo Scribe's format**, and
   works fully without Photo Scribe installed.
2. If Photo Scribe's `people.txt` exists, Video Scribe offers once to use it instead, so there is one list
   of the family. A setting records the choice. Nothing in Photo Scribe has to change for Video Scribe to
   read it.
3. **Local face recognition from `faces.bin`, with the same models, is the strong version** and comes as a
   spike after the pipeline works (milestone 10). Recognising Mum by her face on this computer, then
   telling the describer "Mum is on the left", beats asking an AI to match a written description — and it
   sends no names or faces anywhere it was not already going.

`[untested]` Two problems to solve at milestone 5, written down now so they are not discovered later:

- **Photo Scribe loads `people.txt` once and rewrites the whole file on save** (`People::save`, after a
  one-time `load` into a static). So if Video Scribe adds a person while Photo Scribe is open, Photo
  Scribe's next save silently deletes it. Atomic temp-then-rename writes, which both apps use, stop a torn
  file but not this lost update. Either Video Scribe only *reads* the shared file and keeps its additions in
  its own, or Photo Scribe learns to re-read and merge before saving. The first needs no change to Photo
  Scribe and is the default.
- **Without face recognition, names alone do not identify anyone.** Photo Scribe's notes say who someone
  is, not what they look like, so the describer needs appearance notes from somewhere. Those belong in
  Video Scribe's own file, keyed by Photo Scribe's person number, not added to Photo Scribe's notes — which
  Photo Scribe sends with every photo.

The text-over-JSON argument of the first draft still holds and is why Photo Scribe's format is the right
one to share: a heading and a free body round-trip verbatim, so an older app never drops what a newer one
wrote. Milestone 5's round-trip test checks exactly that against Photo Scribe's parser.

### 5.8 A rule for shipped files, so it does not slide

If the `tract` route in §4.1 works we ship nothing extra and this rule costs us nothing. It is written down
anyway, because the moment one binary is conceded the next is easier:

> A shipped binary needs a **capability** argument, not a convenience one. If Windows genuinely cannot do
> the thing, a dependency may ship. If Windows can do it and the dependency is merely nicer to program
> against, it may not.

By that rule: FFmpeg does not ship, because Media Foundation decodes and encodes media. ONNX Runtime ships
only if no pure-Rust route can run the VAD — which is a measurement, not an opinion, and §4.1 says how to
take it. The rule exists to keep §4.3 honest on the day Media Foundation turns out to be more work than
FFmpeg, which it will be.

### 5.9 Privacy, which family video forces into the design

Family video is not YouTube. Describing it means uploading the family to someone's API, and that deserves to
be an explicit, visible choice rather than a setting buried three dialogs down.

What falls out of that:

- The VAD runs locally, so **the audio never leaves the machine** — only the frames or clips needed for the
  gaps, which is a small fraction of the footage. Say so in the UI; it is genuinely reassuring and true.
- Reuse Photo Scribe's `send_names` flag: folder and file names are often the most identifying thing about a
  family video (`Sophie 3rd birthday.mp4`), and they should not go to a service unless asked.
- Keep Photo Scribe's no-data-collection provider filter for the OpenRouter path.
- Show, before the job starts, which service will be used and what will be sent to it. For an archive of
  one's own family this is the difference between a tool you trust and one you use once.

### 5.10 Extended pauses and output format are one choice, not two

Settled: extended pauses are a **per-job toggle, default on**, and the toggle governs whether video output
is offered. That follows SonarPad, which refuses video export outright when any pause is present, because
pausing the audio would desynchronise a copied video stream (§2.8 step 6).

So the two controls are mutually constrained, and the UI has to make that obvious rather than discovering it
at the moment the user presses Create. `[untested]` **Do not solve this by disabling a control.** A greyed
checkbox with no explanation is a dead end with a screen reader. Better: leave both always enabled and
always meaningful, and have each announce the consequence of changing it —

- ticking **Allow extended pauses** switches **Output** to described audio, and says so;
- choosing **described video** unticks **Allow extended pauses**, and says so.

Every control always does something, nothing is greyed, and the trade-off is spoken at the moment it is
made. This is exactly what the `accessible-windows-apps` probe is for, so settle the wording at milestone 2
(the main window) and verify it before asking for a test round.

**One tension to keep an eye on.** Default-on means video output is not offered by default — yet family
video is precisely where you would want a described file to send to relatives. If that turns out to be the
common case in practice, the honest fix is to revisit the default at milestone 9 with real use behind us,
not to invent a per-source default now and hide behaviour the user did not ask for.

### 5.11 When gaps are scarce — a setting, not a hard-wired answer

Settled (log entry 1, Q1): the answer to §1's worry is a user preference, with spike 0(a)'s numbers
choosing its default rather than anyone's guess. Three strategies:

| Strategy | What you hear | Cost |
|---|---|---|
| **Pause to make room** | the film stops, the description plays, the film resumes | longer output; no video export (§5.10) |
| **Describe over the audio** | the film ducked harder under the description | speech is talked over, briefly |
| **Only the gaps there are** | sparse descriptions, original timing | coverage may be close to none |

The user's answer adds the important part: **pair "pause" with the short description style.** SonarPad
already has a `short | standard | detailed` verbosity (§2.3, `[proven]`), and short descriptions mean
short pauses. So the setting should probably be offered as "when gaps are scarce: pause, using short
descriptions" rather than two independent controls — decided with the probe at milestone 2.

`[untested]` **Milestone 2 offers §5.10 and this setting as one choice** (log entry 17): "When a
description does not fit its pause: pause the film / speak it over the film / leave it out", in the
Describe menu, default pause. Pausing to make room *is* the extended pause, so two controls could
contradict each other. Choosing video output moves pausing to "speak it over the film", and choosing
pause moves the output back to audio, each said aloud. Awaiting test-milestone-2.txt.

`[untested]` Whether one setting covers both corpora, or whether YouTube (gappy, scripted) and family
video (continuous chatter) want different defaults. That is what §5.12's presets are for.

### 5.12 Presets

Feature request (log entry 1). A preset is a named bundle of the job settings — description service and
model, voice and TTS engine, language, description style, scarce-gaps strategy, extended pauses and output
format — so that "French family videos" or "Afrikaans YouTube" is one choice rather than seven.

Shape, following the house style: one plain-text file per preset in `%APPDATA%\Video Scribe\Presets\`,
in the same `key = value` form as `settings.txt`, safe to edit in Notepad. A preset is a starting point,
not a lock: choosing one fills the controls, and changing a control afterwards is announced as "changed
from preset". Comes once the main window and at least two services and voices exist (milestone 9), because
a preset of one service and one voice is nothing.

`[untested]` **Built in milestone 9** (log entry 39): Options, Presets, with "Save these choices as a
preset..." (a window with one name box), each preset (the one in use ticked), and "Show the presets
folder in File Explorer". Choosing one says which choices it changed; changing one afterwards says
"No longer the ... preset". A line taken out of a preset file leaves that choice alone. The output
folder, keeping downloads and sending the file name are never in a preset. Probe clean; awaiting
`test-milestone-9.txt`.

## 6. Proposed architecture

- Rust, edition 2021, `windows` crate 0.62, rc.exe dialog template through `embed-resource` —
  same as StableSound and Photo Scribe. One ordinary window with a taskbar button.
- Crates: `windows`, `serde_json`, `embed-resource` (build only), plus **`tract-onnx`** for the VAD, with
  `ort` as the fallback (§4.1). Everything else is Windows: WinHTTP for HTTPS **and WebSockets** (§4.4),
  Media Foundation for decode/encode and for MP3, SAPI 5 for offline speech, CNG for SHA-256,
  Credential Manager for keys.
  `[proven]` Photo Scribe has since adopted `tract-onnx` 0.21 for its two face models and measured it at
  about 5 MB in the exe, against a 15 MB ONNX Runtime DLL (its `Cargo.toml`, log entries 6 and 7). So the
  dependency is already accepted in the house style; spike 0(b) only has to show Silero's graph runs on it.
- Ships as `videoscribe.exe` with the 1.3 MB model embedded via `include_bytes!`, and nothing else — if
  `tract` works. Otherwise add `onnxruntime.dll`. §5.8 is the rule that decides.
  `[proven]` Milestone 3: one file. The release exe is 7.5 MB with tract and the 1.2 MB sequence model
  (`models/`, committed, pinned in its README), against 192 KB without them. It finds the speech in a
  10.5-minute video in about 4 seconds.
  The `windows` features Photo Scribe already has need `Win32_Media_MediaFoundation`,
  `Win32_Media_Speech` and `Win32_Security_Cryptography` adding; `Win32_Networking_WinHttp` is there.
- Modules, mapping onto what Photo Scribe already proves:

  | Module | Does | Reuse |
  |---|---|---|
  | `ui.rs` | window, controls, progress, hide-controls-while-working | Photo Scribe pattern |
  | `worker.rs` | one job on a background thread, `PostMessageW` events | near-verbatim |
  | `net.rs` | WinHTTP requests **and the WebSocket upgrade** | extend Photo Scribe's |
  | `credential.rs` | Gemini and OpenRouter keys in Credential Manager | extend (two targets) |
  | `ytdlp.rs` | find/fetch yt-dlp, download, progress template, captions | new (SonarPad's logic) |
  | `media.rs` | MF decode to PCM, frame grab, MP3 decode, MF encode | new |
  | `speech.rs` | the `SpeechSource` trait, union, inversion to gaps (§5.3) | new |
  | `speech_captions.rs` | yt-dlp subs, embedded track, sidecar `.srt` | new |
  | `speech_vad.rs` | Silero VAD through `tract-onnx`, hysteresis to intervals | new |
  | `timing.rs` | gaps → long-gap parts → word budgets | new (Omni's 2 words/s rule, MIT) |
  | `describe.rs` | the `Describer` trait and `Evidence` (§4.2) | extend Photo Scribe's |
  | `describe_gemini.rs` | inline-or-Files upload, ACTIVE polling, quota protocol | new |
  | `describe_claude.rs` | `claude -p` with frames | near-verbatim from Photo Scribe |
  | `describe_openrouter.rs` | one HTTPS request with frames | near-verbatim from Photo Scribe |
  | `glossary.rs` | identities across gaps, and the people catalogue (§5.7) | new (Omni's JSON shape, MIT) |
  | `prompts.rs` | description prompt, glossary, verbosity, language, word budget | extend Photo Scribe's |
  | `speak.rs` | the `Voice` trait, exact measured duration | new |
  | `speak_edge.rs` | §2.9 over WinHTTP WebSockets, GEC token, `turn.end` rule | new |
  | `speak_sapi.rs` | SAPI 5 through COM to a stream | new |
  | `schedule.rs` | synthesise → measure → place → reserve → drop/pause | new (our own; method learned from SonarPad, §2.1) |
  | `mix.rs` | duck, fade, insert pauses, encode | new |
  | `project.rs` | `.json` sidecar for re-editing, and the per-chunk checkpoint (§5.6) | new (SonarPad's shape) |
  | `presets.rs` | named bundles of settings (§5.12) | new |

- Output: one audio file (MP3 or M4A) plus a sidecar `.txt`/`.srt` of the descriptions with timings.
  A described **video** file is offered too, but only when extended pauses are off (§5.10), and it arrives
  at milestone 9 rather than with the first working pipeline.
- Pipeline order is non-negotiable and comes straight from §2.8: **synthesise, measure, then schedule.**
  Estimating speech duration from word count and placing before synthesis is the trap.

---

## 7. Decisions

**Settled (2026-10-23):**

1. **Use cases** — YouTube *and* family video with no captions. The second one drives most of what follows.
2. **Description services** — all three: Gemini, Claude Code, OpenRouter. Gemini goes in first because
   SonarPad proves it works, and becomes the quality bar. The shared prompt, glossary and word budget are
   factored out from the start; the `Describer` trait is **extracted at milestone 6**, not designed up
   front (§4.2).
3. **TTS** — Edge TTS as in SonarPad, plus SAPI 5 as the offline fallback, behind the `Voice` trait of §4.4.
   Over WinHTTP WebSockets, so no new crates.
4. **Dialogue timing** — Silero VAD in-process from 1.0, with caption cues as a second source on YouTube.
   Pure Rust first (`tract-onnx`), `ort` and a shipped `onnxruntime.dll` only if that fails (§4.1).
   The merge strategy for captions and VAD is decided by spike 0(a), not here.
5. **Extended pauses** — a per-job toggle, and the toggle governs whether video output is
   offered (§5.10). Default on until log entry 39, Q1; since then speaking over the film is the
   default for new users ("Yes, would make sense"), so a described video works from the start. The UI must make the constraint audible rather than greying a control out; wording to
   be settled and probe-verified at milestone 2.
6. ~~The people catalogue is a new shared `%APPDATA%\Scribe\People\*.txt`.~~ Superseded by 10.

**Settled (2026-10-25, log entries 1 and 2):**

7. **Scarce gaps** — a user setting with three strategies, paired with the short description style;
   spike 0(a) picks the default (§5.11).
8. **Walking skeleton before the main window** — confirmed. Hearing a described clip early is the proof of
   concept.
9. **Presets** — named bundles of job settings, milestone 9 (§5.12).
10. **People catalogue** — Video Scribe keeps its own `people.txt` in Photo Scribe's exact format, and
    offers to read Photo Scribe's when it exists. The apps ship separately and the link is optional. Local
    face recognition from `faces.bin` is a later spike (§5.7).
11. **Repository** — public on GitHub from 2 November 2026, pre-release, so a collaborator can join
    (see `ORIENTATION.md`). It started with a fresh history: the development diary (`log.md`) and the
    manual test files hold details of the user's family videos, so they stay in the private repository
    where the first 96 commits live. "Log entry N" in this plan and in code comments points there.
12. **Bake-off budget** — at most US$2 in total. The user's Gemini key is free tier, where Google may use
    what is sent to improve its products, so family-video material goes only to the services whose terms
    allow it (OpenRouter with the no-data-collection filter, Ollama's cloud, Claude Code) unless the user
    says otherwise.

**Settled (2026-10-25, log entries 3 to 6):**

13. **Cost** — show an estimate of money and time before each job and proceed, stopping to ask only when the
    estimate exceeds a user-set limit, default US$1 (entry 3, Q9).
14. **No paid Google tier for now** (entry 3, Q10). Gemini reaches family video through OpenRouter, which
    sells the same models at Google's prices, accepts base64 video, and with `data_collection = deny`
    routes only to non-training endpoints. `[untested]` whether Gemini hears the audio track via
    OpenRouter, and OpenRouter's request-size ceiling for video — both spike 0(h).

15. **OpenRouter is the describing route at first** (log entry 6, Q11), with Ollama and Claude Code as in
    Photo Scribe. Gemini is reached through OpenRouter, so milestone 4 needs no Files-API upload, ACTIVE
    polling or Google quota protocol. The direct Gemini API's free tier comes later, for YouTube only; the
    user wants good free options eventually.
16. **Our own prompt, not SonarPad's** (log entries 8 and 9, Q12). SonarPad's prompt is read for
    principles only and restated in our words (`spike\prompts\describe.v1.txt`). Talking to its author
    about working together is for later. A name the model hears on the soundtrack is marked "heard" and
    never enters the people catalogue without asking the user.

17. **Edge's v2 `Sec-MS-GEC` token** (log entries 12 and 15, Q13 option 2). Edge refused SonarPad's v1
    token from October 2026; our own implementation of the v2 token, from the constants published by
    edge-tts (LGPL-3.0, credited, no code copied), works on the legacy route with the Edge 143 version
    string. Expect it to break again: SAPI stays the fallback, and the token code is one small file.
18. **Still frames, not video, as the default evidence** (log entries 14 and 15). Across two rounds,
    Gemini given the clip as video placed descriptions at the wrong moments, the second time describing
    footage from a minute earlier; services given three frames per gap stay in place. Services that
    cannot hear get the caption dialogue around each gap as private context, which gives them names and
    the story. Prompt version 2 (`spike\prompts\describe.v2.txt`) is dense and narrative, sets the scene
    first, keeps one appearance-based label per character across requests, and has a text-only "shorten"
    request for anything over its word limit.

**Settled (2026-10-26, log entry 16, the second blind listening test):**

19. **Default service: GLM 5.3 Flash through OpenRouter; Claude Code as the subscription option.** No
    service won on every video: Claude was best on the film, Gemma on the short phone video, GLM on
    the wedding ("nearly professional-grade"). Every service scored 2 of 2 for truth on both family
    videos. GLM ranked best overall (first, second, second), is the fastest (about 1 to 1.5 minutes of
    waiting for 3 to 5 minutes of video) and costs about US$0.08 an hour of video. Gemma is dropped
    from the defaults: slowest by four to seven times, and prone to padding a reply until it is cut
    off. Any OpenRouter model can still be chosen by name.
20. **Sound defaults kept:** Edge's Sonia (en-GB), 10% faster than normal, speeding up to 40% more to
    fit a long description; the voice at −16 dBFS RMS; the film ducked 12 dB under it. The user found
    the volume, ducking and speed right, with room to go a little faster. Voice and rate are user
    settings at milestone 2, and the voice list follows the description language.
21. **The people catalogue is wanted** (milestone 5): without names, the user could not tell who some
    of the people in the wedding were.

**Settled (2026-10-31, log entries 32 and 33, the milestone 8 test):**

22. **Four requests at once stays the default.** In the user's comparison of Sprite Fright, the
    version written four at once "appeared to be more accurate"; one at a time stays in Options.
23. **GLM stays the default for family video, over Gemini's ear.** Gemini rightly left the start of
    the vows empty, but also left places 1 and 2 empty with no speech in them; the user found it "too
    sensitive" and, "on balance", prefers GLM.
24. **Edge's Sonia stays the voice.** Of Grok, Orpheus, Kokoro and Gemini Flash Lite TTS through
    OpenRouter, none sounded better; Gemini's was "nearly as good" and Kokoro's "pleasant". Groq has
    no speech through OpenRouter. A local Kokoro is milestone 11, at the end.
25. **Looks are lasting features first** (test-milestone-8.txt, 2.2): "A blind person is more likely
    to know about the more permanent characteristics of the person, which could include height, body
    type, facial features, hair and eye colour, race, age." Prompt version 8.
26. **The Listen window** (log entry 33): arrows move 2 s, and say the time only when paused ("too
    much audio clutter" while playing); T says it on request; it opens playing; Review opens it at
    a place.
27. **Standard stays the default style** for now (log entry 35, Q1), though the user found
    Detailed the best on the wedding and will experiment further.
28. **Claude knows people better than GLM** (round 5 test, 2.5 and 3.1): with the names sent,
    Claude Sonnet named them "much better"; GLM was hit and miss. GLM stays the default ("Back to
    GLM for now, but will experiment more with Claude").
29. **Nothing is made again when nothing changed** (round 5 test, 2.6): the voice, speed, sound
    and output choices that made a result are kept with it, and the same again makes nothing.
30. **A preset never holds the output folder or sending the file name** (log entry 39): sending
    names stays a choice made on its own (§5.9), not one a preset makes in passing.
31. **Sharper web pictures are a choice, small by default** (log entry 39, Q2: "Keep the default to
    the smallest videos, but give the user a choice"): 480, 720 or 1080 lines, used only when the
    output is described video.
32. **No "Start?" question** (test-milestone-10.txt): finding the speech sends nothing, so Start
    goes straight to it; the question before anything is sent stays.
33. **A folder for each video, named after its cleaned title** (log entry 40, Q2: "Yes, please
    clean up the titles ... put all files pertaining to a particular video in a folder named after
    the cleaned up title and implementing a viewer with bulk-renaming functionality ... See the
    NoteFetch app"). A marker file in the folder names the video, so the folder is found by that,
    not by its name. Other sites keep the film rules (entry 40, Q1: "Allright for now").
34. **The main window is the list of videos** (log entry 41, Q4: "I'd like to change the design of
    the programme so that it opens in this window. An add video button, ctrl+a, will then open the
    current default window"; test round 2, 3.11: a People button, an Add video button, no Close,
    and "a context menu for videos in the list -- see NoteFetch"). Every command for a video works
    on the one chosen in the list; the Video box and the summary moved to the Add a video window.
    Earlier loose results were moved into folders by hand, not by the app (test round 2, 2.3:
    "there should be no reason to code that capability into the app itself"), and file names stay
    as they are (entry 41, Q3).
35. **Faces are tagged, never named, in what is sent** (log entry 42, Q2: "Right for now, we'll
    experiment"). In a family video, faces are found and grouped on this computer; each person is
    "Person n" beside the pictures, the describer says which tag each of its labels has, and a
    name is put in on this computer when the face is known. Photo Scribe's people and faces are
    read, never written (Q1: "Yes"); Video Scribe keeps its own `people.txt` and `faces.bin`, and
    naming someone teaches every later video (Q3: "Yes to both"); Photo Scribe's models are used
    when found, else the user is asked (Q4). A tag counts only when one label alone claims it
    (entry 44). Videos described before faces keep their descriptions.
36. **Add a video is Ctrl+N, not Ctrl+A** (test round 3, 3.4): JAWS's own Ctrl+A script reads a
    list's selected item after the key, so it seemed to be in the empty Add a video window (log
    entry 45). Ctrl+O works in that window too (round 3, 3.6).
37. **Described files carry tags** (round 3, section 5: "Please also implement propper tagging of
    the mp3 files"): title, album, genre, year, a comment of what made it, the list as lyrics,
    through the Windows property system (`tags.rs`), for the MP3 and the MP4.
38. **Categories keep uncategorised videos at the top of the output folder** (log entry 45, Q2:
    "To provide an easier on-ramp for new users, keep uncategorised files in the root of the output
    folder, as they are now"). A category is a folder there holding its videos' folders; a
    described file's album tag is its category (Q3: "Yes").
39. **Languages and voices come before categories** (log entry 46, Q1: "Yes"), French first,
    then Afrikaans (Q2: "French, then Afrikaans (I speak both)"). A family video's tags keep the
    descriptions and the file name for now (Q3). A long-video test on a public film is allowed
    (Q4), and a short blind comparison of prompt versions now and then (Q5: "Ok, sounds good").

Everything else is a spike question in §8.

---

## 8. Draft milestones

Two test corpora from the start, because they fail differently: three YouTube videos (one with real
captions, one auto-only, one with none) and three of your own family videos (one phone MP4, one older
camcorder file, one with music over most of it).

0. **Spikes**, in `spike/` as with the other two apps. Ordered so the two things that could invalidate the
   plan come first — except that 0(b) runs before 0(a), because measuring gap density needs a VAD.
   Family videos live in `corpus/`, which git ignores; the YouTube three are chosen by Claude (entry 2, Q5).
   - (a) **Gap density, and the merge strategy.** Run a VAD over all six videos and print the gaps:
     how many, how long, how much of the running time is protected. This answers §1's premise before a
     line of app code exists. On the YouTube video with real captions, compare VAD intervals against
     caption cues and try all three merge strategies from §4.1. **If the family videos have no usable
     gaps, stop and rethink the product, not the parameters.**
   - (b) **Pure-Rust VAD.** Can `tract-onnx` load and run `silero_vad_16k_op15.onnx`? Confirm Silero's real
     defaults from its own source, and record the release-build size and dependency count against the `ort`
     fallback (§4.1). Decides whether we ship one file or two.
   - (c) **Read the audio description guidance** (§3.1): the Ofcom/ITC standard and Omni Describer's
     prompts. Write the first prompt and the §3.4 rubric. Not code; a morning's reading that shapes
     everything downstream.
   - (d) yt-dlp from Rust: a single muxed MP4 and `--write-auto-subs`, `--progress-template` parsed,
     cancel working.
   - (e) Media Foundation: decode both corpora to PCM, grab a frame at a timestamp as JPEG, decode a
     24 kHz mono MP3, encode PCM back to MP3. Include an HEVC phone video — that is where the Store
     extension question bites. If this fails, §9 says what happens.
   - (f) **Edge TTS over WinHTTP WebSockets** — the upgrade handshake with the custom `Origin` and
     `User-Agent`, the CNG SHA-256 `Sec-MS-GEC` token, binary frame reassembly, and the `turn.end` discard
     rule with the four tests of §2.9 ported. Measure durations against the audible result.
   - (g) SAPI 5 rendered to a stream, measured the same way.
   - (h) **Bake-off**, scored against the §3.4 rubric, on one 3-minute clip from each corpus: Gemini
     whole-file vs Claude Code on three frames per gap vs an OpenRouter model on the same frames.
     Record money and wall time per minute of video, which is what §9's missing cost model needs.
     Decides frames-per-gap and the default service — and the answer may differ between the two corpora.
1. **Walking skeleton.** One hardcoded video, end to end, in one ugly command-line binary: gaps → one
   description → one TTS clip → one mixed MP3. No UI, no settings, no services but one. The point is to
   hear a described clip in week one rather than at milestone 7, because that is the only thing that tells
   us whether any of this is worth building. Throw the binary away afterwards; keep what it taught us.
   **Done (2026-10-26)**, as `spike skeleton`, after two blind listening rounds (log entries 14 to 16).
   What it taught us is §7 decisions 17 to 21 and prompt version 3 (`spike\prompts\describe.v3.txt`).
2. Main window: URL or file in, output folder, service, voice, verbosity, language, output format and the
   extended-pause toggle with its spoken constraint (§5.10); what-gets-sent shown before the job starts
   (§5.9). Probe-verified, then test round 1.
3. Source and speech map: `resolve_source` (§5.2), captions and VAD behind `SpeechSource` (§5.3), the merge
   strategy chosen in 0(a), gaps, intensive slots — printed as a report so the timing can be checked
   before spending a penny on AI.
   **Built (2026-10-26)** on branch `m3-speech-map`, awaiting `test-milestone-3.txt` (log entry 18).
   After round 1 (log entry 19) it also writes a **pause check file**: the video's own sound with a high
   beep where each pause starts and a low one where it ends, the user's own idea for checking the map by
   ear. The Describe and Voice menus became one Options menu, with Describe keeping Start and Stop.
   There are no intensive slots: silence is allowed (§3.1), so no pause is marked as one that must be
   described.
4. Describe, Gemini concretely and without a trait (§4.2), **through OpenRouter** (§7 decision 15): base64
   video clips, retries, the prompt and glossary from 0(c), long gaps split into ~6–10 s stretches (§4.1),
   the chunk checkpoint (§5.6). The direct Gemini API and its free tier come later, for YouTube.
   **First half built (2026-10-26)** on branch `m4-describe` (log entry 20): GLM 5.3 Flash on still
   frames through OpenRouter, as round 2 made it the default, with the checkpoint. The user is asked
   between finding the pauses and sending anything, with the exact count of pictures (§5.9).
   **Second half built (2026-10-26)** (log entry 21): Gemini 3.5 Flash-Lite through OpenRouter on
   short MP4 clips with sound, made by Media Foundation. Only the pause itself is clipped, never the
   speech around it (constraint 7); touching places share a clip, at most 30 s. Wedding copy: 15 clips,
   US$0.02, plainer wording than GLM. Settled (log entry 28): Gemini flags pauses where it heard speech,
   and they are left empty (log entry 21, Q1); to build. Also File, Update yt-dlp.
   **After the user's test (log entry 23):** GLM stays the default. Prompt version 4 stops talky
   stretches being filled from the captions, and an **editor pass** replaces the shortening request:
   the whole draft, text only, 20 places a request, cut to the word limits (each long line is given
   its word count), labels kept steady, dialogue and repetition removed, never lengthened; a failed
   edit keeps the draft. The question before describing counts only what a stopped job still has to
   send. Later, after milestone 7: a **critic and researcher** pass, off by default (log entry 22,
   Q2): a text-only critic names doubtful places, and only their pictures or clips are sent again.
5. **The people catalogue** (§5.7): confirm or correct who someone is, write Video Scribe's own
   `people.txt` atomically in Photo Scribe's format, offer Photo Scribe's when it exists, and load it into
   the next video as the initial glossary. Do this early — it is the feature the family archive is for,
   and it shapes the prompt. Include a round-trip test that an unrecognised line in a section body
   survives being read and rewritten, by our parser and by a copy of Photo Scribe's.
   **Settled (log entry 28):** first, names for one video at a time: a name for each label the
   descriptions used, put in locally before speaking, and a note on who is in the video, sent
   before describing. Names may be sent (Q3). Heard names are only offered (Q4). Face recognition
   (§5.7, point 3) is the goal; looks notes carried between videos only if wanted meanwhile.
   **Built (log entry 30, `m5-people`):** People in this video, Ctrl+P; one text file per video in
   `%LOCALAPPDATA%\Video Scribe\people`; names replace the label with up to two describing words
   before speaking. Not yet: the cross-video catalogue in Photo Scribe's format.
   **Round 2 (log entry 31, `m5-round2`):** everyone the descriptions single out gets a label, with
   their looks; the People window shows how the chosen label's person looks and every description
   that mentions them. Prompt version 7. Short pauses follow the fit choice, as an Options choice.
6. Claude Code and OpenRouter on frames, sharing prompt, glossary and word budget with Gemini. **Extract
   the `Describer` trait now**, from two working implementations rather than from a guess (§4.2).
   **Settled (log entry 28):** wanted as an option, built after milestones 5 and 8. The user has
   the Pro plan; Sonnet is the default, Opus and Haiku in Options, Model. Family pictures may go
   to Anthropic under the user's own claude.ai privacy setting; the question before each job
   names Claude Code as where they go.
   **Built (log entry 34, `m6-claude`), awaiting the user's test:** `claude.rs`, ported from Photo
   Scribe: `claude -p` with no tools, hooks or MCP servers, an empty folder per request, the system
   prompt in a file, and a refusal unless `claude auth status` says `claude.ai`. Still pictures
   only. `describe::Route` (OpenRouter or Claude) is all the abstraction two services needed, so
   no `Describer` trait. Without a schema, Claude sometimes numbered gaps from 1 or not at all;
   those replies are placed by order, or by the time each was seen at. Sprite Fright with Sonnet:
   33 requests, 3 minutes.
   **Tested (test-milestone-6.txt):** all worked, with no long empty stretches. "Claude descriptions
   were very good. Use of language better than GLM, so close tie between the two." Changing the
   model describes afresh, as it should. Not yet tried: Opus (the user will), Haiku, the usage
   limit message.
7. Speak: Edge TTS, then SAPI 5, both measured; tail-silence trim; the `turn.end` rule enforced.
   Then schedule and mix: synthesise, measure, place, reserve, drop or pause; duck, fade, pauses, encode;
   the description sidecar. Print the schedule before mixing.
   **The Language and voice dialog** (test round 1, 2.6 and 4.5) goes in here, in the Options menu: a
   type-ahead list of languages built from what the voices speak (Edge's own voice list), then that
   language's voices, each playing a short sample at the chosen speed as it is reached. The eight
   languages in the menu today are a stopgap. **Keeping YouTube downloads** (log entry 19, Q1): delete
   once the described file is made (the default), or keep them for 24 hours.
   **Built overnight on `m7-speak` (log entry 27), awaiting the user's test:** Edge with SAPI fallback;
   each description tried at its speed, then 10% and 15% faster; then the fit choice (pause the film,
   speak over it, leave it out); duck 0.25 with 0.15 s fades; a stereo 48 kHz MP3 and a text list of
   what is said when. A finished job's descriptions are kept (checkpoint plus its editor pass), so
   making it again with another voice or fit choice sends nothing; a second result gets a number.
   Prompt version 5: the editor marks CUT only what will not fit at 15% faster, and keeps whole
   sentences. Not built yet: the Language and voice dialog, described video. Untested: the SAPI
   fallback, videos much longer than 10 minutes.
   **After the user's test (log entry 29):** speech is fetched from Edge over 6 connections at once,
   and only too-long descriptions are fetched faster (phone copy: 80 s to 26 s); a description that
   still does not fit may spill 0.2 s before and 0.4 s after its pause; 3 words a second instead of
   2.5; a 15% speed step, the default. Four places a request instead of eight, because with eight GLM
   invented a stretch of the wedding. Prompt version 6: who leads a ceremony, each label carries its
   first looks, the editor never guesses a label, and a reaction comes with its cause. "Video
   downloads", not "YouTube downloads".
8. **Review the result** (§9): the descriptions as a list with timings, jump to one and hear it in context,
   see which were dropped and why. AI descriptions will sometimes be wrong, and a blind user cannot scrub a
   waveform to find out. This is not a nice-to-have.
   **Built (log entry 31, `m8-review`), awaiting the user's test:** Review the descriptions, Ctrl+R.
   A list of every place with its time in the described sound and what is said, or why nothing is;
   play the place in the described sound (2 s before to 1 s after) or the same stretch of the film's
   own sound; change or empty the text, or write some for an empty place; restore. Changes are kept in
   the checkpoint and spoken next time the sound is made, which sends nothing. Playing decodes with
   Media Foundation and plays from memory with PlaySound: no file, no new dependency. Not yet: hearing
   a changed line before making the sound again.
   **After the user's test (log entry 33, `m8-round2`):** the review worked as built, 2 s before and
   1 s after is right. Added: **Hear the text in the voice**, which speaks the text box in the chosen
   voice and speed and then says whether it fits its pause (entry 31, Q2); and **Listen to the
   described sound** (Ctrl+L), the user's request: the whole described sound with Play or pause, Back
   and Forward 10 s, Left and Right arrows 5 s, Home to the start, every move said out loud. It
   streams: Media Foundation decodes from where playing starts and waveOut plays a tenth of a second
   at a time, so a long film is never held in memory. From the same test: names put in for labels
   the descriptions did not use word for word ("a bald man" for "a bald man in a brown suit") and
   prompt version 8 (lasting looks, labels word for word, a new place said), the speech detector
   started afresh each batch (§4.1), and the label chosen by the LABELS step kept when it is a role.
   **Round 2 test (log entry 35, `m8-round3`):** all worked; the vows are left clear, new places and
   the first dance are said, and Detailed gave "the best description so far". Changed: the
   arrows move 2 s, silently while playing, and T says where playing is; Listen keeps opening
   playing; **Listen on from here** in Review. A critical bug: no sound on the user's Bluetooth
   earphones. Overnight silent runs had set the app's waveOut volume to nought, which Windows keeps
   for each app and device; tests now write silence instead, and a volume at nought is put back.
   People: an unused label starts with "Old label"; every look noted is shown, not only the first,
   and prompt version 9 asks later requests for the lasting looks a first note lacks, never for
   what someone is doing. Labels whose looks name each other ("holding the bride's train") are
   never joined as one person.
   **Round 3 test (log entry 36):** all worked, sound included on the round 3 build. Changed:
   Alt+F and Alt+B are quiet while playing too; Listen opened from Review says no time, and
   Enter in it goes back to Review on the description just spoken; People lists old labels last
   and reads a label's descriptions from the top.
   **Round 4 (log entries 37 and 38, `m8-known`):** Listen on from here starts 1 s before; skips
   say only the time. Built at the user's request: **Describe again knowing these people**
   (Ctrl+Shift+D). The names typed in People go with the request, each with its old labels and
   looks, and prompt version 10's KNOWN PEOPLE RULE asks for the old label word for word (names
   are put in locally) and for known people at the moments that matter. What was sent is kept in
   the checkpoint, so Start describing reuses those descriptions. Two GLM runs on the wedding were
   mixed: the first said "on her father's arm" where v9 named him, the second
   named nobody new. Later, not now (entry 37, Q2): a still of a labelled person in People, for a
   sighted helper; video while listening waits for milestone 9.
9. The `.json` sidecar: re-edit a line, re-export without re-describing (done in milestone 8, through
   the checkpoint rather than a separate sidecar). Then video muxing, offered only
   when extended pauses are off (§5.10) — this is what makes a described family video shareable.
   Revisit the default-on decision here, with real use behind us. Then **presets** (§5.12), as a
   submenu of Options.
   **Built (log entry 39, `m9-video`), `[untested]` by the user:** Output, Described video makes
   "name - described.mp4" beside the MP3 with `mux.rs`: Media Foundation's sink writer copies the
   compressed pictures unchanged and encodes the described sound as AAC 192 kbps, interleaved half
   a second ahead. Checked by measurement: the MP4's sound correlates 1.000 with the MP3; the
   wedding's took 6 s; an upright phone video keeps its rotation; Sprite Fright's YouTube pictures
   (at most 480 lines) and sound are in step. The default-on question is put to the user (entry 39,
   Q1), as is a sharper YouTube download for video (Q2). Presets built too (§5.12).
10. Later: other video sites and direct links to video files, through yt-dlp (test round 1, 3.6);
    **`[untested]` by the user, built (log entry 40):** `Source::Web` takes any address; YouTube's
    formats are unchanged; other sites are asked only for plain-HTTP formats (an HLS stream would
    need FFmpeg, §5.8), the first video of a page, and a file with both picture and sound serves
    as both. Files go by the site and a hash of the address. Checked on the Sintel trailer's
    direct link, end to end with a described video. **Round 2, built (log entry 41, `m10-names`),
    tested by the user (test-milestone-10-round-2.txt):** a folder for each video (`library.rs`,
    decision 33), a list of videos to listen, review, rename a folder with its files, show it in
    File Explorer or move it to the Recycle Bin; picture
    quality for described web video (decision 31); speak over by default (decision 5); no Start?
    question (decision 32). Face spike (log entry 42): Photo Scribe's YuNet and SFace in
    `faces.rs`, on Tears of Steel every 4 s, found 112 faces in 184 pictures at 0.47 s a picture,
    and grouped them with no actor mixed into another's group (two actors split in two). The plan
    (tags sent instead of names, Photo Scribe's faces read-only, Video Scribe's own `faces.bin`)
    was agreed (entry 42's answers). **Round 3, built (log entry 43), `[untested]` by the user:**
    the list of videos becomes the main window (decision 34): Add a video (Ctrl+A, `addvideo.rs`),
    People, a context menu, F2 and Delete in the list (`videos.rs`). **Faces, built (log entry
    44), `[untested]` by the user:** `faces::tag` (YuNet and SFace, grouped per video, kept in
    `%LOCALAPPDATA%\Video Scribe\faces`), `known.rs` (Video Scribe's catalogue, Photo Scribe's
    read-only), prompt `faces.v1.txt` outside the describe prompt so old checkpoints stand
    (decision 35). Tried on two scratch copies of Blender's "Charge" making-of: 33 pictures, 11
    tags, 0.5 s a picture in release; recognition worked across the copies, but GLM gave one tag
    to two labels across requests sent at once, hence the one-label rule. The user's own test
    (round 3, section 6): "very accurate", and a person named in one family video was named by
    face in the next. **Round 4, built (log entry 45), `[untested]` by the user:** Ctrl+N, Ctrl+O
    in Add a video, tags (decisions 36, 37). **Next: categories, as in NoteFetch** (round 3,
    section 5: "please add implementing categories in the same way as is done in NoteFetch"):
    category folders in the output folder with an "Uncategorised" one, a Category box above the
    list, Move to category (Ctrl+Shift+M), a Category menu; the design waits on entry 45, Q2.
    Still to come:
    more TTS engines.
11. **Languages and voices** (log entries 46 and 47, decision 39). **Built on `m11-languages`,
    `[untested]` by the user:** the Language and voice window (`voicebox.rs`, Ctrl+Shift+V) over
    Edge's whole list of 322 voices (`voices.rs`, fetched weekly into `voices.json`, the built-in
    32 until then), type-ahead languages and a sample for each voice; names put in for labels in
    French and Afrikaans (articles, "l'", "'n", "du", "au", "de" and "d'"); the film's own
    automatic captions when there are none in the description language; a Windows voice only of
    the description language, else a clear stop (this laptop's Windows voices are all English).
    Measured, Edge on one description translated: English 3.07 words a second, French 3.25,
    Afrikaans 3.26, German 2.73; so the 3-words-a-second rule stands for French and Afrikaans,
    and German would need its own. Then categories (decision 38).
12. **Last, and only if wanted: a local neural voice** (log entry 32, Q2). Kokoro (82M, Apache-2.0)
    has British voices and was "pleasant to listen to" in the user's sample, but no voice tried beat
    Edge's Sonia. `[untested]` on this machine, and doubtful: the development laptop is a 2016
    two-core i5-6300U with 8 GB and no GPU, where Kokoro would likely run near or slower than real
    time (a 10-minute film's 100 descriptions, about 7 minutes of speech, could take as long to
    speak), tract's support for its operators is unknown, and it needs a phonemiser, whose usual
    one, eSpeak NG, is GPL. Spike it before promising anything; Edge stays the default.

---

## 9. Risks and open questions

Ordered worst first.

- `[untested]` **Family video may not have usable gaps, which would invalidate the premise rather than a
  parameter.** Continuous chatter over a birthday party means a VAD protects nearly everything, extended
  pauses cannot find even a 1-second anchor, and the scheduler drops every description — producing an
  almost-undescribed file, silently, by working as designed. Spike 0(a) is the gate. Decided response:
  the scarce-gaps setting of §5.11; 0(a) sets its default and tells us whether that is enough.
- **The cost model is an estimate, not a measurement** (log entry 3, from published prices on 2026-10-25
  and Photo Scribe's measured per-photo costs, assuming ~4 gaps a minute). Money turns out small: Gemini 3.5
  Flash-Lite on whole video (~100 tokens/s at default media resolution, $0.30 in / $2.50 out per M) is
  ~$0.0035 a minute, ~$0.40 for two hours; Gemini 3.8 Flash ~$0.80 (doubling 2027-01-01); GLM 5.3 Flash on
  three frames per gap ~$0.50; Ollama's free credit perhaps ~20 hours of video a month. **The binding limits
  are privacy** (free-tier Gemini is not for family video), **Claude Pro's usage windows** (~480 requests for
  two hours at one per gap) and **wall time**. Spike 0(h) replaces the estimate with measurements; the UI
  shows an estimate before each job.
- `[untested]` **Frame extraction is more work than one table row suggests.** Seeking a video stream to an
  exact timestamp, decoding the frame, converting NV12 or YUV to RGB, then JPEG-encoding it — WIC handles
  the last step and Photo Scribe already does it, but the colour conversion and seek accuracy are fiddly.
  Budget real time in spike 0(e), and note this is only needed for the two image services.
- **Language and voice mismatch is barely addressed.** SonarPad carries 17 languages, a BCP 47 table, a
  language-detection correction pass and voice-to-language matching (§2.7). This plan has one line. At
  minimum: the description language must be chosen explicitly, the voice must match it or the user warned,
  and a saved voice that is no longer installed must fall back audibly rather than fail.
- `[untested]` MF decoding both corpora reliably — YouTube MP4 and real phone/camcorder files. Spike 0(e)
  settles it. Phone video is H.264 or HEVC in MP4/MOV with AAC, which is MF's home turf, so this is a
  better bet than it was when WebM was in scope; but **HEVC may need the Store's HEVC Video Extension**,
  exactly the situation Photo Scribe already handles for HEIC, so reuse that pattern and its error message.
  If it fails outright, the honest fallback is FFmpeg DLLs, and §5.8's rule is what we would be breaking.
- `[untested]` **VAD quality on real family video.** This is now the first-order risk, because a VAD marks
  *any* voice as speech — a radio in the kitchen, singing, a crowd — and will protect gaps you would
  happily describe over. Over-protection means too few descriptions, which is a quieter failure than
  talking over dialogue but still a failure. Spike 0(a)'s comparison against caption cues is the only
  honest way to calibrate it, and the threshold may need to be a setting rather than a constant.
- `[untested]` **The WinHTTP WebSocket handshake against Edge's endpoint.** The APIs exist and the bindings
  exist, which is why §4.4 is worth doing, but nobody has pointed WinHTTP at this particular service.
  Spike 0(f) is the gate. Fallback order: a WebSocket crate over a WinHTTP TLS socket, then
  tungstenite + rustls as SonarPad has it, which costs the one-exe goal a good deal of weight.
- **Edge TTS is an undocumented, unsupported endpoint.** §2.9 already carries two routes and two GEC
  versions because it has moved before, and it will move again. SAPI 5 is not decoration — it is what keeps
  the app working on the day Edge breaks. The failure must be a clear message and a fallback, never a hang.
- `[untested]` Auto-caption availability and timing accuracy across real content. Spike 0(a).
- Frame sampling may simply not be good enough — no motion, no continuity within a shot. Spike 0(h) is the
  gate. Gemini going in first means this is not a blocker, just a question of how many services survive.
- **Family video may be the harder content, not the easier.** No script, no shot grammar, poor audio, and
  the thing worth describing is often a glance or who is present rather than an action. The bake-off should
  not be judged only on the YouTube clip, or we will tune for the wrong corpus.
- `[untested]` Silero's licence reads MIT on the repository, and the model file is 1.3 MB; the README's own
  badge contradicts itself (CC BY-NC in the alt text). Read the `LICENSE` file at the pinned revision and
  record the commit, as SonarPad does for pyannote's weights.
- Claude Pro usage limits over a feature-length film: Photo Scribe already has this unresolved
  (`[untested]` in its PLAN.md §4). A 90-minute film with 3 s gaps could be hundreds of requests.
  Gemini free-tier quota has the same shape, which is why §2.3's interactive quota protocol is worth copying.
- Licensing: Omni Describer's prompt shape and verbosity scheme are MIT and need attribution. SonarPad is
  all rights reserved, non-commercial: its scheduling, prompts, Edge TTS code and tests are ideas to learn
  from, never text to copy (§2.1, log entry 8).
  Silero VAD is MIT (confirm the `LICENSE` file, above). pyannote's CC BY 4.0 no longer applies, since we
  are not using its weights. yt-dlp is Unlicense; downloading it at runtime rather than shipping it keeps
  this simple.
- **The people catalogue is the most sensitive file either app will ever write** — real names and physical
  descriptions of your family, in a known location, in plain text. Three consequences: it must never be
  sent anywhere wholesale beyond the entries a job actually needs; it should be mentioned explicitly in the
  help page so its existence is never a surprise; and because both apps may be open at once, every write
  must be temp-file-then-rename (§5.7) — and atomic writes alone do not stop Photo Scribe overwriting
  Video Scribe's additions, which is why Video Scribe only reads the shared file by default. It is not
  encrypted, and should not pretend to be — it is a text file in the user's own profile, like Photo
  Scribe's `settings.txt`.
- `[untested]` Whether the spoken constraint of §5.10 actually reads well with JAWS, or whether two
  controls that reach into each other is more confusing than one combo box with the trade-off in the option
  labels. Settle it with the probe at milestone 2, and be willing to collapse it to one control.
- Copyright of the content itself: the app describes video the user already has access to, and writes a
  derived audio track locally. Worth a line in the help page, not a blocker.
```

---

## 10. Release checklist

Things to do once, before the first release, collected so none is forgotten.

- Done another way (2 November 2026): instead of rewriting history to remove names, the public
  repository started afresh, without `log.md` or the test files, and the real names in test examples
  were replaced by made-up ones. Keep it that way: no family names, file names or places in code,
  tests or commit messages.
- A friendlier finishing message after a job (test-milestone-7.txt, 1.2).
