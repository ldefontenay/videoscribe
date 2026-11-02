//! Remembered settings: `%APPDATA%\Video Scribe\settings.txt`, one `key = value`
//! per line, so it can be read and corrected in Notepad (Photo Scribe's format).

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Where described files are written. None: `Videos\Video Scribe`.
    pub output_folder: Option<PathBuf>,
    pub service: Service,
    pub openrouter_model: String,
    pub claude_model: String,
    /// short, standard or detailed.
    pub style: String,
    /// The language descriptions are written and spoken in.
    pub language: String,
    pub room: Room,
    pub output: Output,
    /// An Edge voice, such as "en-GB-SoniaNeural".
    pub voice: String,
    /// Percent faster than the voice's normal speed (log entry 16: 10 is right).
    pub rate: i32,
    /// Send the video's file name, or its YouTube title, with the pictures.
    pub send_names: bool,
    /// Keep video downloads for a day, rather than deleting them once the
    /// described sound is made (log entry 19, Q1).
    pub keep_downloads: bool,
    /// Short optional pauses follow `room` too, rather than always leaving
    /// out what does not fit (log entry 29, Q1).
    pub short_too: bool,
    /// Description requests sent at once (log entry 31); 1 sends them in
    /// turn, each with the story so far.
    pub together: usize,
    /// The preset last chosen, while the choices still match it; or empty.
    pub preset: String,
    /// Most lines of picture to download from the web when the output is a
    /// described video: 480, 720 or 1080 (log entry 39, Q2).
    pub lines: u32,
    /// Find the faces in a family video's pictures, on this computer, and
    /// know people named before by them (log entry 44).
    pub faces: bool,
}

/// Requests sent at once when describing faster is on (log entry 31).
pub const AT_ONCE: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Service {
    OpenRouter,
    Claude,
}

/// What happens when a description does not fit the pause it is meant for
/// (PLAN.md §5.10 and §5.11, one setting).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Room {
    /// Pause the film while the description is spoken: "extended pauses".
    Pause,
    /// Speak over the film, turned down further.
    Over,
    /// Leave the description out.
    GapsOnly,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Output {
    Audio,
    Video,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            output_folder: None,
            // PLAN.md §7 decision 19.
            service: Service::OpenRouter,
            openrouter_model: "z-ai/glm-5.3-flash".into(),
            claude_model: "sonnet".into(),
            style: "standard".into(),
            language: "English".into(),
            // Log entry 39, Q1: speaking over the film, for new users, so a
            // described video works from the start (PLAN.md §7 decision 5).
            room: Room::Over,
            output: Output::Audio,
            // PLAN.md §7 decision 20.
            voice: "en-GB-SoniaNeural".into(),
            rate: 15,
            send_names: false,
            keep_downloads: false,
            short_too: true,
            // Log entry 31: four at once, for the user to compare by ear.
            together: AT_ONCE,
            preset: String::new(),
            lines: 480,
            faces: true,
        }
    }
}

impl Config {
    fn path() -> Option<PathBuf> {
        let appdata = PathBuf::from(std::env::var_os("APPDATA")?);
        Some(appdata.join("Video Scribe").join("settings.txt"))
    }

    /// `%APPDATA%\Video Scribe`, where settings and the help page live.
    pub fn dir() -> Option<PathBuf> {
        Self::path().and_then(|p| p.parent().map(PathBuf::from))
    }

    /// False the very first time Video Scribe runs on this computer.
    pub fn exists() -> bool {
        Self::path().is_some_and(|p| p.exists())
    }

    pub fn load() -> Config {
        let text = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        Self::parse(&text)
    }

    /// Where described files go when no folder has been chosen.
    pub fn default_output_folder() -> PathBuf {
        let home =
            std::env::var_os("USERPROFILE").map_or_else(|| PathBuf::from("."), PathBuf::from);
        home.join("Videos").join("Video Scribe")
    }

    pub fn output_folder(&self) -> PathBuf {
        self.output_folder
            .clone()
            .unwrap_or_else(Self::default_output_folder)
    }

    pub fn parse(text: &str) -> Config {
        let mut cfg = Config::default();
        // Notepad can save with a byte order mark, which would hide the first key.
        for line in text.trim_start_matches('\u{feff}').lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            let lower = value.to_ascii_lowercase();
            let yes = matches!(lower.as_str(), "yes" | "true" | "1" | "on");
            match key {
                "output folder" if !value.is_empty() => {
                    cfg.output_folder = Some(PathBuf::from(value))
                }
                "service" => {
                    cfg.service = match lower.as_str() {
                        "claude" => Service::Claude,
                        _ => Service::OpenRouter,
                    }
                }
                "openrouter model" if !value.is_empty() => cfg.openrouter_model = value.into(),
                "claude model" if !value.is_empty() => cfg.claude_model = value.into(),
                "style" if !value.is_empty() => cfg.style = lower,
                "language" if !value.is_empty() => cfg.language = value.into(),
                "when a description does not fit" => {
                    cfg.room = match lower.as_str() {
                        "pause the film" => Room::Pause,
                        "speak over the film" => Room::Over,
                        "leave it out" => Room::GapsOnly,
                        _ => cfg.room,
                    }
                }
                "output" => {
                    cfg.output = match lower.as_str() {
                        "video" => Output::Video,
                        _ => Output::Audio,
                    }
                }
                "voice" if !value.is_empty() => cfg.voice = value.into(),
                "faster by percent" => {
                    if let Ok(r) = value.trim_end_matches('%').trim().parse::<i32>() {
                        cfg.rate = r.clamp(0, 50);
                    }
                }
                "send the file name" => cfg.send_names = yes,
                // The old name, from before other sites (test round 7, 4).
                "keep video downloads for a day" | "keep youtube downloads for a day" => {
                    cfg.keep_downloads = yes
                }
                "short pauses follow the same choice" => cfg.short_too = yes,
                "know people by their faces in family videos" => cfg.faces = yes,
                "preset" => cfg.preset = value.into(),
                "lines of picture for a described video from the web" => {
                    if let Ok(n) = value.trim().parse::<u32>() {
                        cfg.lines = [480u32, 720, 1080]
                            .into_iter()
                            .min_by_key(|l| l.abs_diff(n))
                            .unwrap_or(480);
                    }
                }
                "description requests at once" => {
                    if let Ok(n) = value.trim().parse::<usize>() {
                        cfg.together = n.clamp(1, 8);
                    }
                }
                _ => {}
            }
        }
        // A hand-edited file can ask for both; pausing would put the film out
        // of step with the sound, so the pause wins and the output is audio.
        if cfg.room == Room::Pause && cfg.output == Output::Video {
            cfg.output = Output::Audio;
        }
        cfg
    }

    pub fn to_text(&self) -> String {
        let yn = |b: bool| if b { "yes" } else { "no" };
        format!(
            "# Video Scribe settings. Safe to edit while Video Scribe is closed.\r\n\
             output folder = {}\r\n\
             service = {}\r\n\
             openrouter model = {}\r\n\
             claude model = {}\r\n\
             style = {}\r\n\
             language = {}\r\n\
             when a description does not fit = {}\r\n\
             output = {}\r\n\
             voice = {}\r\n\
             faster by percent = {}\r\n\
             send the file name = {}\r\n\
             keep video downloads for a day = {}\r\n\
             short pauses follow the same choice = {}\r\n\
             description requests at once = {}\r\n\
             lines of picture for a described video from the web = {}\r\n\
             know people by their faces in family videos = {}\r\n\
             preset = {}\r\n",
            self.output_folder
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            match self.service {
                Service::OpenRouter => "openrouter",
                Service::Claude => "claude",
            },
            self.openrouter_model,
            self.claude_model,
            self.style,
            self.language,
            match self.room {
                Room::Pause => "pause the film",
                Room::Over => "speak over the film",
                Room::GapsOnly => "leave it out",
            },
            match self.output {
                Output::Audio => "audio",
                Output::Video => "video",
            },
            self.voice,
            self.rate,
            yn(self.send_names),
            yn(self.keep_downloads),
            yn(self.short_too),
            self.together,
            self.lines,
            yn(self.faces),
            self.preset,
        )
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.to_text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let cfg = Config {
            output_folder: Some(PathBuf::from(r"D:\Described")),
            service: Service::Claude,
            openrouter_model: "google/gemma-4-31b-it".into(),
            claude_model: "haiku".into(),
            style: "short".into(),
            language: "French".into(),
            room: Room::Over,
            output: Output::Video,
            voice: "fr-FR-DeniseNeural".into(),
            rate: 20,
            send_names: true,
            keep_downloads: true,
            short_too: false,
            together: 4,
            preset: "French films".into(),
            lines: 720,
            faces: false,
        };
        assert_eq!(Config::parse(&cfg.to_text()), cfg);
    }

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(Config::parse(""), Config::default());
    }

    #[test]
    fn an_old_settings_file_keeps_its_choice_of_pausing() {
        let cfg = Config::parse("when a description does not fit = pause the film\n");
        assert_eq!(cfg.room, Room::Pause);
        assert_eq!(Config::default().room, Room::Over);
        assert_eq!(
            Config::parse("lines of picture for a described video from the web = 700\n").lines,
            720
        );
    }

    #[test]
    fn pausing_and_video_cannot_both_be_chosen() {
        let cfg =
            Config::parse("when a description does not fit = pause the film\noutput = video\n");
        assert_eq!((cfg.room, cfg.output), (Room::Pause, Output::Audio));
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_line() {
        let cfg = Config::parse("\u{feff}output folder = D:\\Out\r\n");
        assert_eq!(cfg.output_folder, Some(PathBuf::from("D:\\Out")));
    }
}
