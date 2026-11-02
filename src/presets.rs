//! Presets (PLAN.md §5.12): a named set of the choices in the Options menu,
//! so that "French family videos" is one choice rather than seven. One plain
//! text file each in `%APPDATA%\Video Scribe\Presets`, in the same
//! `key = value` form as settings.txt, safe to edit in Notepad.
//!
//! A preset is a starting point, not a lock: choosing one sets the choices
//! it holds and leaves the rest alone, and changing a choice afterwards is
//! said ("No longer the ... preset").

use std::path::PathBuf;

use crate::config::Config;

/// Lines of settings.txt that belong to this computer, not to a preset.
/// Sending the file name stays a choice made on its own, never one a preset
/// makes in passing (CLAUDE.md constraint 7).
const NOT_IN_A_PRESET: &[&str] = &[
    "output folder",
    "keep video downloads",
    "send the file name",
    "preset",
];

/// Characters Windows does not allow in a file name.
const NOT_IN_A_NAME: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

pub fn dir() -> Option<PathBuf> {
    Config::dir().map(|d| d.join("Presets"))
}

/// The presets, by name, in alphabetical order.
pub fn list() -> Vec<String> {
    let Some(dir) = dir() else { return Vec::new() };
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()? != "txt" {
                return None;
            }
            Some(p.file_stem()?.to_string_lossy().into_owned())
        })
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// Why `name` cannot be a preset's name, if it cannot.
pub fn bad_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("Type a name for the preset, such as French family videos.".into());
    }
    if name.chars().count() > 60 {
        return Some("The name is too long: 60 letters at most.".into());
    }
    if let Some(c) = name
        .chars()
        .find(|c| NOT_IN_A_NAME.contains(c) || c.is_control())
    {
        return Some(format!(
            "A preset's name cannot hold the character {c}, because it becomes a file name. \
             Letters, numbers, spaces and most punctuation are fine."
        ));
    }
    if name.ends_with('.') {
        return Some("A preset's name cannot end with a full stop.".into());
    }
    None
}

fn path(name: &str) -> Option<PathBuf> {
    dir().map(|d| d.join(format!("{}.txt", name.trim())))
}

pub fn exists(name: &str) -> bool {
    path(name).is_some_and(|p| p.is_file())
}

/// The preset's lines for `cfg`: its settings, less this computer's own.
fn text(cfg: &Config) -> String {
    let mut out = vec![
        "# A Video Scribe preset: the choices it sets when chosen in Options, Presets.".to_string(),
        "# Safe to edit while Video Scribe is closed. A line taken out is left as it is."
            .to_string(),
    ];
    out.extend(
        cfg.to_text()
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter(|l| !NOT_IN_A_PRESET.iter().any(|k| l.starts_with(k)))
            .map(str::to_string),
    );
    out.join("\r\n") + "\r\n"
}

/// Saves `cfg`'s choices as the preset `name`, written beside and renamed.
pub fn save(name: &str, cfg: &Config) -> Result<(), String> {
    let path = path(name).ok_or("Windows did not say where settings go.")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let part = path.with_extension("txt.part");
    std::fs::write(&part, text(cfg))
        .and_then(|_| std::fs::rename(&part, &path))
        .map_err(|e| format!("The preset could not be saved in {}: {e}", path.display()))
}

/// `cfg` with the preset `text` laid over it: what the preset says wins,
/// and what it leaves out stays as it was.
pub fn over(cfg: &Config, text: &str) -> Config {
    let mut next = Config::parse(&format!(
        "{}\n{}",
        cfg.to_text(),
        text.trim_start_matches('\u{feff}')
    ));
    next.preset = cfg.preset.clone();
    next
}

/// `cfg` with the preset `name` chosen.
pub fn choose(name: &str, cfg: &Config) -> Result<Config, String> {
    let path = path(name).ok_or("Windows did not say where settings go.")?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("The preset {name} could not be read: {e}"))?;
    let mut next = over(cfg, &text);
    next.preset = name.trim().to_string();
    Ok(next)
}

/// What choosing a preset changed, in the Options menu's words.
pub fn changed(before: &Config, after: &Config) -> Vec<&'static str> {
    let mut out = Vec::new();
    if before.service != after.service {
        out.push("the model provider");
    }
    if before.openrouter_model != after.openrouter_model
        || before.claude_model != after.claude_model
    {
        out.push("the model");
    }
    if before.style != after.style {
        out.push("the style");
    }
    if before.language != after.language {
        out.push("the language");
    }
    if before.voice != after.voice {
        out.push("the voice");
    }
    if before.rate != after.rate {
        out.push("the speed");
    }
    if before.room != after.room || before.short_too != after.short_too {
        out.push("what happens when a description does not fit");
    }
    if before.output != after.output {
        out.push("the output");
    }
    if before.together != after.together {
        out.push("describing four requests at once");
    }
    if before.lines != after.lines {
        out.push("the picture quality");
    }
    out
}

/// Whether `cfg` still holds every choice of the preset `name`.
pub fn still(name: &str, cfg: &Config) -> bool {
    let Some(text) = path(name).and_then(|p| std::fs::read_to_string(p).ok()) else {
        return false;
    };
    over(cfg, &text).to_text() == cfg.to_text()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Room;

    #[test]
    fn a_preset_sets_what_it_holds_and_nothing_else() {
        let mut french = Config::default();
        french.language = "French".into();
        french.voice = "fr-FR-DeniseNeural".into();
        french.room = Room::Over;
        let text = text(&french);
        assert!(!text.contains("output folder"));
        assert!(!text.contains("keep video downloads"));
        assert!(!text.contains("send the file name"));
        let mut mine = Config::default();
        mine.output_folder = Some(PathBuf::from(r"C:\Films"));
        mine.keep_downloads = true;
        let next = over(&mine, &text);
        assert_eq!(next.language, "French");
        assert_eq!(next.room, Room::Over);
        assert_eq!(next.output_folder, Some(PathBuf::from(r"C:\Films")));
        assert!(next.keep_downloads);
        // A line taken out by hand is left as it was.
        let short = "language = German\r\n";
        let next = over(&mine, short);
        assert_eq!(next.language, "German");
        assert_eq!(next.voice, mine.voice);
    }

    #[test]
    fn names_that_cannot_be_file_names_are_refused() {
        assert!(bad_name("French family videos").is_none());
        assert!(bad_name("  ").is_some());
        assert!(bad_name("Films: French").unwrap().contains(':'));
        assert!(bad_name("Mum's.").is_some());
        assert!(bad_name(&"x".repeat(61)).is_some());
    }
}
