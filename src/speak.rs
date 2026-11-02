//! Speaking the descriptions: Edge's voice, or SAPI 5 when Edge cannot be
//! reached (PLAN.md §4.4), as mono samples at the mixing rate, with the
//! silence at either end trimmed and the level evened out, so each one's real
//! length can be measured before it is placed (PLAN.md §2.8: synthesise first,
//! then schedule).

use crate::{edge, media, sapi};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

/// The mixing rate: Edge's 24 kHz and the film's sound both convert to it.
pub const RATE: u32 = 48_000;
/// Loudness of every description, as in the listening rounds (spike skeleton).
const VOICE_RMS: f32 = 0.16;
/// Edge requests in flight at once.
const CONNECTIONS: usize = 6;

pub struct Voice {
    /// An Edge voice, such as "en-GB-SoniaNeural".
    edge: String,
    /// The user's speed, percent faster than normal.
    rate: i32,
    /// Where Edge's MP3 is put to be decoded.
    work: PathBuf,
    /// Set once Edge has failed: the rest of the job uses Windows' voice.
    fallback: Option<sapi::Voice>,
    /// What happened to Edge, for the report.
    pub note: Option<String>,
}

impl Voice {
    pub fn new(edge: &str, rate: i32, work: PathBuf) -> Voice {
        Voice {
            edge: edge.to_string(),
            rate,
            work,
            fallback: None,
            note: None,
        }
    }

    /// The speed for an attempt `factor` times as fast as the user's, as
    /// SonarPad Mobile does it (log entry 24): 1.0, 1.10 or 1.15.
    pub fn percent(&self, factor: f64) -> i32 {
        ((1.0 + self.rate as f64 / 100.0) * factor * 100.0 - 100.0).round() as i32
    }

    /// `text` spoken at `percent` faster than normal: trimmed, levelled mono
    /// samples at RATE.
    pub fn say(&mut self, text: &str, percent: i32) -> Result<Vec<i16>, String> {
        if self.fallback.is_none() {
            match edge_speak(&self.edge, percent, text) {
                Ok(mp3) => return Ok(level(&trim(&self.decode(&mp3)?))),
                Err(why) => {
                    let v = windows_voice(&self.edge)
                        .ok_or_else(|| no_windows_voice(&self.edge, &why))?;
                    self.note = Some(format!(
                        "Edge's voices could not be reached, so Windows' own voice, {}, was \
                         used.",
                        v.name
                    ));
                    self.fallback = Some(v);
                }
            }
        }
        let v = self.fallback.as_ref().expect("set above");
        // SAPI's rate runs from -10 to 10, about 10% a step.
        let s = sapi::speak_hz(v, text, (percent / 10).clamp(-10, 10), RATE)
            .map_err(|e| format!("Windows' voice could not speak: {}", e.message()))?;
        Ok(level(&trim(&s)))
    }

    /// Each `(text, percent)` spoken as `say` does, Edge's part fetched over
    /// several connections at once: one at a time took about a second each
    /// (milestone 7 test, 1.x). SonarPad fetches 8 Edge chunks at once. None
    /// if stopped. `progress` hears how many are done, of how many.
    pub fn say_all(
        &mut self,
        jobs: &[(&str, i32)],
        stop: &AtomicBool,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<Option<Vec<Vec<i16>>>, String> {
        let mut fetched: Vec<Option<Vec<u8>>> = vec![None; jobs.len()];
        if self.fallback.is_none() && !jobs.is_empty() {
            let next = AtomicUsize::new(0);
            let (tx, rx) = mpsc::channel();
            std::thread::scope(|s| {
                for _ in 0..CONNECTIONS.min(jobs.len()) {
                    let tx = tx.clone();
                    let next = &next;
                    let voice = &self.edge;
                    s.spawn(move || loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= jobs.len() || stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let (text, percent) = jobs[i];
                        let mp3 = edge_speak(voice, percent, text).ok();
                        if tx.send((i, mp3)).is_err() {
                            break;
                        }
                    });
                }
                drop(tx);
                let mut done = 0;
                for (i, mp3) in rx {
                    fetched[i] = mp3;
                    done += 1;
                    progress(done, jobs.len());
                }
            });
        }
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let mut out = Vec::with_capacity(jobs.len());
        for (i, (text, percent)) in jobs.iter().enumerate() {
            out.push(match &fetched[i] {
                Some(mp3) => level(&trim(&self.decode(mp3)?)),
                // Not fetched: one more try, and Windows' voice if Edge is gone.
                None => self.say(text, *percent)?,
            });
        }
        Ok(Some(out))
    }

    /// `text` in Edge's voice alone, for a sample of the voice: a Windows
    /// voice would not be a sample of it.
    pub fn sample(&self, text: &str) -> Result<Vec<i16>, String> {
        let mp3 = edge_speak(&self.edge, self.percent(1.0), text).map_err(|why| {
            format!("Edge's voices could not be reached, so there is no sample. {why}")
        })?;
        Ok(level(&trim(&self.decode(&mp3)?)))
    }

    fn decode(&self, mp3: &[u8]) -> Result<Vec<i16>, String> {
        std::fs::create_dir_all(&self.work).map_err(|e| format!("{}: {e}", self.work.display()))?;
        let path = self.work.join("voice.mp3");
        std::fs::write(&path, mp3).map_err(|e| format!("{}: {e}", path.display()))?;
        let stop = AtomicBool::new(false);
        let mut out = Vec::new();
        media::each_mono(
            &path.to_string_lossy(),
            RATE,
            &stop,
            &mut |_| {},
            &mut |s| {
                out.extend_from_slice(s);
                Ok(())
            },
        )?;
        Ok(out)
    }
}

/// Edge's speech for `text`, unless VIDEOSCRIBE_NO_EDGE is set, which
/// tries the Windows voice instead, for testing it.
fn edge_speak(voice: &str, percent: i32, text: &str) -> Result<Vec<u8>, String> {
    if std::env::var_os("VIDEOSCRIBE_NO_EDGE").is_some() {
        return Err("Edge was turned off for a test.".into());
    }
    edge::speak(voice, percent, text).map(|r| r.0)
}

/// A Windows voice that speaks the language of an Edge voice. Never one of
/// another language: an English voice reading French is worse than a clear
/// message (log entry 46, point 3).
fn windows_voice(edge: &str) -> Option<sapi::Voice> {
    let lang = crate::voices::code(edge);
    let mut all = sapi::voices().ok()?;
    let at = all.iter().position(|v| v.language == lang)?;
    Some(all.swap_remove(at))
}

/// Why nothing could be spoken, and what to do about it.
fn no_windows_voice(edge: &str, why: &str) -> String {
    let language = crate::voices::language_of(edge).unwrap_or_else(|| "this language".into());
    let a = if language.starts_with(['A', 'E', 'I', 'O', 'U']) {
        "an"
    } else {
        "a"
    };
    format!(
        "Edge's voices could not be reached, and no Windows voice on this computer speaks \
         {language}, so nothing was spoken. What was described is kept: try again later, or add \
         {a} {language} voice in Windows Settings, Time and language, Speech, then describe again. \
         ({why})"
    )
}

/// Leading silence cut to 30 ms and trailing to 50 ms, as in the spike.
fn trim(samples: &[i16]) -> Vec<i16> {
    let (lead, trail) = sapi::silence(samples, RATE);
    let a = ((lead - 0.03).max(0.0) * RATE as f64) as usize;
    let b = samples
        .len()
        .saturating_sub(((trail - 0.05).max(0.0) * RATE as f64) as usize);
    samples[a.min(b)..b].to_vec()
}

/// Scaled to VOICE_RMS, without letting the peaks clip.
fn level(samples: &[i16]) -> Vec<i16> {
    let n = samples.len().max(1) as f64;
    let rms = (samples.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / n).sqrt() as f32;
    let peak = samples.iter().map(|&s| (s as i32).abs()).max().unwrap_or(0) as f32;
    if rms < 1.0 {
        return samples.to_vec();
    }
    let k = (VOICE_RMS * 32767.0 / rms).min(32000.0 / peak.max(1.0));
    samples.iter().map(|&s| (s as f32 * k) as i16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faster_attempts_scale_the_users_speed() {
        let v = Voice::new("en-GB-SoniaNeural", 10, PathBuf::new());
        assert_eq!(v.percent(1.0), 10);
        assert_eq!(v.percent(1.10), 21);
        assert_eq!(v.percent(1.15), 26);
    }

    /// Words a second for each language's voice, on the same descriptions
    /// translated: `cargo test words_a_second -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn words_a_second() {
        let _s = media::Session::start().unwrap();
        let texts = [
            ("en-GB-SoniaNeural", "A grey-haired woman in a blue dress walks slowly across the garden, holding a small boy by the hand. She stops beside the rose bushes, bends down and points at a bright red flower. The boy laughs and runs towards the gate."),
            ("fr-FR-DeniseNeural", "Une femme aux cheveux gris en robe bleue traverse lentement le jardin, tenant un petit garçon par la main. Elle s'arrête près des rosiers, se penche et montre une fleur rouge vif. Le garçon rit et court vers le portail."),
            ("af-ZA-AdriNeural", "'n Grysharige vrou in 'n blou rok stap stadig oor die tuin en hou 'n klein seuntjie aan die hand vas. Sy gaan staan langs die rosebosse, buk en wys na 'n helderrooi blom. Die seuntjie lag en hardloop na die hek."),
            ("de-DE-KatjaNeural", "Eine grauhaarige Frau in einem blauen Kleid geht langsam durch den Garten und hält einen kleinen Jungen an der Hand. Sie bleibt neben den Rosenbüschen stehen, bückt sich und zeigt auf eine leuchtend rote Blume. Der Junge lacht und rennt zum Tor."),
        ];
        let work = std::env::temp_dir();
        for (voice, text) in texts {
            let mut v = Voice::new(voice, 0, work.clone());
            let samples = v.say(text, 0).unwrap();
            let secs = samples.len() as f64 / RATE as f64;
            let words = text.split_whitespace().count();
            println!(
                "{voice}: {words} words, {secs:.2} s, {:.2} words a second",
                words as f64 / secs
            );
        }
    }

    #[test]
    fn silence_is_trimmed_and_the_level_evened() {
        let mut s = vec![0i16; RATE as usize];
        s.extend((0..RATE).map(|i| ((i as f32 * 0.05).sin() * 3000.0) as i16));
        s.extend(vec![0i16; RATE as usize]);
        let t = level(&trim(&s));
        let secs = t.len() as f64 / RATE as f64;
        assert!((1.0..1.1).contains(&secs), "{secs}");
        let rms = (t.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / t.len() as f64).sqrt();
        assert!((rms / 32767.0 - 0.16).abs() < 0.02, "{rms}");
    }
}
