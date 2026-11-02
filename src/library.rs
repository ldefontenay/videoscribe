//! The described videos: one folder for each video in the output folder,
//! named after the video's title, cleaned up, holding every file made for
//! it (log entry 40, Q2, after NoteFetch's way of keeping recordings).
//!
//! A folder is Video Scribe's when it holds `Video Scribe.json`, which names
//! the video it belongs to. The folder is found by that, never by its name,
//! so a folder renamed in File Explorer is still found. Results from before
//! folders, loose in the output folder, are left where they are.
//!
//! `clean` and `shorten` are NoteFetch's (`naming.rs`, MIT, same author).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MARKER: &str = "Video Scribe.json";
const UNTITLED: &str = "Untitled";
/// Longest name made from a title: room is left for " - described 2.mp4"
/// and for the output folder itself, under Windows' usual 260.
const LONGEST: usize = 80;
/// Characters Windows forbids in a name.
const NOT_IN_A_NAME: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Make text safe as a Windows file name. Characters Windows forbids are
/// replaced with readable ones, runs of spaces are collapsed, trailing dots
/// and spaces are removed, and a reserved device name gets a leading
/// underscore. Empty text becomes "Untitled".
pub fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let replacement = match c {
            ':' => Some('.'),
            '/' | '\\' | '|' => Some('-'),
            '"' => Some('\''),
            '<' | '>' | '?' | '*' => None,
            c if c.is_control() => Some(' '),
            c => Some(c),
        };
        if let Some(r) = replacement {
            if r.is_whitespace() {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
            } else {
                out.push(r);
            }
        }
    }
    let trimmed = out.trim_matches(' ').trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        return UNTITLED.to_string();
    }
    if is_reserved(trimmed) {
        return format!("_{trimmed}");
    }
    trimmed.to_string()
}

fn is_reserved(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.chars().count() == 4
            && stem.chars().nth(3).is_some_and(|c| c.is_ascii_digit()))
}

/// Cut text to at most `max` characters, at a word boundary where one is
/// near, without leaving trailing dots or spaces.
pub fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    let at_word = match cut.rfind(' ') {
        Some(i) if cut[..i].chars().count() * 3 >= max * 2 => &cut[..i],
        _ => &cut[..],
    };
    let tidy = at_word.trim_end_matches(['.', ' ', ',', '-']);
    if tidy.is_empty() {
        cut
    } else {
        tidy.to_string()
    }
}

/// The name a title gives a folder and its files.
pub fn name_for(title: &str) -> String {
    shorten(&clean(title), LONGEST)
}

/// Why a name typed for a video cannot be used, in words that say how to
/// write one that can; None when it is fine.
pub fn bad_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("Type a name for the video, such as Sprite Fright.".into());
    }
    if name.chars().count() > LONGEST {
        return Some(format!("The name is too long: {LONGEST} letters at most."));
    }
    if let Some(c) = name
        .chars()
        .find(|c| NOT_IN_A_NAME.contains(c) || c.is_control())
    {
        return Some(format!(
            "A video's name cannot hold the character {c}, because it becomes a folder name. \
             Letters, numbers, spaces and most punctuation are fine."
        ));
    }
    if name.ends_with('.') {
        return Some("A video's name cannot end with a full stop.".into());
    }
    if is_reserved(name) {
        return Some(format!(
            "Windows keeps the name {name} for itself. Add a word to it."
        ));
    }
    None
}

/// One described video's folder.
#[derive(Clone, Debug)]
pub struct Entry {
    pub folder: PathBuf,
    /// What the folder is called, which is also what its files start with.
    pub name: String,
    /// The key its descriptions, people and results go by (`people::key`).
    pub key: String,
    /// The web address, or the video file's path, it was made from.
    pub source: String,
    /// When something in it was last made.
    pub made: SystemTime,
    pub sound: bool,
    pub video: bool,
}

impl Entry {
    /// What the folder holds, in words.
    pub fn holds(&self) -> &'static str {
        match (self.sound, self.video) {
            (true, true) => "described sound and video",
            (true, false) => "described sound",
            (false, true) => "described video",
            (false, false) => "not described yet",
        }
    }
}

fn read_marker(folder: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(folder.join(MARKER)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some((
        v["key"].as_str()?.to_string(),
        v["source"].as_str().unwrap_or_default().to_string(),
    ))
}

/// The folders in `output` that are Video Scribe's.
fn folders(output: &Path) -> Vec<(PathBuf, String, String)> {
    let Ok(list) = std::fs::read_dir(output) else {
        return Vec::new();
    };
    list.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let (key, source) = read_marker(&e.path())?;
            Some((e.path(), key, source))
        })
        .collect()
}

/// The folder in `output` made for the video `key`, whatever it is called now.
pub fn find(output: &Path, key: &str) -> Option<PathBuf> {
    folders(output)
        .into_iter()
        .find(|(_, k, _)| k == key)
        .map(|(f, _, _)| f)
}

/// Where a file made for the video `key` is now, if its folder was renamed,
/// in Video Scribe or in File Explorer, since `path` was recorded.
pub fn relocate(path: &Path, key: &str) -> PathBuf {
    if path.exists() {
        return path.to_path_buf();
    }
    let found = (|| {
        let old = path.parent()?;
        let new = find(old.parent()?, key)?;
        [
            moved(path, old, &new),
            path.file_name().map(|n| new.join(n)),
        ]
        .into_iter()
        .flatten()
        .find(|p| p.exists())
        .or_else(|| same_kind(path, old, &new))
    })();
    found.unwrap_or_else(|| path.to_path_buf())
}

/// The one file in `new` that is what `path` was in `old`: the same ending
/// after the name, such as " - described 2.mp3", whatever the name is now.
/// So a folder renamed in Video Scribe, then again in File Explorer, is
/// still followed (test round 2 of milestone 10, 3.8).
fn same_kind(path: &Path, old: &Path, new: &Path) -> Option<PathBuf> {
    let base = old.file_name()?.to_string_lossy().into_owned();
    let name = path.file_name()?.to_string_lossy().into_owned();
    // The name the file had: the folder's first name, or another before it.
    let ending = match name.strip_prefix(&base) {
        Some(rest) if rest.starts_with(" - ") => rest.to_string(),
        _ => name[name.find(" - ")?..].to_string(),
    };
    let mut found = std::fs::read_dir(new)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(&ending))
        });
    let one = found.next()?;
    found.next().is_none().then_some(one)
}

/// The folder for the video `key`: the one already made for it, wherever it
/// was renamed to, else a new one named after `title`, with a number when
/// another video has that name.
pub fn folder_for(output: &Path, key: &str, title: &str, source: &str) -> Result<PathBuf, String> {
    if let Some(f) = find(output, key) {
        return Ok(f);
    }
    let name = name_for(title);
    let folder = (1..)
        .map(|n| {
            if n == 1 {
                output.join(&name)
            } else {
                output.join(format!("{name} {n}"))
            }
        })
        .find(|f| !f.exists())
        .expect("a free name");
    std::fs::create_dir_all(&folder).map_err(|e| {
        format!(
            "The folder {} could not be made: {e}. Choose another output folder in the File menu.",
            folder.display()
        )
    })?;
    let v = serde_json::json!({
        "what": "Video Scribe keeps this file to know which video this folder is for. \
                 Rename the folder as you like, but keep this file in it.",
        "key": key,
        "title": title,
        "source": source,
    });
    std::fs::write(folder.join(MARKER), v.to_string())
        .map_err(|e| format!("{}: {e}", folder.display()))?;
    Ok(folder)
}

/// Every described video in `output`, most recently made first.
pub fn list(output: &Path) -> Vec<Entry> {
    let mut out: Vec<Entry> = folders(output)
        .into_iter()
        .map(|(folder, key, source)| {
            let mut made = std::fs::metadata(&folder)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let (mut sound, mut video) = (false, false);
            for e in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                let n = e.file_name().to_string_lossy().to_lowercase();
                if n.contains(" - described") {
                    sound |= n.ends_with(".mp3");
                    video |= n.ends_with(".mp4");
                }
                if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
                    made = made.max(t);
                }
            }
            Entry {
                name: folder
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                folder,
                key,
                source,
                made,
                sound,
                video,
            }
        })
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.made));
    out
}

/// Renames a video's folder, and every file in it named after the folder,
/// to `new`. Either everything is renamed or everything is put back.
/// Returns the new folder, and how many files in it were renamed.
pub fn rename(folder: &Path, new: &str) -> Result<(PathBuf, usize), String> {
    let new = new.trim();
    if let Some(why) = bad_name(new) {
        return Err(why);
    }
    let old = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or("That folder has no name.")?;
    if old == new {
        return Ok((folder.to_path_buf(), 0));
    }
    let parent = folder.parent().ok_or("That folder has no parent.")?;
    let target = parent.join(new);
    // A change of capitals alone is the same folder to Windows.
    if target.exists() && !old.eq_ignore_ascii_case(new) {
        return Err(format!(
            "There is already something called {new} in the output folder. Choose another name."
        ));
    }
    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
    let undo = |done: &[(PathBuf, PathBuf)]| {
        for (from, to) in done.iter().rev() {
            let _ = std::fs::rename(to, from);
        }
    };
    let entries: Vec<_> = std::fs::read_dir(folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?
        .flatten()
        .collect();
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        // The marker keeps its name, even in a folder called "Video Scribe".
        if name == MARKER {
            continue;
        }
        let Some(rest) = name.strip_prefix(&old) else {
            continue;
        };
        // "Old - described.mp3" and "Old.txt", not "Older - described.mp3".
        if !(rest.starts_with(" - ") || rest.starts_with('.')) {
            continue;
        }
        let to = folder.join(format!("{new}{rest}"));
        if let Err(err) = std::fs::rename(e.path(), &to) {
            undo(&done);
            return Err(in_use(&name, &err));
        }
        done.push((e.path(), to));
    }
    if let Err(err) = std::fs::rename(folder, &target) {
        undo(&done);
        return Err(in_use(&old, &err));
    }
    // A described list names its sound and video files at the top.
    for (_, to) in &done {
        let Some(name) = to.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if name.contains(" - described") && name.ends_with(".txt") {
            let file = target.join(&name);
            if let Ok(text) = std::fs::read_to_string(&file) {
                let from = format!("{old} - described");
                if text.contains(&from) {
                    let _ =
                        std::fs::write(&file, text.replace(&from, &format!("{new} - described")));
                }
            }
        }
    }
    Ok((target, done.len()))
}

fn in_use(name: &str, err: &std::io::Error) -> String {
    // ERROR_SHARING_VIOLATION and ERROR_ACCESS_DENIED: open elsewhere.
    if matches!(err.raw_os_error(), Some(32) | Some(5)) {
        format!(
            "{name} is open in another program, such as a media player, so Windows would not \
             rename it. Close that program and try again. Nothing was renamed."
        )
    } else {
        format!("{name} could not be renamed: {err}. Nothing was renamed.")
    }
}

/// `path` moved from `from` (a video's old folder) to `to`, with its name's
/// start changed from the old folder's name to the new one, as `rename`
/// does; None when `path` was not in `from`.
pub fn moved(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    if path.parent()? != from {
        return None;
    }
    let old = from.file_name()?.to_string_lossy().into_owned();
    let new = to.file_name()?.to_string_lossy().into_owned();
    let name = path.file_name()?.to_string_lossy().into_owned();
    let renamed = match name.strip_prefix(&old) {
        Some(rest) if rest.starts_with(" - ") || rest.starts_with('.') => format!("{new}{rest}"),
        _ => name,
    };
    Some(to.join(renamed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_become_safe_names() {
        assert_eq!(
            clean("Sprite Fright - Blender Open Movie"),
            "Sprite Fright - Blender Open Movie"
        );
        assert_eq!(
            clean("What? A film: part 1/2 \"live\"..."),
            "What A film. part 1-2 'live'"
        );
        assert_eq!(clean("  \t "), "Untitled");
        assert_eq!(clean("con"), "_con");
        let long = "word ".repeat(40);
        assert!(name_for(&long).chars().count() <= LONGEST);
        assert!(!name_for(&long).ends_with(' '));
    }

    #[test]
    fn bad_names_say_why() {
        assert!(bad_name("Films: French").unwrap().contains(':'));
        assert!(bad_name("  ").is_some());
        assert!(bad_name("The end.").is_some());
        assert!(bad_name("Kate and Sam's wedding").is_none());
    }

    #[test]
    fn a_folder_is_found_again_after_it_is_renamed_and_renames_its_files() {
        let out = std::env::temp_dir().join(format!("vs-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        let f = folder_for(
            &out,
            "_cMxraX_5RE",
            "Sprite Fright | Blender",
            "https://youtu.be/_cMxraX_5RE",
        )
        .unwrap();
        assert_eq!(f, out.join("Sprite Fright - Blender"));
        for n in [
            "Sprite Fright - Blender - described.mp3",
            "Sprite Fright - Blender - described.txt",
            "notes.txt",
        ] {
            std::fs::write(f.join(n), "x").unwrap();
        }
        // Another video with the same title gets a number.
        let other = folder_for(&out, "other", "Sprite Fright | Blender", "x").unwrap();
        assert_eq!(other, out.join("Sprite Fright - Blender 2"));
        std::fs::write(
            f.join("Sprite Fright - Blender - described.txt"),
            "\u{feff}Sprite Fright - Blender - described.mp3 is the sound",
        )
        .unwrap();
        let (to, files) = rename(&f, "Sprite Fright").unwrap();
        assert_eq!(files, 2);
        assert_eq!(
            std::fs::read_to_string(to.join("Sprite Fright - described.txt")).unwrap(),
            "\u{feff}Sprite Fright - described.mp3 is the sound"
        );
        assert_eq!(to, out.join("Sprite Fright"));
        assert!(to.join("Sprite Fright - described.mp3").is_file());
        assert!(to.join("notes.txt").is_file());
        assert_eq!(folder_for(&out, "_cMxraX_5RE", "ignored", "").unwrap(), to);
        assert_eq!(
            moved(
                &f.join("Sprite Fright - Blender - described 2.mp4"),
                &f,
                &to
            ),
            Some(to.join("Sprite Fright - described 2.mp4"))
        );
        assert_eq!(
            relocate(
                &f.join("Sprite Fright - Blender - described.mp3"),
                "_cMxraX_5RE"
            ),
            to.join("Sprite Fright - described.mp3")
        );
        // Renamed again, by hand: only the folder's name changes.
        let by_hand = out.join("Sprite by hand");
        std::fs::rename(&to, &by_hand).unwrap();
        assert_eq!(
            relocate(
                &f.join("Sprite Fright - Blender - described.mp3"),
                "_cMxraX_5RE"
            ),
            by_hand.join("Sprite Fright - described.mp3")
        );
        std::fs::rename(&by_hand, &to).unwrap();
        assert!(rename(&to, "Sprite Fright - Blender 2")
            .unwrap_err()
            .contains("already"));
        let all = list(&out);
        assert_eq!(all.len(), 2);
        let sprite = all.iter().find(|e| e.key == "_cMxraX_5RE").unwrap();
        assert_eq!(sprite.holds(), "described sound");
        let _ = std::fs::remove_dir_all(&out);
    }
}
