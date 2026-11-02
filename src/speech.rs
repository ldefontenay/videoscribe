//! The speech map: where people speak, and the pauses between, where a
//! description may go (PLAN.md §5.3).
//!
//! Captions and the VAD both give speech as a list of spans, so nothing after
//! the merge knows which ran. With captions, the captions lead (log entry 15);
//! without them, the VAD's speech is the map. Then the map is inverted once
//! into pauses, and long pauses are cut into parts.

/// Spans of time in seconds, `(start, end)`.
pub type Spans = Vec<(f64, f64)>;

/// A pause shorter than this gets no description (log entry 15).
pub const MIN_GAP: f64 = 1.0;
/// Long pauses are cut into equal parts of at most this, so a description
/// comes about every five seconds, as in the reference (log entry 15).
pub const MAX_PART: f64 = 5.0;
/// A cue the VAD hears less of than this is kept whole (log entry 15).
const HEARD: f64 = 0.3;

/// Sorted, with overlapping or touching spans joined.
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

/// Where two sorted lists of spans overlap.
pub fn intersect(a: &[(f64, f64)], b: &[(f64, f64)]) -> Spans {
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

pub fn total(v: &[(f64, f64)]) -> f64 {
    // An empty f64 sum is -0.0, which prints as "-0".
    v.iter().map(|(s, e)| e - s).sum::<f64>() + 0.0
}

/// The silences between sorted speech spans, from 0 to `secs`.
pub fn invert(speech: &[(f64, f64)], secs: f64) -> Spans {
    let mut gaps = Vec::new();
    let mut at = 0.0;
    for &(s, e) in speech {
        if s > at {
            gaps.push((at, s.min(secs)));
        }
        at = f64::max(at, e);
    }
    if secs > at {
        gaps.push((at, secs));
    }
    gaps.retain(|(s, e)| e > s);
    gaps
}

/// Captions lead (log entry 15): each cue is cut down to the VAD speech inside
/// it, so the cue's padding goes; a cue the VAD barely hears is kept whole,
/// since it is still a line someone says. VAD speech outside every cue is
/// laughter, shouts, effects or music, which a description may sit beside, so
/// it is dropped.
pub fn captions_led(vad: &[(f64, f64)], cues: &[(f64, f64)]) -> Spans {
    let mut out = Vec::new();
    for &(s, e) in cues {
        let heard = intersect(vad, &[(s, e)]);
        if total(&heard) < HEARD * (e - s) {
            out.push((s, e));
        } else {
            out.push((heard[0].0, heard[heard.len() - 1].1));
        }
    }
    merge(out)
}

/// How much two maps agree: the share of `a`'s time that `b` also calls speech.
pub fn agreement(a: &[(f64, f64)], b: &[(f64, f64)]) -> f64 {
    let t = total(a);
    if t <= 0.0 {
        return 1.0;
    }
    total(&intersect(a, b)) / t
}

/// Pauses shorter than this are optional: described only when something
/// important happens (log entry 24, after SonarPad Mobile's 1.5 s).
pub const OPTIONAL_BELOW: f64 = 1.5;

/// A place for one description.
#[derive(Clone, Debug, PartialEq)]
pub struct Gap {
    pub start: f64,
    pub end: f64,
    /// The most words that fit, at the house rate.
    pub words: usize,
    /// Part `n` of `of`, when a long pause is cut up.
    pub part: Option<(usize, usize)>,
}

impl Gap {
    pub fn optional(&self) -> bool {
        self.end - self.start < OPTIONAL_BELOW
    }
}

/// Word limit for a pause: floor((seconds - 0.4) * 3), at least 1. Edge
/// voices speak 3.0 to 3.5 words a second but pause at full stops. It was 2.5
/// (log entry 15) until each description was measured as spoken, and sped up
/// or spilled over the pause's edges to fit (milestone 7 test, log entry 29).
pub const WORDS_A_SECOND: f64 = 3.0;

pub fn word_limit(secs: f64) -> usize {
    // The small nudge stops a whole number landing just below it.
    (((secs - 0.4) * WORDS_A_SECOND + 1e-9).floor() as i64).max(1) as usize
}

/// The silences at least MIN_GAP long, long ones cut into equal parts of at
/// most MAX_PART seconds. Times stay on the video's clock.
pub fn plan_gaps(speech: &[(f64, f64)], secs: f64) -> Vec<Gap> {
    let mut out = Vec::new();
    for (s, e) in invert(speech, secs) {
        if e - s < MIN_GAP {
            continue;
        }
        let parts = ((e - s) / MAX_PART).ceil().max(1.0) as usize;
        let len = (e - s) / parts as f64;
        for p in 0..parts {
            let (a, b) = (s + p as f64 * len, s + (p + 1) as f64 * len);
            out.push(Gap {
                start: a,
                end: b,
                words: word_limit(b - a),
                part: (parts > 1).then_some((p + 1, parts)),
            });
        }
    }
    out
}

/// `1:02:03.4`, or `2:03.4` under an hour.
pub fn clock(t: f64) -> String {
    // Round to tenths first, so 119.97 is 2:00.0 rather than 1:60.0.
    let tenths = (t.max(0.0) * 10.0).round() as u64;
    let (h, m, s, f) = (
        tenths / 36_000,
        tenths / 600 % 60,
        tenths / 10 % 60,
        tenths % 10,
    );
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{f}")
    } else {
        format!("{m}:{s:02}.{f}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_joins_overlaps_and_touches() {
        assert_eq!(
            merge(vec![(5.0, 6.0), (0.0, 2.0), (1.0, 3.0), (3.0, 4.0)]),
            vec![(0.0, 4.0), (5.0, 6.0)]
        );
    }

    #[test]
    fn inverting_gives_the_silences_to_the_end() {
        assert_eq!(
            invert(&[(1.0, 2.0), (4.0, 5.0)], 7.0),
            vec![(0.0, 1.0), (2.0, 4.0), (5.0, 7.0)]
        );
        assert_eq!(invert(&[], 3.0), vec![(0.0, 3.0)]);
        assert_eq!(invert(&[(0.0, 3.0)], 3.0), vec![]);
    }

    #[test]
    fn captions_lead_and_are_trimmed_to_what_the_vad_hears() {
        // A padded cue round heard speech shrinks to the speech.
        let vad = vec![(1.2, 2.8), (10.0, 11.0)];
        let cues = vec![(1.0, 3.0), (5.0, 6.0)];
        // The second cue is not heard at all: kept whole. The VAD's uncued
        // speech at 10 s is dropped.
        assert_eq!(captions_led(&vad, &cues), vec![(1.2, 2.8), (5.0, 6.0)]);
    }

    #[test]
    fn word_limits_follow_the_three_words_a_second_rule() {
        assert_eq!(word_limit(1.0), 1);
        assert_eq!(word_limit(1.4), 3);
        assert_eq!(word_limit(2.0), 4);
        assert_eq!(word_limit(5.0), 13);
    }

    #[test]
    fn long_silences_split_into_equal_parts() {
        let g = plan_gaps(&[(0.0, 1.0), (13.0, 14.0)], 14.0);
        assert_eq!(g.len(), 3);
        assert_eq!(g[0].part, Some((1, 3)));
        assert!((g[0].end - 5.0).abs() < 1e-9);
        assert!((g[2].end - 13.0).abs() < 1e-9);
    }

    #[test]
    fn short_silences_are_not_places() {
        let g = plan_gaps(&[(0.0, 1.0), (1.5, 3.0)], 3.0);
        assert!(g.is_empty(), "{g:?}");
    }

    #[test]
    fn clock_rounds_and_shows_hours_only_when_needed() {
        assert_eq!(clock(119.97), "2:00.0");
        assert_eq!(clock(3723.4), "1:02:03.4");
    }
}
