//! One job on a background thread, its news posted to the window, which owns
//! all state (PLAN.md §5.5, Photo Scribe's shape). Stopping is a flag the
//! job checks between and inside its steps (§5.1).
//!
//! Two jobs, with a question to the user between them. The first finds the
//! pauses: YouTube is downloaded with yt-dlp, a file is read where it is, the
//! sound goes through the speech detector, captions lead when there are any,
//! and the map is written as a report with a pause check file. Nothing has
//! been sent at that point. The window then says exactly what describing would
//! send, and only if the user agrees does the second job send it (PLAN.md §5.9).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::choices::Source;
use crate::frames::Frames;
use crate::report::{self, CaptionsUsed, Map};
use crate::{beeps, cache, captions, clips, describe, media, mix, speak, speech, vad, ytdlp};

/// The window hears from the job by this message; its LPARAM is a boxed Event.
pub const WM_JOB: u32 = WM_APP + 20;

pub enum Event {
    /// A new step: said out loud and shown.
    Step(String),
    /// How far a step has got: shown, not said.
    Progress(String),
    /// The first job's result: the pauses, and all describing needs.
    Mapped(Box<Mapped>),
    /// The second job's: the descriptions report, and where it was written.
    Described {
        report: String,
        path: PathBuf,
        /// False when nothing had changed, so the last result stands.
        made: bool,
    },
    /// A small job's result, said and shown.
    Said(String),
    Failed(String),
    Stopped,
}

/// A video with its pauses found.
pub struct Mapped {
    /// The speech map report, where it was written, and the pause check
    /// file, or why it could not be made.
    pub report: String,
    pub path: PathBuf,
    pub check: Result<PathBuf, String>,
    /// The file name, or the YouTube title and id; for reports only.
    pub name: String,
    /// The file name, or the YouTube title, for the model when the user
    /// turns sending it on.
    pub sendable_name: String,
    /// The key the video's descriptions, people and results go by: a
    /// YouTube id, a web address's key, or a file's name (`people::key`).
    pub saved_as: String,
    /// The title the video's folder is named after when it is made, and the
    /// web address or file it came from (`library.rs`).
    pub title: String,
    pub source: String,
    pub family: bool,
    /// The picture to take stills from and its clock shift, if there is one.
    pub video: Option<(PathBuf, f64)>,
    /// The sound, for clips: the video itself, or YouTube's separate file.
    pub sound: PathBuf,
    pub secs: f64,
    pub gaps: Vec<speech::Gap>,
    pub cues: Vec<(f64, f64, String)>,
    /// The YouTube video id, whose download is deleted when done.
    pub download: Option<String>,
}

/// What the second job needs besides the map.
pub struct Describe {
    /// OpenRouter with its key, or Claude Code (milestone 6).
    pub route: describe::Route,
    pub model: String,
    /// "GLM 5.3 Flash, through OpenRouter", for the report.
    pub model_label: String,
    pub language: String,
    pub style: String,
    pub send_name: bool,
    /// Short clips with their sound, rather than still pictures (Gemini).
    pub clips: bool,
    pub output_folder: PathBuf,
    /// The Edge voice and how much faster than normal it speaks.
    pub voice: String,
    pub rate: i32,
    /// What to do with a description too long for its pause.
    pub room: crate::config::Room,
    /// Short optional pauses follow `room` too (log entry 29, Q1).
    pub short_too: bool,
    /// The user chose described video (milestone 9).
    pub wants_video: bool,
    /// Keep a YouTube download for a day, rather than deleting it now.
    pub keep_downloads: bool,
    /// The names and the note the user gave for this video (milestone 5).
    pub people: crate::people::People,
    /// Describe again knowing these people (`describe::known_lines`); None
    /// keeps the people the kept descriptions were written with.
    pub known: Option<String>,
    /// Requests sent at once (log entry 31).
    pub together: usize,
    /// Tag the faces in the pictures, and know people named before by them
    /// (log entry 44).
    pub faces: bool,
    /// The user agreed to download the face models first.
    pub fetch_faces: bool,
}

pub struct Settings {
    pub source: Source,
    /// The description language's code, such as "en", for the captions to
    /// look for.
    pub lang: String,
    pub output_folder: PathBuf,
    /// The user agreed to fetch yt-dlp, which is not on this computer yet.
    pub fetch_ytdlp: bool,
    /// The most lines of picture to download (log entry 39, Q2).
    pub lines: u32,
}

/// Starts the first job; set the returned flag to stop it.
pub fn start(hwnd: HWND, settings: Settings) -> Arc<AtomicBool> {
    spawn(hwnd, move |stop, post| {
        Ok(run(&settings, stop, post)?.map(|m| Event::Mapped(Box::new(m))))
    })
}

/// Starts the second job, on a video the first one mapped.
pub fn describe(hwnd: HWND, mapped: Box<Mapped>, d: Describe) -> Arc<AtomicBool> {
    spawn(hwnd, move |stop, post| {
        Ok(
            describing(&mapped, &d, stop, post)?.map(|(report, path, made)| Event::Described {
                report,
                path,
                made,
            }),
        )
    })
}

/// Updates yt-dlp, on request from the File menu.
pub fn update_ytdlp(hwnd: HWND) -> Arc<AtomicBool> {
    spawn(hwnd, |stop, post| {
        let Some(exe) = ytdlp::find() else {
            return Ok(Some(Event::Said(
                "yt-dlp is not on this computer yet. Video Scribe asks to download it the first \
                 time you describe a web video."
                    .into(),
            )));
        };
        post(Event::Step("Updating yt-dlp.".into()));
        Ok(Some(Event::Said(match ytdlp::update(&exe, stop) {
            Ok(()) => "yt-dlp is up to date.".into(),
            Err(why) => format!("yt-dlp could not update itself. It said: {why}"),
        })))
    })
}

/// Runs `work` on its own thread, posting its news and its end to the window.
fn spawn<F>(hwnd: HWND, work: F) -> Arc<AtomicBool>
where
    F: FnOnce(&AtomicBool, Post) -> Result<Option<Event>, String> + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    // A window handle is not Send; its number is.
    let window = hwnd.0 as isize;
    std::thread::spawn(move || {
        let post = |e: Event| unsafe {
            let raw = Box::into_raw(Box::new(e));
            if PostMessageW(
                Some(HWND(window as *mut _)),
                WM_JOB,
                WPARAM(0),
                LPARAM(raw as isize),
            )
            .is_err()
            {
                // The window has gone: nobody to tell.
                drop(Box::from_raw(raw));
            }
        };
        let end = match work(&flag, &post) {
            Ok(Some(e)) => e,
            Ok(None) => Event::Stopped,
            Err(why) => Event::Failed(why),
        };
        post(end);
    });
    stop
}

/// Takes back an event the job posted.
///
/// # Safety
/// `lparam` must come from a WM_JOB message, and be taken only once.
pub unsafe fn take(lparam: LPARAM) -> Box<Event> {
    Box::from_raw(lparam.0 as *mut Event)
}

/// Shows progress at most twice a second, so the Status box is not rewritten
/// thousands of times.
struct Every {
    last: Option<Instant>,
}

impl Every {
    fn due(&mut self) -> bool {
        let due = self.last.is_none_or(|t| t.elapsed().as_millis() >= 500);
        if due {
            self.last = Some(Instant::now());
        }
        due
    }
}

type Post<'a> = &'a dyn Fn(Event);

fn run(s: &Settings, stop: &AtomicBool, post: Post) -> Result<Option<Mapped>, String> {
    let _session = media::Session::start()?;
    // The name in the report, the one the model may be given, and the one
    // the output files are saved under.
    let (sound_file, name, sendable_name, saved_as, captions_file, video) = match &s.source {
        Source::File(path) => (
            path.clone(),
            file_name(path),
            file_name(path),
            stem(&file_name(path)),
            captions::beside(path, &s.lang),
            Some((path.clone(), 0.0)),
        ),
        Source::Web(url) => {
            let Some(got) = download(url, s, stop, post)? else {
                return Ok(None);
            };
            let name = if crate::choices::is_youtube(url) {
                format!("{}, YouTube video {}", got.title, got.id)
            } else {
                format!("{}, from {}", got.title, crate::choices::site(url))
            };
            let video = match &got.video {
                // One file with picture and sound: read as it is, like a
                // file of the user's, so the two stay on one clock.
                Some(v) if *v == got.audio => Some((v.clone(), 0.0)),
                Some(v) => Some((v.clone(), ytdlp::drop_edit_list(v)?)),
                None => None,
            };
            (got.audio, name, got.title, got.id, got.captions, video)
        }
    };

    let (secs, heard) = match cache::heard(&sound_file) {
        Some(found) => {
            post(Event::Step(
                "The speech in this sound was found before; using that.".into(),
            ));
            found
        }
        None => match listen(&sound_file, &name, stop, post)? {
            Some(found) => {
                cache::keep_heard(&sound_file, found.0, &found.1);
                found
            }
            None => return Ok(None),
        },
    };

    let mut used = None;
    let mut cues = Vec::new();
    let mut speech_map = heard.clone();
    if let Some(path) = &captions_file {
        if let Some(c) = captions::read(path)? {
            speech_map = speech::captions_led(&heard, &c.speech);
            cues = c.cues.clone();
            used = Some(CaptionsUsed {
                file_name: file_name(path),
                automatic: c.automatic,
                lines: c.cues.len(),
                speech: c.speech,
            });
        }
    }
    if stop.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let map = Map {
        name: name.clone(),
        secs,
        vad: heard,
        captions: used,
        gaps: speech::plan_gaps(&speech_map, secs),
        speech: speech_map,
    };
    let text = report::text(&map);
    std::fs::create_dir_all(&s.output_folder).map_err(|e| {
        format!(
            "The output folder {} could not be made: {e}. Choose another in the File menu.",
            s.output_folder.display()
        )
    })?;
    // One folder for each video, named after its title (log entry 40, Q2).
    let title = match &s.source {
        Source::File(_) => stem(&name),
        // yt-dlp says NA for a title it was not given.
        Source::Web(_) if !matches!(sendable_name.trim(), "" | "NA") => sendable_name.clone(),
        Source::Web(_) => saved_as.clone(),
    };
    let source = match &s.source {
        Source::File(p) => p.display().to_string(),
        Source::Web(url) => url.clone(),
    };
    let (folder, base) = video_folder(&s.output_folder, &saved_as, &title, &source)?;
    let path = folder.join(format!("{base} - speech map.txt"));
    write_new(&path, &text)?;

    let check = folder.join(format!("{base} - pause check.mp3"));
    let check = if cache::check_done(&sound_file, &check, &map.gaps) {
        Ok(check)
    } else {
        post(Event::Step("Making the pause check file.".into()));
        match pause_check(&sound_file, &check, &map.gaps, stop, post) {
            Ok(true) => {
                cache::check_made(&sound_file, &check, &map.gaps);
                Ok(check)
            }
            Ok(false) => return Ok(None),
            Err(why) => Err(why),
        }
    };
    Ok(Some(Mapped {
        report: text,
        path,
        check,
        name,
        sendable_name,
        saved_as: saved_as.clone(),
        title,
        source,
        family: matches!(s.source, Source::File(_)),
        video,
        sound: sound_file.clone(),
        secs,
        gaps: map.gaps,
        cues,
        download: match &s.source {
            Source::Web(_) => Some(saved_as.clone()),
            Source::File(_) => None,
        },
    }))
}

/// Where a job's answers are kept until it finishes (PLAN.md §5.6).
/// The record of a video's last described sound, for reviewing it.
pub fn result_path(saved_as: &str) -> PathBuf {
    checkpoint(saved_as).with_extension("result.json")
}

/// Each place of the finished job: where it falls in the described sound and
/// in the film, what was said, and what happened to it.
fn keep_result(
    m: &Mapped,
    placed: &mix::Placed,
    described: &[String],
    sound_path: &Path,
    video_path: Option<&Path>,
    overrides: &[(usize, String)],
    recipe: &str,
) -> Result<(), String> {
    let lines = mix::lines(placed);
    let times = mix::on_output(placed, &m.gaps);
    let places: Vec<serde_json::Value> = m
        .gaps
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let (start, said) = times.get(i).copied().unwrap_or((g.start, None));
            serde_json::json!({
                "place": i + 1,
                "film": [g.start, g.end],
                "start": start,
                "said": said.map(|(a, s)| vec![a, s]),
                "line": lines.get(i).cloned().unwrap_or_default(),
                "described": described.get(i).cloned().unwrap_or_default(),
                "yours": overrides
                    .iter()
                    .find(|(p, _)| *p == i + 1)
                    .map(|(_, t)| t.clone()),
            })
        })
        .collect();
    let v = serde_json::json!({
        "output": sound_path.to_string_lossy(),
        "film": m.sound.to_string_lossy(),
        "places": places,
        "recipe": recipe,
        "video": video_path.map(|v| v.to_string_lossy()),
    });
    let path = result_path(&m.saved_as);
    let part = path.with_extension("part");
    std::fs::write(&part, v.to_string())
        .and_then(|_| std::fs::rename(&part, &path))
        .map_err(|e| e.to_string())
}

/// Everything the described sound is made from, as one fingerprint: the
/// text spoken at each place, the voice and its speed, and the fit choices.
fn recipe(texts: &[String], d: &Describe) -> String {
    let s = format!(
        "{texts:?}\n{}\n{}\n{:?}\n{}\n{}",
        d.voice, d.rate, d.room, d.short_too, d.wants_video
    );
    crate::net::hex(&crate::net::sha256(s.as_bytes()))
}

/// The last result's report and its path, when it was made from exactly
/// `recipe` and its files are still there, in the video's `folder`: one
/// from before folders, or in another output folder, is made again.
fn unchanged(saved_as: &str, recipe: &str, folder: &Path) -> Option<(String, PathBuf)> {
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(result_path(saved_as)).ok()?).ok()?;
    if v["recipe"].as_str()? != recipe {
        return None;
    }
    let sound = crate::library::relocate(Path::new(v["output"].as_str()?), saved_as);
    let list = sound.with_extension("txt");
    if !sound.is_file() || !list.is_file() || sound.parent() != Some(folder) {
        return None;
    }
    if let Some(video) = v["video"].as_str() {
        if !crate::library::relocate(Path::new(video), saved_as).is_file() {
            return None;
        }
    }
    let report =
        format!(
        "Nothing has changed since {} was made: the same descriptions, voice and choices. So it \
         was not made again, and nothing was sent. Listen to it with Ctrl+L, or review it with \
         Ctrl+R.\r\n\r\n{}",
        file_name(&sound),
        std::fs::read_to_string(&list).unwrap_or_default().trim_start_matches('\u{feff}')
    );
    Some((report, list))
}

/// The user's own text for places of this video's descriptions.
pub fn overrides(saved_as: &str) -> Vec<(usize, String)> {
    describe::overrides(&checkpoint(saved_as))
}

/// One place of a video's last described sound, as the review window shows it.
pub struct Reviewed {
    pub place: usize,
    /// Its pause in the film, and where it starts in the described sound.
    pub film: (f64, f64),
    pub start: f64,
    /// When it is said in the described sound, and for how long.
    pub said: Option<(f64, f64)>,
    /// The report's line for it, as it is in the sound.
    pub line: String,
    /// The description, with the user's names, before any text of theirs.
    pub described: String,
    /// The user's own text that is in the sound, if any.
    pub yours: Option<String>,
}

/// A video's last described sound and its places, for reviewing it; None if
/// it has not been described yet.
pub struct Review {
    pub output: PathBuf,
    pub film: PathBuf,
    pub places: Vec<Reviewed>,
}

pub fn review(saved_as: &str) -> Option<Review> {
    let text = std::fs::read_to_string(result_path(saved_as)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let pair = |x: &serde_json::Value| Some((x[0].as_f64()?, x[1].as_f64()?));
    let places = v["places"]
        .as_array()?
        .iter()
        .filter_map(|p| {
            Some(Reviewed {
                place: p["place"].as_u64()? as usize,
                film: pair(&p["film"])?,
                start: p["start"].as_f64()?,
                said: pair(&p["said"]),
                line: p["line"].as_str()?.to_string(),
                described: p["described"].as_str().unwrap_or_default().to_string(),
                yours: p["yours"].as_str().map(str::to_string),
            })
        })
        .collect();
    Some(Review {
        output: crate::library::relocate(Path::new(v["output"].as_str()?), saved_as),
        film: PathBuf::from(v["film"].as_str()?),
        places,
    })
}

pub fn set_overrides(saved_as: &str, list: &[(usize, String)]) -> Result<(), String> {
    describe::set_overrides(&checkpoint(saved_as), list)
}

fn checkpoint(saved_as: &str) -> PathBuf {
    ytdlp::local_dir()
        .join("checkpoints")
        .join(format!("{saved_as}.json"))
}

/// The video's folder in `output`, and what its files are named after:
/// the folder's own name, which the user may have changed.
fn video_folder(
    output: &Path,
    key: &str,
    title: &str,
    source: &str,
) -> Result<(PathBuf, String), String> {
    let folder = crate::library::folder_for(output, key, title, source)?;
    let base = file_name(&folder);
    Ok((folder, base))
}

/// The first of "name", "name 2", "name 3"... with neither an MP3 nor a text
/// file in the folder.
fn free_stem(folder: &Path, name: &str) -> String {
    (1..)
        .map(|n| {
            if n == 1 {
                name.to_string()
            } else {
                format!("{name} {n}")
            }
        })
        .find(|s| {
            !folder.join(format!("{s}.mp3")).exists() && !folder.join(format!("{s}.txt")).exists()
        })
        .expect("a free name")
}

/// Deletes checkpoints not touched for 30 days.
fn tidy_checkpoints(dir: Option<&Path>) {
    let Some(Ok(list)) = dir.map(std::fs::read_dir) else {
        return;
    };
    let month = std::time::Duration::from_secs(30 * 24 * 3600);
    for e in list.flatten() {
        let old = e
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > month);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// The labels an earlier run of this video's descriptions used, with what
/// it said about each.
pub fn cast(key: &str) -> describe::Cast {
    describe::cast(&checkpoint(key))
}

fn brief(m: &Mapped, d: &Describe) -> describe::Brief {
    describe::Brief {
        family: m.family,
        language: d.language.clone(),
        style: d.style.clone(),
        name: d.send_name.then(|| m.sendable_name.clone()),
        cues: m.cues.clone(),
        hears: d.clips,
        who: d.people.who.clone(),
        known: d
            .known
            .clone()
            .unwrap_or_else(|| describe::known_of(&checkpoint(&m.saved_as))),
        together: d.together,
        faces: d.faces,
        tagged: None,
    }
}

/// What a stopped job still has to send: the places not described yet.
pub fn left(m: &Mapped, d: &Describe) -> Vec<Vec<speech::Gap>> {
    describe::left(&m.gaps, &brief(m, d), &d.model, &checkpoint(&m.saved_as))
}

/// Whether an earlier run of this job finished its editor pass too.
pub fn edited(m: &Mapped, d: &Describe) -> bool {
    describe::edited(&m.gaps, &brief(m, d), &d.model, &checkpoint(&m.saved_as))
}

fn describing(
    m: &Mapped,
    d: &Describe,
    stop: &AtomicBool,
    post: Post,
) -> Result<Option<(String, PathBuf, bool)>, String> {
    let _session = media::Session::start()?;
    let Some((video, shift)) = &m.video else {
        return Err(format!("{} has no picture to describe.", m.name));
    };
    let frames;
    let look = if d.clips {
        describe::Look::Clips(
            clips::Source {
                video,
                shift: *shift,
                audio: &m.sound,
            },
            ytdlp::local_dir().join("work"),
        )
    } else {
        frames = Frames::open(&video.to_string_lossy(), *shift)?;
        describe::Look::Pictures(&frames)
    };
    let mut brief = brief(m, d);
    if d.faces {
        let Some(tagged) = find_faces(m, d, video, *shift, stop, post)? else {
            return Ok(None);
        };
        brief.tagged = Some(tagged);
    }
    post(Event::Step(format!("Describing with {}.", d.model_label)));
    let started = Instant::now();
    let ck = checkpoint(&m.saved_as);
    let out = describe::describe(
        &m.gaps,
        &brief,
        &look,
        &d.route,
        &d.model,
        &ck,
        stop,
        &mut |what| post(Event::Progress(what)),
    )?;
    let Some(out) = out else { return Ok(None) };
    let head = report::descriptions_head(&report::Described {
        name: &m.name,
        model: &d.model_label,
        plan: matches!(d.route, describe::Route::Claude { .. }),
        gaps: &m.gaps,
        outcome: &out,
        secs: started.elapsed().as_secs_f64(),
    });

    // People known by their faces get their names, on this computer.
    let (names, recognised) = match &brief.tagged {
        Some(tagged) => recognise(m, d, tagged, &ck),
        None => (d.people.names.clone(), Vec::new()),
    };

    // Speak, place and mix (milestone 7).
    post(Event::Step("Speaking the descriptions.".into()));
    let texts: Vec<String> = (1..=m.gaps.len())
        .map(|n| {
            out.described
                .iter()
                .find(|x| x.place == n)
                // The user's names, put in here on this computer.
                .map(|x| crate::people::apply(&x.text, &names))
                .unwrap_or_default()
        })
        .collect();
    // The user's own text from reviewing the result wins (milestone 8).
    let described = texts.clone();
    let mut texts = texts;
    for (n, t) in &out.overrides {
        if let Some(slot) = texts.get_mut(n.wrapping_sub(1)) {
            *slot = t.clone();
        }
    }
    // Nothing changed since the last result, which is still there: it is
    // not made again (round 5 test, 2.6).
    let recipe = recipe(&texts, d);
    let (folder, base) = video_folder(&d.output_folder, &m.saved_as, &m.title, &m.source)?;
    let folder = &folder;
    if let Some((report, list)) = unchanged(&m.saved_as, &recipe, folder) {
        return Ok(Some((report, list, false)));
    }
    let mut voice = speak::Voice::new(&d.voice, d.rate, ytdlp::local_dir().join("work"));
    let mut every = Every { last: None };
    let placed = mix::place(
        &m.gaps,
        &texts,
        d.room,
        d.short_too,
        &mut voice,
        stop,
        &mut |what| {
            if every.due() {
                post(Event::Progress(what));
            }
        },
    )?;
    let Some(placed) = placed else {
        return Ok(None);
    };
    post(Event::Step(
        "Mixing the descriptions into the sound.".into(),
    ));
    // A second result never replaces the first: it gets a number.
    let stem = free_stem(folder, &format!("{base} - described"));
    let sound_path = folder.join(format!("{stem}.mp3"));
    let mixed = placed.mix(&m.sound, &sound_path, stop, &mut |f| {
        if every.due() {
            post(Event::Progress(format!("Mixing: {:.0}%.", f * 100.0)));
        }
    })?;
    if !mixed {
        return Ok(None);
    }
    // Tags, for media players and File Explorer (test round 3, section 5).
    // Like the described video, they never fail the job.
    let tags = crate::tags::Tags {
        title: format!("{base}, described"),
        comment: format!(
            "Audio described by Video Scribe {}: descriptions by {}, spoken by {}. Made from {}.",
            env!("CARGO_PKG_VERSION"),
            d.model_label,
            crate::choices::voice_label(&d.voice),
            // A family video's file name only, never its folders.
            if m.family { &m.name } else { &m.source }
        ),
        lyrics: mix::lines(&placed).join("\r\n"),
        year: crate::tags::this_year(),
    };
    let mut tag_note = crate::tags::write(&sound_path, &tags)
        .err()
        .map(|why| format!("The described sound's tags could not be written. {why}"));
    // The described video (milestone 9): the film's pictures with this
    // sound. It never fails the job: the sound is made either way.
    let mut video_note = None;
    let mut video_path = None;
    if d.wants_video {
        match (&m.video, d.room) {
            (_, crate::config::Room::Pause) => {
                video_note = Some(
                    "No described video was made: the film pauses for descriptions, so the \
                     sound no longer matches its pictures. Choose another fit choice in \
                     Options to make one."
                        .to_string(),
                )
            }
            (None, _) => {
                video_note = Some("No described video was made: there is no picture.".into())
            }
            (Some((film, shift)), _) => {
                post(Event::Step("Making the described video.".into()));
                let path = folder.join(format!("{stem}.mp4"));
                let made = crate::mux::make(film, *shift, &sound_path, &path, stop, &mut |f| {
                    if every.due() {
                        post(Event::Progress(format!(
                            "Making the described video: {:.0}%.",
                            f * 100.0
                        )));
                    }
                });
                match made {
                    Ok(true) => {
                        if let Err(why) = crate::tags::write(&path, &tags) {
                            tag_note = Some(format!(
                                "The described video's tags could not be written. {why}"
                            ));
                        }
                        video_path = Some(path);
                    }
                    Ok(false) => return Ok(None),
                    Err(why) => video_note = Some(format!("No described video was made. {why}")),
                }
            }
        }
    }

    let fates = &placed.fates;
    let spoken = fates
        .iter()
        .filter(|f| matches!(f, mix::Fate::Spoken { .. }))
        .count();
    let left = fates
        .iter()
        .filter(|f| matches!(f, mix::Fate::Left { .. }))
        .count();
    let over = fates
        .iter()
        .filter(|f| matches!(f, mix::Fate::Spoken { over: true, .. }))
        .count();
    let (holds, held) = placed.paused();
    let mut summary = vec![format!(
        "{} is the sound with {spoken} descriptions spoken in it.",
        file_name(&sound_path)
    )];
    if left > 0 {
        summary.push(if left == 1 {
            "1 was left out because it did not fit; the list below says why.".to_string()
        } else {
            format!("{left} were left out because they did not fit; the list below says why.")
        });
    }
    if over > 0 {
        summary.push(if over == 1 {
            "1 runs on over the film, because it did not fit its pause.".to_string()
        } else {
            format!("{over} run on over the film, because they did not fit their pauses.")
        });
    }
    if holds > 0 {
        summary.push(if holds == 1 {
            format!(
                "The film waits once, for {}, for a description to finish.",
                report::secs(held)
            )
        } else {
            format!(
                "The film waits {holds} times, {} in all, for a description to finish.",
                report::secs(held)
            )
        });
    }
    if let Some(note) = &voice.note {
        summary.push(note.clone());
    }
    if let Some(v) = &video_path {
        summary.push(format!(
            "{} is the described video: the film's own pictures with this sound.",
            file_name(v)
        ));
    }
    if let Some(note) = video_note {
        summary.push(note);
    }
    if let Some(note) = tag_note {
        summary.push(note);
    }
    if !recognised.is_empty() {
        let list: Vec<String> = recognised
            .iter()
            .map(|(label, name)| format!("{name} ({label})"))
            .collect();
        summary.push(format!(
            "Known by their faces, and named on this computer: {}. Correct a name in People in \
             this video, Ctrl+P.",
            list.join(", ")
        ));
    }
    let mut lines = summary;
    lines.push(String::new());
    lines.extend(head);
    lines.push(String::new());
    lines.push("Each place, at its time in the described sound:".into());
    lines.extend(mix::lines(&placed));
    let text = lines.join("\r\n") + "\r\n";
    let path = folder.join(format!("{stem}.txt"));
    write_new(&path, &text)?;
    // What the review window needs (milestone 8). Failing to keep it costs
    // only the review, so it never fails the job.
    let _ = keep_result(
        m,
        &placed,
        &described,
        &sound_path,
        video_path.as_deref(),
        &out.overrides,
        &recipe,
    );
    // The checkpoint is kept, so describing again with another voice or fit
    // choice sends nothing; a month on, it goes.
    tidy_checkpoints(ck.parent());
    if !m.family && !d.keep_downloads {
        if let Some(id) = &m.download {
            ytdlp::forget(&ytdlp::local_dir().join("downloads"), id);
        }
    }
    Ok(Some((text, path, true)))
}

/// Finds and tags the faces in the pictures of every place, on this
/// computer, downloading the models first when the user agreed to. None if
/// stopped.
fn find_faces(
    m: &Mapped,
    d: &Describe,
    video: &Path,
    shift: f64,
    stop: &AtomicBool,
    post: Post,
) -> Result<Option<crate::faces::Tagged>, String> {
    if !crate::faces::models_present() {
        if !d.fetch_faces {
            return Err("The face models are not on this computer.".into());
        }
        post(Event::Step(
            "Downloading the face models, 39 MB, from GitHub.".into(),
        ));
        let mut every = Every { last: None };
        let done = crate::faces::download_models(stop, &mut |got, of| {
            if every.due() && of > 0 {
                post(Event::Progress(format!(
                    "Downloading the face models: {:.0}%.",
                    got as f64 * 100.0 / of as f64
                )));
            }
        })?;
        if !done {
            return Ok(None);
        }
    }
    post(Event::Step(
        "Finding the faces in the pictures, on this computer.".into(),
    ));
    let frames = Frames::open(&video.to_string_lossy(), shift)?;
    let times: Vec<f64> = m.gaps.iter().flat_map(describe::picture_times).collect();
    let mut every = Every { last: None };
    crate::faces::tag(&m.saved_as, &frames, &times, stop, &mut |i, of| {
        if every.due() {
            post(Event::Progress(format!(
                "Finding the faces, on this computer: picture {} of {of}.",
                i + 1
            )));
        }
    })
}

/// The names to put in: the user's own for this video, then the name of
/// each person known by their face whose label the user has not named.
/// Those are kept in the video's people file, where the user can correct
/// them. Also returns them, as (label, name), to say so.
#[allow(clippy::type_complexity)]
fn recognise(
    m: &Mapped,
    d: &Describe,
    tagged: &crate::faces::Tagged,
    ck: &Path,
) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let mut names = d.people.names.clone();
    let known = crate::known::Known::load();
    let who = known.recognise(&m.saved_as, &tagged.people, &crate::known::photo_scribe());
    let mut found = Vec::new();
    for (label, tag) in describe::cast(ck).sure_tags() {
        let Some(Some(name)) = tag.checked_sub(1).and_then(|i| who.get(i)) else {
            continue;
        };
        let named = names
            .iter()
            .any(|(l, n)| l.eq_ignore_ascii_case(&label) && !n.trim().is_empty());
        if named {
            continue;
        }
        names.retain(|(l, _)| !l.eq_ignore_ascii_case(&label));
        names.push((label.clone(), name.clone()));
        found.push((label, name.clone()));
    }
    if !found.is_empty() {
        let mut people = d.people.clone();
        people.names = names.clone();
        // Not keeping them costs only the People window's view of them.
        let _ = crate::people::save(&m.saved_as, &people);
    }
    (names, found)
}

/// Decodes the sound and finds the speech in it: its length, and the speech.
/// None if stopped.
fn listen(
    sound_file: &Path,
    name: &str,
    stop: &AtomicBool,
    post: Post,
) -> Result<Option<(f64, speech::Spans)>, String> {
    post(Event::Step("Reading the sound.".into()));
    let mut every = Every { last: None };
    let sound = media::decode_mono(&sound_file.to_string_lossy(), vad::RATE, stop, &mut |f| {
        if every.due() {
            post(Event::Progress(format!(
                "Reading the sound: {:.0}%.",
                f * 100.0
            )));
        }
    })?;
    let Some(sound) = sound else { return Ok(None) };
    let secs = sound.secs();
    if secs < 1.0 {
        return Err(format!(
            "{name} has less than a second of sound: nothing to describe."
        ));
    }
    post(Event::Step("Finding the speech.".into()));
    let probs = vad::probabilities(&sound.samples, stop, &mut |f| {
        if every.due() {
            post(Event::Progress(format!(
                "Finding the speech: {:.0}%.",
                f * 100.0
            )));
        }
    })?;
    let Some(probs) = probs else { return Ok(None) };
    Ok(Some((secs, vad::intervals(&probs, sound.samples.len()))))
}

/// The sound again, with beeps where each pause starts and ends, as an MP3.
/// False if stopped, and then no file is left behind.
fn pause_check(
    sound: &Path,
    out: &Path,
    gaps: &[speech::Gap],
    stop: &AtomicBool,
    post: Post,
) -> Result<bool, String> {
    const RATE: u32 = 44_100;
    let mut mp3 = media::Mp3::create(out, RATE)?;
    let mut mixer = beeps::Mixer::new(beeps::marks(gaps), RATE);
    let mut every = Every { last: None };
    let mut piece = Vec::new();
    let done = media::each_mono(
        &sound.to_string_lossy(),
        RATE,
        stop,
        &mut |f| {
            if every.due() {
                post(Event::Progress(format!(
                    "Making the pause check file: {:.0}%.",
                    f * 100.0
                )));
            }
        },
        &mut |s| {
            piece.clear();
            piece.extend_from_slice(s);
            mixer.mix(&mut piece);
            mp3.write(&piece)
        },
    );
    match done {
        Ok(true) => mp3.finish().map(|_| true),
        Ok(false) => {
            mp3.abandon();
            Ok(false)
        }
        Err(why) => {
            mp3.abandon();
            Err(why)
        }
    }
}

/// Downloads a web address with yt-dlp, or finds it downloaded earlier.
/// yt-dlp is fetched if agreed, and updated if it is our own old copy.
fn download(
    url: &str,
    s: &Settings,
    stop: &AtomicBool,
    post: Post,
) -> Result<Option<ytdlp::Downloaded>, String> {
    let dir = ytdlp::local_dir().join("downloads");
    ytdlp::tidy(&dir);
    if let Some(d) = ytdlp::earlier(&dir, url, s.lines) {
        post(Event::Step(
            "Using the copy of this video downloaded earlier.".into(),
        ));
        return Ok(Some(d));
    }
    let exe = match ytdlp::find() {
        Some(exe) => exe,
        None if s.fetch_ytdlp => {
            post(Event::Step("Downloading yt-dlp from GitHub.".into()));
            let mut every = Every { last: None };
            let got = ytdlp::fetch(stop, &mut |done, total| {
                if total > 0 && every.due() {
                    post(Event::Progress(format!(
                        "Downloading yt-dlp: {:.0}%.",
                        done as f64 * 100.0 / total as f64
                    )));
                }
            })?;
            match got {
                Some(exe) => exe,
                None => return Ok(None),
            }
        }
        None => {
            return Err(
                "Video Scribe needs yt-dlp to download videos, and it is not on this computer."
                    .into(),
            )
        }
    };
    if ytdlp::stale(&exe) {
        post(Event::Step("Updating yt-dlp.".into()));
        // An old copy may still work, so a failed update is not a reason to stop.
        let _ = ytdlp::update(&exe, stop);
    }
    if stop.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let site = crate::choices::site(url);
    post(Event::Step(format!(
        "Downloading the video and captions from {site}."
    )));
    let mut every = Every { last: None };
    ytdlp::download(&exe, url, &dir, &s.lang, s.lines, stop, &mut |p| {
        if every.due() {
            let what = p.what;
            let text = match p.total {
                Some(t) if t > 0 => format!(
                    "Downloading the {what} from {site}: {:.0}%.",
                    p.done as f64 * 100.0 / t as f64
                ),
                _ => format!(
                    "Downloading the {what} from {site}: {:.1} MB.",
                    p.done as f64 / 1e6
                ),
            };
            post(Event::Progress(text));
        }
    })
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map_or_else(|| name.to_string(), |s| s.to_string_lossy().into_owned())
}

/// Written beside, then renamed, so a half-written report never replaces a whole one.
fn write_new(path: &Path, text: &str) -> Result<(), String> {
    let part = path.with_extension("txt.part");
    // A byte order mark, so Notepad never guesses the wrong encoding.
    let body = format!("\u{feff}{text}");
    std::fs::write(&part, body).map_err(|e| format!("{}: {e}", part.display()))?;
    std::fs::rename(&part, path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("The report could not be written to {}: {e}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_result_gets_a_number() {
        let dir = std::env::temp_dir().join(format!("vs-stem-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(free_stem(&dir, "a - described"), "a - described");
        std::fs::write(dir.join("a - described.txt"), "").unwrap();
        assert_eq!(free_stem(&dir, "a - described"), "a - described 2");
        std::fs::write(dir.join("a - described 2.mp3"), "").unwrap();
        assert_eq!(free_stem(&dir, "a - described"), "a - described 3");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
