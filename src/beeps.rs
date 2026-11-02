//! The pause check file: the video's own sound with a short high beep where
//! each pause starts and a low one where it ends, so the speech map can be
//! checked by ear in one listen (test round 2, 2.1: the report alone was
//! "much too cumbersome with a screen reader").

use crate::speech::Gap;

const HIGH_HZ: f64 = 1500.0;
const LOW_HZ: f64 = 500.0;
const LENGTH: f64 = 0.07;
const FADE: f64 = 0.01;
/// About a third of full scale: heard over music without startling.
const LOUDNESS: f64 = 0.3 * 32767.0;

/// One beep: when it starts, and whether it is the high one.
#[derive(Debug, PartialEq)]
pub struct Beep {
    pub at: f64,
    pub high: bool,
}

/// Two beeps for each pause, the parts of a cut-up pause counting as one.
/// The high beep starts with the pause and the low one ends with it, so both
/// are heard inside the pause, never over speech.
pub fn marks(gaps: &[Gap]) -> Vec<Beep> {
    let mut pauses: Vec<(f64, f64)> = Vec::new();
    for g in gaps {
        match pauses.last_mut() {
            Some(last) if g.start - last.1 < 0.01 => last.1 = g.end,
            _ => pauses.push((g.start, g.end)),
        }
    }
    let mut out = Vec::new();
    for (s, e) in pauses {
        out.push(Beep { at: s, high: true });
        out.push(Beep {
            at: (e - LENGTH).max(s + LENGTH),
            high: false,
        });
    }
    out
}

/// Adds the beeps to the sound as it streams past.
pub struct Mixer {
    beeps: Vec<Beep>,
    rate: f64,
    /// Samples already seen.
    at: u64,
}

impl Mixer {
    pub fn new(beeps: Vec<Beep>, rate: u32) -> Mixer {
        Mixer {
            beeps,
            rate: rate as f64,
            at: 0,
        }
    }

    pub fn mix(&mut self, piece: &mut [i16]) {
        let from = self.at as f64 / self.rate;
        let to = (self.at + piece.len() as u64) as f64 / self.rate;
        for b in self
            .beeps
            .iter()
            .filter(|b| b.at < to && b.at + LENGTH > from)
        {
            let hz = if b.high { HIGH_HZ } else { LOW_HZ };
            for (i, s) in piece.iter_mut().enumerate() {
                let t = (self.at + i as u64) as f64 / self.rate - b.at;
                if !(0.0..LENGTH).contains(&t) {
                    continue;
                }
                // Faded in and out, so the beep does not click.
                let edge = (t.min(LENGTH - t) / FADE).min(1.0);
                let v = (t * hz * std::f64::consts::TAU).sin() * LOUDNESS * edge;
                *s = (*s as f64 + v).clamp(-32768.0, 32767.0) as i16;
            }
        }
        self.at += piece.len() as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gap(start: f64, end: f64) -> Gap {
        Gap {
            start,
            end,
            words: 1,
            part: None,
        }
    }

    #[test]
    fn a_cut_up_pause_has_one_pair_of_beeps() {
        let m = marks(&[gap(1.0, 2.0), gap(5.0, 9.0), gap(9.0, 13.0)]);
        assert_eq!(m.len(), 4);
        assert_eq!(
            m[0],
            Beep {
                at: 1.0,
                high: true
            }
        );
        assert!((m[1].at - 1.93).abs() < 1e-9 && !m[1].high);
        assert_eq!(
            m[2],
            Beep {
                at: 5.0,
                high: true
            }
        );
        assert!((m[3].at - 12.93).abs() < 1e-9);
    }

    #[test]
    fn beeps_land_where_they_should_across_pieces() {
        let mut mixer = Mixer::new(
            vec![Beep {
                at: 0.5,
                high: true,
            }],
            8000,
        );
        let mut sound = vec![0i16; 8000];
        let (a, b) = sound.split_at_mut(4200);
        mixer.mix(a);
        mixer.mix(b);
        assert!(sound[..4000].iter().all(|&s| s == 0));
        assert!(sound[4100..4500].iter().any(|&s| s.abs() > 5000));
        assert!(sound[4560..].iter().all(|&s| s == 0));
    }
}
