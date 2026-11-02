//! What was worked out from a sound file, kept so that starting the same video
//! again skips the slow steps (test round 3, 6.4): the speech the detector
//! heard, and which pause check file was made from which pauses.
//!
//! Kept in %LOCALAPPDATA%\Video Scribe\cache, one small JSON file per sound
//! file, named by a hash of its size and its first and last megabyte. A
//! changed file gets a new name, so nothing stale is ever used.

use crate::net;
use crate::speech::{Gap, Spans};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Bump when the detector or its settings change, so old results are not reused.
const VERSION: &str = "silero-5 v3, afresh each batch";

fn entry(sound: &Path) -> Option<PathBuf> {
    use std::io::{Read, Seek, SeekFrom};
    // By content, not path or time: a YouTube video downloaded again is the
    // same sound with a new time. Its size and first and last megabyte are
    // enough to tell sound files apart, and quick to read.
    const SAMPLE: u64 = 1 << 20;
    let mut f = std::fs::File::open(sound).ok()?;
    let len = f.metadata().ok()?.len();
    let mut bytes = format!(
        "{VERSION}
{len}
"
    )
    .into_bytes();
    let mut head = Vec::new();
    (&mut f).take(SAMPLE).read_to_end(&mut head).ok()?;
    bytes.extend(head);
    if len > SAMPLE {
        f.seek(SeekFrom::Start(len.saturating_sub(SAMPLE).max(SAMPLE)))
            .ok()?;
        f.read_to_end(&mut bytes).ok()?;
    }
    Some(
        crate::ytdlp::local_dir()
            .join("cache")
            .join(format!("{}.json", &net::hex(&net::sha256(&bytes))[..32])),
    )
}

fn load(sound: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(entry(sound)?).ok()?).ok()
}

/// Written beside and renamed, so a half-written entry is never read.
fn save(sound: &Path, v: &Value) {
    let Some(path) = entry(sound) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let part = path.with_extension("part");
    if std::fs::write(&part, v.to_string()).is_ok() {
        let _ = std::fs::rename(&part, &path);
    }
}

/// The length and the speech heard, if this sound was listened to before.
pub fn heard(sound: &Path) -> Option<(f64, Spans)> {
    let v = load(sound)?;
    let secs = v["secs"].as_f64()?;
    let spans = v["heard"]
        .as_array()?
        .iter()
        .map(|p| Some((p[0].as_f64()?, p[1].as_f64()?)))
        .collect::<Option<Spans>>()?;
    Some((secs, spans))
}

pub fn keep_heard(sound: &Path, secs: f64, spans: &Spans) {
    let mut v = load(sound).unwrap_or_else(|| json!({}));
    v["secs"] = json!(secs);
    v["heard"] = json!(spans.iter().map(|s| [s.0, s.1]).collect::<Vec<_>>());
    save(sound, &v);
}

fn gaps_print(gaps: &[Gap]) -> String {
    let s: String = gaps
        .iter()
        .map(|g| format!("{:.3} {:.3}\n", g.start, g.end))
        .collect();
    net::hex(&net::sha256(s.as_bytes()))
}

/// Whether `check` is still there and was made from this sound with these pauses.
pub fn check_done(sound: &Path, check: &Path, gaps: &[Gap]) -> bool {
    check.exists()
        && load(sound).is_some_and(|v| {
            v["checks"][check.to_string_lossy().as_ref()] == gaps_print(gaps).as_str()
        })
}

pub fn check_made(sound: &Path, check: &Path, gaps: &[Gap]) {
    let mut v = load(sound).unwrap_or_else(|| json!({}));
    if !v["checks"].is_object() {
        v["checks"] = json!({});
    }
    v["checks"][check.to_string_lossy().as_ref()] = json!(gaps_print(gaps));
    save(sound, &v);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_heard_comes_back_and_a_changed_file_is_not_reused() {
        let dir = std::env::temp_dir().join(format!("vs-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("LOCALAPPDATA", &dir);
        let sound = dir.join("a.m4a");
        std::fs::write(&sound, b"one").unwrap();
        assert!(heard(&sound).is_none());
        keep_heard(&sound, 12.5, &vec![(1.0, 2.5), (4.0, 6.0)]);
        assert_eq!(heard(&sound), Some((12.5, vec![(1.0, 2.5), (4.0, 6.0)])));

        let check = dir.join("a - pause check.mp3");
        std::fs::write(&check, b"mp3").unwrap();
        let gaps = crate::speech::plan_gaps(&[(1.0, 2.5)], 12.5);
        assert!(!check_done(&sound, &check, &gaps));
        check_made(&sound, &check, &gaps);
        assert!(check_done(&sound, &check, &gaps));
        let other = crate::speech::plan_gaps(&[(1.0, 3.5)], 12.5);
        assert!(!check_done(&sound, &check, &other));

        std::fs::write(&sound, b"changed").unwrap();
        assert!(heard(&sound).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
