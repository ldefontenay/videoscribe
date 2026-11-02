//! The speech map as plain text, so the timing can be checked by ear against
//! the video before a penny is spent on describing (PLAN.md §8, milestone 3).
//! One line per stretch of speech or pause, in order, readable with the
//! arrow keys.

use crate::speech::{self, clock, Gap, Spans};

pub struct Map {
    /// The file name, or the YouTube title and id. Stays on this computer.
    pub name: String,
    pub secs: f64,
    /// What the speech detector heard, before any captions.
    pub vad: Spans,
    pub captions: Option<CaptionsUsed>,
    /// The speech that protects the soundtrack: the detector's, or the
    /// captions' when they lead.
    pub speech: Spans,
    pub gaps: Vec<Gap>,
}

pub struct CaptionsUsed {
    pub file_name: String,
    pub automatic: bool,
    /// Lines of dialogue in them.
    pub lines: usize,
    pub speech: Spans,
}

/// "4.3 s", or "3 min 5 s" from a minute up, which is easier to take in by ear.
pub fn secs(t: f64) -> String {
    if t >= 59.95 {
        let whole = t.round() as u64;
        format!("{} min {} s", whole / 60, whole % 60)
    } else {
        format!("{t:.1} s")
    }
}

/// A length from `s` to `e` that agrees with their clock times, both rounded
/// to tenths: 1:01.4 to 1:02.9 is 1.5 s, never 1.4.
fn span(s: f64, e: f64) -> String {
    let tenths = (e * 10.0).round() - (s * 10.0).round();
    secs(tenths / 10.0)
}

fn words(n: usize) -> String {
    if n == 1 {
        "1 word".into()
    } else {
        format!("{n} words")
    }
}

pub fn text(map: &Map) -> String {
    let mut out = Vec::new();
    out.push(format!("Speech map of {}", map.name));
    out.push(format!(
        "Made by Video Scribe {} on this computer. Nothing was sent anywhere to make it.",
        env!("CARGO_PKG_VERSION")
    ));
    out.push(String::new());
    out.push(format!("Running time: {}.", clock(map.secs)));
    match &map.captions {
        Some(c) => {
            let kind = if c.automatic {
                "YouTube's automatic captions"
            } else {
                "captions written by a person"
            };
            out.push(format!(
                "How speech was found: from {kind} ({}, {} lines), each line trimmed to the speech Video \
                 Scribe's own detector hears in it. Sounds with no caption, such as laughter, \
                 music and effects, may have descriptions beside them.",
                c.file_name, c.lines
            ));
            out.push(format!(
                "The captions and the detector agree on {:.0}% of the captions' speech. The \
                 detector heard {} with no caption.",
                100.0 * speech::agreement(&c.speech, &map.vad),
                secs(speech::total(&uncaptioned(&map.vad, &c.speech)))
            ));
        }
        None => out.push(
            "How speech was found: by Video Scribe's own speech detector alone, as there are no \
             captions. It can miss quiet speech straight after loud music."
                .into(),
        ),
    }
    out.push(format!(
        "Speech: {:.0}% of the running time.",
        100.0 * speech::total(&map.speech) / map.secs.max(0.001)
    ));
    let room: f64 = map.gaps.iter().map(|g| g.end - g.start).sum();
    out.push(format!(
        "Places for a description: {}, {} in all, about {:.1} a minute. A pause shorter than \
         {:.0} second is not used, and a pause longer than {:.0} seconds is cut into equal parts.",
        map.gaps.len(),
        secs(room),
        map.gaps.len() as f64 * 60.0 / map.secs.max(1.0),
        speech::MIN_GAP,
        speech::MAX_PART
    ));
    out.push(String::new());
    out.push(
        "In order, from the start. Speech includes any pause under a second; each place says the \
         most words that fit in it."
            .into(),
    );
    out.extend(timeline(map));
    out.join("\r\n") + "\r\n"
}

/// Detector speech that no caption covers.
fn uncaptioned(vad: &[(f64, f64)], cues: &[(f64, f64)]) -> Spans {
    let mut out = Vec::new();
    for &(s, e) in vad {
        let mut at = s;
        for &(cs, ce) in cues.iter().filter(|c| c.1 > s && c.0 < e) {
            if cs > at {
                out.push((at, cs));
            }
            at = at.max(ce);
        }
        if e > at {
            out.push((at, e));
        }
    }
    out
}

fn timeline(map: &Map) -> Vec<String> {
    let mut lines = Vec::new();
    let mut at = 0.0;
    let mut n = 0;
    let mut i = 0;
    while i < map.gaps.len() {
        // A pause is one gap, or all the parts of one cut-up pause.
        let parts = map.gaps[i].part.map_or(1, |p| p.1);
        let pause = &map.gaps[i..(i + parts).min(map.gaps.len())];
        let (s, e) = (pause[0].start, pause[pause.len() - 1].end);
        if s - at > 0.05 {
            lines.push(speech_line(at, s));
        }
        if parts == 1 {
            n += 1;
            lines.push(format!(
                "Place {n}: {} to {}, a pause of {}, up to {}.",
                clock(s),
                clock(e),
                span(s, e),
                words(pause[0].words)
            ));
        } else {
            lines.push(format!(
                "Places {} to {}: {} to {}, a pause of {} cut into {parts} parts, up to {} each.",
                n + 1,
                n + parts,
                clock(s),
                clock(e),
                span(s, e),
                words(pause[0].words)
            ));
            n += parts;
        }
        at = e;
        i += parts;
    }
    if map.secs - at > 0.05 {
        lines.push(speech_line(at, map.secs));
    }
    lines
}

fn speech_line(s: f64, e: f64) -> String {
    format!("Speech: {} to {}, {}.", clock(s), clock(e), span(s, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::plan_gaps;

    #[test]
    fn the_timeline_alternates_speech_and_places() {
        let speech = vec![(0.0, 4.0), (5.5, 6.0), (6.5, 8.0), (20.0, 21.0)];
        let map = Map {
            name: "party.mp4".into(),
            secs: 22.0,
            vad: speech.clone(),
            captions: None,
            gaps: plan_gaps(&speech, 22.0),
            speech,
        };
        let t = text(&map);
        assert!(t.contains("Speech: 0:00.0 to 0:04.0, 4.0 s."), "{t}");
        assert!(
            t.contains("Place 1: 0:04.0 to 0:05.5, a pause of 1.5 s, up to 3 words."),
            "{t}"
        );
        // 5.5 to 8.0 has two short pauses: all speech.
        assert!(t.contains("Speech: 0:05.5 to 0:08.0, 2.5 s."), "{t}");
        assert!(
            t.contains(
                "Places 2 to 4: 0:08.0 to 0:20.0, a pause of 12.0 s cut into 3 parts, up to 10 \
                 words each."
            ),
            "{t}"
        );
        assert!(
            t.contains("Place 5: 0:21.0 to 0:22.0, a pause of 1.0 s"),
            "{t}"
        );
        assert_eq!(secs(185.4), "3 min 5 s");
        assert!(t.contains("Places for a description: 5,"), "{t}");
    }

    #[test]
    fn the_descriptions_report_counts_empty_long_and_edited_places() {
        use crate::describe::{Described as One, Outcome};
        let gaps = plan_gaps(&[(2.0, 4.0), (6.0, 8.0), (10.0, 12.0)], 14.0);
        let one = |place: usize, text: &str| One {
            place,
            text: text.into(),
            seen_at: None,
            setting: String::new(),
        };
        let outcome = Outcome {
            described: vec![
                one(1, "A dog."),
                one(2, "A big red ball bounces away"),
                one(4, ""),
            ],
            people: vec!["the dog".into()],
            cost: 0.0021,
            requests: 1,
            pictures: 8,
            clips: 0,
            clip_secs: 0.0,
            edited: 3,
            emptied: 1,
            edit_failed: None,
            reused: 0,
            speech: Vec::new(),
            overrides: Vec::new(),
        };
        let t = descriptions(&Described {
            name: "party.mp4",
            model: "GLM",
            plan: false,
            gaps: &gaps,
            outcome: &outcome,
            secs: 12.0,
        });
        assert!(t.contains("under 1 US cent"), "{t}");
        assert!(t.contains("Places: 4. Described: 2. Left empty: 2."), "{t}");
        assert!(
            t.contains("Longer than their word limit: 1, 0 of them by one word"),
            "{t}"
        );
        assert!(
            t.contains("Place 1: 0:00.0 to 0:02.0, up to 4 words: A dog.\r\n"),
            "{t}"
        );
        assert!(!t.contains("words for"), "{t}");
        assert!(t.contains("changed 3 descriptions and left 1 empty"), "{t}");
        assert!(
            t.contains("Place 3: 0:08.0 to 0:10.0, up to 4 words: (left empty)"),
            "{t}"
        );
    }

    #[test]
    fn uncaptioned_speech_is_what_no_cue_covers() {
        let v = uncaptioned(&[(0.0, 10.0)], &[(2.0, 3.0), (5.0, 6.0)]);
        assert_eq!(v, vec![(0.0, 2.0), (3.0, 5.0), (6.0, 10.0)]);
    }
}

/// What the second job made, for its report.
pub struct Described<'a> {
    pub name: &'a str,
    /// "GLM 5.3 Flash, through OpenRouter".
    pub model: &'a str,
    /// Written through Claude Code, on the user's Claude plan: no money.
    pub plan: bool,
    pub gaps: &'a [Gap],
    pub outcome: &'a crate::describe::Outcome,
    /// How long describing took.
    pub secs: f64,
}

/// The descriptions, one line per place, in order, with what they cost.
#[cfg(test)]
pub fn descriptions(d: &Described) -> String {
    let o = d.outcome;
    let mut out = descriptions_head(d);
    out.push(String::new());
    for (i, g) in d.gaps.iter().enumerate() {
        let n = i + 1;
        let text = o
            .described
            .iter()
            .find(|x| x.place == n)
            .map(|x| x.text.as_str())
            .filter(|t| !t.is_empty());
        let mut line = format!(
            "Place {n}: {} to {}, up to {}: ",
            clock(g.start),
            clock(g.end),
            words(g.words)
        );
        match text {
            // No notes on the line: they were noise to the listener (test
            // round 3, 2.4). The count above keeps the measure.
            Some(t) => line += t,
            None => line += "(left empty)",
        }
        out.push(line);
    }
    out.join("\r\n") + "\r\n"
}

/// What the descriptions cost and how many there are, without the list.
pub fn descriptions_head(d: &Described) -> Vec<String> {
    let o = d.outcome;
    let mut out = Vec::new();
    out.push(format!("Descriptions of {}", d.name));
    let money = if d.plan {
        "nothing beyond your Claude plan".to_string()
    } else if o.cost < 0.01 {
        format!("under 1 US cent (US${:.4})", o.cost)
    } else {
        format!("US${:.2}", o.cost)
    };
    if o.requests == 0 && o.reused > 0 {
        // Nothing was sent this time: say so, rather than "0 requests".
        out.push(format!(
            "Written by {} in an earlier run of this job, for {money}. Those descriptions were \
             kept, so nothing was sent this time.",
            d.model
        ));
    } else {
        out.push(format!(
            "Written by {}, from {} in {} requests, in {}, for {money}.",
            d.model,
            if o.clips > 0 {
                format!(
                    "{} short clips with their sound, {} of video",
                    o.clips,
                    clock(o.clip_secs)
                )
            } else {
                format!("{} still pictures", o.pictures)
            },
            o.requests,
            secs(d.secs)
        ));
    }
    if o.reused > 0 && o.requests > 0 {
        out.push(format!(
            "{} answers were kept from an earlier run of this job and not sent again, so \
             their pictures are not counted above.",
            o.reused
        ));
    }
    let empty = d
        .gaps
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            !o.described
                .iter()
                .any(|x| x.place == i + 1 && !x.text.is_empty())
        })
        .count();
    // How many words over its limit each description is.
    let over: Vec<usize> = o
        .described
        .iter()
        .filter_map(|x| {
            let g = d.gaps.get(x.place - 1)?;
            crate::describe::words(&x.text).checked_sub(g.words + 1)
        })
        .collect();
    let optional = d.gaps.iter().filter(|g| g.optional()).count();
    let optional_empty = d
        .gaps
        .iter()
        .enumerate()
        .filter(|(i, g)| {
            g.optional()
                && !o
                    .described
                    .iter()
                    .any(|x| x.place == i + 1 && !x.text.is_empty())
        })
        .count();
    out.push(format!(
        "Places: {}. Described: {}. Left empty: {empty}.",
        d.gaps.len(),
        d.gaps.len() - empty
    ));
    if !o.speech.is_empty() {
        let list: Vec<String> = o.speech.iter().map(|p| p.to_string()).collect();
        out.push(format!(
            "Pauses where the model heard someone speaking, so left empty and nothing is said \
             over them: {}, place{} {}.",
            o.speech.len(),
            if o.speech.len() == 1 { "" } else { "s" },
            list.join(", ")
        ));
    }
    if optional > 0 {
        out.push(format!(
            "Short pauses, under {} seconds, are optional, described only when something \
             important happens: {optional}, of which {optional_empty} were left empty.",
            crate::speech::OPTIONAL_BELOW
        ));
    }
    if o.edited + o.emptied > 0 {
        out.push(format!(
            "The editor pass, text only, changed {} description{} and left {} empty.",
            o.edited,
            if o.edited == 1 { "" } else { "s" },
            o.emptied
        ));
    }
    if let Some(why) = &o.edit_failed {
        out.push(format!(
            "The editor pass could not finish, so some descriptions are as first written. {why}"
        ));
    }
    if !over.is_empty() {
        out.push(format!(
            "Longer than their word limit: {}, {} of them by one word. The limit is cautious: \
             what matters is whether each one fits when spoken.",
            over.len(),
            over.iter().filter(|&&n| n == 0).count()
        ));
    }
    if !o.people.is_empty() {
        out.push(format!("People and labels used: {}.", o.people.join(", ")));
    }
    out
}
