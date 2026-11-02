//! Spike 0(a): are there gaps to describe into? (PLAN.md §1, §4.1)
//!
//! Runs the VAD over a 16 kHz mono WAV and, given captions, compares the two
//! and tries the three merge strategies: union, captions only, intersection.

use crate::vad;

pub type Spans = Vec<(f64, f64)>;

/// Sorted, overlapping or touching spans joined.
pub fn merge(mut v: Spans) -> Spans {
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Spans = Vec::new();
    for (s, e) in v {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

fn intersect(a: &Spans, b: &Spans) -> Spans {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let s = a[i].0.max(b[j].0);
        let e = a[i].1.min(b[j].1);
        if e > s {
            out.push((s, e));
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// Captions lead (log entry 15): each cue is cut down to the VAD speech inside it, so the cue padding
/// goes; a cue the VAD barely hears is kept whole, since it is still a line someone says. VAD speech
/// outside every cue is laughter, shouts, effects or music, which descriptions may sit next to, so it is
/// dropped unless `keep_uncued` is at least as long as it.
pub fn captions_led(vad: &Spans, cues: &Spans, keep_uncued: f64) -> Spans {
    let mut out = Vec::new();
    for &(s, e) in cues {
        let heard = intersect(vad, &vec![(s, e)]);
        if total(&heard) < 0.3 * (e - s) {
            out.push((s, e));
        } else {
            out.push((heard[0].0, heard[heard.len() - 1].1));
        }
    }
    for &(s, e) in vad {
        if e - s >= keep_uncued && total(&intersect(cues, &vec![(s, e)])) == 0.0 {
            out.push((s, e));
        }
    }
    merge(out)
}

fn total(v: &Spans) -> f64 {
    // An empty f64 sum is -0.0, which prints as "-0.0%".
    v.iter().map(|(s, e)| e - s).sum::<f64>() + 0.0
}

pub fn invert(speech: &Spans, secs: f64) -> Spans {
    let mut gaps = Vec::new();
    let mut at = 0.0;
    for &(s, e) in speech {
        if s > at {
            gaps.push((at, s));
        }
        at = at.max(e);
    }
    if secs > at {
        gaps.push((at, secs));
    }
    gaps
}

fn summary(label: &str, speech: &Spans, secs: f64) {
    let gaps = invert(speech, secs);
    let mins = secs / 60.0;
    let mut cols = Vec::new();
    for min in [1.0, 2.0, 3.0, 5.0] {
        let g: Vec<_> = gaps.iter().filter(|(s, e)| e - s >= min).collect();
        let t: f64 = g.iter().map(|(s, e)| e - s).sum::<f64>() + 0.0;
        cols.push(format!(
            ">={min:.0}s: {:>3} ({:4.1}/min, {:5.0}s)",
            g.len(),
            g.len() as f64 / mins,
            t
        ));
    }
    println!(
        "{label:<14} protected {:5.1}%  {}",
        100.0 * total(speech) / secs,
        cols.join("  ")
    );
}

pub fn run(wav: &str, captions: Option<&str>, list: bool) -> Result<(), String> {
    let (vad_speech, secs) = vad::detect(wav, None)?;
    println!("file      {wav}  ({:.1} min)", secs / 60.0);
    // Silero's 100 ms min-silence splits speech at every breath; for gaps we
    // only count a second or more, so the summary lines are unaffected.
    summary("vad", &vad_speech, secs);
    if let Some(path) = captions {
        let cap = speech_from(path)?;
        let both = intersect(&vad_speech, &cap);
        println!(
            "agreement     captions say speech for {:.0} s; the VAD agrees on {:.0}% of it. VAD speech is {:.0} s; captions cover {:.0}% of it.",
            total(&cap),
            100.0 * total(&both) / total(&cap).max(0.001),
            total(&vad_speech),
            100.0 * total(&both) / total(&vad_speech).max(0.001)
        );
        if list {
            // Where they disagree: cues the VAD mostly missed, and VAD speech with no cue.
            for &(s, e) in &cap {
                let heard = total(&intersect(&vad_speech, &vec![(s, e)]));
                if heard < 0.3 * (e - s) {
                    println!(
                        "  cue, VAD hears {:3.0}%   {} - {}  {:.1} s",
                        100.0 * heard / (e - s),
                        clock(s),
                        clock(e),
                        e - s
                    );
                }
            }
            for &(s, e) in &vad_speech {
                let cued = total(&intersect(&cap, &vec![(s, e)]));
                if cued == 0.0 && e - s >= 0.5 {
                    println!(
                        "  VAD speech, no cue     {} - {}  {:.1} s",
                        clock(s),
                        clock(e),
                        e - s
                    );
                }
            }
        }
        summary("captions only", &cap, secs);
        summary(
            "union",
            &merge(vad_speech.iter().chain(cap.iter()).copied().collect()),
            secs,
        );
        summary("intersection", &both, secs);
    }
    if list {
        for (s, e) in invert(&vad_speech, secs)
            .iter()
            .filter(|(s, e)| e - s >= 1.0)
        {
            println!("  gap {} - {}  {:.1} s", clock(*s), clock(*e), e - s);
        }
    }
    Ok(())
}

fn speech_from(path: &str) -> Result<Spans, String> {
    crate::vtt::speech(path)
}

pub fn clock(t: f64) -> String {
    // Round to tenths first, so 119.97 is 02:00.0 rather than 01:60.0.
    let tenths = (t * 10.0).round() as u64;
    format!(
        "{:02}:{:02}.{}",
        tenths / 600,
        tenths / 10 % 60,
        tenths % 10
    )
}
