//! Caption cues as speech (PLAN.md §5.3): YouTube's captions from yt-dlp, or
//! a `.srt` or `.vtt` file beside a video on this computer.
//!
//! Hand-made captions are one cue per line of dialogue: the cue is the speech.
//! YouTube's automatic captions roll, repeating each line in the next cue, but
//! tag each new word with its start time (`<00:00:05.040>`); for those, each
//! word runs from its start to the next word's, capped, and the cue boundaries
//! are ignored (spike 0(a), log entry 5).

use std::path::{Path, PathBuf};

use crate::speech::{merge, Spans};

/// A word with no successor is taken to last at most this long.
const MAX_WORD: f64 = 1.0;

pub struct Captions {
    /// Where people speak.
    pub speech: Spans,
    /// What they say, cue by cue, for the describer (milestone 4).
    pub cues: Vec<(f64, f64, String)>,
    /// YouTube's machine captions, not a person's.
    pub automatic: bool,
}

/// Reads a `.vtt` or `.srt` file. None when it holds no spoken cue at all.
pub fn read(path: &Path) -> Result<Option<Captions>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(parse(&text))
}

pub fn parse(text: &str) -> Option<Captions> {
    let text = text.trim_start_matches('\u{feff}');
    let raw = raw_cues(text);
    let automatic = raw
        .iter()
        .any(|(_, _, b)| b.iter().any(|l| l.contains("<c>")));
    let mut speech = Vec::new();
    let mut cues = Vec::new();
    if automatic {
        // Word starts, in order; the first word of a tagged line starts at the cue.
        let mut words: Vec<(f64, f64)> = Vec::new(); // (start, cue end)
        for (start, end, body) in &raw {
            for l in body.iter().filter(|l| l.contains("<c>")) {
                words.push((*start, *end));
                let mut rest = l.as_str();
                while let Some(i) = rest.find('<') {
                    let tag_end = rest[i..].find('>').map(|j| i + j).unwrap_or(rest.len() - 1);
                    if let Some(t) = time(&rest[i + 1..tag_end]) {
                        words.push((t, *end));
                    }
                    rest = &rest[tag_end + 1..];
                }
                cues.push((*start, *end, said(&[strip_tags(l)])));
            }
        }
        words.sort_by(|a, b| a.0.total_cmp(&b.0));
        for i in 0..words.len() {
            let (s, cue_end) = words[i];
            let next = words.get(i + 1).map(|w| w.0).unwrap_or(f64::MAX);
            let e = next.min(cue_end).min(s + MAX_WORD);
            if e > s {
                speech.push((s, e));
            }
        }
    } else {
        for (s, e, body) in raw {
            let words = said(&body.iter().map(|l| strip_tags(l)).collect::<Vec<_>>());
            if !spoken(&words) || e <= s {
                continue;
            }
            speech.push((s, e));
            cues.push((s, e, words));
        }
    }
    cues.retain(|c| !c.2.is_empty());
    if speech.is_empty() {
        return None;
    }
    Some(Captions {
        speech: merge(speech),
        cues,
        automatic,
    })
}

/// A cue that only names the language, or only marks music, is no one speaking.
fn spoken(words: &str) -> bool {
    let w = words.trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace());
    let only_sound = words.starts_with('[') && words.ends_with(']')
        || words.starts_with('(') && words.ends_with(')')
        || words
            .chars()
            .all(|c| c == '♪' || c == '♫' || c.is_whitespace());
    // Spring's captions are empty cues and a language label (log entry 5).
    !w.is_empty() && w != "ENGLISH" && !only_sound
}

/// Every cue as (start, end, lines), WebVTT or SubRip.
fn raw_cues(text: &str) -> Vec<(f64, f64, Vec<String>)> {
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((a, b)) = line.split_once("-->") else {
            continue;
        };
        let (Some(s), Some(e)) = (
            time(a.trim()),
            time(b.split_whitespace().next().unwrap_or("")),
        ) else {
            continue;
        };
        let mut body = Vec::new();
        while let Some(l) = lines.peek() {
            if l.trim().is_empty() || l.contains("-->") {
                break;
            }
            body.push(lines.next().unwrap_or_default().to_string());
        }
        out.push((s, e, body));
    }
    out
}

fn said(lines: &[String]) -> String {
    lines
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_tags(l: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in l.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

/// `00:01:02.345`, `01:02.345`, or SubRip's `00:01:02,345`, to seconds.
fn time(s: &str) -> Option<f64> {
    let s = s.replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() < 2 {
        return None;
    }
    let mut total: f64 = parts.last()?.parse().ok()?;
    let mut mult = 60.0;
    for p in parts.iter().rev().skip(1) {
        total += p.parse::<f64>().ok()? * mult;
        mult *= 60.0;
    }
    Some(total)
}

/// A captions file beside a video: `party.srt` or `party.vtt`, or one with a
/// language in its name, `party.en.srt`, preferring the description language,
/// then any other (`party.fr.srt` for a French film described in English).
pub fn beside(video: &Path, lang: &str) -> Option<PathBuf> {
    let stem = video.file_stem()?.to_string_lossy().to_lowercase();
    let dir = video.parent().filter(|d| !d.as_os_str().is_empty())?;
    let mut found: Vec<(u8, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let Some(rest) = name.strip_prefix(&stem) else {
            continue;
        };
        let rank = match rest {
            ".srt" | ".vtt" => 0,
            r if r == format!(".{lang}.srt") || r == format!(".{lang}.vtt") => 1,
            r if r.starts_with(&format!(".{lang}-"))
                && (r.ends_with(".srt") || r.ends_with(".vtt")) =>
            {
                2
            }
            r if r.starts_with('.')
                && r.matches('.').count() == 2
                && (r.ends_with(".srt") || r.ends_with(".vtt")) =>
            {
                3
            }
            _ => continue,
        };
        found.push((rank, path));
    }
    found.sort();
    found.into_iter().next().map(|f| f.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subrip_cues_are_speech() {
        let srt = "1\r\n00:00:01,000 --> 00:00:02,500\r\nHello there.\r\n\r\n\
                   2\r\n00:00:04,000 --> 00:00:05,000\r\n[MUSIC]\r\n\r\n\
                   3\r\n00:00:06,000 --> 00:00:07,000\r\n<i>Over here!</i>\r\n";
        let c = parse(srt).unwrap();
        assert_eq!(c.speech, vec![(1.0, 2.5), (6.0, 7.0)]);
        assert_eq!(c.cues[1].2, "Over here!");
        assert!(!c.automatic);
    }

    #[test]
    fn rolling_automatic_captions_use_the_word_times() {
        let vtt = "WEBVTT\n\n\
            00:00:01.000 --> 00:00:03.000 align:start position:0%\n\
            hello<00:00:01.500><c> there</c><00:00:02.000><c> friend</c>\n\n\
            00:00:03.000 --> 00:00:03.010\nhello there friend\n\n";
        let c = parse(vtt).unwrap();
        assert!(c.automatic);
        assert_eq!(c.speech, vec![(1.0, 3.0)]);
        assert_eq!(c.cues.len(), 1);
        assert_eq!(c.cues[0].2, "hello there friend");
    }

    #[test]
    fn a_file_with_only_a_language_label_is_no_captions() {
        assert!(parse("WEBVTT\n\n00:00:00.000 --> 00:00:05.000\nENGLISH\n").is_none());
    }

    #[test]
    fn captions_beside_a_video_are_found() {
        let dir = std::env::temp_dir().join(format!("vs-captions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("Party.mp4");
        std::fs::write(&video, b"").unwrap();
        assert_eq!(beside(&video, "en"), None);
        std::fs::write(dir.join("party.en.vtt"), b"").unwrap();
        std::fs::write(dir.join("party.fr.vtt"), b"").unwrap();
        assert_eq!(beside(&video, "en"), Some(dir.join("party.en.vtt")));
        assert_eq!(beside(&video, "fr"), Some(dir.join("party.fr.vtt")));
        // Described in German: the film's own, whichever it is.
        assert!(beside(&video, "de").is_some());
        std::fs::write(dir.join("Party.srt"), b"").unwrap();
        assert_eq!(beside(&video, "en"), Some(dir.join("Party.srt")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
