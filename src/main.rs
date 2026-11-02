#![windows_subsystem = "windows"]

//! Video Scribe: adds spoken audio description to YouTube and family videos.
//! See PLAN.md for decisions and milestones.

mod addvideo;
mod announce;
mod beeps;
mod cache;
mod captions;
mod choices;
mod claude;
mod clips;
mod config;
mod describe;
mod edge;
mod faces;
mod frames;
mod gec2;
mod job;
mod known;
mod library;
mod listen;
mod media;
mod message;
mod mix;
mod mux;
mod namebox;
mod net;
mod openrouter;
mod people;
mod peopleui;
mod play;
mod player;
mod presets;
mod report;
mod review;
mod sapi;
mod speak;
mod speech;
mod tags;
mod ui;
mod vad;
mod videos;
mod voicebox;
mod voices;
mod ytdlp;

use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

fn main() {
    // The UI thread is a single-threaded apartment: the file picker is a COM
    // object that insists on it.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let first_run = !config::Config::exists();
    ui::run(config::Config::load(), first_run);
}

/// A string as a null-terminated UTF-16 buffer for Win32.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
