//! Spike 0(b): can tract run Silero VAD v6.2.3, and how fast?
//!
//! Every window is 512 new samples at 16 kHz with the previous 64 prepended
//! (576 in all), as `OnnxWrapper.__call__` does in silero_vad/utils_vad.py at
//! the pinned commit 5cd7945676eb32225748052e2e6a0580e4686a08. The per-window
//! exports also take a recurrent state [2, 1, 128] and the sample rate; the
//! sequence export takes [N, 576] and the LSTM's h and c.

use std::time::Instant;
use tract_onnx::prelude::*;

const RATE: u32 = 16_000;
const WINDOW: usize = 512;
const CONTEXT: usize = 64;
/// The only export tract 0.21 can run: the others contain If nodes whose two
/// branches disagree on shape, which tract refuses to type (log entry 4).
const DEFAULT_MODEL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/models/silero_vad_16k_sequence.onnx"
);

/// Silero's own defaults from `get_speech_timestamps` at the pinned commit.
pub struct Params {
    pub threshold: f32,
    pub neg_threshold: f32,
    pub min_speech_ms: u32,
    pub min_silence_ms: u32,
    pub pad_ms: u32,
}

pub const SILERO_DEFAULTS: Params = Params {
    threshold: 0.5,
    neg_threshold: 0.35,
    min_speech_ms: 250,
    min_silence_ms: 100,
    pad_ms: 30,
};

pub fn run(wav_path: &str, model_path: Option<&str>) -> Result<(), String> {
    let audio = crate::wav::read(wav_path)?;
    if audio.rate != RATE {
        return Err(format!("need 16 kHz audio, got {} Hz", audio.rate));
    }
    let model_path = model_path.unwrap_or(DEFAULT_MODEL);
    let t = Instant::now();
    let model = load(model_path)?;
    println!(
        "model     {model_path}, loaded and optimised in {:.2} s",
        t.elapsed().as_secs_f64()
    );

    let t = Instant::now();
    let probs = probabilities(&model, &audio.samples)?;
    let secs = audio.samples.len() as f64 / RATE as f64;
    let took = t.elapsed().as_secs_f64();
    println!(
        "audio     {secs:.1} s, {} frames in {took:.2} s ({:.0}x real time)",
        probs.len(),
        secs / took
    );

    let speech = intervals(&probs, audio.samples.len(), &SILERO_DEFAULTS);
    crate::vad::report(&speech, secs);
    Ok(())
}

/// Speech intervals with Silero's defaults, and the running time, quietly.
pub fn detect(wav_path: &str, model_path: Option<&str>) -> Result<(Vec<(f64, f64)>, f64), String> {
    let audio = crate::wav::read(wav_path)?;
    if audio.rate != RATE {
        return Err(format!("need 16 kHz audio, got {} Hz", audio.rate));
    }
    let model = load(model_path.unwrap_or(DEFAULT_MODEL))?;
    let probs = probabilities(&model, &audio.samples)?;
    let secs = audio.samples.len() as f64 / RATE as f64;
    // SPIKE_VAD_DUMP writes one probability per line, to compare with ONNX Runtime.
    if let Ok(path) = std::env::var("SPIKE_VAD_DUMP") {
        let text: String = probs.iter().map(|p| format!("{p}\n")).collect();
        std::fs::write(&path, text).map_err(|e| format!("{path}: {e}"))?;
    }
    // SPIKE_VAD_THRESHOLD tries other thresholds; the negative one follows it as in Silero.
    let mut p = SILERO_DEFAULTS;
    if let Some(t) = std::env::var("SPIKE_VAD_THRESHOLD")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        p.threshold = t;
        p.neg_threshold = (t - 0.15).max(0.01);
    }
    Ok((intervals(&probs, audio.samples.len(), &p), secs))
}

/// The inputs and outputs a model declares, before we impose any facts.
pub fn info(path: &str) -> Result<(), String> {
    let m = tract_onnx::onnx()
        .model_for_path(path)
        .map_err(|e| format!("{e:?}"))?;
    for (i, o) in m
        .input_outlets()
        .map_err(|e| e.to_string())?
        .iter()
        .enumerate()
    {
        println!(
            "input  {i}: {} {:?}",
            m.node(o.node).name,
            m.outlet_fact(*o).map_err(|e| e.to_string())?
        );
    }
    for (i, o) in m
        .output_outlets()
        .map_err(|e| e.to_string())?
        .iter()
        .enumerate()
    {
        println!(
            "output {i}: {} {:?}",
            m.node(o.node).name,
            m.outlet_fact(*o).map_err(|e| e.to_string())?
        );
    }
    Ok(())
}

type Plan = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

#[derive(Clone, Copy, PartialEq)]
enum Input {
    Audio,
    State,
    Rate,
    H,
    C,
}

/// Silero's exports take differently named inputs in different orders. The
/// `_sequence` export takes many windows at once and carries the LSTM's h
/// and c separately; the others take one window and a combined state.
pub struct Model {
    plan: Plan,
    order: Vec<Input>,
}

/// Windows per call for the sequence export: about 8 seconds of audio.
fn batch() -> usize {
    std::env::var("SPIKE_VAD_BATCH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256)
}

fn load(path: &str) -> Result<Model, String> {
    let e = |e: TractError| format!("{path}: {e:?}");
    let mut m = tract_onnx::onnx().model_for_path(path).map_err(e)?;
    let mut order = Vec::new();
    for i in 0..m.input_outlets().map_err(e)?.len() {
        let name = m.node(m.input_outlets().map_err(e)?[i].node).name.clone();
        order.push(match name.as_str() {
            "input" => Input::Audio,
            "state" => Input::State,
            "sr" => Input::Rate,
            "h" => Input::H,
            "c" => Input::C,
            other => return Err(format!("{path}: unexpected input {other}")),
        });
    }
    let sequence = order.contains(&Input::H);
    for (i, kind) in order.iter().enumerate() {
        let fact: InferenceFact = match kind {
            Input::Audio if sequence => f32::fact([batch(), CONTEXT + WINDOW]).into(),
            Input::Audio => f32::fact([1, CONTEXT + WINDOW]).into(),
            Input::State => f32::fact([2, 1, 128]).into(),
            Input::Rate => InferenceFact::from(tensor0(RATE as i64)),
            Input::H | Input::C => f32::fact([1, 1, 128]).into(),
        };
        m = m.with_input_fact(i, fact).map_err(e)?;
    }
    let plan = m.into_optimized().map_err(e)?.into_runnable().map_err(e)?;
    Ok(Model { plan, order })
}

pub fn probabilities(model: &Model, samples: &[f32]) -> Result<Vec<f32>, String> {
    if model.order.contains(&Input::H) {
        probabilities_sequence(model, samples)
    } else {
        probabilities_windowed(model, samples)
    }
}

fn probabilities_sequence(model: &Model, samples: &[f32]) -> Result<Vec<f32>, String> {
    let s = |e: TractError| e.to_string();
    let windows = samples.len().div_ceil(WINDOW);
    let mut h = Tensor::zero::<f32>(&[1, 1, 128]).map_err(s)?;
    let mut c = Tensor::zero::<f32>(&[1, 1, 128]).map_err(s)?;
    let mut out = Vec::with_capacity(windows);
    let sample = |i: isize| -> f32 {
        if i < 0 {
            0.0
        } else {
            samples.get(i as usize).copied().unwrap_or(0.0)
        }
    };
    let mut first = 0;
    while first < windows {
        // Each row is the 64 samples before the window, then the window.
        let batch = batch();
        let mut x = vec![0f32; batch * (CONTEXT + WINDOW)];
        for row in 0..batch {
            let start = ((first + row) * WINDOW) as isize - CONTEXT as isize;
            for k in 0..CONTEXT + WINDOW {
                x[row * (CONTEXT + WINDOW) + k] = sample(start + k as isize);
            }
        }
        let input = Tensor::from_shape(&[batch, CONTEXT + WINDOW], &x).map_err(s)?;
        let inputs: TVec<TValue> = model
            .order
            .iter()
            .map(|k| match k {
                Input::Audio => input.clone().into(),
                Input::H => h.clone().into(),
                _ => c.clone().into(),
            })
            .collect();
        let result = model.plan.run(inputs).map_err(|e| format!("{e:?}"))?;
        let probs = result[0].as_slice::<f32>().map_err(s)?;
        let real = (windows - first).min(batch);
        out.extend_from_slice(&probs[..real]);
        let (hi, ci) = if std::env::var("SPIKE_VAD_SWAP").is_ok() {
            (2, 1)
        } else {
            (1, 2)
        };
        h = result[hi].clone().into_tensor();
        c = result[ci].clone().into_tensor();
        first += batch;
    }
    Ok(out)
}

fn probabilities_windowed(model: &Model, samples: &[f32]) -> Result<Vec<f32>, String> {
    let mut state = Tensor::zero::<f32>(&[2, 1, 128]).map_err(|e| e.to_string())?;
    let mut context = vec![0f32; CONTEXT];
    let sr = tensor0(RATE as i64);
    let mut out = Vec::with_capacity(samples.len() / WINDOW + 1);
    for chunk in samples.chunks(WINDOW) {
        let mut x = Vec::with_capacity(CONTEXT + WINDOW);
        x.extend_from_slice(&context);
        x.extend_from_slice(chunk);
        x.resize(CONTEXT + WINDOW, 0.0); // the last chunk, zero-padded as Silero does
        context.copy_from_slice(&x[WINDOW..]);
        let input = Tensor::from_shape(&[1, CONTEXT + WINDOW], &x).map_err(|e| e.to_string())?;
        let inputs: TVec<TValue> = model
            .order
            .iter()
            .map(|k| match k {
                Input::Audio => input.clone().into(),
                Input::State => state.clone().into(),
                _ => sr.clone().into(),
            })
            .collect();
        let result = model.plan.run(inputs).map_err(|e| format!("{e:?}"))?;
        // Output is [1, 1]: one probability for the window.
        let p = result[0].as_slice::<f32>().map_err(|e| e.to_string())?[0];
        out.push(p);
        state = result[1].clone().into_tensor();
    }
    Ok(out)
}

/// Silero's hysteresis, without the max-speech splitting we never use:
/// speech starts at `threshold`, ends after `min_silence_ms` below
/// `neg_threshold`, short runs are dropped, and each run is padded.
pub fn intervals(probs: &[f32], total_samples: usize, p: &Params) -> Vec<(f64, f64)> {
    let ms = |m: u32| (RATE as usize * m as usize) / 1000;
    let (min_speech, min_silence, pad) = (ms(p.min_speech_ms), ms(p.min_silence_ms), ms(p.pad_ms));
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    let mut temp_end = 0usize;
    for (i, &prob) in probs.iter().enumerate() {
        let cur = i * WINDOW;
        if prob >= p.threshold {
            temp_end = 0;
            if start.is_none() {
                start = Some(cur);
            }
        } else if prob < p.neg_threshold {
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
        if total_samples - s > min_speech {
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

pub fn report(speech: &[(f64, f64)], secs: f64) {
    let spoken: f64 = speech.iter().map(|(s, e)| e - s).sum();
    println!(
        "speech    {} runs, {:.1} s, {:.0}% of the running time",
        speech.len(),
        spoken,
        100.0 * spoken / secs
    );
    for (s, e) in speech {
        println!("  speech  {s:7.2} - {e:7.2}  ({:.2} s)", e - s);
    }
    let mut gaps = Vec::new();
    let mut at = 0.0;
    for &(s, e) in speech {
        if s > at {
            gaps.push((at, s));
        }
        at = e;
    }
    if secs > at {
        gaps.push((at, secs));
    }
    for (label, min) in [
        ("gaps >= 1 s", 1.0),
        ("gaps >= 2 s", 2.0),
        ("gaps >= 3 s", 3.0),
    ] {
        let g: Vec<_> = gaps.iter().filter(|(s, e)| e - s >= min).collect();
        let total: f64 = g.iter().map(|(s, e)| e - s).sum();
        println!("{label}  {} of them, {total:.1} s in all", g.len());
    }
}
