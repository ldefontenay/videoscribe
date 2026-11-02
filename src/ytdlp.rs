//! YouTube through yt-dlp (PLAN.md §4.5), one of the two programs Video
//! Scribe may start (CLAUDE.md constraint 6).
//!
//! yt-dlp is found on the PATH, or fetched from its official GitHub release
//! into `%LOCALAPPDATA%\Video Scribe\tools`, checked against the release's
//! SHA2-256SUMS. Without a JavaScript runtime YouTube offers no muxed format
//! worth having, so the sound is downloaded on its own and never merged,
//! which would need FFmpeg (spike 0(d), log entry 5). `--ffmpeg-location`
//! points at an empty folder, so a machine that happens to have FFmpeg
//! behaves like one that does not. The picture is downloaded too, as a
//! separate file at 480 lines or fewer, for the still pictures (milestone 4).
//!
//! Other sites, and links straight to a video file, go the same way
//! (milestone 10), but may offer only one file with picture and sound
//! together, which is then both. Only formats fetched over plain HTTP are
//! asked for: a stream in pieces (HLS) would need FFmpeg to be put together
//! into a file Windows can read.

use std::io::{BufRead, BufReader};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::*;

use crate::net;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Marks our lines so nothing else yt-dlp prints is mistaken for one.
const PROGRESS: &str = "VSPROGRESS";
const FILE: &str = "VSFILE";
const RELEASE: &str = "/yt-dlp/yt-dlp/releases/latest/download/";
/// Our own copy is updated with `-U` when it is older than this: YouTube
/// changes often, and an old yt-dlp is the usual reason a download fails.
const UPDATE_AFTER: Duration = Duration::from_secs(14 * 24 * 3600);

/// `%LOCALAPPDATA%\Video Scribe`, for downloads and tools: they can be
/// fetched again, so they do not belong with the settings in roaming AppData.
pub fn local_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Video Scribe")
}

fn own_copy() -> PathBuf {
    local_dir().join("tools").join("yt-dlp.exe")
}

/// yt-dlp on the PATH, or our own copy. None when neither is there.
pub fn find() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("yt-dlp.exe"))
        .find(|p| p.is_file())
        .or_else(|| Some(own_copy()).filter(|p| p.is_file()))
}

/// Fetches yt-dlp.exe from its latest release and checks it against the
/// release's SHA2-256SUMS before keeping it. None if stopped.
pub fn fetch(
    stop: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Option<PathBuf>, String> {
    let sums = match net::download(
        "github.com",
        &format!("{RELEASE}SHA2-256SUMS"),
        stop,
        &mut |_, _| {},
    )? {
        Some(s) => String::from_utf8_lossy(&s).into_owned(),
        None => return Ok(None),
    };
    let Some(want) = expected_hash(&sums, "yt-dlp.exe") else {
        return Err("yt-dlp's release has no checksum for yt-dlp.exe, so it was not kept.".into());
    };
    let Some(exe) = net::download(
        "github.com",
        &format!("{RELEASE}yt-dlp.exe"),
        stop,
        progress,
    )?
    else {
        return Ok(None);
    };
    let got = net::hex(&net::sha256(&exe));
    if got != want {
        return Err(
            "The yt-dlp that was downloaded does not match its published checksum, so it was not \
             kept. A new release may have come out during the download: try again."
                .into(),
        );
    }
    let path = own_copy();
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // Written beside, then renamed, so a half-written exe is never run.
    let part = path.with_extension("exe.part");
    std::fs::write(&part, &exe).map_err(|e| format!("{}: {e}", part.display()))?;
    std::fs::rename(&part, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(path))
}

/// The hash for `name` in a `<hash>  <name>` list.
fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let hash = it.next()?;
        let file = it.next()?.trim_start_matches('*');
        (file == name && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

/// True for our own copy when it is due an update.
pub fn stale(exe: &Path) -> bool {
    exe == own_copy()
        && std::fs::metadata(exe)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > UPDATE_AFTER)
}

/// What came down for one video.
pub struct Downloaded {
    pub id: String,
    pub title: String,
    pub audio: PathBuf,
    /// The picture, when YouTube offered one Windows can read.
    pub video: Option<PathBuf>,
    pub captions: Option<PathBuf>,
    /// The most lines of picture asked for.
    pub lines: u32,
}

#[derive(Debug, PartialEq)]
pub struct Progress {
    /// What the file holds, in words: "picture", "sound", or "video" when
    /// the site did not say, which is usually both.
    pub what: &'static str,
    pub done: u64,
    pub total: Option<u64>,
}

/// One `--progress-template` line: `VSPROGRESS <vcodec> <downloaded> <total>
/// <estimate>`, where yt-dlp writes `NA` for a value it does not know and
/// `none` for the sound's video codec.
fn parse_progress(line: &str) -> Option<Progress> {
    let mut it = line.trim().strip_prefix(PROGRESS)?.split_whitespace();
    let codec = it.next()?;
    let done = it.next()?.parse().ok()?;
    let num = |s: Option<&str>| s.and_then(|s| s.parse::<f64>().ok()).map(|v| v as u64);
    let total = num(it.next());
    let estimate = num(it.next());
    Some(Progress {
        what: match codec {
            "none" => "sound",
            "NA" => "video",
            _ => "picture",
        },
        done,
        total: total.or(estimate),
    })
}

/// Caption languages to ask for: hand-made ones in the description language,
/// and the original automatic captions in whatever language the film speaks
/// (".*-orig": a French film described in English still has its timing,
/// log entry 46). A wildcard such as "en.*" would also ask for every machine
/// translation, one request each, and YouTube answers with 429s (log entry
/// 5); there is only ever one "-orig" track.
fn sub_langs(lang: &str) -> String {
    match lang {
        "en" => "en,en-orig,en-GB,en-US,.*-orig".into(),
        l => format!("{l},{l}-orig,.*-orig"),
    }
}

/// What to ask for, with pictures at most `lines` high. YouTube: the picture
/// and the sound apart, H.264 first. Another site: the same when it has them
/// apart, else one file with both, MP4 first; always over plain HTTP.
fn formats(youtube: bool, lines: u32) -> String {
    if youtube {
        format!("bv*[vcodec^=avc1][height<={lines}]/bv*[height<={lines}],ba[ext=m4a]/ba")
    } else {
        format!(
            "(bv*[vcodec^=avc1][height<={lines}][protocol^=http]\
             /bv*[height<={lines}][protocol^=http],ba[ext=m4a][protocol^=http]/ba[protocol^=http])\
             /b[ext=mp4][height<={lines}][protocol^=http]/b[height<={lines}][protocol^=http]\
             /b[protocol^=http]"
        )
    }
}

/// Without `subs`, no captions are asked for: YouTube sometimes refuses them
/// with a 429 while it gives the video (log entry 47).
fn args(
    url: &str,
    out_dir: &Path,
    no_ffmpeg: &Path,
    lang: &str,
    lines: u32,
    subs: bool,
) -> Vec<String> {
    let out = out_dir.display().to_string();
    let none = no_ffmpeg.display().to_string();
    // Files are named by YouTube's id, or for another site by our own key,
    // as its ids may clash with another site's or hold any character.
    let youtube = crate::choices::is_youtube(url);
    let name = if youtube {
        "%(id)s".to_string()
    } else {
        key(url).unwrap_or_default()
    };
    let formats = formats(youtube, lines);
    let media = format!("{name}.f%(format_id)s.%(ext)s");
    let subtitles = format!("subtitle:{name}.%(ext)s");
    let langs = sub_langs(lang);
    let captions = [
        "--write-subs",
        "--write-auto-subs",
        "--sub-langs",
        &langs,
        "--sub-format",
        "vtt",
    ];
    let captions: &[&str] = if subs { &captions } else { &[] };
    [
        "--ignore-config",
        // Titles in any language: piped, yt-dlp otherwise writes Windows'
        // old code page, where "Les Misérables" is not UTF-8 and the line
        // naming the file is lost (found in VidFetch, 2 November 2026).
        "--encoding",
        "utf-8",
        "--no-playlist",
        "--newline",
        // "--print" below implies quiet mode, which hides progress; this brings it back.
        "--progress",
        "--no-colors",
        // Files dated when they were downloaded, so a day's keeping is a day.
        "--no-mtime",
        "--retries",
        "5",
        "--fragment-retries",
        "5",
        // H.264 first: Media Foundation decodes it everywhere. 480 lines is
        // plenty for 512-pixel pictures; more is for a described video.
        "-f",
        &formats,
        "--ffmpeg-location",
        &none,
    ]
    .iter()
    .chain(captions)
    .chain(&[
        "--progress-template",
        &format!(
            "download:{PROGRESS} %(info.vcodec)s %(progress.downloaded_bytes)s \
             %(progress.total_bytes)s %(progress.total_bytes_estimate)s"
        ),
        "-P",
        &out,
        // The id, never the title: a title would become a file name to clean.
        "-o",
        &media,
        // A page with several videos, as archive.org's can be: the first.
        "--playlist-items",
        "1",
        // Captions as <id>.<lang>.vtt.
        "-o",
        &subtitles,
        "--print",
        &format!("after_move:{FILE} %(filepath)s\t%(vcodec)s\t%(acodec)s\t%(title)s"),
        "--",
        url,
    ])
    .map(|s| s.to_string())
    .collect()
}

/// A job object that ends yt-dlp, and the Python process it starts, when it
/// is closed or when Video Scribe ends: killing the first process alone would
/// leave the second downloading.
struct Job(HANDLE);

impl Job {
    fn new() -> Option<Job> {
        unsafe {
            let job = CreateJobObjectW(None, PCWSTR::null()).ok()?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .ok()?;
            Some(Job(job))
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

enum Line {
    Out(String),
    Err(String),
}

/// Runs yt-dlp with `args`, handing each line of its output to `line`.
/// Returns the exit status and the error lines, or None if stopped.
fn run(
    exe: &Path,
    args: &[String],
    stop: &AtomicBool,
    line: &mut dyn FnMut(&str),
) -> Result<Option<(bool, Vec<String>)>, String> {
    let job = Job::new();
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("yt-dlp would not start ({}): {e}", exe.display()))?;
    if let Some(job) = &job {
        unsafe {
            let _ = AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle()));
        }
    }
    let (tx, rx) = mpsc::channel();
    let out = child.stdout.take().map(|o| {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for l in BufReader::new(o).lines().map_while(Result::ok) {
                let _ = tx.send(Line::Out(l));
            }
        })
    });
    let err = child.stderr.take().map(|e| {
        std::thread::spawn(move || {
            for l in BufReader::new(e).lines().map_while(Result::ok) {
                let _ = tx.send(Line::Err(l));
            }
        })
    });
    let mut errors = Vec::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            if let Some(job) = &job {
                unsafe {
                    let _ = TerminateJobObject(job.0, 1);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Line::Out(l)) => line(&l),
            Ok(Line::Err(l)) => {
                if !l.trim().is_empty() {
                    errors.push(l);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    for t in [out, err].into_iter().flatten() {
        let _ = t.join();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    Ok(Some((status.success(), errors)))
}

/// `yt-dlp -U` on our own copy. A failed update is not a reason to stop.
/// What yt-dlp said, when it said why it could not update.
pub fn update(exe: &Path, stop: &AtomicBool) -> Result<(), String> {
    match run(exe, &["-U".to_string()], stop, &mut |_| {}) {
        Ok(Some((true, _))) => {
            // Touch the file, so an up-to-date copy is not checked every time.
            let _ = std::fs::OpenOptions::new()
                .append(true)
                .open(exe)
                .and_then(|f| f.set_modified(SystemTime::now()));
            Ok(())
        }
        Ok(Some((false, errors))) => Err(errors
            .iter()
            .rev()
            .find(|e| e.starts_with("ERROR"))
            .or(errors.last())
            .map(|e| e.trim_start_matches("ERROR:").trim().to_string())
            .unwrap_or_else(|| "it stopped without saying why".into())),
        Ok(None) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Downloads the sound, picture and captions of `url` into `dir`, the
/// picture at most `lines` high. None if stopped.
#[allow(clippy::too_many_arguments)]
pub fn download(
    exe: &Path,
    url: &str,
    dir: &Path,
    lang: &str,
    lines: u32,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Option<Downloaded>, String> {
    let no_ffmpeg = dir.join("no-ffmpeg");
    std::fs::create_dir_all(&no_ffmpeg).map_err(|e| format!("{}: {e}", no_ffmpeg.display()))?;
    let mut subs = true;
    let Some(id) = key(url) else {
        return Err(format!(
            "{url} does not name one video. Open the video itself in the browser and copy its address."
        ));
    };
    let mut errors = Vec::new();
    // YouTube now and then answers 403 for a moment; yt-dlp resumes where it was.
    for _attempt in 0..3 {
        let args = args(url, dir, &no_ffmpeg, lang, lines, subs);
        let mut audio: Option<(PathBuf, String)> = None;
        let mut video: Option<PathBuf> = None;
        let result = run(exe, &args, stop, &mut |l| {
            if let Some(p) = parse_progress(l) {
                progress(p);
            } else if let Some(file) = parse_file(l) {
                if file.sound {
                    audio = Some((file.path.clone(), file.title));
                }
                if file.picture {
                    video = Some(file.path);
                }
            }
        })?;
        let Some((ok, errs)) = result else {
            return Ok(None);
        };
        if let (true, Some((audio, title))) = (ok, audio) {
            let captions = captions_for(dir, &id, lang);
            let d = Downloaded {
                id,
                title,
                audio,
                video,
                captions,
                lines,
            };
            keep_record(dir, &d);
            return Ok(Some(d));
        }
        let forbidden = errs.iter().any(|e| e.contains("403"));
        // Captions refused: the video without them, and the speech
        // detector alone finds the speech.
        let captions_refused = subs && errs.iter().any(|e| e.contains("subtitles"));
        errors = errs;
        if captions_refused {
            subs = false;
        } else if !forbidden {
            break;
        }
    }
    Err(explain(&errors, &crate::choices::site(url)))
}

/// One file yt-dlp saved, from a `VSFILE <path> <vcodec> <acodec> <title>`
/// line, tab-separated. yt-dlp writes `none` for what a file lacks, and
/// often `NA` for what another site did not say, which may be either: a
/// file is taken as holding both unless it says it lacks one.
struct Saved {
    path: PathBuf,
    picture: bool,
    sound: bool,
    title: String,
}

fn parse_file(line: &str) -> Option<Saved> {
    let mut it = line.strip_prefix(FILE)?.trim_start().splitn(4, '\t');
    let path = PathBuf::from(it.next()?);
    let vcodec = it.next()?;
    let acodec = it.next()?;
    Some(Saved {
        path,
        picture: vcodec != "none",
        sound: acodec != "none",
        title: it.next().unwrap_or_default().to_string(),
    })
}

/// The name a video's download, results and people go by: a YouTube
/// video's id, or for another address its site and a short hash of it, such
/// as "archive-org-3f2a9c1b0d". No dots, as files are matched to their
/// video by what comes before the first.
pub fn key(url: &str) -> Option<String> {
    if crate::choices::is_youtube(url) {
        return video_id(url);
    }
    let site: String = crate::choices::site(url)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if site.is_empty() {
        return None;
    }
    let hash = net::hex(&net::sha256(url.trim().as_bytes()));
    Some(format!("{site}-{}", &hash[..10]))
}

/// The 11-character video id in a YouTube address, if there is one.
pub fn video_id(url: &str) -> Option<String> {
    let ok = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    let after = ["v=", "youtu.be/", "/shorts/", "/live/", "/embed/"]
        .iter()
        .find_map(|m| url.find(m).map(|i| &url[i + m.len()..]))?;
    let id: String = after.chars().take_while(|&c| ok(c)).collect();
    (id.len() == 11).then_some(id)
}

/// What a download was saved as, so the same video can be used again
/// without asking YouTube (test round 3, 6.4).
fn record(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.download.json"))
}

fn keep_record(dir: &Path, d: &Downloaded) {
    let v = serde_json::json!({
        "id": d.id,
        "title": d.title,
        "audio": d.audio,
        "video": d.video,
        "captions": d.captions,
        "lines": d.lines,
    });
    let _ = std::fs::write(record(dir, &d.id), v.to_string());
}

/// Deletes the files of one video's download.
pub fn forget(dir: &Path, id: &str) {
    let Ok(list) = std::fs::read_dir(dir) else {
        return;
    };
    for e in list.flatten() {
        if e.file_name()
            .to_string_lossy()
            .starts_with(&format!("{id}."))
        {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Deletes downloads more than a day old, kept by the user's choice: a
/// video's files go when its record is a day old, and anything left over
/// without a record when it is.
pub fn tidy(dir: &Path) {
    let Ok(list) = std::fs::read_dir(dir) else {
        return;
    };
    let day = std::time::Duration::from_secs(24 * 3600);
    let old = |e: &std::fs::DirEntry| {
        e.metadata()
            .ok()
            .filter(|m| m.is_file())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > day)
    };
    let entries: Vec<_> = list.flatten().collect();
    let names: Vec<String> = entries
        .iter()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let recorded: Vec<&str> = names
        .iter()
        .filter_map(|n| n.strip_suffix(".download.json"))
        .collect();
    for (e, name) in entries.iter().zip(&names) {
        let id = name.split('.').next().unwrap_or_default();
        let record_old = recorded.contains(&id)
            && entries
                .iter()
                .zip(&names)
                .any(|(r, n)| *n == format!("{id}.download.json") && old(r));
        if record_old || (!recorded.contains(&id) && old(e)) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// The earlier download of this address, if its files are all still there
/// and its picture was asked for at least `lines` high.
pub fn earlier(dir: &Path, url: &str, lines: u32) -> Option<Downloaded> {
    let id = key(url)?;
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(record(dir, &id)).ok()?).ok()?;
    let path = |k: &str| v[k].as_str().map(PathBuf::from);
    let d = Downloaded {
        id,
        title: v["title"].as_str()?.to_string(),
        audio: path("audio")?,
        video: path("video"),
        captions: path("captions"),
        // Records from before the choice were all 480.
        lines: v["lines"].as_u64().map_or(480, |l| l as u32),
    };
    if d.lines < lines {
        return None;
    }
    let there = d.audio.exists()
        && d.video.as_ref().is_none_or(|p| p.exists())
        && d.captions.as_ref().is_none_or(|p| p.exists());
    there.then_some(d)
}

/// The captions yt-dlp wrote for `id`: hand-made in the language first, then
/// the original automatic ones, in the language or, last, in the film's own.
fn captions_for(dir: &Path, id: &str, lang: &str) -> Option<PathBuf> {
    let mut found: Vec<(u8, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let code = name
                .strip_prefix(&format!("{id}."))?
                .strip_suffix(".vtt")?
                .to_string();
            let rank = if code == lang {
                0
            } else if code.starts_with(&format!("{lang}-")) && code != format!("{lang}-orig") {
                1
            } else if code == format!("{lang}-orig") {
                2
            } else if code.ends_with("-orig") {
                3
            } else {
                return None;
            };
            Some((rank, e.path()))
        })
        .collect();
    found.sort();
    found.into_iter().next().map(|f| f.1)
}

/// yt-dlp's last error, in words a person can act on.
fn explain(errors: &[String], site: &str) -> String {
    let last = errors
        .iter()
        .rev()
        .find(|e| e.starts_with("ERROR"))
        .or(errors.last())
        .map(|s| s.trim_start_matches("ERROR:").trim().to_string())
        .unwrap_or_else(|| "it stopped without saying why".into());
    let lower = last.to_lowercase();
    let hint = if lower.contains("requested format is not available") && site != "YouTube" {
        // Our formats ask for plain HTTP only (formats()).
        " This site offers the video only as a stream in pieces, which Video Scribe cannot put \
         together without FFmpeg. Look for a download link to the video file itself, and paste \
         that instead."
    } else if lower.contains("unsupported url") {
        " yt-dlp found no video at that address. Open the video itself in the browser and copy \
         its address."
    } else if lower.contains("private video")
        || lower.contains("sign in")
        || lower.contains("logged-in")
        || lower.contains("cookies")
    {
        " The video may be private, age-restricted or need signing in, which Video Scribe cannot do."
    } else if lower.contains("unavailable") || lower.contains("not available") {
        " The video may have been removed, or not be available in this country."
    } else if lower.contains("unable to download") || lower.contains("getaddrinfo") {
        " Check the internet connection, and try again."
    } else {
        " yt-dlp may need updating: video sites change often."
    };
    format!("The video from {site} could not be downloaded. yt-dlp said: {last}{hint}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines_parse() {
        assert_eq!(
            parse_progress("VSPROGRESS none 1024 4096 NA"),
            Some(Progress {
                what: "sound",
                done: 1024,
                total: Some(4096)
            })
        );
        assert_eq!(
            parse_progress("VSPROGRESS avc1.4d401e 10 NA 5000.5")
                .unwrap()
                .total,
            Some(5000)
        );
        assert_eq!(
            parse_progress("VSPROGRESS none 10 NA NA").unwrap().total,
            None
        );
        assert_eq!(
            parse_progress("VSPROGRESS avc1 10 NA NA").unwrap().what,
            "picture"
        );
        assert_eq!(
            parse_progress("VSPROGRESS NA 10 NA NA").unwrap().what,
            "video"
        );
        assert_eq!(parse_progress("[download] 10%"), None);
    }

    #[test]
    fn the_url_comes_after_the_options_end() {
        let a = args(
            "-o evil",
            Path::new("out"),
            Path::new("none"),
            "en",
            480,
            true,
        );
        let i = a.iter().position(|s| s == "--").unwrap();
        assert_eq!(a[i + 1], "-o evil");
        assert_eq!(i + 2, a.len());
        assert!(a.iter().any(|s| s == "--write-subs"));
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--encoding" && w[1] == "utf-8"));
        // Captions refused once: asked for again without them.
        let b = args("x", Path::new("out"), Path::new("none"), "en", 480, false);
        assert!(!b.iter().any(|s| s.contains("subs")));
    }

    #[test]
    fn other_sites_get_their_own_names_and_formats() {
        let page = "https://archive.org/details/BigBuckBunny_124";
        let a = args(page, Path::new("out"), Path::new("none"), "en", 480, true);
        let k = key(page).unwrap();
        assert!(k.starts_with("archive-org-") && !k.contains('.'), "{k}");
        assert!(a.contains(&format!("{k}.f%(format_id)s.%(ext)s")));
        assert!(a.iter().any(|s| s == &formats(false, 480)));
        let yt = "https://youtu.be/_cMxraX_5RE";
        let y = args(yt, Path::new("out"), Path::new("none"), "en", 1080, true);
        assert!(y.iter().any(|s| s == "%(id)s.f%(format_id)s.%(ext)s"));
        assert!(y.iter().any(|s| s == &formats(true, 1080)));
        assert!(formats(true, 1080).contains("[height<=1080]"));
        // The same address, the same name; another, another.
        assert_eq!(key(page), Some(k.clone()));
        assert_ne!(key("https://archive.org/details/Other"), Some(k));
        assert_eq!(key(yt).as_deref(), Some("_cMxraX_5RE"));
    }

    #[test]
    fn a_stream_in_pieces_is_explained() {
        let said = |e: &str, site: &str| explain(&[format!("ERROR: [x] 1: {e}")], site);
        assert!(said("Requested format is not available.", "dailymotion.com").contains("pieces"));
        assert!(said("Requested format is not available.", "YouTube").contains("removed"));
        assert!(said(
            "The web client only works when logged-in. Use --cookies",
            "vimeo.com"
        )
        .contains("signing in"));
        assert!(said("Unsupported URL: https://example.com/", "example.com")
            .starts_with("The video from example.com could not"));
    }

    #[test]
    fn a_file_with_both_is_picture_and_sound() {
        let f = |v: &str, a: &str| {
            parse_file(&format!("{FILE} C:\\d\\x.mp4\t{v}\t{a}\tTitle")).unwrap()
        };
        let both = f("NA", "NA");
        assert!(both.picture && both.sound);
        assert_eq!(both.title, "Title");
        assert!(!f("none", "mp4a.40.2").picture);
        assert!(!f("avc1.4D401E", "none").sound);
        assert!(parse_file("[download] x").is_none());
    }

    #[test]
    fn the_checksum_list_is_read() {
        let sums = "aa  yt-dlp_linux\n\
            9CCDACC4719D8AA7E45A77536BFABEC45A03BA1F2FAD5E241AB4060B24238A85  yt-dlp.exe\n";
        assert_eq!(
            expected_hash(sums, "yt-dlp.exe").unwrap(),
            "9ccdacc4719d8aa7e45a77536bfabec45a03ba1f2fad5e241ab4060b24238a85"
        );
        assert_eq!(expected_hash(sums, "yt-dlp_x86.exe"), None);
    }

    #[test]
    fn hand_made_captions_come_before_automatic_ones() {
        let dir = std::env::temp_dir().join(format!("vs-ytdlp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("abc.en-orig.vtt"), b"").unwrap();
        assert_eq!(
            captions_for(&dir, "abc", "en"),
            Some(dir.join("abc.en-orig.vtt"))
        );
        std::fs::write(dir.join("abc.en-GB.vtt"), b"").unwrap();
        assert_eq!(
            captions_for(&dir, "abc", "en"),
            Some(dir.join("abc.en-GB.vtt"))
        );
        std::fs::write(dir.join("abc.en.vtt"), b"").unwrap();
        assert_eq!(
            captions_for(&dir, "abc", "en"),
            Some(dir.join("abc.en.vtt"))
        );
        // Described in French: the film's own automatic captions, for timing.
        assert_eq!(
            captions_for(&dir, "abc", "fr"),
            Some(dir.join("abc.en-orig.vtt"))
        );
        std::fs::write(dir.join("abc.fr.vtt"), b"").unwrap();
        assert_eq!(
            captions_for(&dir, "abc", "fr"),
            Some(dir.join("abc.fr.vtt"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// One MP4 box: (type, start of box, start of body, end).
type Mp4Box = ([u8; 4], usize, usize, usize);

fn boxes(data: &[u8], start: usize, end: usize) -> Vec<Mp4Box> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos + 8 <= end {
        let size32 = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = data[pos + 4..pos + 8].try_into().unwrap();
        let (size, body) = match size32 {
            1 if pos + 16 <= end => (
                u64::from_be_bytes(data[pos + 8..pos + 16].try_into().unwrap()) as usize,
                pos + 16,
            ),
            0 => (end - pos, pos + 8),
            n => (n, pos + 8),
        };
        if size < 8 || pos + size > end {
            break;
        }
        out.push((kind, pos, body, pos + size));
        pos += size;
    }
    out
}

/// YouTube's DASH video carries an edit list (`edts`) that shifts it by the
/// B-frame delay, and Media Foundation's MP4 reader then delivers no frames
/// at all (log entry 13). In our own downloaded copy, never the user's file,
/// the box is renamed `free`, which every reader skips. Returns the shift in
/// seconds, so that a time t on the video's clock is t + shift to Media
/// Foundation. A copy fixed on an earlier run still has its edit list inside
/// the `free` box, so the shift is read from there.
pub fn drop_edit_list(path: &Path) -> Result<f64, String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let fail = |e: std::io::Error| format!("The downloaded picture could not be prepared: {e}");
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(fail)?;
    // The moov sits at the front of these files; 64 KB is plenty.
    let mut head = vec![0u8; 65_536];
    let n = f.read(&mut head).map_err(fail)?;
    head.truncate(n);
    let Some(moov) = boxes(&head, 0, head.len())
        .into_iter()
        .find(|b| &b.0 == b"moov")
    else {
        return Ok(0.0);
    };
    let mut shift = 0.0;
    for trak in boxes(&head, moov.2, moov.3)
        .into_iter()
        .filter(|b| &b.0 == b"trak")
    {
        let inner = boxes(&head, trak.2, trak.3);
        let elst_in = |b: &Mp4Box| boxes(&head, b.2, b.3).into_iter().find(|c| &c.0 == b"elst");
        let Some((edts, elst)) = inner
            .iter()
            .filter(|b| &b.0 == b"edts" || &b.0 == b"free")
            .find_map(|b| elst_in(b).map(|e| (*b, e)))
        else {
            continue;
        };
        let timescale = inner
            .iter()
            .find(|b| &b.0 == b"mdia")
            .and_then(|m| boxes(&head, m.2, m.3).into_iter().find(|b| &b.0 == b"mdhd"))
            .map(|h| {
                let at = if head[h.2] == 1 { h.2 + 20 } else { h.2 + 12 };
                u32::from_be_bytes(head[at..at + 4].try_into().unwrap())
            })
            .unwrap_or(0);
        let v = head[elst.2];
        let count = u32::from_be_bytes(head[elst.2 + 4..elst.2 + 8].try_into().unwrap());
        if count >= 1 && timescale > 0 {
            let e = elst.2 + 8;
            let media_time = if v == 1 {
                i64::from_be_bytes(head[e + 8..e + 16].try_into().unwrap())
            } else {
                i32::from_be_bytes(head[e + 4..e + 8].try_into().unwrap()) as i64
            };
            if media_time > 0 {
                shift = media_time as f64 / timescale as f64;
            }
        }
        if &edts.0 == b"edts" {
            f.seek(SeekFrom::Start(edts.1 as u64 + 4)).map_err(fail)?;
            f.write_all(b"free").map_err(fail)?;
        }
    }
    Ok(shift)
}

#[cfg(test)]
mod edit_list_tests {
    use super::*;

    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(kind);
        b.extend_from_slice(body);
        b
    }

    #[test]
    fn the_video_id_is_found_in_every_kind_of_address() {
        for url in [
            "https://www.youtube.com/watch?v=_cMxraX_5RE",
            "https://youtube.com/watch?feature=share&v=_cMxraX_5RE&t=10",
            "https://youtu.be/_cMxraX_5RE?si=abc",
            "https://www.youtube.com/shorts/_cMxraX_5RE",
        ] {
            assert_eq!(video_id(url).as_deref(), Some("_cMxraX_5RE"), "{url}");
        }
        assert_eq!(video_id("https://www.youtube.com/watch?v=short"), None);
    }

    #[test]
    fn a_download_is_used_again_until_it_is_forgotten() {
        let dir = std::env::temp_dir().join(format!("vs-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = |n: &str| {
            let p = dir.join(n);
            std::fs::write(&p, b"x").unwrap();
            p
        };
        let d = Downloaded {
            id: "_cMxraX_5RE".into(),
            title: "Sprite Fright".into(),
            audio: file("_cMxraX_5RE.f140.m4a"),
            video: Some(file("_cMxraX_5RE.f135.mp4")),
            captions: Some(file("_cMxraX_5RE.en.vtt")),
            lines: 720,
        };
        let other = file("pNPPVCPuWG8.en.vtt");
        keep_record(&dir, &d);
        let url = "https://youtu.be/_cMxraX_5RE";
        assert_eq!(
            earlier(&dir, url, 480).map(|e| e.title),
            Some("Sprite Fright".into())
        );
        tidy(&dir);
        assert!(
            earlier(&dir, url, 720).is_some(),
            "nothing is a day old yet"
        );
        assert!(
            earlier(&dir, url, 1080).is_none(),
            "a sharper picture is wanted"
        );
        forget(&dir, "_cMxraX_5RE");
        assert!(earlier(&dir, url, 480).is_none());
        assert!(!d.audio.exists() && other.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_edit_list_goes_and_its_shift_is_still_known_after() {
        // elst version 0: one entry, duration 0, media time 1001 at 30000 a second.
        let mut elst = vec![0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        elst.extend_from_slice(&1001i32.to_be_bytes());
        elst.extend_from_slice(&[0, 1, 0, 0]);
        let mut mdhd = vec![0u8; 12];
        mdhd.extend_from_slice(&30000u32.to_be_bytes());
        mdhd.extend_from_slice(&[0u8; 8]);
        let trak = [
            mp4_box(b"edts", &mp4_box(b"elst", &elst)),
            mp4_box(b"mdia", &mp4_box(b"mdhd", &mdhd)),
        ]
        .concat();
        let file = [
            mp4_box(b"ftyp", b"isom"),
            mp4_box(b"moov", &mp4_box(b"trak", &trak)),
        ]
        .concat();
        let path = std::env::temp_dir().join(format!("vs-edts-{}.mp4", std::process::id()));
        std::fs::write(&path, &file).unwrap();
        let want = 1001.0 / 30000.0;
        assert!((drop_edit_list(&path).unwrap() - want).abs() < 1e-9);
        let after = std::fs::read(&path).unwrap();
        assert!(!after.windows(4).any(|w| w == b"edts"));
        assert_eq!(after.len(), file.len());
        // A second run finds the shift inside the renamed box.
        assert!((drop_edit_list(&path).unwrap() - want).abs() < 1e-9);
        let _ = std::fs::remove_file(&path);
    }
}
