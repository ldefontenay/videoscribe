//! Placing the spoken descriptions and mixing them into the film's sound
//! (PLAN.md §2.8, milestone 7). Each description is spoken first and measured,
//! then placed in its pause. One that does not fit is tried 10%, then 15%,
//! faster (log entry 24); if it still does not fit, the user's choice decides:
//! pause the film, speak over it, or leave it out. Short optional pauses
//! follow that choice too, unless the user said otherwise (log entry 29, Q1).
//!
//! The mix streams: the film's sound is read a piece at a time, turned down
//! under each description with smooth fades, and written straight to the MP3,
//! so an hour-long film is never held whole. The original file is only read.

use crate::config::Room;
use crate::media;
use crate::speak::{Voice, RATE};
use crate::speech::{clock, Gap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// The fastest a description is spoken to fit its pause: 15% faster, as
/// SonarPad Mobile does (log entry 24).
pub const FASTEST: f64 = 1.15;

/// Kept clear at each end of a pause, so speech is never clipped.
const MARGIN: f64 = 0.1;
/// How far a description may start before its pause, over the end of the
/// speech, and run on after it, when it would not fit otherwise.
const SPILL_BEFORE: f64 = 0.2;
const SPILL_AFTER: f64 = 0.4;
/// Kept between one description and the next.
const BETWEEN: f64 = 0.15;
/// The film's level under a description: about -12 dB, as in round 2.
const DUCK: f32 = 0.25;
/// Fade down before a description, and back up after it.
const FADE: f64 = 0.15;
/// Held down this long after a description ends.
const HOLD: f64 = 0.25;
/// The film fades out and in this fast either side of a pause in it.
const PAUSE_FADE: f64 = 0.03;

/// What happened to one place.
pub enum Fate {
    Empty,
    Spoken {
        text: String,
        /// Film time it starts at, in seconds.
        at: f64,
        secs: f64,
        /// Percent faster than normal it was spoken at.
        percent: i32,
        /// The film is paused this long, while it finishes.
        pause: f64,
        /// It runs on past the pause, over the film.
        over: bool,
        /// Seconds it starts before its pause or ends after it, to fit.
        spill: f64,
    },
    Left {
        text: String,
        why: &'static str,
    },
}

pub struct Placed {
    pub fates: Vec<Fate>,
    voices: Vec<(usize, Vec<i16>)>,
}

/// Where a description `secs` long starts in its pause, after `cursor`, the
/// end of the one before; None if it does not fit. With `spill`, it may start
/// up to SPILL_BEFORE early and end up to SPILL_AFTER late, over the film's
/// edges of the pause, by only as much as it needs (milestone 7 test, 1.x).
fn slot(g: &Gap, cursor: f64, secs: f64, spill: bool) -> Option<f64> {
    let start = (g.start + MARGIN).max(cursor + BETWEEN);
    if !spill {
        return (start + secs <= g.end - MARGIN).then_some(start);
    }
    let earliest = (g.start - SPILL_BEFORE).max(cursor + BETWEEN);
    let latest = g.end + SPILL_AFTER;
    let at = start.min(latest - secs).max(earliest);
    (at + secs <= latest + 1e-9).then_some(at)
}

/// Speaks and places every description. `texts[i]` belongs to `gaps[i]`.
/// None if stopped.
pub fn place(
    gaps: &[Gap],
    texts: &[String],
    room: Room,
    short_too: bool,
    voice: &mut Voice,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(String),
) -> Result<Option<Placed>, String> {
    let speeds = [1.0, 1.10, FASTEST].map(|f| voice.percent(f));
    let text = |i: usize| texts.get(i).map_or("", |t| t.trim());
    let wanted: Vec<usize> = (0..gaps.len()).filter(|&i| !text(i).is_empty()).collect();

    // Everything at the user's speed, all at once.
    let jobs: Vec<(&str, i32)> = wanted.iter().map(|&i| (text(i), speeds[0])).collect();
    let Some(first) = voice.say_all(&jobs, stop, &mut |done, all| {
        progress(format!("Speaking the descriptions: {done} of {all}."))
    })?
    else {
        return Ok(None);
    };
    // samples[i][k]: place i at speeds[k], once spoken.
    let mut samples: Vec<[Option<Vec<i16>>; 3]> = vec![[None, None, None]; gaps.len()];
    for (&i, s) in wanted.iter().zip(first) {
        samples[i][0] = Some(s);
    }

    // Then, only for those too long for their pause, the faster speeds.
    let long: Vec<usize> = wanted
        .iter()
        .copied()
        .filter(|&i| {
            let secs = samples[i][0].as_ref().map_or(0, |s| s.len()) as f64 / RATE as f64;
            secs > gaps[i].end - gaps[i].start - 2.0 * MARGIN
        })
        .collect();
    let jobs: Vec<(&str, i32)> = long
        .iter()
        .flat_map(|&i| [(text(i), speeds[1]), (text(i), speeds[2])])
        .collect();
    let Some(faster) = voice.say_all(&jobs, stop, &mut |done, all| {
        progress(format!(
            "Speaking the longer descriptions faster: {done} of {all}."
        ))
    })?
    else {
        return Ok(None);
    };
    let mut faster = faster.into_iter();
    for &i in &long {
        samples[i][1] = faster.next();
        samples[i][2] = faster.next();
    }

    let mut fates = Vec::new();
    let mut voices = Vec::new();
    // Film time the last description ends at.
    let mut cursor = 0.0f64;
    for (i, g) in gaps.iter().enumerate() {
        let text = text(i);
        if text.is_empty() {
            fates.push(Fate::Empty);
            continue;
        }
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let start = (g.start + MARGIN).max(cursor + BETWEEN);
        let room_secs = g.end - MARGIN - start;
        // Faster first, inside the pause; then spilling a little over its
        // edges.
        let mut fitted = None;
        'tries: for spill in [false, true] {
            for k in 0..3 {
                if samples[i][k].is_none() {
                    // Not fetched ahead: the one before ran on into this pause.
                    samples[i][k] = Some(voice.say(text, speeds[k])?);
                }
                let secs = samples[i][k].as_ref().map_or(0, |s| s.len()) as f64 / RATE as f64;
                if let Some(at) = slot(g, cursor, secs, spill) {
                    fitted = Some((k, at, secs));
                    break 'tries;
                }
            }
        }
        let left = |why| Fate::Left {
            text: text.to_string(),
            why,
        };
        let secs_at = |k: usize| samples[i][k].as_ref().map_or(0, |s| s.len()) as f64 / RATE as f64;
        let (k, at, secs, pause, over) = match fitted {
            Some((k, at, secs)) => (k, at, secs, 0.0, false),
            None if g.optional() && !short_too => {
                fates.push(left("too long for its short pause, even spoken faster"));
                continue;
            }
            None if room_secs <= 0.0 => {
                fates.push(left("no room: the description before it ran on"));
                continue;
            }
            None => match room {
                Room::GapsOnly => {
                    fates.push(left("too long for its pause, even spoken 15% faster"));
                    continue;
                }
                // At the user's own speed: the film waits, so there is no hurry.
                Room::Pause => (0, start, secs_at(0), secs_at(0) - room_secs, false),
                Room::Over => (2, start, secs_at(2), 0.0, true),
            },
        };
        let spill = (g.start - at).max(0.0) + (at + secs - pause - g.end).max(0.0);
        cursor = at + secs - pause;
        fates.push(Fate::Spoken {
            text: text.to_string(),
            at,
            secs,
            percent: speeds[k],
            pause,
            over,
            spill: if over { 0.0 } else { spill },
        });
        voices.push((fates.len() - 1, samples[i][k].take().expect("spoken above")));
    }
    Ok(Some(Placed { fates, voices }))
}

/// One description on the output's clock, in frames.
struct Cue<'a> {
    from: usize,
    samples: &'a [i16],
}

/// A pause in the film: at this film frame, this many frames of stillness.
struct Hold {
    at: usize,
    frames: usize,
}

impl Placed {
    fn holds(&self) -> Vec<Hold> {
        self.fates
            .iter()
            .filter_map(|f| match f {
                Fate::Spoken {
                    at, secs, pause, ..
                } if *pause > 0.0 => Some(Hold {
                    at: ((at + secs - pause) * RATE as f64) as usize,
                    frames: (pause * RATE as f64) as usize,
                }),
                _ => None,
            })
            .collect()
    }

    fn cues(&self, holds: &[Hold]) -> Vec<Cue<'_>> {
        self.voices
            .iter()
            .map(|(i, s)| {
                let Fate::Spoken { at, .. } = &self.fates[*i] else {
                    unreachable!("only spoken places have a voice")
                };
                let film = (at * RATE as f64) as usize;
                let before: usize = holds.iter().filter(|h| h.at < film).map(|h| h.frames).sum();
                Cue {
                    from: film + before,
                    samples: s,
                }
            })
            .collect()
    }

    /// Seconds of stillness added to the film.
    pub fn paused(&self) -> (usize, f64) {
        let h = self.holds();
        (
            h.len(),
            h.iter().map(|h| h.frames).sum::<usize>() as f64 / RATE as f64,
        )
    }

    /// The film's sound with every description in it, as a stereo MP3 at
    /// `out`. False if stopped, and then no file is left.
    pub fn mix(
        &self,
        film: &Path,
        out: &Path,
        stop: &AtomicBool,
        progress: &mut dyn FnMut(f64),
    ) -> Result<bool, String> {
        let holds = self.holds();
        let cues = self.cues(&holds);
        let mut mp3 = media::Mp3::create_with(out, RATE, 2, 192)?;
        let mut m = Mixer {
            cues: &cues,
            holds: &holds,
            next_hold: 0,
            film_at: 0,
            out_at: 0,
            cue: 0,
            buf: Vec::new(),
        };
        let done = media::each(&film.to_string_lossy(), RATE, 2, stop, progress, &mut |s| {
            m.buf.clear();
            m.film(s);
            mp3.write(&m.buf)
        });
        match done {
            Ok(true) => {
                // Descriptions that run past the film's end are heard out.
                m.buf.clear();
                m.tail();
                mp3.write(&m.buf)?;
                mp3.finish().map(|_| true)
            }
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
}

struct Mixer<'a> {
    cues: &'a [Cue<'a>],
    holds: &'a [Hold],
    next_hold: usize,
    /// Frames of the film read so far, and of the output written.
    film_at: usize,
    out_at: usize,
    /// The first cue that has not finished.
    cue: usize,
    buf: Vec<i16>,
}

impl Mixer<'_> {
    /// The film's level at an output frame: down under each description,
    /// with fades either side.
    fn gain(&self, at: usize) -> f32 {
        let t = at as f64 / RATE as f64;
        let mut g = 1.0f32;
        let from = self.cue.saturating_sub(1);
        for c in self.cues.iter().skip(from).take(3) {
            let s = c.from as f64 / RATE as f64;
            let e = s + c.samples.len() as f64 / RATE as f64;
            let k = if t < s - FADE || t > e + HOLD + FADE {
                1.0
            } else if t < s {
                1.0 - (t - (s - FADE)) / FADE * (1.0 - DUCK as f64)
            } else if t > e + HOLD {
                DUCK as f64 + (t - e - HOLD) / FADE * (1.0 - DUCK as f64)
            } else {
                DUCK as f64
            };
            g = g.min(k as f32);
        }
        g
    }

    fn voice(&mut self, at: usize) -> f32 {
        while self.cue < self.cues.len()
            && at >= self.cues[self.cue].from + self.cues[self.cue].samples.len()
        {
            self.cue += 1;
        }
        match self.cues.get(self.cue) {
            Some(c) if at >= c.from => c.samples[at - c.from] as f32,
            _ => 0.0,
        }
    }

    fn put(&mut self, l: f32, r: f32) {
        let g = self.gain(self.out_at);
        let v = self.voice(self.out_at);
        self.buf.push((l * g + v).clamp(-32768.0, 32767.0) as i16);
        self.buf.push((r * g + v).clamp(-32768.0, 32767.0) as i16);
        self.out_at += 1;
    }

    /// How far the film's own level is faded either side of a pause in it.
    fn edge(&self) -> f32 {
        let near = |h: &Hold| self.film_at.abs_diff(h.at);
        let d = self
            .holds
            .iter()
            .skip(self.next_hold.saturating_sub(1))
            .take(2)
            .map(near)
            .min()
            .unwrap_or(usize::MAX);
        let f = (PAUSE_FADE * RATE as f64) as usize;
        if d >= f {
            1.0
        } else {
            d as f32 / f as f32
        }
    }

    fn film(&mut self, stereo: &[i16]) {
        for pair in stereo.as_chunks::<2>().0 {
            while self.next_hold < self.holds.len() && self.holds[self.next_hold].at <= self.film_at
            {
                for _ in 0..self.holds[self.next_hold].frames {
                    self.put(0.0, 0.0);
                }
                self.next_hold += 1;
            }
            let k = self.edge();
            self.put(pair[0] as f32 * k, pair[1] as f32 * k);
            self.film_at += 1;
        }
    }

    fn tail(&mut self) {
        let end = self.cues.last().map_or(0, |c| c.from + c.samples.len());
        while self.out_at < end {
            self.put(0.0, 0.0);
        }
    }
}

/// The report's lines, one for each place, on the described file's clock.
pub fn lines(placed: &Placed) -> Vec<String> {
    let mut added = 0.0;
    placed
        .fates
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let n = i + 1;
            match f {
                Fate::Empty => format!("Place {n}: left empty."),
                Fate::Left { text, why } => format!("Place {n}: left out, {why}: {text}"),
                Fate::Spoken {
                    text,
                    at,
                    percent,
                    pause,
                    over,
                    spill,
                    ..
                } => {
                    let mut how = Vec::new();
                    // Against the user's chosen speed, which the rest are at.
                    let base = placed_rate(placed);
                    if *percent > base {
                        let more =
                            ((100.0 + *percent as f64) / (100.0 + base as f64) - 1.0) * 100.0;
                        how.push(format!("spoken {:.0}% faster", more));
                    }
                    if *pause > 0.0 {
                        how.push(format!("the film waits {pause:.1} s"));
                    }
                    if *over {
                        how.push("runs on over the film".into());
                    }
                    if *spill >= 0.05 {
                        how.push(format!("spills {spill:.1} s over the edges of its pause"));
                    }
                    let line = format!(
                        "Place {n}, at {}{}: {text}",
                        clock(at + added),
                        if how.is_empty() {
                            String::new()
                        } else {
                            format!(", {}", how.join(", "))
                        }
                    );
                    added += pause;
                    line
                }
            }
        })
        .collect()
}

/// Where each place falls in the described sound, for reviewing it: when its
/// pause starts there, and for a spoken one, when it is said and for how
/// long. The film's own pauses move everything after them on.
pub fn on_output(placed: &Placed, gaps: &[Gap]) -> Vec<(f64, Option<(f64, f64)>)> {
    let mut added = 0.0;
    placed
        .fates
        .iter()
        .zip(gaps)
        .map(|(f, g)| {
            let start = g.start + added;
            match f {
                Fate::Spoken {
                    at, secs, pause, ..
                } => {
                    let said = (at + added, *secs);
                    added += pause;
                    (start, Some(said))
                }
                _ => (start, None),
            }
        })
        .collect()
}

/// The slowest speed anything was spoken at: the user's own.
fn placed_rate(placed: &Placed) -> i32 {
    placed
        .fates
        .iter()
        .filter_map(|f| match f {
            Fate::Spoken { percent, .. } => Some(*percent),
            _ => None,
        })
        .min()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_spills_over_the_pause_only_as_far_as_it_must() {
        let g = Gap {
            start: 10.0,
            end: 12.0,
            words: 4,
            part: None,
        };
        // Fits inside: starts just after the pause begins.
        assert_eq!(slot(&g, 0.0, 1.5, false), Some(10.1));
        assert_eq!(slot(&g, 0.0, 1.9, false), None);
        // Too long by a little: moved only as early as needed, then late.
        let at = slot(&g, 0.0, 2.0, true).unwrap();
        assert!((at - 10.1).abs() < 1e-9, "{at}");
        let at = slot(&g, 0.0, 2.5, true).unwrap();
        assert!((at - 9.9).abs() < 1e-9, "{at}");
        // Never more than 0.2 s early and 0.4 s late.
        assert_eq!(slot(&g, 0.0, 2.7, true), None);
        // Never over the description before.
        assert_eq!(slot(&g, 10.0, 2.5, true), None);
    }

    fn placed(fates: Vec<Fate>, voices: Vec<(usize, Vec<i16>)>) -> Placed {
        Placed { fates, voices }
    }

    #[test]
    fn the_film_is_turned_down_under_a_description_and_back_up_after() {
        let p = placed(
            vec![Fate::Spoken {
                text: "A dog.".into(),
                at: 1.0,
                secs: 1.0,
                percent: 10,
                pause: 0.0,
                over: false,
                spill: 0.0,
            }],
            vec![(0, vec![1000; RATE as usize])],
        );
        let holds = p.holds();
        let cues = p.cues(&holds);
        let mut m = Mixer {
            cues: &cues,
            holds: &holds,
            next_hold: 0,
            film_at: 0,
            out_at: 0,
            cue: 0,
            buf: Vec::new(),
        };
        m.film(&vec![10000; 2 * 3 * RATE as usize]);
        let at = |t: f64| m.buf[2 * (t * RATE as f64) as usize];
        assert_eq!(at(0.5), 10000);
        assert_eq!(at(1.5), 2500 + 1000);
        assert_eq!(at(2.9), 10000);
        assert_eq!(m.buf.len(), 2 * 3 * RATE as usize);
    }

    #[test]
    fn places_on_the_described_sound_move_on_after_a_pause() {
        let gap = |start: f64, end: f64| Gap {
            start,
            end,
            words: 4,
            part: None,
        };
        let spoken = |at: f64, pause: f64| Fate::Spoken {
            text: "A dog.".into(),
            at,
            secs: 2.0,
            percent: 10,
            pause,
            over: false,
            spill: 0.0,
        };
        let p = placed(
            vec![spoken(1.1, 1.5), Fate::Empty, spoken(8.1, 0.0)],
            Vec::new(),
        );
        let t = on_output(&p, &[gap(1.0, 2.0), gap(4.0, 5.0), gap(8.0, 11.0)]);
        assert_eq!(t[0], (1.0, Some((1.1, 2.0))));
        assert_eq!(t[1], (5.5, None));
        assert_eq!(t[2], (9.5, Some((9.6, 2.0))));
    }

    #[test]
    fn a_pause_in_the_film_adds_stillness_and_moves_what_follows() {
        let p = placed(
            vec![
                Fate::Spoken {
                    text: "A long one.".into(),
                    at: 1.0,
                    secs: 2.0,
                    percent: 10,
                    pause: 1.0,
                    over: false,
                    spill: 0.0,
                },
                Fate::Spoken {
                    text: "Next.".into(),
                    at: 4.0,
                    secs: 0.5,
                    percent: 10,
                    pause: 0.0,
                    over: false,
                    spill: 0.0,
                },
            ],
            vec![
                (0, vec![1000; 2 * RATE as usize]),
                (1, vec![1000; RATE as usize / 2]),
            ],
        );
        assert_eq!(p.paused(), (1, 1.0));
        let holds = p.holds();
        assert_eq!(holds[0].at, 2 * RATE as usize);
        let cues = p.cues(&holds);
        assert_eq!(cues[1].from, 5 * RATE as usize);
        let mut m = Mixer {
            cues: &cues,
            holds: &holds,
            next_hold: 0,
            film_at: 0,
            out_at: 0,
            cue: 0,
            buf: Vec::new(),
        };
        m.film(&vec![10000; 2 * 6 * RATE as usize]);
        assert_eq!(m.buf.len(), 2 * 7 * RATE as usize);
        // During the pause only the voice is heard.
        assert_eq!(m.buf[2 * (2.5 * RATE as f64) as usize], 1000);
        let l = lines(&p);
        assert_eq!(
            l[0],
            "Place 1, at 0:01.0, the film waits 1.0 s: A long one."
        );
        assert_eq!(l[1], "Place 2, at 0:05.0: Next.");
    }
}
