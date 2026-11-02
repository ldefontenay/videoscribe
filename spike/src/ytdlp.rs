//! Spike 0(d): yt-dlp from Rust (PLAN.md §4.5, §8).
//!
//! What 0(d)'s first half found (log entry 6): without a JavaScript runtime YouTube offers no muxed
//! format worth having, so the picture and the sound are downloaded as **two files and never merged**,
//! which would need FFmpeg. Media Foundation reads each on its own (spike 0(e)). FFmpeg is kept out on
//! purpose: `--ffmpeg-location` points at an empty folder, so a machine that happens to have FFmpeg
//! behaves like one that does not.
//!
//! Progress comes from `--progress-template` on its own lines; cancel kills the process and removes
//! the `.part` files it leaves.

use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Marks our progress lines so nothing else yt-dlp prints is mistaken for one.
const TAG: &str = "VSPROGRESS";

pub fn exe() -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    base.join("VideoScribe").join("tools").join("yt-dlp.exe")
}

#[derive(Debug, PartialEq)]
pub struct Progress {
    pub format: String,
    pub done: u64,
    pub total: Option<u64>,
}

/// One `--progress-template` line: `VSPROGRESS <format_id> <downloaded> <total> <estimate>`,
/// where yt-dlp writes `NA` for a value it does not know.
pub fn parse_progress(line: &str) -> Option<Progress> {
    let mut it = line.trim().strip_prefix(TAG)?.split_whitespace();
    let format = it.next()?.to_string();
    let done = it.next()?.parse().ok()?;
    let num = |s: Option<&str>| s.and_then(|s| s.parse::<f64>().ok()).map(|v| v as u64);
    let total = num(it.next());
    let estimate = num(it.next());
    Some(Progress {
        format,
        done,
        total: total.or(estimate),
    })
}

pub fn args(url: &str, out_dir: &str, no_ffmpeg_dir: &str) -> Vec<String> {
    [
        "--ignore-config",
        "--no-playlist",
        "--newline",
        // "--print" below implies quiet mode, which hides progress; this brings it back.
        "--progress",
        "--no-colors",
        "--retries", "5",
        "--fragment-retries", "5",
        // H.264 so Media Foundation needs no Store extension; at most 720p, enough for descriptions.
        "-f", "bv*[vcodec^=avc1][height<=720]/bv*[height<=720],ba[ext=m4a]/ba",
        "--ffmpeg-location", no_ffmpeg_dir,
        "--write-subs", "--write-auto-subs",
        // Hand-made English and the original automatic captions only; "en.*" also asks for every
        // machine translation into English, one request each, and YouTube answers with 429s.
        "--sub-langs", "en,en-orig,en-GB,en-US",
        "--sub-format", "vtt",
        "--progress-template", &format!("download:{TAG} %(info.format_id)s %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s"),
        "-P", out_dir,
        // The id, never the title: titles become file names the app would otherwise have to clean.
        "-o", "%(id)s.f%(format_id)s.%(ext)s",
        "--print", "after_move:VSFILE %(filepath)s",
        "--", url,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Runs yt-dlp, printing progress; `cancel_after` kills it to test cancelling.
pub fn download(
    url: &str,
    out_dir: &str,
    cancel_after: Option<Duration>,
) -> Result<Vec<String>, String> {
    let empty = format!("{out_dir}\\no-ffmpeg");
    std::fs::create_dir_all(&empty).map_err(|e| e.to_string())?;
    let mut child = Command::new(exe())
        .args(args(url, out_dir, &empty))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("could not start yt-dlp: {e}"))?;
    let stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
            .collect::<Vec<_>>()
    });
    let cancel = Arc::new(AtomicBool::new(false));
    let pid = child.id();
    if let Some(after) = cancel_after {
        let c = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(after);
            c.store(true, Ordering::SeqCst);
            // The app would hold the Child; here a second handle by id does the same job.
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
        });
    }
    let t = Instant::now();
    let mut files = Vec::new();
    let mut last_report = Instant::now() - Duration::from_secs(10);
    for line in BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map_while(Result::ok)
    {
        if let Some(p) = parse_progress(&line) {
            if last_report.elapsed() >= Duration::from_secs(1) || p.total == Some(p.done) {
                let pct = p
                    .total
                    .map(|t| format!("{:.0}%", p.done as f64 * 100.0 / t.max(1) as f64))
                    .unwrap_or("?".into());
                println!(
                    "  {:6.1}s format {} {} of {}",
                    t.elapsed().as_secs_f64(),
                    p.format,
                    pct,
                    p.total.map(|t| t.to_string()).unwrap_or("?".into())
                );
                last_report = Instant::now();
            }
        } else if let Some(f) = line.strip_prefix("VSFILE ") {
            files.push(f.to_string());
        } else if !line.trim().is_empty() {
            println!("  | {line}");
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let errs = errors.join().unwrap_or_default();
    for e in errs.iter().filter(|e| !e.trim().is_empty()) {
        println!("  ! {e}");
    }
    if cancel.load(Ordering::SeqCst) {
        let mut removed = 0;
        for entry in std::fs::read_dir(out_dir)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".part") || name.ends_with(".ytdl") || name.contains(".part-Frag") {
                if std::fs::remove_file(entry.path()).is_ok() {
                    removed += 1;
                }
            }
        }
        return Err(format!(
            "cancelled after {:.1}s; removed {removed} partial files",
            t.elapsed().as_secs_f64()
        ));
    }
    if !status.success() {
        return Err(format!("yt-dlp failed ({status})"));
    }
    println!("done in {:.1}s", t.elapsed().as_secs_f64());
    Ok(files)
}

/// `ytdlp <url> <out_dir> [cancel_seconds]` | `ytdlp fix <video.mp4>`
pub fn run(args: &[&str]) -> Result<(), String> {
    match args {
        ["fix", path] => println!(
            "edit list removed; video runs {:.4} s late",
            drop_edit_list(path)?
        ),
        [url, out] => {
            for f in download(url, out, None)? {
                println!("file: {f}");
            }
        }
        [url, out, secs] => {
            let s: f64 = secs.parse().map_err(|_| "seconds")?;
            match download(url, out, Some(Duration::from_secs_f64(s))) {
                Ok(_) => println!("finished before the cancel"),
                Err(e) => println!("{e}"),
            }
            let left: Vec<String> = std::fs::read_dir(out)
                .map_err(|e| e.to_string())?
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            println!("left in folder: {left:?}");
        }
        _ => return Err("usage: spike ytdlp <url> <out_dir> [cancel_after_seconds]".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines_parse() {
        assert_eq!(
            parse_progress("VSPROGRESS 136 1024 4096 NA"),
            Some(Progress {
                format: "136".into(),
                done: 1024,
                total: Some(4096)
            })
        );
        assert_eq!(
            parse_progress("VSPROGRESS 140 10 NA 5000.5"),
            Some(Progress {
                format: "140".into(),
                done: 10,
                total: Some(5000)
            })
        );
        assert_eq!(
            parse_progress("VSPROGRESS 140 10 NA NA").unwrap().total,
            None
        );
        assert_eq!(parse_progress("[download] 10%"), None);
    }

    #[test]
    fn the_url_comes_after_the_options_end() {
        let a = args("-o evil", "out", "none");
        let i = a.iter().position(|s| s == "--").unwrap();
        assert_eq!(a[i + 1], "-o evil");
        assert_eq!(i + 2, a.len());
    }
}

/// One MP4 box: (type, start of box, start of body, end).
fn boxes(data: &[u8], start: usize, end: usize) -> Vec<([u8; 4], usize, usize, usize)> {
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

/// YouTube's DASH video carries an edit list (`edts`) that shifts it by the B-frame delay, and Media
/// Foundation's MP4 reader then delivers no frames at all (log entry 13). In our own downloaded copy,
/// rename the box to `free`, which every reader skips, and return the shift in seconds so timestamps
/// can be corrected: a frame Media Foundation calls t is really at t - shift.
/// Only the box's 4-byte type changes; sizes and offsets stay as they were.
pub fn drop_edit_list(path: &str) -> Result<f64, String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    // The moov sits at the front of these files; 64 KB is plenty and spares reading 50 MB.
    let mut head = vec![0u8; 65_536];
    let n = f.read(&mut head).map_err(|e| e.to_string())?;
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
        let Some(edts) = inner.iter().find(|b| &b.0 == b"edts") else {
            continue;
        };
        let timescale = inner
            .iter()
            .find(|b| &b.0 == b"mdia")
            .and_then(|m| boxes(&head, m.2, m.3).into_iter().find(|b| &b.0 == b"mdhd"))
            .map(|h| {
                let v = head[h.2];
                let at = if v == 1 { h.2 + 4 + 16 } else { h.2 + 4 + 8 };
                u32::from_be_bytes(head[at..at + 4].try_into().unwrap())
            })
            .unwrap_or(0);
        if let Some(elst) = boxes(&head, edts.2, edts.3)
            .into_iter()
            .find(|b| &b.0 == b"elst")
        {
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
        }
        f.seek(SeekFrom::Start(edts.1 as u64 + 4))
            .map_err(|e| e.to_string())?;
        f.write_all(b"free").map_err(|e| e.to_string())?;
    }
    Ok(shift)
}
