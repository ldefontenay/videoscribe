//! Caption cues from a WebVTT file, as speech intervals.
//!
//! Hand-made captions are one cue per line of dialogue: the cue is the
//! interval. YouTube's automatic captions roll, repeating each line in the
//! next cue, but tag each new word with its start time (`<00:00:05.040>`);
//! for those, every word runs from its start to the next word's, capped, and
//! the cue boundaries are ignored.

/// A word with no successor is assumed to last at most this long.
const MAX_WORD: f64 = 1.0;

pub fn speech(path: &str) -> Result<Vec<(f64, f64)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut cues: Vec<(f64, f64, Vec<String>)> = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some((a, b)) = line.split_once("-->") {
            let start = time(a.trim());
            let end = time(b.trim().split_whitespace().next().unwrap_or(""));
            let (Some(start), Some(end)) = (start, end) else {
                continue;
            };
            let mut body = Vec::new();
            while let Some(l) = lines.peek() {
                if l.trim().is_empty() && !body.is_empty() {
                    break;
                }
                if l.contains("-->") {
                    break;
                }
                body.push(lines.next().unwrap().to_string());
            }
            cues.push((start, end, body));
        }
    }
    let rolling = cues
        .iter()
        .any(|(_, _, b)| b.iter().any(|l| l.contains("<c>")));
    let mut out = Vec::new();
    if rolling {
        // Word starts, in order; the first word of a tagged line starts at the cue.
        let mut words: Vec<(f64, f64)> = Vec::new(); // (start, cue end)
        for (start, end, body) in &cues {
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
            }
        }
        words.sort_by(|a, b| a.0.total_cmp(&b.0));
        for i in 0..words.len() {
            let (s, cue_end) = words[i];
            let next = words.get(i + 1).map(|w| w.0).unwrap_or(f64::MAX);
            let e = next.min(cue_end).min(s + MAX_WORD);
            if e > s {
                out.push((s, e));
            }
        }
    } else {
        for (s, e, body) in cues {
            let said: String = body.join(" ");
            let said = said.trim();
            // Spring's captions are empty cues and a language label: not speech.
            if !said.is_empty() && said != "ENGLISH" && e > s {
                out.push((s, e));
            }
        }
    }
    Ok(crate::gaps::merge(out))
}

/// Hand-made cues with their text, tags removed, for giving a service the dialogue (log entry 15).
/// Rolling automatic captions repeat each line, so for those only the lines with word tags are kept.
pub fn cues(path: &str) -> Result<Vec<(f64, f64, String)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let rolling = text.contains("<c>");
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((a, b)) = line.split_once("-->") else {
            continue;
        };
        let (Some(s), Some(e)) = (
            time(a.trim()),
            time(b.trim().split_whitespace().next().unwrap_or("")),
        ) else {
            continue;
        };
        let mut body = Vec::new();
        while let Some(l) = lines.peek() {
            if l.trim().is_empty() || l.contains("-->") {
                break;
            }
            let l = lines.next().unwrap();
            if !rolling || l.contains("<c>") {
                body.push(strip_tags(l));
            }
        }
        let said = body
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !said.is_empty() && said != "ENGLISH" {
            out.push((s, e, said));
        }
    }
    Ok(out)
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

/// `00:01:02.345` or `01:02.345` to seconds.
fn time(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    let secs: f64 = parts.last()?.parse().ok()?;
    let mut total = secs;
    let mut mult = 60.0;
    for p in parts.iter().rev().skip(1) {
        total += p.parse::<f64>().ok()? * mult;
        mult *= 60.0;
    }
    Some(total)
}
