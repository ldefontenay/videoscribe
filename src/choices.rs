//! What the menus offer, what each choice says when it is made, and the
//! "what will be sent" summary shown before a job. No Win32 here, so all of
//! it is tested.
//!
//! PLAN.md §5.10: no control is greyed out to express a constraint. When one
//! choice changes another, the change is made and said in the same sentence.

use std::path::{Path, PathBuf};

use crate::config::{Config, Output, Room, Service};

pub const SERVICES: &[(Service, &str)] = &[
    (
        Service::OpenRouter,
        "OpenRouter: many models, paid per use with a key",
    ),
    (
        Service::Claude,
        "Claude Code: uses your Claude subscription",
    ),
];

/// (model, what the menu says). The first is the default (PLAN.md §7 decision 19).
pub const OPENROUTER_MODELS: &[(&str, &str)] = &[
    (
        "z-ai/glm-5.3-flash",
        "GLM 5.3 Flash, from Z.ai: the fastest, about 8 US cents an hour of video",
    ),
    (
        "google/gemini-3.5-flash-lite",
        "Gemini 3.5 Flash-Lite, from Google: watches short clips and hears their sound, about 25 US cents an hour of video",
    ),
    (
        "google/gemma-4-31b-it",
        "Gemma 4 31B, from Google: slow, about 20 US cents an hour of video",
    ),
];

pub const CLAUDE_MODELS: &[(&str, &str)] = &[
    ("sonnet", "Sonnet: the one tested so far"),
    ("haiku", "Haiku: faster, not tested yet"),
    ("opus", "Opus: slowest, not tested yet"),
];

pub const STYLES: &[(&str, &str)] = &[
    ("short", "Short: a few words in each pause"),
    ("standard", "Standard: fills each pause"),
    ("detailed", "Detailed: more about people and places"),
];

pub struct Voice {
    pub id: &'static str,
    pub name: &'static str,
    pub language: &'static str,
    /// How the menu describes it after the name.
    pub kind: &'static str,
}

const fn v(
    id: &'static str,
    name: &'static str,
    language: &'static str,
    kind: &'static str,
) -> Voice {
    Voice {
        id,
        name,
        language,
        kind,
    }
}

/// Edge's neural voices, from its own voice list (26 October 2026): the
/// first of each language is the one chosen with the language, and this
/// list stands in for Edge's whole list until that is fetched (`voices.rs`).
pub const VOICES: &[Voice] = &[
    v("en-GB-SoniaNeural", "Sonia", "English", "British, woman"),
    v("en-GB-LibbyNeural", "Libby", "English", "British, woman"),
    v("en-GB-RyanNeural", "Ryan", "English", "British, man"),
    v("en-GB-ThomasNeural", "Thomas", "English", "British, man"),
    v("en-GB-MaisieNeural", "Maisie", "English", "British, girl"),
    v("en-US-AriaNeural", "Aria", "English", "American, woman"),
    v("en-US-JennyNeural", "Jenny", "English", "American, woman"),
    v("en-US-AvaNeural", "Ava", "English", "American, woman"),
    v("en-US-GuyNeural", "Guy", "English", "American, man"),
    v("en-US-AndrewNeural", "Andrew", "English", "American, man"),
    v("fr-FR-DeniseNeural", "Denise", "French", "woman"),
    v("fr-FR-HenriNeural", "Henri", "French", "man"),
    v("fr-FR-EloiseNeural", "Eloise", "French", "girl"),
    v("de-DE-KatjaNeural", "Katja", "German", "woman"),
    v("de-DE-AmalaNeural", "Amala", "German", "woman"),
    v("de-DE-ConradNeural", "Conrad", "German", "man"),
    v("de-DE-KillianNeural", "Killian", "German", "man"),
    v("es-ES-ElviraNeural", "Elvira", "Spanish", "woman"),
    v("es-ES-XimenaNeural", "Ximena", "Spanish", "woman"),
    v("es-ES-AlvaroNeural", "Alvaro", "Spanish", "man"),
    v("it-IT-ElsaNeural", "Elsa", "Italian", "woman"),
    v("it-IT-IsabellaNeural", "Isabella", "Italian", "woman"),
    v("it-IT-DiegoNeural", "Diego", "Italian", "man"),
    v("nl-NL-ColetteNeural", "Colette", "Dutch", "woman"),
    v("nl-NL-FennaNeural", "Fenna", "Dutch", "woman"),
    v("nl-NL-MaartenNeural", "Maarten", "Dutch", "man"),
    v(
        "pt-PT-RaquelNeural",
        "Raquel",
        "Portuguese",
        "Portugal, woman",
    ),
    v(
        "pt-PT-DuarteNeural",
        "Duarte",
        "Portuguese",
        "Portugal, man",
    ),
    v(
        "pt-BR-FranciscaNeural",
        "Francisca",
        "Portuguese",
        "Brazil, woman",
    ),
    v(
        "pt-BR-AntonioNeural",
        "Antonio",
        "Portuguese",
        "Brazil, man",
    ),
    v("af-ZA-AdriNeural", "Adri", "Afrikaans", "woman"),
    v("af-ZA-WillemNeural", "Willem", "Afrikaans", "man"),
];

/// Percent faster than normal. 10 is the default (PLAN.md §7 decision 20).
pub const RATES: &[i32] = &[0, 10, 15, 20, 30];

pub const ROOMS: &[(Room, &str)] = &[
    (Room::Pause, "Pause the film while it is spoken"),
    (Room::Over, "Speak it over the film"),
    (Room::GapsOnly, "Leave it out"),
];

pub const OUTPUTS: &[(Output, &str)] = &[
    (Output::Audio, "Described audio, an MP3 file"),
    (Output::Video, "Described video, an MP4 file"),
];

/// How sharp a described video from the web is (log entry 39, Q2).
pub const QUALITIES: &[(u32, &str)] = &[
    (480, "Small, 480 lines: the quickest download"),
    (720, "Medium, 720 lines"),
    (1080, "Sharp, 1080 lines: the biggest download"),
];

pub fn voice(id: &str) -> Option<&'static Voice> {
    VOICES.iter().find(|v| v.id == id)
}

/// "Sonia (British, woman)" for a voice Edge lists; the bare id otherwise,
/// for one typed into settings.txt by hand.
pub fn voice_label(id: &str) -> String {
    crate::voices::label(id)
}

pub fn rate_label(rate: i32) -> String {
    if rate == 0 {
        "normal speed".into()
    } else {
        format!("{rate}% faster than normal")
    }
}

/// What the menu says for a model, up to the colon: "GLM 5.3 Flash, from Z.ai".
pub fn model_label(cfg: &Config) -> String {
    let (list, current) = match cfg.service {
        Service::OpenRouter => (OPENROUTER_MODELS, &cfg.openrouter_model),
        Service::Claude => (CLAUDE_MODELS, &cfg.claude_model),
    };
    list.iter()
        .find(|m| m.0 == current)
        .map_or_else(|| current.clone(), |m| short(m.1).to_string())
}

/// US dollars for an hour of video, as measured in round 2; None when not
/// measured yet.
pub fn cost_per_hour(model: &str) -> Option<f64> {
    match model {
        "z-ai/glm-5.3-flash" => Some(0.08),
        "google/gemma-4-31b-it" => Some(0.20),
        // One run so far: the wedding copy, US$0.02 for 5 minutes (log entry 21).
        "google/gemini-3.5-flash-lite" => Some(0.25),
        _ => None,
    }
}

/// Models that are sent clips with their sound rather than still pictures:
/// Gemini watches and hears video (PLAN.md §4.2).
pub fn watches_video(model: &str) -> bool {
    model.starts_with("google/gemini")
}

/// The language as the model is asked to write it: the voice's own variety,
/// so a British voice reads British spelling.
pub fn writing_language(cfg: &Config) -> String {
    crate::voices::writing_language(&cfg.voice, &cfg.language)
}

/// The style line of the prompt (prompt version 3, VALUES).
pub fn style_line(style: &str) -> &'static str {
    match style {
        "short" => {
            "Brief: only the most important visible fact in each gap, in as few words as it takes."
        }
        "detailed" => {
            "Rich and specific: name the action, the object and who does it, with visible detail \
             about people and places. Fill each gap."
        }
        _ => "Vivid and specific: name the action, the object and who does it. Fill each gap.",
    }
}

fn short(text: &str) -> &str {
    text.split(':').next().unwrap_or(text)
}

fn room_label(room: Room) -> &'static str {
    match room {
        Room::Pause => "the film pauses while it is spoken",
        Room::Over => "it is spoken over the film",
        Room::GapsOnly => "it is left out",
    }
}

/// The voice chosen with a language: the first of the built-in list, else
/// the first Edge lists.
pub fn first_voice(language: &str) -> Option<String> {
    VOICES
        .iter()
        .find(|v| v.language == language)
        .map(|v| v.id.to_string())
        .or_else(|| {
            let all = crate::voices::all();
            crate::voices::of_language(&all, language)
                .first()
                .map(|v| v.id.clone())
        })
}

/// A voice, and its language with it.
pub fn choose_voice(cfg: &mut Config, id: &str) -> String {
    cfg.voice = id.to_string();
    if let Some(l) = crate::voices::language_of(id) {
        cfg.language = l;
    }
    format!("Voice: {}.", voice_label(id))
}

pub fn choose_rate(cfg: &mut Config, rate: i32) -> String {
    cfg.rate = rate;
    format!("Speed: {}.", rate_label(rate))
}

/// PLAN.md §5.10: pausing the film would put a copied picture out of step with
/// the sound, so choosing to pause also switches the output to audio.
pub fn choose_room(cfg: &mut Config, room: Room) -> String {
    cfg.room = room;
    let mut text = format!(
        "When a description does not fit its pause, {}.",
        room_label(room)
    );
    if room == Room::Pause && cfg.output == Output::Video {
        cfg.output = Output::Audio;
        text += " The output is now described audio, because a paused film cannot stay in step \
                 with its picture. Speaking it over the film or leaving it out both work with \
                 described video.";
    }
    text
}

pub fn choose_output(cfg: &mut Config, output: Output) -> String {
    cfg.output = output;
    match output {
        Output::Audio => "Output: described audio, an MP3 file.".into(),
        Output::Video => {
            let mut text = String::from("Output: described video, an MP4 file.");
            if cfg.room == Room::Pause {
                cfg.room = Room::Over;
                text += " A description that does not fit its pause is now spoken over the film, \
                         because pausing would put the picture out of step. Leaving it out \
                         works with video too.";
            }
            text
        }
    }
}

pub fn choose_quality(cfg: &mut Config, lines: u32) -> String {
    cfg.lines = lines;
    let mut text = format!(
        "A described video from the web now has at most {lines} lines of picture{}.",
        if lines == 480 {
            ", the quickest to download"
        } else {
            ""
        }
    );
    if cfg.output == Output::Audio {
        text += " This matters only when the output is described video.";
    }
    text
}

pub fn choose_service(cfg: &mut Config, service: Service) -> String {
    cfg.service = service;
    format!("Describing with {}.", service_label(cfg))
}

pub fn choose_model(cfg: &mut Config, model: &str) -> String {
    match cfg.service {
        Service::OpenRouter => cfg.openrouter_model = model.to_string(),
        Service::Claude => cfg.claude_model = model.to_string(),
    }
    format!("Describing with {}.", service_label(cfg))
}

pub fn choose_style(cfg: &mut Config, style: &str) -> String {
    cfg.style = style.to_string();
    let label = STYLES
        .iter()
        .find(|s| s.0 == style)
        .map_or(style, |s| short(s.1));
    format!("Description style: {label}.")
}

pub fn service_label(cfg: &Config) -> String {
    match cfg.service {
        Service::OpenRouter => format!("{}, through OpenRouter", model_label(cfg)),
        Service::Claude => format!("Claude {}, through Claude Code", model_label(cfg)),
    }
}

/// What is in the Video box, once it makes sense.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// A web address that yt-dlp downloads: YouTube, another video site, or a
    /// link straight to a video file (milestone 10).
    Web(String),
    File(PathBuf),
}

/// Whether `url` is a YouTube address.
pub fn is_youtube(url: &str) -> bool {
    let host = host(url);
    host == "youtube.com" || host == "youtu.be" || host.ends_with(".youtube.com")
}

/// The address's host, lower case, without "www." or "m.".
fn host(url: &str) -> String {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit('@').next().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default();
    host.trim_start_matches("www.")
        .trim_start_matches("m.")
        .to_string()
}

/// The site, as said to the user: "YouTube", or the address's host.
pub fn site(url: &str) -> String {
    if is_youtube(url) {
        "YouTube".into()
    } else {
        host(url)
    }
}

/// Whether `text` looks like a web address without its "https://", such
/// as "vimeo.com/123": a host with a dot, then a slash, and no spaces.
fn bare_address(text: &str) -> bool {
    let Some((host, _)) = text.split_once('/') else {
        return false;
    };
    host.contains('.')
        && !host.contains(['\\', ':', ' '])
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// The Video box's text as a source, or what is wrong with it and how to put it right.
pub fn parse_source(text: &str) -> Result<Source, String> {
    // Explorer's "Copy as path" puts quotes round it.
    let text = text.trim().trim_matches('"').trim();
    if text.is_empty() {
        return Err(
            "The Video box is empty. Type or paste a web address, such as a YouTube one, or \
                    press Alt+F to choose a video file."
                .into(),
        );
    }
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        if host(text).is_empty() {
            return Err(format!(
                "{text} is not a whole web address. Copy it again from the browser's address \
                 bar, with Alt+D then Ctrl+C."
            ));
        }
        return Ok(Source::Web(text.to_string()));
    }
    let path = Path::new(text);
    // "youtu.be/..." or "www.example.com/...", typed without "https://".
    if !path.exists() && (lower.starts_with("www.") || bare_address(text)) {
        if is_youtube(text) {
            return Ok(Source::Web(text.to_string()));
        }
        return Ok(Source::Web(format!("https://{text}")));
    }
    if path.is_file() {
        return Ok(Source::File(path.to_path_buf()));
    }
    if path.is_dir() {
        return Err(
            "That is a folder, not a video file. Press Alt+F to choose a file in it.".into(),
        );
    }
    Err(format!(
        "No file was found at {text}. Check the spelling, or press Alt+F to choose the file."
    ))
}

/// The summary shown before a job starts, and in the main window as the
/// choices change (PLAN.md §5.9). `source` is None when the Video box does not
/// make sense yet.
pub fn plan(cfg: &Config, source: Option<&Source>) -> String {
    let mut lines = Vec::new();
    match source {
        Some(Source::File(p)) => lines.push(format!(
            "Video: {}, a file on this computer.",
            p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned()
            )
        )),
        Some(Source::Web(url)) => lines.push(format!(
            "Video: from {}, downloaded to this computer first{}.",
            site(url),
            if cfg.output == Output::Video && cfg.lines > 480 {
                format!(", with pictures up to {} lines", cfg.lines)
            } else {
                String::new()
            }
        )),
        None => lines.push("Video: none chosen yet.".into()),
    }
    let clips = cfg.service == Service::OpenRouter && watches_video(&cfg.openrouter_model);
    lines.push(if clips {
        "Video Scribe finds the speech on this computer. Only the sound inside the pauses is \
         sent, as part of each clip, so the model can hear what happens there; no speech that \
         Video Scribe found is sent."
            .into()
    } else {
        "The sound stays on this computer: Video Scribe finds the speech itself, so no service \
         hears it."
            .into()
    });
    let to = match cfg.service {
        Service::OpenRouter => format!(
            "Sent to {}, through OpenRouter, using only providers that do not keep or train on \
             what they are sent:",
            model_label(cfg)
        ),
        Service::Claude => format!(
            "Sent to Claude {}, through Claude Code on this computer, using your Claude \
             subscription:",
            model_label(cfg)
        ),
    };
    let mut what = String::from(if clips {
        " a short clip, picture and sound, of each pause long enough for a description"
    } else {
        " three still pictures from each pause long enough for a description"
    });
    if matches!(source, Some(Source::Web(_))) {
        what += ", and the captions around each pause, when the video has them";
    } else if cfg.faces && matches!(source, Some(Source::File(_))) {
        // Family video: the faces are tagged on this computer (log entry 44).
        what += ", each with a tag such as \"Person 3\" for each face in it, found on this \
                 computer; never a name";
    }
    what += ".";
    let names = if cfg.send_names {
        match source {
            Some(Source::Web(_)) => " The video's title is sent too.".to_string(),
            _ => " The file name is sent too.".to_string(),
        }
    } else {
        let sound = if clips {
            "the rest of the sound"
        } else {
            "the sound"
        };
        match source {
            Some(Source::Web(_)) => {
                format!(" Nothing else: not {sound}, the address or the title.")
            }
            _ => format!(" Nothing else: not {sound}, and not the file or folder name."),
        }
    };
    lines.push(format!("{to}{what}{names}"));
    let style = STYLES
        .iter()
        .find(|s| s.0 == cfg.style)
        .map_or(cfg.style.as_str(), |s| short(s.1));
    lines.push(format!(
        "Descriptions: {} style, in {}, spoken by {}, at {}.",
        style.to_lowercase(),
        cfg.language,
        voice_label(&cfg.voice),
        rate_label(cfg.rate)
    ));
    lines.push(format!(
        "When a description does not fit its pause, {}.",
        room_label(cfg.room)
    ));
    let output = match cfg.output {
        Output::Audio => "described audio, an MP3 file",
        Output::Video => "described video, an MP4 file",
    };
    lines.push(format!(
        "Result: {output}, written as a new file in a folder of its own in {}. The original is never changed.",
        cfg.output_folder().display()
    ));
    lines.join("\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_voice_and_model_are_in_the_lists() {
        let cfg = Config::default();
        assert!(voice(&cfg.voice).is_some());
        assert_eq!(OPENROUTER_MODELS[0].0, cfg.openrouter_model);
        assert!(CLAUDE_MODELS.iter().any(|m| m.0 == cfg.claude_model));
        assert!(RATES.contains(&cfg.rate));
    }

    #[test]
    fn a_language_starts_with_its_first_built_in_voice() {
        assert_eq!(first_voice("French").as_deref(), Some("fr-FR-DeniseNeural"));
        assert_eq!(
            first_voice("Afrikaans").as_deref(),
            Some("af-ZA-AdriNeural")
        );
        assert_eq!(first_voice("English").as_deref(), Some("en-GB-SoniaNeural"));
    }

    #[test]
    fn a_voice_brings_its_language() {
        let mut cfg = Config::default();
        let said = choose_voice(&mut cfg, "fr-FR-HenriNeural");
        assert_eq!(cfg.language, "French");
        assert!(said.contains("Henri"), "{said}");
    }

    #[test]
    fn pausing_switches_video_to_audio_and_says_so() {
        let mut cfg = Config::default();
        cfg.room = Room::Over;
        cfg.output = Output::Video;
        let said = choose_room(&mut cfg, Room::Pause);
        assert_eq!(cfg.output, Output::Audio);
        assert!(said.contains("now described audio"), "{said}");
    }

    #[test]
    fn video_stops_the_pausing_and_says_so() {
        let mut cfg = Config {
            room: Room::Pause,
            ..Config::default()
        };
        let said = choose_output(&mut cfg, Output::Video);
        assert_eq!(cfg.room, Room::Over);
        assert!(said.contains("now spoken over the film"), "{said}");
        // With no pausing chosen there is nothing to change or say.
        let said = choose_output(&mut cfg, Output::Video);
        assert_eq!(said, "Output: described video, an MP4 file.");
    }

    #[test]
    fn web_addresses_are_recognised() {
        for a in [
            "https://www.youtube.com/watch?v=_cMxraX_5RE",
            "youtu.be/_cMxraX_5RE",
            "https://m.youtube.com/watch?v=x",
            "www.youtube.com/shorts/x",
        ] {
            assert_eq!(parse_source(a), Ok(Source::Web(a.into())), "{a}");
            assert!(is_youtube(a), "{a}");
            assert_eq!(site(a), "YouTube");
        }
        for (a, url, at) in [
            ("https://vimeo.com/1", "https://vimeo.com/1", "vimeo.com"),
            (
                "archive.org/details/BigBuckBunny_124",
                "https://archive.org/details/BigBuckBunny_124",
                "archive.org",
            ),
            (
                "www.ted.com/talks/x",
                "https://www.ted.com/talks/x",
                "ted.com",
            ),
            (
                "HTTPS://Example.com:8080/films/a.mp4?x=1",
                "HTTPS://Example.com:8080/films/a.mp4?x=1",
                "example.com",
            ),
        ] {
            assert_eq!(parse_source(a), Ok(Source::Web(url.into())), "{a}");
            assert!(!is_youtube(a), "{a}");
            assert_eq!(site(a), at);
        }
        // Not mistaken for a web address.
        assert!(!is_youtube("https://notyoutube.com/watch?v=x"));
        assert!(parse_source(r"Films\holiday.mp4").is_err());
        assert!(parse_source("https://")
            .unwrap_err()
            .contains("not a whole"));
        assert!(parse_source("   ").unwrap_err().contains("empty"));
        assert!(parse_source(r"C:\no such\file.mp4")
            .unwrap_err()
            .contains("No file"));
    }

    #[test]
    fn the_plan_never_names_the_file_unless_asked() {
        let cfg = Config::default();
        let src = Source::File(PathBuf::from(r"C:\Family\Sophie 3rd birthday.mp4"));
        let text = plan(&cfg, Some(&src));
        assert!(
            text.contains("not the sound, and not the file or folder name"),
            "{text}"
        );
        // It is shown to the user, though: that is how they know which video.
        assert!(text.contains("Sophie 3rd birthday.mp4"));
        assert!(!text.contains(r"C:\Family"));
        let mut cfg = cfg;
        cfg.send_names = true;
        assert!(plan(&cfg, Some(&src)).contains("The file name is sent too."));
    }

    #[test]
    fn gemini_is_sent_clips_and_the_plan_says_so() {
        let mut cfg = Config::default();
        assert!(!plan(&cfg, None).contains("clip"));
        cfg.openrouter_model = "google/gemini-3.5-flash-lite".into();
        let text = plan(&cfg, None);
        assert!(
            text.contains("Only the sound inside the pauses is sent"),
            "{text}"
        );
        assert!(text.contains("a short clip, picture and sound"), "{text}");
        assert!(text.contains("not the rest of the sound"), "{text}");
    }

    #[test]
    fn the_plan_mentions_captions_only_for_youtube() {
        let cfg = Config::default();
        let yt = Source::Web("https://youtu.be/x".into());
        assert!(plan(&cfg, Some(&yt)).contains("captions"));
        assert!(!plan(&cfg, None).contains("captions"));
    }
}
