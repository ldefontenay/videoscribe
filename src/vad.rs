//! Silero VAD v6.2.3 (MIT), in-process through tract, pure Rust (PLAN.md
//! §4.1). The model is embedded in the exe; models/README.txt pins it.
//!
//! The sequence export takes many windows at once, each 512 new samples at
//! 16 kHz with the previous 64 before them, and carries the LSTM's h and c
//! from one batch to the next, as `OnnxWrapper.__call__` does in Silero's
//! utils_vad.py at the pinned commit (spike 0(b), log entry 4).

use std::sync::atomic::{AtomicBool, Ordering};

use tract_onnx::prelude::*;

use crate::speech::Spans;

pub const RATE: u32 = 16_000;
const WINDOW: usize = 512;
const CONTEXT: usize = 64;
/// Windows per call: about 8 seconds of sound.
const BATCH: usize = 256;
/// Windows of run-up before each batch's own, half a second: the model starts
/// afresh each batch (see `probabilities`).
const RUN_UP: usize = 16;
const MODEL: &[u8] = include_bytes!("../models/silero_vad_16k_sequence.onnx");

/// Silero's own defaults from `get_speech_timestamps` at the pinned commit.
/// Its 100 ms minimum silence splits speech at every breath; pauses under a
/// second are never used, so that does no harm.
const THRESHOLD: f32 = 0.5;
const NEG_THRESHOLD: f32 = 0.35;
const MIN_SPEECH_MS: usize = 250;
const MIN_SILENCE_MS: usize = 100;
const PAD_MS: usize = 30;

type Plan = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

fn load() -> Result<Plan, String> {
    let e = |e: TractError| format!("The speech detector would not load: {e}");
    let mut m = tract_onnx::onnx()
        .model_for_read(&mut std::io::Cursor::new(MODEL))
        .map_err(e)?;
    // Inputs in the export's order: input [N, 576], h [1, 1, 128], c [1, 1, 128].
    m = m
        .with_input_fact(0, f32::fact([BATCH, CONTEXT + WINDOW]).into())
        .map_err(e)?
        .with_input_fact(1, f32::fact([1, 1, 128]).into())
        .map_err(e)?
        .with_input_fact(2, f32::fact([1, 1, 128]).into())
        .map_err(e)?;
    m.into_optimized().map_err(e)?.into_runnable().map_err(e)
}

/// One speech probability per 32 ms window. None if `stop` is set.
///
/// Each batch starts the model afresh, after a short run-up from the sound
/// just before it, rather than carrying its memory through the whole film.
/// Carried through, it drifted: on the wedding copy, after the choir, it heard
/// none of the first ten seconds of the vows, which it scored at 0.9 and more
/// when started a minute later. Started afresh every 7.7 s, it agreed better
/// with Sprite Fright's captions both ways (68% to 75% of the captioned speech
/// heard, 65% to 75% of what it heard captioned) and found 97 places, not 91.
pub fn probabilities(
    samples: &[i16],
    stop: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Option<Vec<f32>>, String> {
    let plan = load()?;
    let s = |e: TractError| format!("The speech detector failed: {e}");
    let windows = samples.len().div_ceil(WINDOW);
    let kept = BATCH - RUN_UP;
    let mut out = Vec::with_capacity(windows);
    let sample = |i: isize| -> f32 {
        if i < 0 {
            0.0
        } else {
            samples.get(i as usize).map_or(0.0, |&v| v as f32 / 32768.0)
        }
    };
    let mut first = 0;
    let mut x = vec![0f32; BATCH * (CONTEXT + WINDOW)];
    while first < windows {
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        // Each row is the 64 samples before the window, then the window;
        // the first rows are the run-up, from before `first`.
        let from = first as isize - RUN_UP as isize;
        for row in 0..BATCH {
            let start = (from + row as isize) * WINDOW as isize - CONTEXT as isize;
            for k in 0..CONTEXT + WINDOW {
                x[row * (CONTEXT + WINDOW) + k] = sample(start + k as isize);
            }
        }
        let input = Tensor::from_shape(&[BATCH, CONTEXT + WINDOW], &x).map_err(s)?;
        let h = Tensor::zero::<f32>(&[1, 1, 128]).map_err(s)?;
        let c = Tensor::zero::<f32>(&[1, 1, 128]).map_err(s)?;
        let result = plan
            .run(tvec![input.into(), h.into(), c.into()])
            .map_err(s)?;
        let probs = result[0].as_slice::<f32>().map_err(s)?;
        let real = (windows - first).min(kept);
        out.extend_from_slice(&probs[RUN_UP..RUN_UP + real]);
        first += kept;
        progress(first.min(windows) as f64 / windows.max(1) as f64);
    }
    Ok(Some(out))
}

/// Silero's hysteresis, without the max-speech splitting we never use:
/// speech starts at the threshold, ends after the minimum silence below the
/// negative threshold, short runs are dropped, and each run is padded.
pub fn intervals(probs: &[f32], total_samples: usize) -> Spans {
    let ms = |m: usize| RATE as usize * m / 1000;
    let (min_speech, min_silence, pad) = (ms(MIN_SPEECH_MS), ms(MIN_SILENCE_MS), ms(PAD_MS));
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    let mut temp_end = 0usize;
    for (i, &prob) in probs.iter().enumerate() {
        let cur = i * WINDOW;
        if prob >= THRESHOLD {
            temp_end = 0;
            if start.is_none() {
                start = Some(cur);
            }
        } else if prob < NEG_THRESHOLD {
            if let Some(s) = start {
                if temp_end == 0 {
                    temp_end = cur;
                }
                if cur - temp_end >= min_silence {
                    if temp_end - s > min_speech {
                        runs.push((s, temp_end));
                    }
                    start = None;
                    temp_end = 0;
                }
            }
        }
    }
    if let Some(s) = start {
        if total_samples.saturating_sub(s) > min_speech {
            runs.push((s, total_samples));
        }
    }
    // Pad, splitting the silence where two padded runs would touch: Silero's own loop.
    let n = runs.len();
    for i in 0..n {
        if i == 0 {
            runs[0].0 = runs[0].0.saturating_sub(pad);
        }
        if i + 1 < n {
            let silence = runs[i + 1].0 - runs[i].1;
            if silence < 2 * pad {
                runs[i].1 += silence / 2;
                runs[i + 1].0 -= silence / 2;
            } else {
                runs[i].1 = (runs[i].1 + pad).min(total_samples);
                runs[i + 1].0 = runs[i + 1].0.saturating_sub(pad);
            }
        } else {
            runs[i].1 = (runs[i].1 + pad).min(total_samples);
        }
    }
    runs.iter()
        .map(|&(s, e)| (s as f64 / RATE as f64, e as f64 / RATE as f64))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_model_loads() {
        load().unwrap();
    }

    #[test]
    fn silence_is_not_speech() {
        let stop = AtomicBool::new(false);
        let probs = probabilities(&vec![0i16; RATE as usize * 3], &stop, &mut |_| {})
            .unwrap()
            .unwrap();
        assert_eq!(probs.len(), (RATE as usize * 3).div_ceil(WINDOW));
        assert!(intervals(&probs, RATE as usize * 3).is_empty());
    }

    #[test]
    fn hysteresis_needs_a_long_enough_silence_to_end_speech() {
        // 1 s of speech, a 64 ms dip (too short to end it), 1 s more, then silence.
        let mut p = vec![0.9f32; 31];
        p.extend([0.1, 0.1]);
        p.extend(vec![0.9f32; 31]);
        p.extend(vec![0.0f32; 40]);
        let spans = intervals(&p, p.len() * WINDOW);
        assert_eq!(spans.len(), 1, "{spans:?}");
        assert!(
            (spans[0].1 - (64.0 * 0.032 + 0.03)).abs() < 0.01,
            "{spans:?}"
        );
    }
}
