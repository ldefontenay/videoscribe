//! Milestone 0 spikes (PLAN.md §8). One binary, one subcommand per spike.
//!
//!   vad <file.wav> [model.onnx]             spike 0(b): Silero VAD through tract
//!   gaps <file.wav> [captions.vtt] [--list] spike 0(a): gap density, merge strategies
//!   onnx-info <model.onnx>                  a model's declared inputs and outputs
//!   sapi [out_dir]                          spike 0(g): SAPI 5 voices rendered to memory, timed
//!   mf <command> ...                        spike 0(e): Media Foundation decode, frames, MP3, clips
//!   edge voices | say | measure            spike 0(f): Edge TTS over WinHTTP WebSockets
//!   ytdlp <url> <out_dir> [cancel_s]        spike 0(d): yt-dlp download, progress, captions, cancel
//!   skeleton <video> <start> <end> <service> <out_dir> [captions] [--family]   milestone 1 and spike 0(h)
//!
//! WAVs must be 16 kHz mono 16-bit until spike 0(e) brings Media Foundation.

mod edge;
mod gaps;
mod gec2;
mod mf;
mod net;
mod sapi;
mod skeleton;
mod vad;
mod vtt;
mod wav;
mod ytdlp;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let list = args.iter().any(|a| a == "--list");
    let pos: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with("--"))
        .collect();
    let result = match pos.as_slice() {
        ["vad", wav, rest @ ..] => vad::run(wav, rest.first().copied()),
        ["gaps", wav, rest @ ..] => gaps::run(wav, rest.first().copied(), list),
        ["onnx-info", model] => vad::info(model),
        ["sapi", rest @ ..] => sapi::run(rest.first().copied()),
        ["mf", rest @ ..] => mf::run(rest),
        ["edge", rest @ ..] => edge::run(rest),
        ["ytdlp", rest @ ..] => ytdlp::run(rest),
        ["transcribe", rest @ ..] => skeleton::transcribe(rest),
        ["skeleton", rest @ ..] => skeleton::run(rest, args.iter().any(|a| a == "--family")),
        _ => Err("usage: spike vad <file.wav> [model.onnx] | gaps <file.wav> [captions.vtt] [--list] | onnx-info <model.onnx>".into()),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
