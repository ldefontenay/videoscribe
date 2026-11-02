//! Listening to a whole described sound, with pause and skipping (asked for
//! in the milestone 8 test): Media Foundation decodes from wherever playing
//! starts, and Windows' waveOut plays it a tenth of a second at a time, so a
//! long film is never held in memory and skipping starts again at once.
//!
//! The player has a thread of its own, as Media Foundation must not start on
//! the window's thread. The window sends it commands and reads where it is.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Win32::Media::Audio::*;
use windows::Win32::Media::{MMTIME, TIME_SAMPLES};

use crate::media;

const RATE: u32 = 48_000;
const CHANNELS: u32 = 2;
/// Frames in one buffer, a tenth of a second, and buffers queued at once: a
/// second ahead, enough that decoding never makes it stutter.
const BLOCK: usize = 4_800;
const BUFFERS: usize = 10;

enum Command {
    Play,
    Pause,
    Seek(f64),
    Quit,
}

/// What the window can read at any time.
#[derive(Default)]
struct Shared {
    /// Where playing is, in milliseconds.
    at_ms: AtomicU64,
    playing: AtomicBool,
    /// Played to the end, and not yet told.
    ended: AtomicBool,
    /// Why it stopped, if it failed.
    failed: Mutex<Option<String>>,
}

impl Shared {
    fn set_at(&self, secs: f64) {
        self.at_ms
            .store((secs.max(0.0) * 1000.0) as u64, Ordering::Relaxed);
    }
}

pub struct Player {
    send: Sender<Command>,
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    secs: f64,
}

impl Player {
    /// A player for `path`, paused at the start.
    pub fn open(path: &Path) -> Result<Player, String> {
        let secs = {
            let p = path.to_string_lossy().to_string();
            std::thread::spawn(move || {
                let _session = media::Session::start()?;
                media::duration(&p)
            })
            .join()
            .map_err(|_| "The sound could not be read.".to_string())??
        };
        let (send, receive) = channel();
        let shared = Arc::new(Shared::default());
        let path = path.to_path_buf();
        let theirs = shared.clone();
        let thread = std::thread::spawn(move || {
            if let Err(why) = run(&path, receive, &theirs) {
                theirs.playing.store(false, Ordering::Relaxed);
                *theirs.failed.lock().unwrap() = Some(why);
            }
        });
        Ok(Player {
            send,
            shared,
            thread: Some(thread),
            secs,
        })
    }

    /// The sound's length in seconds.
    pub fn secs(&self) -> f64 {
        self.secs
    }

    /// Where playing is, in seconds.
    pub fn at(&self) -> f64 {
        (self.shared.at_ms.load(Ordering::Relaxed) as f64 / 1000.0).min(self.secs)
    }

    pub fn playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    /// True once, after playing reaches the end.
    pub fn take_ended(&self) -> bool {
        self.shared.ended.swap(false, Ordering::Relaxed)
    }

    pub fn take_failure(&self) -> Option<String> {
        self.shared.failed.lock().unwrap().take()
    }

    pub fn play(&self) {
        // From the start again once it has played to the end.
        if self.at() >= self.secs - 0.05 {
            self.seek(0.0);
        }
        self.shared.playing.store(true, Ordering::Relaxed);
        let _ = self.send.send(Command::Play);
    }

    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
        let _ = self.send.send(Command::Pause);
    }

    /// Moves to `secs`, kept within the sound, playing on if it was playing.
    /// Where it lands.
    pub fn seek(&self, secs: f64) -> f64 {
        let to = secs.clamp(0.0, self.secs);
        self.shared.set_at(to);
        let _ = self.send.send(Command::Seek(to));
        to
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.send.send(Command::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Puts back this app's volume on the default output if it is at nought.
/// Silencing is done by writing silence, never by the volume: Windows keeps
/// an app's volume for each device, so a volume set to nought once muted the
/// user's earphones for good (round 2 test). PlaySound shares that volume.
pub fn unmute_default() {
    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 1,
        nSamplesPerSec: 48_000,
        nAvgBytesPerSec: 96_000,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let mut device = HWAVEOUT::default();
    unsafe {
        if waveOutOpen(
            Some(&mut device),
            WAVE_MAPPER,
            &format,
            None,
            None,
            CALLBACK_NULL,
        ) == 0
        {
            unmute(device);
            waveOutClose(device);
        }
    }
}

fn unmute(device: HWAVEOUT) {
    let mut volume = 0u32;
    unsafe {
        if waveOutGetVolume(Some(device), &mut volume) == 0 && volume == 0 {
            waveOutSetVolume(Some(device), u32::MAX);
        }
    }
}

/// The output device and its buffers. The buffers' memory stays put while
/// Windows plays from it: each is allocated once and never resized.
struct Out {
    device: HWAVEOUT,
    /// Tests play silence, and so does the app when checked overnight.
    silent: bool,
    headers: Vec<WAVEHDR>,
    data: Vec<Vec<u8>>,
    /// Which buffers Windows has and has not finished with.
    queued: Vec<bool>,
}

const HEADER: u32 = std::mem::size_of::<WAVEHDR>() as u32;

impl Out {
    fn open() -> Result<Out, String> {
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: CHANNELS as u16,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * CHANNELS * 2,
            nBlockAlign: (CHANNELS * 2) as u16,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let mut device = HWAVEOUT::default();
        let r = unsafe {
            waveOutOpen(
                Some(&mut device),
                WAVE_MAPPER,
                &format,
                None,
                None,
                CALLBACK_NULL,
            )
        };
        if r != 0 {
            return Err(format!(
                "Windows could not open the sound output (error {r}). Check that a speaker or \
                 headphones are connected."
            ));
        }
        unmute(device);
        Ok(Out {
            device,
            silent: cfg!(test) || std::env::var_os("VIDEOSCRIBE_SILENT").is_some(),
            headers: vec![WAVEHDR::default(); BUFFERS],
            data: (0..BUFFERS)
                .map(|_| vec![0u8; BLOCK * CHANNELS as usize * 2])
                .collect(),
            queued: vec![false; BUFFERS],
        })
    }

    /// Takes back the buffers Windows has finished with. Whether any are
    /// still queued.
    fn reap(&mut self) -> bool {
        for i in 0..BUFFERS {
            if self.queued[i] && self.headers[i].dwFlags & WHDR_DONE != 0 {
                unsafe {
                    waveOutUnprepareHeader(self.device, &mut self.headers[i], HEADER);
                }
                self.queued[i] = false;
            }
        }
        self.queued.iter().any(|&q| q)
    }

    fn free(&mut self) -> Option<usize> {
        self.reap();
        self.queued.iter().position(|&q| !q)
    }

    fn write(&mut self, i: usize, samples: &[i16]) {
        let bytes = &mut self.data[i];
        for (k, s) in samples.iter().enumerate() {
            let s = if self.silent { 0 } else { *s };
            bytes[k * 2..k * 2 + 2].copy_from_slice(&s.to_le_bytes());
        }
        let h = &mut self.headers[i];
        *h = WAVEHDR::default();
        h.lpData = windows::core::PSTR(bytes.as_mut_ptr());
        h.dwBufferLength = (samples.len() * 2) as u32;
        unsafe {
            waveOutPrepareHeader(self.device, h, HEADER);
            waveOutWrite(self.device, h, HEADER);
        }
        self.queued[i] = true;
    }

    fn pause(&self) {
        unsafe {
            waveOutPause(self.device);
        }
    }

    fn resume(&self) {
        unsafe {
            waveOutRestart(self.device);
        }
    }

    /// Stops at once and takes back every buffer; the count of what was
    /// played starts again at nought.
    fn reset(&mut self) {
        unsafe {
            waveOutReset(self.device);
        }
        self.reap();
    }

    /// Seconds played since the last reset.
    fn played(&self) -> f64 {
        let mut t = MMTIME {
            wType: TIME_SAMPLES,
            ..Default::default()
        };
        unsafe {
            waveOutGetPosition(self.device, &mut t, std::mem::size_of::<MMTIME>() as u32);
            t.u.sample as f64 / RATE as f64
        }
    }
}

impl Drop for Out {
    fn drop(&mut self) {
        self.reset();
        unsafe {
            waveOutClose(self.device);
        }
    }
}

/// Playing from one place: the device, where it started, and whether it is
/// paused. Answers commands between buffers.
struct Run<'a> {
    out: Out,
    commands: &'a Receiver<Command>,
    shared: &'a Shared,
    base: f64,
    paused: bool,
    /// A command that ends this run: a seek, or quitting.
    next: Option<Command>,
}

impl Run<'_> {
    /// Waits up to 15 ms for a command and acts on it. False once a command
    /// ends the run.
    fn listen(&mut self) -> bool {
        if !self.paused {
            self.shared.set_at(self.base + self.out.played());
        }
        match self.commands.recv_timeout(Duration::from_millis(15)) {
            Ok(Command::Pause) => {
                if !self.paused {
                    self.out.pause();
                    self.paused = true;
                    self.shared.set_at(self.base + self.out.played());
                }
            }
            Ok(Command::Play) => {
                if self.paused {
                    self.out.resume();
                    self.paused = false;
                }
            }
            Ok(c) => self.next = Some(c),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => self.next = Some(Command::Quit),
        }
        self.next.is_none()
    }

    /// A free buffer, waiting for one; None if a command ends the run.
    fn room(&mut self) -> Option<usize> {
        loop {
            if !self.paused {
                if let Some(i) = self.out.free() {
                    return Some(i);
                }
            }
            if !self.listen() {
                return None;
            }
        }
    }
}

/// Why decoding was broken off.
const BROKEN_OFF: &str = "\u{1}broken off";

/// The player's thread: paused until told to play, then decoding and queueing
/// from where it is.
fn run(path: &Path, commands: Receiver<Command>, shared: &Shared) -> Result<(), String> {
    let _session = media::Session::start()?;
    let path: PathBuf = path.to_path_buf();
    let mut run = Run {
        out: Out::open()?,
        commands: &commands,
        shared,
        base: 0.0,
        paused: false,
        next: None,
    };
    let mut playing = false;
    let per = CHANNELS as usize;
    loop {
        if !playing {
            match commands.recv() {
                Ok(Command::Play) => playing = true,
                Ok(Command::Pause) => {}
                Ok(Command::Seek(to)) => run.base = to,
                Ok(Command::Quit) | Err(_) => return Ok(()),
            }
            continue;
        }
        shared.ended.store(false, Ordering::Relaxed);
        run.out.reset();
        // A reset device may still be paused from before.
        run.out.resume();
        run.paused = false;
        run.next = None;
        let first = (run.base * RATE as f64).round() as i64;
        let mut block: Vec<i16> = Vec::with_capacity(BLOCK * per);
        let stop = AtomicBool::new(false);
        let decoded = media::from(
            &path.to_string_lossy(),
            run.base,
            RATE,
            CHANNELS,
            &stop,
            &mut |at, s| {
                let start = (at * RATE as f64).round() as i64;
                for (k, frame) in s.chunks_exact(per).enumerate() {
                    // Seeking lands near the place, not on it.
                    if start + k as i64 >= first {
                        block.extend_from_slice(frame);
                    }
                    if block.len() == BLOCK * per {
                        let Some(i) = run.room() else {
                            return Err(BROKEN_OFF.into());
                        };
                        run.out.write(i, &block);
                        block.clear();
                    }
                }
                Ok(())
            },
        );
        match decoded {
            Err(why) if why != BROKEN_OFF => return Err(why),
            Err(_) => {}
            Ok(_) => {
                // The last of it, then play out what is queued.
                if !block.is_empty() {
                    if let Some(i) = run.room() {
                        run.out.write(i, &block);
                    }
                }
                while run.next.is_none() && (run.paused || run.out.reap()) {
                    run.listen();
                }
                if run.next.is_none() {
                    let end = run.base + run.out.played();
                    run.out.reset();
                    run.base = end;
                    shared.set_at(end);
                    playing = false;
                    shared.playing.store(false, Ordering::Relaxed);
                    shared.ended.store(true, Ordering::Relaxed);
                    continue;
                }
            }
        }
        // Paused when moved: it stays paused at the new place.
        if run.paused {
            playing = false;
        }
        match run.next.take() {
            Some(Command::Seek(to)) => {
                run.out.reset();
                run.base = to;
                shared.set_at(to);
            }
            Some(Command::Quit) | None => return Ok(()),
            Some(_) => {}
        }
    }
}

#[cfg(test)]
mod on_a_file {
    use super::*;

    /// Needs a sound file: `VS_TEST_SOUND=<path> cargo test -- --ignored`.
    /// Plays at no volume.
    #[test]
    #[ignore]
    fn plays_pauses_and_skips() {
        let path = std::env::var("VS_TEST_SOUND").unwrap();
        let p = Player::open(Path::new(&path)).unwrap();
        assert!(p.secs() > 70.0);
        let wait = |ms| std::thread::sleep(Duration::from_millis(ms));
        p.play();
        wait(1500);
        let a = p.at();
        assert!(a > 0.5 && a < 1.6, "{a}");
        p.pause();
        wait(300);
        let b = p.at();
        wait(500);
        assert!((p.at() - b).abs() < 0.05, "moved while paused");
        assert!(!p.playing());
        // Moved while paused: stays paused there.
        p.seek(60.0);
        wait(300);
        assert!((p.at() - 60.0).abs() < 0.05, "{}", p.at());
        p.play();
        wait(1000);
        let c = p.at();
        assert!(c > 60.3 && c < 61.1, "{c}");
        // Moved while playing: plays on from there.
        p.seek(30.0);
        wait(1000);
        let d = p.at();
        assert!(d > 30.3 && d < 31.1, "{d}");
        // To the end.
        p.seek(p.secs() - 0.5);
        wait(1500);
        assert!(p.take_ended());
        assert!(!p.playing());
        assert!(p.take_failure().is_none());
    }
}
