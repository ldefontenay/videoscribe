//! The people Video Scribe knows by their faces, across videos (PLAN.md
//! §5.7, log entry 44). Two files in the settings folder, as Photo Scribe
//! keeps them:
//!
//! - `people.txt`: each person's name and notes, in Photo Scribe's exact
//!   format, readable and correctable in Notepad while Video Scribe is closed.
//! - `faces.bin`: fingerprints of faces the user named, each with the video
//!   and the tag it came from, so naming someone again replaces them. The
//!   fingerprints are biometric data: this file never leaves the computer.
//!
//! Photo Scribe's own two files are read too, never written (log entry 42,
//! Q1): only the faces the user named there.
//!
//! Names never go into a request: a recognised face's name is put in on this
//! computer, in place of the label the describer gave that person.

use std::path::PathBuf;

use crate::faces::{similarity, EMBEDDING};

/// A face at least this similar to one the user named is taken to be that
/// person (Photo Scribe's figure)...
const RECOGNISE: f32 = 0.42;
/// ...and it must be clearly closer to that person than to anyone else.
const MARGIN: f32 = 0.05;
/// At most this many named faces per person are compared, to keep it quick.
const EXEMPLARS: usize = 200;

#[derive(Clone, Debug, PartialEq)]
pub struct Person {
    pub id: u32,
    pub name: String,
    pub notes: String,
}

/// One named face: who, and the video and tag it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub person: u32,
    pub video: String,
    pub tag: usize,
    pub embedding: Vec<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Known {
    pub persons: Vec<Person>,
    pub faces: Vec<Face>,
}

/// `%APPDATA%\Video Scribe`, or the folder in VIDEOSCRIBE_PEOPLE_DIR, so
/// that testing never touches the user's own files (CLAUDE.md, rule 8).
fn dir() -> Option<PathBuf> {
    match std::env::var_os("VIDEOSCRIBE_PEOPLE_DIR") {
        Some(d) => Some(PathBuf::from(d)),
        None => crate::config::Config::dir(),
    }
}

/// Photo Scribe's folder, or the one in VIDEOSCRIBE_PHOTOSCRIBE_DIR.
fn photo_scribe_dir() -> Option<PathBuf> {
    match std::env::var_os("VIDEOSCRIBE_PHOTOSCRIBE_DIR") {
        Some(d) => Some(PathBuf::from(d)),
        None => crate::faces::photo_scribe_dir(),
    }
}

impl Known {
    pub fn load() -> Known {
        let Some(dir) = dir() else {
            return Known::default();
        };
        let mut k = Known {
            persons: std::fs::read_to_string(dir.join("people.txt"))
                .map(|t| parse_people(&t))
                .unwrap_or_default(),
            faces: std::fs::read(dir.join("faces.bin"))
                .ok()
                .and_then(|b| decode(&b))
                .unwrap_or_default(),
        };
        // A face whose person was taken out in Notepad is forgotten.
        let ids: Vec<u32> = k.persons.iter().map(|p| p.id).collect();
        k.faces.retain(|f| ids.contains(&f.person));
        k
    }

    /// Both files, each written beside and moved into place.
    pub fn save(&self) -> Result<(), String> {
        let dir = dir().ok_or("The settings folder could not be found.")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let write = |name: &str, data: &[u8]| -> Result<(), String> {
            let tmp = dir.join(format!("{name}.tmp"));
            std::fs::write(&tmp, data)
                .and_then(|_| std::fs::rename(&tmp, dir.join(name)))
                .map_err(|e| format!("{name} could not be saved: {e}"))
        };
        write("people.txt", people_text(&self.persons).as_bytes())?;
        write("faces.bin", &encode(&self.faces))
    }

    /// The person called `name`, added if new. Names match whatever their case.
    fn person(&mut self, name: &str) -> u32 {
        if let Some(p) = self
            .persons
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
        {
            return p.id;
        }
        let id = self.persons.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        self.persons.push(Person {
            id,
            name: name.to_string(),
            notes: String::new(),
        });
        id
    }

    /// The user named tag `tag` of video `video` (an empty name takes the
    /// name away): its faces are kept under that name, in place of whatever
    /// that tag was named before. True if anything changed.
    pub fn learn(&mut self, video: &str, tag: usize, name: &str, faces: &[Vec<f32>]) -> bool {
        let before = self.faces.len();
        self.faces.retain(|f| !(f.video == video && f.tag == tag));
        let name = name.trim();
        if name.is_empty() || faces.is_empty() {
            return self.faces.len() != before;
        }
        let person = self.person(name);
        for e in faces {
            self.faces.push(Face {
                person,
                video: video.to_string(),
                tag,
                embedding: e.clone(),
            });
        }
        true
    }

    /// The name of the person each of `people` is (each a tag's
    /// fingerprints), when it is clear, from the faces named here and in
    /// `others` (Photo Scribe's, from `photo_scribe`). Video `video`'s own
    /// faces are left out, so a tag is never recognised from itself.
    pub fn recognise(
        &self,
        video: &str,
        people: &[Vec<Vec<f32>>],
        others: &[(String, Vec<f32>)],
    ) -> Vec<Option<String>> {
        let mut named: Vec<(String, Vec<&[f32]>)> = Vec::new();
        for f in self.faces.iter().filter(|f| f.video != video) {
            let Some(p) = self.persons.iter().find(|p| p.id == f.person) else {
                continue;
            };
            exemplar(&mut named, &p.name, &f.embedding);
        }
        for (name, e) in others {
            exemplar(&mut named, name, e);
        }
        people.iter().map(|faces| who(&named, faces)).collect()
    }
}

fn exemplar<'a>(named: &mut Vec<(String, Vec<&'a [f32]>)>, name: &str, e: &'a [f32]) {
    match named.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        Some((_, list)) if list.len() < EXEMPLARS => list.push(e),
        Some(_) => {}
        None => named.push((name.to_string(), vec![e])),
    }
}

/// Who a tag's faces are: each face votes for the named person it is
/// clearly most like, and a name needs the votes of at least half the faces
/// and more than any other name has.
fn who(named: &[(String, Vec<&[f32]>)], faces: &[Vec<f32>]) -> Option<String> {
    if faces.is_empty() {
        return None;
    }
    let mut votes = vec![0usize; named.len()];
    for f in faces {
        let mut best: Vec<(usize, f32)> = named
            .iter()
            .enumerate()
            .map(|(i, (_, list))| {
                let s = list
                    .iter()
                    .map(|e| similarity(f, e))
                    .fold(f32::MIN, f32::max);
                (i, s)
            })
            .collect();
        best.sort_by(|a, b| b.1.total_cmp(&a.1));
        let &(i, top) = best.first()?;
        let second = best.get(1).map_or(f32::MIN, |b| b.1);
        if top >= RECOGNISE && top - second >= MARGIN {
            votes[i] += 1;
        }
    }
    let (i, &most) = votes.iter().enumerate().max_by_key(|(_, v)| **v)?;
    let rivals = votes.iter().filter(|&&v| v == most).count();
    (most * 2 >= faces.len() && rivals == 1).then(|| named[i].0.clone())
}

/// The faces the user named in Photo Scribe, each with the name, read and
/// never written. Empty when Photo Scribe has no such files.
pub fn photo_scribe() -> Vec<(String, Vec<f32>)> {
    let Some(dir) = photo_scribe_dir() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(dir.join("people.txt")) else {
        return Vec::new();
    };
    let persons = parse_people(&text);
    let Some(faces) = std::fs::read(dir.join("faces.bin"))
        .ok()
        .and_then(|b| decode_photo_scribe(&b))
    else {
        return Vec::new();
    };
    faces
        .into_iter()
        .filter_map(|(person, e)| {
            let p = persons.iter().find(|p| p.id == person)?;
            Some((p.name.clone(), e))
        })
        .collect()
}

// ------------------------------------------------------------ people.txt

const PEOPLE_INTRO: &str = "# The people Video Scribe knows by their faces\n\n\
Names and notes, one heading per person. You can correct them in Notepad while Video Scribe is closed.\n\
Keep each heading's number: it is how Video Scribe knows which faces belong to whom.\n\
Nothing in this file is sent anywhere.\n";

fn people_text(persons: &[Person]) -> String {
    let mut s = PEOPLE_INTRO.to_string();
    for p in persons {
        s += &format!("\n## {}: {}\n", p.id, p.name);
        if !p.notes.is_empty() {
            s += &format!("\n{}\n", p.notes.trim());
        }
    }
    s.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Photo Scribe's format: "## 3: Name", then notes, for each person.
fn parse_people(text: &str) -> Vec<Person> {
    let mut out: Vec<Person> = Vec::new();
    let mut notes: Vec<&str> = Vec::new();
    let finish = |out: &mut Vec<Person>, notes: &mut Vec<&str>| {
        if let Some(p) = out.last_mut() {
            p.notes = notes.join("\n").trim().to_string();
        }
        notes.clear();
    };
    for line in text.trim_start_matches('\u{feff}').lines() {
        let heading = line.strip_prefix("## ").and_then(|h| {
            let (id, name) = h.split_once(':')?;
            Some((id.trim().parse::<u32>().ok()?, name.trim()))
        });
        match heading {
            Some((id, name)) if id != 0 && !name.is_empty() => {
                finish(&mut out, &mut notes);
                out.push(Person {
                    id,
                    name: name.to_string(),
                    notes: String::new(),
                });
            }
            _ if !out.is_empty() => notes.push(line),
            _ => {}
        }
    }
    finish(&mut out, &mut notes);
    out
}

// ------------------------------------------------------------ faces.bin

const MAGIC: &[u8; 8] = b"VSFACES1";
const PHOTO_SCRIBE_MAGIC: &[u8; 8] = b"PSFACES1";

fn encode(faces: &[Face]) -> Vec<u8> {
    let mut b = Vec::with_capacity(12 + faces.len() * 560);
    b.extend_from_slice(MAGIC);
    b.extend_from_slice(&(faces.len() as u32).to_le_bytes());
    for f in faces {
        b.extend_from_slice(&f.person.to_le_bytes());
        let video = f.video.as_bytes();
        b.extend_from_slice(&(video.len() as u16).to_le_bytes());
        b.extend_from_slice(video);
        b.extend_from_slice(&(f.tag as u32).to_le_bytes());
        for v in &f.embedding {
            b.extend_from_slice(&v.to_le_bytes());
        }
    }
    b
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        (self.0.len() >= n).then(|| {
            let (a, rest) = self.0.split_at(n);
            self.0 = rest;
            a
        })
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn embedding(&mut self) -> Option<Vec<f32>> {
        (0..EMBEDDING).map(|_| self.f32()).collect()
    }
}

fn decode(b: &[u8]) -> Option<Vec<Face>> {
    let mut r = Reader(b);
    if r.take(8)? != MAGIC {
        return None;
    }
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n.min(1_000_000));
    for _ in 0..n {
        let person = r.u32()?;
        let len = r.u16()? as usize;
        let video = String::from_utf8(r.take(len)?.to_vec()).ok()?;
        let tag = r.u32()? as usize;
        out.push(Face {
            person,
            video,
            tag,
            embedding: r.embedding()?,
        });
    }
    Some(out)
}

/// The faces in Photo Scribe's `faces.bin` that the user named, as (person,
/// fingerprint): not the ones Photo Scribe only guessed, nor ones left out.
fn decode_photo_scribe(b: &[u8]) -> Option<Vec<(u32, Vec<f32>)>> {
    let mut r = Reader(b);
    if r.take(8)? != PHOTO_SCRIBE_MAGIC {
        return None;
    }
    let photos = r.u32()? as usize;
    let mut out = Vec::new();
    for _ in 0..photos {
        let len = r.u16()? as usize;
        // Path, size, modified, year.
        r.take(len + 8 + 8 + 2)?;
        let count = r.u16()? as usize;
        for _ in 0..count {
            // Box and score.
            r.take(5 * 4)?;
            let embedding = r.embedding()?;
            let person = r.u32()?;
            let _not_person = r.u32()?;
            let ignored = r.take(1)?[0] != 0;
            if person != 0 && !ignored {
                out.push((person, embedding));
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(seed: usize) -> Vec<f32> {
        let mut v = vec![0.0; EMBEDDING];
        v[seed] = 1.0;
        v
    }

    /// Mostly `seed`, a little of `other`: still clearly `seed`'s person.
    fn near(seed: usize, other: usize) -> Vec<f32> {
        let mut v = vec![0.0; EMBEDDING];
        v[seed] = 0.9;
        v[other] = 0.43;
        v
    }

    #[test]
    fn people_text_reads_back() {
        let persons = vec![
            Person {
                id: 1,
                name: "Ruth".into(),
                notes: "The bride's mother.".into(),
            },
            Person {
                id: 4,
                name: "Ben".into(),
                notes: String::new(),
            },
        ];
        assert_eq!(parse_people(&people_text(&persons)), persons);
    }

    #[test]
    fn faces_read_back() {
        let faces = vec![Face {
            person: 2,
            video: "wedding".into(),
            tag: 3,
            embedding: unit(5),
        }];
        assert_eq!(decode(&encode(&faces)), Some(faces));
        assert!(decode(b"something else").is_none());
    }

    #[test]
    fn photo_scribe_named_faces_only() {
        // One photo, three faces: named, guessed (not saved as named), left out.
        let mut b = PHOTO_SCRIBE_MAGIC.to_vec();
        b.extend_from_slice(&1u32.to_le_bytes());
        let path = b"C:\\photo.jpg";
        b.extend_from_slice(&(path.len() as u16).to_le_bytes());
        b.extend_from_slice(path);
        b.extend_from_slice(&[0u8; 18]);
        b.extend_from_slice(&3u16.to_le_bytes());
        for (person, ignored, seed) in [(7u32, 0u8, 1), (0, 0, 2), (7, 1, 3)] {
            b.extend_from_slice(&[0u8; 20]);
            for v in unit(seed) {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b.extend_from_slice(&person.to_le_bytes());
            b.extend_from_slice(&0u32.to_le_bytes());
            b.push(ignored);
        }
        assert_eq!(decode_photo_scribe(&b), Some(vec![(7, unit(1))]));
    }

    #[test]
    fn naming_a_tag_again_replaces_it() {
        let mut k = Known::default();
        assert!(k.learn("wedding", 3, "Ruth", &[unit(1), unit(2)]));
        assert_eq!(k.faces.len(), 2);
        assert!(k.learn("wedding", 3, "Ann", &[unit(1)]));
        assert_eq!(k.faces.len(), 1);
        assert_eq!(k.persons.len(), 2);
        let ann = k.persons.iter().find(|p| p.name == "Ann").unwrap().id;
        assert_eq!(k.faces[0].person, ann);
        // An empty name forgets the tag's faces.
        assert!(k.learn("wedding", 3, "", &[unit(1)]));
        assert!(k.faces.is_empty());
        // The same name, whatever its case, is the same person.
        k.learn("party", 1, "ann", &[unit(4)]);
        assert_eq!(k.persons.len(), 2);
    }

    #[test]
    fn recognised_only_when_clear() {
        let mut k = Known::default();
        k.learn("wedding", 1, "Ruth", &[unit(1)]);
        k.learn("wedding", 2, "Ben", &[unit(2)]);
        let found = k.recognise(
            "party",
            &[
                vec![near(1, 9), near(1, 8)],
                vec![unit(30)],
                vec![near(2, 9), unit(31), unit(32)],
            ],
            &[],
        );
        assert_eq!(found[0].as_deref(), Some("Ruth"));
        // Nobody known.
        assert_eq!(found[1], None);
        // Only one face in three looks like Ben: not enough.
        assert_eq!(found[2], None);
        // A video's own named faces never recognise its tags.
        assert_eq!(k.recognise("wedding", &[vec![unit(1)]], &[])[0], None);
    }
}
