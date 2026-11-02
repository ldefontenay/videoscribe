//! Writing the descriptions: still pictures or clips from each place, prompt
//! version 10 (`prompts/describe.v10.txt`), one request for every four places,
//! then a text-only editor pass over the whole draft (log entry 22). Ported from the
//! walking skeleton (spike/src/skeleton.rs), which the listening rounds tested.
//!
//! The requests go one at a time, each carrying what the earlier ones said,
//! or several at once (log entry 31): then each is written from its pictures
//! alone, and a text-only request afterwards makes the labels agree.
//!
//! Every reply is saved to a checkpoint as it arrives (PLAN.md §5.6), so a job
//! that is stopped or fails carries on where it was without paying twice.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use crate::clips;
use crate::frames::Frames;
use crate::speech::Gap;
use crate::{net, openrouter};

const PROMPT: &str = include_str!("../prompts/describe.v10.txt");
/// The faces rule, added when a family video's faces were tagged (log entry
/// 44). Apart from PROMPT, so that videos described before keep their
/// descriptions.
const FACES: &str = include_str!("../prompts/faces.v1.txt");
/// Where requests go: OpenRouter with its key, or Claude Code on this
/// computer (milestone 6). The same prompt and pictures go either way.
pub enum Route {
    OpenRouter { key: String },
    Claude { exe: std::path::PathBuf },
}

impl Route {
    /// Sends one request. `schema` and `effort` are OpenRouter's: Claude is
    /// asked for the same JSON by the prompt alone, and thinks as it will.
    fn ask(
        &self,
        model: &str,
        system: &str,
        content: Vec<Value>,
        schema: Option<Value>,
        effort: openrouter::Effort,
        stop: &AtomicBool,
    ) -> Result<Option<openrouter::Reply>, String> {
        match self {
            Route::OpenRouter { key } => {
                openrouter::ask(key, model, system, content, schema, effort, stop)
            }
            Route::Claude { exe } => crate::claude::ask(exe, model, system, content, stop),
        }
    }
}

/// Places in one request, as in round 2.
pub const PLACES_PER_REQUEST: usize = 4;
/// Places the editor sees at once: text only, so many more. Not the whole
/// video: given 60, GLM thought until it ran out of room (log entry 31).
const EDIT_PLACES: usize = 20;
/// Edited descriptions from before its part the editor is given, for
/// continuity.
const EDIT_EARLIER: usize = 10;
/// Words the editor may add to say a new place: "At the reception," (prompt
/// v8).
const NEW_PLACE_WORDS: usize = 5;

/// What every request knows besides its places.
pub struct Brief {
    /// A family video gets the home-video rules; YouTube the film rules.
    pub family: bool,
    /// Such as "British English".
    pub language: String,
    /// The style line of the prompt.
    pub style: String,
    /// The title or file name, only when the user turned that on.
    pub name: Option<String>,
    /// Caption lines (start, end, text), for the model's understanding only.
    pub cues: Vec<(f64, f64, String)>,
    /// The model is sent clips with their sound, rather than still pictures.
    pub hears: bool,
    /// Who is in the video, in the user's words (milestone 5), or empty.
    pub who: String,
    /// The people the user named after an earlier describing, one per line
    /// with how they looked (`known_lines`), or empty (log entry 37).
    pub known: String,
    /// Requests sent at once: 1 sends them in turn, each with the story so
    /// far; more sends each with nothing but its own places.
    pub together: usize,
    /// The faces in the pictures are tagged, "Person 3", and the tags go
    /// with them (log entry 44).
    pub faces: bool,
    /// The tags, once found: filled in just before describing.
    pub tagged: Option<crate::faces::Tagged>,
}

impl Brief {
    /// Whether this job sends its requests several at once.
    fn at_once(&self) -> bool {
        self.together > 1
    }
}

/// What the model is shown of each place.
pub enum Look<'a> {
    Pictures(&'a Frames),
    /// Clips cut from these files, in a folder for the work files.
    Clips(clips::Source<'a>, PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Described {
    /// The place's number, from 1.
    pub place: usize,
    pub text: String,
    pub seen_at: Option<f64>,
    /// Where it happens, in a few words ("the church"), so the editor can
    /// say when the place changes (prompt v8). Empty if not given.
    pub setting: String,
}

pub struct Outcome {
    pub described: Vec<Described>,
    /// Labels and names the model used, in order of first use.
    pub people: Vec<String>,
    pub cost: f64,
    pub requests: usize,
    pub pictures: usize,
    pub clips: usize,
    /// Seconds of video in the clips.
    pub clip_secs: f64,
    /// Descriptions the editor pass changed, and emptied.
    pub edited: usize,
    pub emptied: usize,
    /// Why the editor pass could not finish, if it could not.
    pub edit_failed: Option<String>,
    /// Requests answered from the checkpoint rather than sent again.
    pub reused: usize,
    /// Places where the model heard someone speaking, so left empty.
    pub speech: Vec<usize>,
    /// The user's own text for some places, from reviewing the result.
    pub overrides: Vec<(usize, String)>,
}

/// Picture times for a place: just inside each end and the middle; two for a
/// place under two seconds.
pub fn picture_times(g: &Gap) -> Vec<f64> {
    let len = g.end - g.start;
    if len < 2.0 {
        vec![g.start + len * 0.3, g.start + len * 0.7]
    } else {
        vec![g.start + 0.3, (g.start + g.end) / 2.0, g.end - 0.3]
    }
}

pub fn picture_count(gaps: &[Gap]) -> usize {
    gaps.iter().map(|g| picture_times(g).len()).sum()
}

/// Words as a listener hears them: anything with a letter or digit in it.
pub fn words(text: &str) -> usize {
    text.split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count()
}

/// The prompt file's `=== NAME ===` section, notes (`#` lines) removed.
fn section(name: &str) -> String {
    let marker = format!("=== {name}");
    let mut inside = false;
    let mut lines = Vec::new();
    for line in PROMPT.lines() {
        if line.starts_with("=== ") {
            inside = line.starts_with(&marker);
            continue;
        }
        if inside && !line.starts_with('#') {
            lines.push(line);
        }
    }
    lines.join("\n").trim().to_string()
}

/// No more than one blank line in a row, and none at either end.
fn tidy(s: &str) -> String {
    let mut out = String::new();
    let mut blank = 0;
    for line in s.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

/// The faces rule's own text, without its notes.
fn faces_rule() -> String {
    FACES
        .lines()
        .filter(|l| !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn system_prompt(brief: &Brief) -> String {
    let pick = |on: bool, name: &str| if on { section(name) } else { String::new() };
    let system = tidy(
        &section("SYSTEM")
            .replace(
                "{heard_names_rule}",
                &pick(!brief.cues.is_empty() || brief.hears, "HEARD NAMES RULE"),
            )
            .replace(
                "{heard_speech_rule}",
                &pick(brief.hears, "HEARD SPEECH RULE"),
            )
            .replace(
                "{known_people_rule}",
                &pick(!brief.known.is_empty(), "KNOWN PEOPLE RULE"),
            )
            .replace("{family_rules}", &pick(brief.family, "FAMILY RULES"))
            .replace("{youtube_rules}", &pick(!brief.family, "YOUTUBE RULES")),
    );
    if brief.faces {
        format!("{system}\n\n{}", faces_rule())
    } else {
        system
    }
}

fn place_line(n: usize, g: &Gap) -> String {
    let mut s = format!(
        "{n}: {:.1} to {:.1}, up to {} words",
        g.start, g.end, g.words
    );
    if let Some((p, of)) = g.part {
        s.push_str(&format!(", part {p} of {of}"));
    }
    if g.optional() {
        s.push_str(", optional");
    }
    s
}

/// The user's note on who is in the video, as rule 15's known people.
fn known_people(who: &str, known: &str) -> String {
    let mut out = Vec::new();
    if !who.trim().is_empty() {
        out.push(format!(
            "Known people, from the person who filmed this video (use a name only when you are \
             sure it is that person):\n{}",
            who.trim()
        ));
    }
    if !known.trim().is_empty() {
        out.push(format!(
            "People the family named after an earlier description of this video, with the label \
             it gave them and how they looked (rule 15):\n{}",
            known.trim()
        ));
    }
    out.join("\n")
}

/// The people the user named, for describing again knowing them: one line
/// for each name, with the labels it was given and how that person looked.
/// Empty if nobody is named.
pub fn known_lines(names: &[(String, String)], looks: &[(String, String)]) -> String {
    let mut people: Vec<(&str, Vec<&str>)> = Vec::new();
    for (label, name) in names.iter().filter(|(_, n)| !n.trim().is_empty()) {
        let name = name.trim();
        match people
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            Some((_, labels)) => labels.push(label),
            None => people.push((name, vec![label])),
        }
    }
    people
        .iter()
        .map(|(name, labels)| {
            let mut seen: Vec<&str> = Vec::new();
            for label in labels {
                if let Some((_, k)) = looks.iter().find(|(l, _)| l.eq_ignore_ascii_case(label)) {
                    if !seen.contains(&k.as_str()) {
                        seen.push(k);
                    }
                }
            }
            let called: Vec<String> = labels.iter().map(|l| format!("\"{l}\"")).collect();
            let mut line = format!("- {name}, labelled {}", called.join(" and "));
            if !seen.is_empty() {
                line += &format!(": {}", seen.join("; "));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The known people an earlier describing of this video was given, so
/// describing it again without new names keeps its descriptions.
pub fn known_of(checkpoint: &Path) -> String {
    std::fs::read_to_string(checkpoint)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["known"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// What an earlier run of a video's descriptions says about its people, for
/// naming them after describing (milestone 5).
#[derive(Default)]
pub struct Cast {
    /// Every label, in order of first use.
    pub labels: Vec<String>,
    /// The labels that are names the model heard spoken, not ones it made up
    /// from how someone looks (HEARD NAMES RULE).
    pub heard: Vec<String>,
    /// How each label's person looked: when first labelled, then anything
    /// noted later, after ". Also: " (round 2 test, 3.5: a first look taken
    /// from afar said little).
    pub looks: Vec<(String, String)>,
    /// Each place's description, edited when the editor pass finished.
    pub texts: Vec<(usize, String)>,
    /// Each label with the tag of its face, "Person 3" as 3, as the replies
    /// gave them when the faces were tagged (log entry 44): a label may
    /// claim more than one tag, and a tag more than one label.
    pub tags: Vec<(String, usize)>,
}

impl Cast {
    /// The labels whose tag is sure: the only tag the label was given, and
    /// given to no other label. The describer sometimes gives one face's
    /// tag to two people (log entry 44), and then neither can be named by
    /// face, nor teach Video Scribe a face.
    pub fn sure_tags(&self) -> Vec<(String, usize)> {
        self.tags
            .iter()
            .filter(|(label, tag)| {
                self.tags
                    .iter()
                    .filter(|(l, t)| l.eq_ignore_ascii_case(label) || t == tag)
                    .count()
                    == 1
            })
            .cloned()
            .collect()
    }
}

/// Between a person's first look and what was noted later.
const ALSO: &str = ". Also: ";

/// Adds `look` to what `name` looks like, unless it says nothing new.
fn add_look(looks: &mut Vec<(String, String)>, name: &str, look: &str) {
    let look = look.trim().trim_end_matches('.').trim();
    if look.is_empty() {
        return;
    }
    match looks.iter_mut().find(|(c, _)| c.eq_ignore_ascii_case(name)) {
        None => looks.push((name.to_string(), look.to_string())),
        Some((_, had)) => {
            let lower = look.to_lowercase();
            if !had.split(ALSO).any(|p| p.to_lowercase() == lower) {
                had.push_str(ALSO);
                had.push_str(look);
            }
        }
    }
}

/// Reads a checkpoint's cast, whatever job it was for.
pub fn cast(checkpoint: &Path) -> Cast {
    let Some(v) = std::fs::read_to_string(checkpoint)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return Cast::default();
    };
    let mut out = Cast::default();
    let new = |list: &[String], name: &str| !list.iter().any(|c| c.eq_ignore_ascii_case(name));
    for r in v["replies"].as_array().into_iter().flatten() {
        let text = r["text"].as_str().unwrap_or("");
        for name in parse_people(text) {
            if new(&out.labels, &name) {
                out.labels.push(name);
            }
        }
        for (name, look) in parse_looks(text) {
            add_look(&mut out.looks, &name, &look);
        }
        for (name, tag) in parse_tags(text) {
            if !out
                .tags
                .iter()
                .any(|(l, t)| l.eq_ignore_ascii_case(&name) && *t == tag)
            {
                out.tags.push((name, tag));
            }
        }
        if let Some(reply) = first_object(text) {
            for p in reply["people"].as_array().into_iter().flatten() {
                if p["heard"].as_bool() == Some(true) {
                    if let Some(name) = p["name"].as_str().map(str::trim) {
                        if !name.is_empty() && new(&out.heard, name) {
                            out.heard.push(name.to_string());
                        }
                    }
                }
            }
        }
        for d in parse_reply(text).unwrap_or_default() {
            if !out.texts.iter().any(|(p, _)| *p == d.place) {
                out.texts.push((d.place, d.text));
            }
        }
    }
    // Requests sent at once: the labels as they were made to agree.
    if let Some(same) = v["same"]["text"].as_str() {
        let drafts: Vec<String> = out.texts.iter().map(|(_, t)| t.clone()).collect();
        let map = parse_same(same, &out.labels, &drafts, &out.looks);
        for (_, t) in &mut out.texts {
            *t = crate::people::apply(t, &map);
        }
        out.labels = relabelled(&out.labels, &map);
        let tags = std::mem::take(&mut out.tags);
        for (mut l, tag) in tags {
            if let Some((_, to)) = map.iter().find(|(f, _)| f.eq_ignore_ascii_case(&l)) {
                l = to.clone();
            }
            if !out
                .tags
                .iter()
                .any(|(c, t)| c.eq_ignore_ascii_case(&l) && *t == tag)
            {
                out.tags.push((l, tag));
            }
        }
        let looks = std::mem::take(&mut out.looks);
        for (mut l, look) in looks {
            if let Some((_, to)) = map.iter().find(|(f, _)| f.eq_ignore_ascii_case(&l)) {
                l = to.clone();
            }
            for part in look.split(ALSO) {
                add_look(&mut out.looks, &l, part);
            }
        }
    }
    for t in v["edited"]["texts"].as_array().into_iter().flatten() {
        let (Some(place), Some(text)) = (t["place"].as_u64(), t["text"].as_str()) else {
            continue;
        };
        match out.texts.iter_mut().find(|(p, _)| *p == place as usize) {
            Some(e) => e.1 = text.to_string(),
            None => out.texts.push((place as usize, text.to_string())),
        }
    }
    out.texts.retain(|(_, t)| !t.trim().is_empty());
    out.texts.sort_by_key(|(p, _)| *p);
    out
}

/// How many earlier descriptions each request is reminded of, for the story.
const PREVIOUS: usize = 10;

/// The request for places `first..first + batch.len()` (numbered from 1).
fn request_prompt(
    batch: &[Gap],
    first: usize,
    brief: &Brief,
    previous: &[(f64, String)],
    characters: &[String],
) -> String {
    let previous = if previous.is_empty() {
        String::new()
    } else {
        let mut s = "Already said:".to_string();
        for (t, text) in previous.iter().rev().take(PREVIOUS).rev() {
            s.push_str(&format!("\n- {t:.1}: {text}"));
        }
        s
    };
    let characters = if characters.is_empty() {
        String::new()
    } else {
        format!(
            "Characters so far (use these labels exactly; after a colon, how that one looked \
             when first labelled):\n{}",
            characters
                .iter()
                .map(|c| format!("- {c}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let dialogue = match (batch.first(), batch.last()) {
        (Some(a), Some(b)) => {
            let lines: Vec<String> = brief
                .cues
                .iter()
                .filter(|(s, e, _)| *e > a.start - 8.0 && *s < b.end + 8.0)
                .map(|(s, e, t)| format!("- {s:.1} to {e:.1}: {t}"))
                .collect();
            if lines.is_empty() {
                String::new()
            } else {
                format!(
                    "What is said around these gaps, for your understanding only; never repeat \
                     it:\n{}",
                    lines.join("\n")
                )
            }
        }
        _ => String::new(),
    };
    let mut context = Vec::new();
    if first == 1 {
        context.push("The listener has heard nothing yet: this is the start.".to_string());
    }
    if let Some(name) = &brief.name {
        context.push(if brief.family {
            format!("The file is called \"{name}\".")
        } else {
            format!("The video is titled \"{name}\".")
        });
    }
    let places: Vec<String> = batch
        .iter()
        .enumerate()
        .map(|(i, g)| place_line(first + i, g))
        .collect();
    let evidence = if brief.hears {
        "For each gap, or run of touching gaps, you are given a short video clip with its sound, \
         labelled with the gaps it covers and the second it starts at. Times inside a clip count \
         from its start: add the start to give seen_at in seconds from the start of the video. \
         You can hear the soundtrack: use it only to understand who is who and what is \
         happening, never as something to describe."
            .to_string()
    } else {
        format!(
        "For each gap you are given {} still frames, labelled with their gap number and time in \
         seconds. You cannot hear the soundtrack, so describe only the pictures.",
        if batch.iter().all(|g| picture_times(g).len() == 3) {
            "three"
        } else {
            "two or three"
        }
    )
    };
    tidy(
        &section("REQUEST")
            .replace("{language}", &brief.language)
            .replace("{style}", &brief.style)
            .replace("{context}", &context.join(" "))
            .replace("{known_people}", &known_people(&brief.who, &brief.known))
            .replace("{characters}", &characters)
            .replace("{dialogue}", &dialogue)
            .replace("{previous}", &previous)
            .replace("{gaps}", &places.join("\n"))
            .replace("{evidence_note}", &evidence),
    )
}

/// The first JSON object in a reply, skipping code fences and chatter, and
/// ignoring anything after it (Claude once sent a second object, log entry 15).
fn first_object(text: &str) -> Option<Value> {
    let a = text.find('{')?;
    serde_json::Deserializer::from_str(&text[a..])
        .into_iter::<Value>()
        .next()?
        .ok()
}

/// A reply's descriptions put at the places asked about. Claude, without a
/// schema to hold it, sometimes numbers the gaps from 1 or not at all: then a
/// full set goes by order, and anything else by the time it was seen at.
fn placed(mut list: Vec<Described>, batch: &[Gap], first: usize) -> Vec<Described> {
    let range = first..first + batch.len();
    if list.iter().all(|d| range.contains(&d.place)) {
        return list;
    }
    if list.len() == batch.len() && !list.iter().any(|d| range.contains(&d.place)) {
        for (n, d) in list.iter_mut().enumerate() {
            d.place = first + n;
        }
        return list;
    }
    let taken: Vec<usize> = list
        .iter()
        .map(|d| d.place)
        .filter(|p| range.contains(p))
        .collect();
    let mut placed = Vec::new();
    for mut d in list {
        if !range.contains(&d.place) {
            let Some(t) = d.seen_at else { continue };
            let Some(n) = batch.iter().position(|g| g.start <= t && t <= g.end) else {
                continue;
            };
            if taken.contains(&(first + n))
                || placed.iter().any(|o: &Described| o.place == first + n)
            {
                continue;
            }
            d.place = first + n;
        }
        placed.push(d);
    }
    placed
}

/// The descriptions in a reply. A reply cut off before its end keeps every
/// complete description (Gemma, log entry 15).
pub fn parse_reply(text: &str) -> Result<Vec<Described>, String> {
    let list: Vec<Value> = match first_object(text) {
        Some(v) => v["descriptions"]
            .as_array()
            .ok_or("The model's answer had no list of descriptions.")?
            .clone(),
        None => text
            .match_indices('{')
            .filter_map(|(i, _)| {
                serde_json::Deserializer::from_str(&text[i..])
                    .into_iter::<Value>()
                    .next()?
                    .ok()
            })
            .filter(|v| v.get("gap").is_some())
            .collect(),
    };
    if list.is_empty() && first_object(text).is_none() {
        return Err("The model's answer was not in the shape asked for.".into());
    }
    Ok(list
        .iter()
        .map(|d| Described {
            // Unnumbered, it is placed by its time (see `placed`).
            place: d["gap"].as_u64().unwrap_or(0) as usize,
            text: d["text"].as_str().unwrap_or("").trim().to_string(),
            seen_at: d["seen_at"].as_f64(),
            setting: d["place"].as_str().unwrap_or("").trim().to_string(),
        })
        .collect())
}

/// The gaps a reply says it heard speech in.
fn heard_speech(text: &str) -> Vec<usize> {
    first_object(text)
        .and_then(|v| {
            v["descriptions"].as_array().map(|l| {
                l.iter()
                    .filter(|d| d["speech"].as_bool() == Some(true))
                    .filter_map(|d| Some(d["gap"].as_u64()? as usize))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// The names and labels in a reply's "people" list.
/// Each person's label with the looks the model noted, where it noted any.
fn parse_looks(text: &str) -> Vec<(String, String)> {
    first_object(text)
        .and_then(|v| {
            v["people"].as_array().map(|l| {
                l.iter()
                    .filter_map(|p| {
                        let name = p["name"].as_str()?.trim();
                        let look = p["new"].as_str()?.trim();
                        (!name.is_empty() && !look.is_empty())
                            .then(|| (name.to_string(), look.to_string()))
                    })
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Each person's label with the tag of their face, where the reply gave one.
fn parse_tags(text: &str) -> Vec<(String, usize)> {
    first_object(text)
        .and_then(|v| {
            v["people"].as_array().map(|l| {
                l.iter()
                    .filter_map(|p| {
                        let name = p["name"].as_str()?.trim();
                        let tag = crate::faces::tag_number(p["tag"].as_str()?)?;
                        (!name.is_empty()).then(|| (name.to_string(), tag))
                    })
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn parse_people(text: &str) -> Vec<String> {
    first_object(text)
        .and_then(|v| {
            v["people"].as_array().map(|l| {
                l.iter()
                    .filter_map(|p| p["name"].as_str().map(|n| n.trim().to_string()))
                    .filter(|n| !n.is_empty())
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// The reply shape as a structured-output schema: GLM once answered with "g"
/// for "gap" (log entry 15).
/// The editor's answer: text for each gap, nothing else.
fn edit_schema() -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "edited_description",
            "strict": true,
            "schema": {
                "type": "object",
                "properties": {
                    "descriptions": { "type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "gap": { "type": "integer" },
                            "text": { "type": "string" }
                        },
                        "required": ["gap", "text"],
                        "additionalProperties": false
                    }}
                },
                "required": ["descriptions"],
                "additionalProperties": false
            }
        }
    })
}

fn reply_schema(hears: bool, faces: bool) -> Value {
    // A service that hears also says where it heard speech (log entry 21).
    let (mut item, mut required) = (
        json!({
            "gap": { "type": "integer" },
            "text": { "type": "string" },
            "seen_at": { "type": "number" },
            "place": { "type": "string" }
        }),
        vec!["gap", "text", "seen_at", "place"],
    );
    if hears {
        item["speech"] = json!({ "type": "boolean" });
        required.push("speech");
    }
    // A family video with tagged faces also says each person's tag.
    let mut person = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "new": { "type": "string" },
            "heard": { "type": "boolean" }
        },
        "required": ["name", "new", "heard"],
        "additionalProperties": false
    });
    if faces {
        person["properties"]["tag"] = json!({ "type": "string" });
        person["required"] = json!(["name", "new", "heard", "tag"]);
    }
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "audio_description",
            "strict": true,
            "schema": {
                "type": "object",
                "properties": {
                    "descriptions": { "type": "array", "items": {
                        "type": "object",
                        "properties": item,
                        "required": required,
                        "additionalProperties": false
                    }},
                    "people": { "type": "array", "items": person }
                },
                "required": ["descriptions", "people"],
                "additionalProperties": false
            }
        }
    })
}

/// Each label, with the looks it was first given when there are any.
fn labelled(people: &[String], looks: &[(String, String)]) -> Vec<String> {
    people
        .iter()
        .map(
            |p| match looks.iter().find(|(c, _)| c.eq_ignore_ascii_case(p)) {
                Some((_, look)) => format!("{p}: {look}"),
                None => p.clone(),
            },
        )
        .collect()
}

/// Whether a shortened description still shares a word of four letters or
/// more with the original: a guard against an answer filed under the wrong
/// number (log entry 15).
fn same_subject(original: &str, short: &str) -> bool {
    let long_words = |t: &str| -> Vec<String> {
        t.split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() >= 4)
            .map(|w| w.to_lowercase())
            .collect()
    };
    let a = long_words(original);
    long_words(short).iter().any(|w| a.contains(w))
}

/// What the checkpoint holds: the answers so far, for exactly this job.
fn fingerprint(gaps: &[Gap], brief: &Brief, model: &str) -> String {
    let mut s = format!(
        "{model}\n{}\n{}\n{}\n{:?}\n{}\n{PLACES_PER_REQUEST}\n",
        brief.language, brief.style, brief.family, brief.name, brief.hears
    );
    // Requests sent at once are written without the story so far.
    if brief.at_once() {
        s.push_str("at once\n");
    }
    // A new note on who is in the video is a new job.
    if !brief.who.is_empty() {
        s.push_str(&format!("who: {}\n", brief.who));
    }
    // So are new names to describe knowing.
    if !brief.known.is_empty() {
        s.push_str(&format!("known: {}\n", brief.known));
    }
    // And tagged faces. The tags themselves come from the pictures, the
    // same each time, so the rule alone tells the jobs apart.
    if brief.faces {
        s.push_str(&format!("faces: {}\n", FACES.replace("\r\n", "\n")));
    }
    for g in gaps {
        s.push_str(&format!("{:.2} {:.2} {}\n", g.start, g.end, g.words));
    }
    // A new prompt version starts afresh.
    // By its text, not its line endings: git on Windows may turn LF into
    // CRLF on checkout, which must not throw kept descriptions away.
    s.push_str(&PROMPT.replace("\r\n", "\n"));
    net::hex(&net::sha256(s.as_bytes()))
}

/// A finished editor pass: each place's text, then how many were changed
/// and emptied.
type Edited = (Vec<(usize, String)>, usize, usize);

struct Checkpoint<'a> {
    path: &'a Path,
    print: String,
    /// Each request's answer and cost, in order; None for one not answered
    /// yet, as requests sent at once come back in any order.
    replies: Vec<Option<(String, f64)>>,
    /// The answer that made the labels agree, when requests went at once.
    same: Option<(String, f64)>,
    /// The finished editor pass, kept so a job described again with other
    /// voice or fit choices sends nothing.
    edited: Option<Edited>,
    /// The user's own text for some places, from reviewing the result
    /// (milestone 8): spoken as written, in place of the description.
    overrides: Vec<(usize, String)>,
    /// The known people these descriptions were written with (`known_of`).
    known: String,
}

impl Checkpoint<'_> {
    fn load<'a>(path: &'a Path, print: String) -> Checkpoint<'a> {
        let replies = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .filter(|v| v["fingerprint"] == print.as_str())
            .and_then(|v| {
                v["replies"].as_array().map(|l| {
                    l.iter()
                        .map(|r| Some((r["text"].as_str()?.to_string(), r["cost"].as_f64()?)))
                        .collect()
                })
            })
            .unwrap_or_default();
        let same = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .filter(|v| v["fingerprint"] == print.as_str())
            .and_then(|v| {
                Some((
                    v["same"]["text"].as_str()?.to_string(),
                    v["same"]["cost"].as_f64()?,
                ))
            });
        let edited = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .filter(|v| v["fingerprint"] == print.as_str())
            .and_then(|v| {
                let e = &v["edited"];
                let texts = e["texts"]
                    .as_array()?
                    .iter()
                    .filter_map(|t| {
                        Some((
                            t["place"].as_u64()? as usize,
                            t["text"].as_str()?.to_string(),
                        ))
                    })
                    .collect();
                Some((
                    texts,
                    e["changed"].as_u64()? as usize,
                    e["emptied"].as_u64()? as usize,
                ))
            });
        let overrides = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .filter(|v| v["fingerprint"] == print.as_str())
            .map(|v| read_overrides(&v))
            .unwrap_or_default();
        Checkpoint {
            path,
            print,
            replies,
            same,
            edited,
            overrides,
            known: String::new(),
        }
    }

    /// Written beside and renamed, so a crash never leaves half a checkpoint.
    fn reply(&self, bi: usize) -> Option<(String, f64)> {
        self.replies.get(bi).cloned().flatten()
    }

    fn keep(&mut self, bi: usize, text: String, cost: f64) -> Result<(), String> {
        if self.replies.len() <= bi {
            self.replies.resize(bi + 1, None);
        }
        self.replies[bi] = Some((text, cost));
        self.save()
    }

    fn save(&self) -> Result<(), String> {
        let mut v = json!({
            "fingerprint": self.print,
            "replies": self.replies.iter().map(|r| match r {
                Some((t, c)) => json!({"text": t, "cost": c}),
                None => Value::Null,
            }).collect::<Vec<_>>(),
        });
        if let Some((t, c)) = &self.same {
            v["same"] = json!({"text": t, "cost": c});
        }
        if !self.overrides.is_empty() {
            v["overrides"] = overrides_json(&self.overrides);
        }
        if !self.known.is_empty() {
            v["known"] = json!(self.known);
        }
        if let Some((texts, changed, emptied)) = &self.edited {
            v["edited"] = json!({
                "texts": texts.iter().map(|(p, t)| json!({"place": p, "text": t})).collect::<Vec<_>>(),
                "changed": changed,
                "emptied": emptied,
            });
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let part = self.path.with_extension("part");
        std::fs::write(&part, v.to_string())
            .and_then(|_| std::fs::rename(&part, self.path))
            .map_err(|e| format!("The checkpoint could not be saved: {e}"))
    }
}

fn read_overrides(v: &Value) -> Vec<(usize, String)> {
    v["overrides"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            Some((
                o["place"].as_u64()? as usize,
                o["text"].as_str()?.to_string(),
            ))
        })
        .collect()
}

fn overrides_json(list: &[(usize, String)]) -> Value {
    list.iter()
        .map(|(p, t)| json!({"place": p, "text": t}))
        .collect::<Vec<_>>()
        .into()
}

/// The user's own text for places of the job in this checkpoint, whatever
/// job it was.
pub fn overrides(checkpoint: &Path) -> Vec<(usize, String)> {
    std::fs::read_to_string(checkpoint)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|v| read_overrides(&v))
        .unwrap_or_default()
}

/// Keeps the user's own text for places, in the checkpoint beside the
/// descriptions it replaces, so describing afresh forgets it with them.
/// Written beside and renamed, like every checkpoint.
pub fn set_overrides(checkpoint: &Path, list: &[(usize, String)]) -> Result<(), String> {
    let mut v = std::fs::read_to_string(checkpoint)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .ok_or("This video's descriptions are no longer kept, so the changes could not be kept.")?;
    if list.is_empty() {
        if let Some(o) = v.as_object_mut() {
            o.remove("overrides");
        }
    } else {
        v["overrides"] = overrides_json(list);
    }
    let part = checkpoint.with_extension("part");
    std::fs::write(&part, v.to_string())
        .and_then(|_| std::fs::rename(&part, checkpoint))
        .map_err(|e| format!("Your changes could not be kept: {e}"))
}

/// What a stopped job still has to send, so the question can count only
/// that: the places of each request not answered yet.
pub fn left(gaps: &[Gap], brief: &Brief, model: &str, checkpoint: &Path) -> Vec<Vec<Gap>> {
    let saved = Checkpoint::load(checkpoint, fingerprint(gaps, brief, model));
    let rest = gaps
        .chunks(PLACES_PER_REQUEST)
        .enumerate()
        .filter(|(bi, _)| saved.reply(*bi).is_none())
        .map(|(_, b)| b.to_vec())
        .collect();
    rest
}

/// Whether the checkpoint holds a finished editor pass for exactly this job.
pub fn edited(gaps: &[Gap], brief: &Brief, model: &str, checkpoint: &Path) -> bool {
    Checkpoint::load(checkpoint, fingerprint(gaps, brief, model))
        .edited
        .is_some()
}

/// Describes every place. `progress` hears what is happening, for Status.
/// None if stopped; what was described so far stays in the checkpoint.
#[allow(clippy::too_many_arguments)]
pub fn describe(
    gaps: &[Gap],
    brief: &Brief,
    look: &Look,
    route: &Route,
    model: &str,
    checkpoint: &Path,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(String),
) -> Result<Option<Outcome>, String> {
    let system = system_prompt(brief);
    let mut saved = Checkpoint::load(checkpoint, fingerprint(gaps, brief, model));
    saved.known = brief.known.clone();
    let batches: Vec<&[Gap]> = gaps.chunks(PLACES_PER_REQUEST).collect();
    let reused = (0..batches.len())
        .filter(|&bi| saved.reply(bi).is_some())
        .count();
    let mut out = Outcome {
        described: Vec::new(),
        people: Vec::new(),
        cost: 0.0,
        requests: 0,
        pictures: 0,
        clips: 0,
        clip_secs: 0.0,
        edited: 0,
        emptied: 0,
        edit_failed: None,
        reused: reused.min(batches.len()),
        speech: Vec::new(),
        overrides: saved.overrides.clone(),
    };
    let mut looks: Vec<(String, String)> = Vec::new();
    // Every look given to every label, for making the labels agree.
    let mut all_looks: Vec<(String, String)> = Vec::new();
    if brief.at_once() {
        let all = Together {
            batches: &batches,
            brief,
            system: &system,
            route,
            model,
        };
        if !all.send(look, &mut saved, &mut out, stop, progress)? {
            return Ok(None);
        }
    }
    for (bi, batch) in batches.iter().enumerate() {
        progress(format!(
            "Describing: {bi} of {} requests answered.",
            batches.len()
        ));
        let first = bi * PLACES_PER_REQUEST + 1;
        let (text, cost) = match saved.reply(bi) {
            Some(r) => r,
            None => {
                if stop.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                let previous: Vec<(f64, String)> = out
                    .described
                    .iter()
                    .filter(|d| !d.text.is_empty())
                    .filter_map(|d| {
                        Some((gaps.get(d.place.checked_sub(1)?)?.start, d.text.clone()))
                    })
                    .collect();
                let prompt = request_prompt(
                    batch,
                    first,
                    brief,
                    &previous,
                    &labelled(&out.people, &looks),
                );
                let Some(content) = content(batch, first, prompt, look, brief, stop, &mut out)?
                else {
                    return Ok(None);
                };
                let Some(reply) = route.ask(
                    model,
                    &system,
                    content,
                    Some(reply_schema(brief.hears, brief.faces)),
                    openrouter::LOOK,
                    stop,
                )?
                else {
                    return Ok(None);
                };
                out.requests += 1;
                saved.keep(bi, reply.text.clone(), reply.cost)?;
                (reply.text, reply.cost)
            }
        };
        out.cost += cost;
        let range = first..first + batch.len();
        let speech = heard_speech(&text);
        for mut d in placed(parse_reply(&text)?, batch, first) {
            // Only the places asked about, once each.
            if range.contains(&d.place) && !out.described.iter().any(|o| o.place == d.place) {
                // Someone speaks here: say nothing over them.
                if speech.contains(&d.place) {
                    d.text.clear();
                    out.speech.push(d.place);
                }
                out.described.push(d);
            }
        }
        for name in parse_people(&text) {
            if !out.people.iter().any(|c| c.eq_ignore_ascii_case(&name)) {
                out.people.push(name);
            }
        }
        // The first looks noted for each label are the ones to check against.
        for (name, look) in parse_looks(&text) {
            if !looks.iter().any(|(c, _)| c.eq_ignore_ascii_case(&name)) {
                looks.push((name.clone(), look.clone()));
            }
            if !all_looks.contains(&(name.clone(), look.clone())) {
                all_looks.push((name, look));
            }
        }
    }
    out.described.sort_by_key(|d| d.place);

    // Written apart, the requests labelled the same person differently: make
    // the labels agree, on this computer, from one text-only answer.
    if brief.at_once() {
        let Some(map) = same_people(
            &mut out, &all_looks, route, model, &mut saved, stop, progress,
        )?
        else {
            return Ok(None);
        };
        relabel(&mut out, &map);
    }

    // Edited before, by a job that finished: take that, and send nothing.
    if let Some((texts, changed, emptied)) = &saved.edited {
        for d in &mut out.described {
            if let Some((_, t)) = texts.iter().find(|(p, _)| *p == d.place) {
                d.text = t.clone();
            }
        }
        out.edited = *changed;
        out.emptied = *emptied;
        return Ok(Some(out));
    }

    // The editor pass: the whole draft, text only (log entry 22).
    let places: Vec<usize> = (0..gaps.len()).collect();
    let parts = places.len().div_ceil(EDIT_PLACES);
    for (k, part) in places.chunks(EDIT_PLACES).enumerate() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        progress(format!(
            "All {} requests answered. Now editing the descriptions, text only: part {} of {parts}.",
            batches.len(),
            k + 1
        ));
        // The draft stands on its own: a failed edit keeps it as it is.
        if let Err(why) = edit(gaps, part, &mut out, route, model, stop) {
            // Run out of room thinking, most often: half as much at a time
            // has finished (prompt v8 test).
            let (a, b) = part.split_at(part.len() / 2);
            for half in [a, b] {
                if half.is_empty() || stop.load(Ordering::Relaxed) {
                    continue;
                }
                if edit(gaps, half, &mut out, route, model, stop).is_err() {
                    // The other parts can still be edited.
                    out.edit_failed = Some(why.clone());
                }
            }
        }
    }
    // Kept only when whole: a failed part is tried again next time.
    if out.edit_failed.is_none() && !stop.load(Ordering::Relaxed) {
        saved.edited = Some((
            out.described
                .iter()
                .map(|d| (d.place, d.text.clone()))
                .collect(),
            out.edited,
            out.emptied,
        ));
        saved.save()?;
    }
    Ok(Some(out))
}

/// One request's content: its prompt, then the pictures or clips of its
/// places. None if stopped.
fn content(
    batch: &[Gap],
    first: usize,
    prompt: String,
    look: &Look,
    brief: &Brief,
    stop: &AtomicBool,
    out: &mut Outcome,
) -> Result<Option<Vec<Value>>, String> {
    let tagged = brief.tagged.as_ref().filter(|_| brief.faces);
    let mut content = vec![json!({ "type": "text", "text": prompt })];
    match look {
        Look::Pictures(frames) => {
            for (i, g) in batch.iter().enumerate() {
                for t in picture_times(g) {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    let jpg = frames.at(t)?;
                    let mut label = format!("Gap {}, {t:.1} s:", first + i);
                    if let Some(line) = tagged.and_then(|f| f.line_at(t)) {
                        label = format!("{label} {line}");
                    }
                    content.push(json!({
                        "type": "text",
                        "text": label,
                    }));
                    content.push(json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:image/jpeg;base64,{}", net::base64(&jpg)) },
                    }));
                    out.pictures += 1;
                }
            }
        }
        Look::Clips(src, work) => {
            for (a, b, start, end) in clips::plan(batch, first) {
                if stop.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                let mp4 = clips::clip(src, start, end, work)?;
                let gaps = if a == b {
                    format!("Gap {a}")
                } else {
                    format!("Gaps {a} to {b}")
                };
                let mut label = format!(
                    "{gaps}: a clip starting at {start:.1} s, {:.1} s long:",
                    end - start
                );
                if let Some(line) = tagged.and_then(|f| f.line_between(start, end)) {
                    label = format!("{label} {line}");
                }
                content.push(json!({
                    "type": "text",
                    "text": label,
                }));
                content.push(json!({
                    "type": "video_url",
                    "video_url": { "url": format!("data:video/mp4;base64,{}", net::base64(&mp4)) },
                }));
                out.clips += 1;
                out.clip_secs += end - start;
            }
        }
    }
    Ok(Some(content))
}

/// Which labels name the same person, as (label, the label to use), from a
/// text-only request given every label with where it is used and how its
/// person looks. Kept in the checkpoint like any reply. None if stopped.
#[allow(clippy::too_many_arguments)]
fn same_people(
    out: &mut Outcome,
    looks: &[(String, String)],
    route: &Route,
    model: &str,
    saved: &mut Checkpoint,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(String),
) -> Result<Option<Vec<(String, String)>>, String> {
    if out.people.len() < 2 {
        return Ok(Some(Vec::new()));
    }
    let text = match &saved.same {
        Some((text, cost)) => {
            out.cost += cost;
            text.clone()
        }
        None => {
            progress("Describing: all answered. Now making the labels agree, text only.".into());
            let lines: Vec<String> = out
                .people
                .iter()
                .map(|label| {
                    let places: Vec<String> = out
                        .described
                        .iter()
                        .filter(|d| crate::people::mentions(&d.text, label))
                        .map(|d| d.place.to_string())
                        .collect();
                    let seen: Vec<&str> = looks
                        .iter()
                        .filter(|(l, _)| l.eq_ignore_ascii_case(label))
                        .map(|(_, k)| k.as_str())
                        .collect();
                    format!(
                        "- {label}: places {}; looks: {}",
                        if places.is_empty() {
                            "none".to_string()
                        } else {
                            places.join(", ")
                        },
                        if seen.is_empty() {
                            "not noted".to_string()
                        } else {
                            seen.join(" / ")
                        }
                    )
                })
                .collect();
            let prompt = format!("{}\n\nLabels:\n{}", section("LABELS"), lines.join("\n"));
            let Some(reply) = route.ask(
                model,
                "You edit audio description.",
                vec![json!({ "type": "text", "text": prompt })],
                Some(same_schema()),
                openrouter::TEXT,
                stop,
            )?
            else {
                return Ok(None);
            };
            out.requests += 1;
            out.cost += reply.cost;
            saved.same = Some((reply.text.clone(), reply.cost));
            saved.save()?;
            reply.text
        }
    };
    let drafts: Vec<String> = out.described.iter().map(|d| d.text.clone()).collect();
    Ok(Some(parse_same(&text, &out.people, &drafts, looks)))
}

/// The answer as (label, label to use) pairs, for labels that were asked
/// about. Checked here, not taken on trust: two labels in one description
/// are two people, so they are never joined, nor two whose looks name each
/// other ("holding the bride's train" was joined to the bride, round 2 test);
/// and a person's label is one of their own, never a bare "the girl" when
/// they have a better one.
fn parse_same(
    text: &str,
    labels: &[String],
    drafts: &[String],
    looks: &[(String, String)],
) -> Vec<(String, String)> {
    use crate::people::mentions;
    let asked = |l: &str| {
        labels
            .iter()
            .find(|c| c.eq_ignore_ascii_case(l.trim()))
            .cloned()
    };
    let used = |l: &str| drafts.iter().filter(|t| mentions(t, l)).count();
    // Both in one description, once the longer label's own mentions are
    // taken out: "the girl" is inside every "the ponytailed girl".
    let names = |a: &str, b: &str| {
        looks
            .iter()
            .any(|(l, k)| l.eq_ignore_ascii_case(a) && mentions(k, b))
    };
    let together = |a: &str, b: &str| {
        let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
        let gone = [(long.to_string(), "\u{1}".to_string())];
        names(a, b)
            || names(b, a)
            || drafts
                .iter()
                .any(|t| mentions(t, long) && mentions(&crate::people::apply(t, &gone), short))
    };
    let mut out: Vec<(String, String)> = Vec::new();
    let Some(v) = first_object(text) else {
        return out;
    };
    for p in v["people"].as_array().into_iter().flatten() {
        let mut group: Vec<String> = Vec::new();
        for l in p["labels"].as_array().into_iter().flatten() {
            if let Some(l) = l.as_str().and_then(asked) {
                // Each label belongs to one person only.
                if !group.contains(&l) && !out.iter().any(|(f, _)| *f == l) {
                    group.push(l);
                }
            }
        }
        if group.len() < 2 {
            continue;
        }
        let said = p["use"].as_str().and_then(asked);
        // The label the model chose, a role such as "the groom" included,
        // unless it is a bare "the girl": a role gave way to "a bald guest on
        // the phone" when the longer label always won (prompt v8 test).
        let to = match said.filter(|s| group.contains(s)) {
            Some(s) if !bare(&s) || group.iter().all(|g| bare(g)) => s,
            _ => group
                .iter()
                .filter(|g| !bare(g))
                .max_by_key(|g| used(g))
                .unwrap_or(&group[0])
                .clone(),
        };
        for l in group {
            if l != to && !together(&l, &to) {
                out.push((l, to.clone()));
            }
        }
    }
    out
}

/// Whether a label says no more than what kind of person it is: "the girl",
/// "a man".
fn bare(label: &str) -> bool {
    const KINDS: &[&str] = &[
        "man",
        "woman",
        "girl",
        "boy",
        "person",
        "child",
        "kid",
        "guest",
        "figure",
        "lady",
        "gentleman",
        "people",
        "creature",
    ];
    let words: Vec<String> = label.split_whitespace().map(str::to_lowercase).collect();
    match words.as_slice() {
        [one] => KINDS.contains(&one.as_str()),
        [_, two] => KINDS.contains(&two.as_str()),
        _ => false,
    }
}

/// The editor's text without the place marks it was shown, should it copy
/// them: "[place: the church]" was spoken in the first test of prompt v8.
fn unmarked(text: &str) -> String {
    let mut out = text.to_string();
    while let Some(a) = out.to_lowercase().find("[place:") {
        let Some(b) = out[a..].find(']') else {
            out.truncate(a);
            break;
        };
        out.replace_range(a..a + b + 1, "");
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Puts the agreed labels into the descriptions and the list of people.
fn relabel(out: &mut Outcome, map: &[(String, String)]) {
    if map.is_empty() {
        return;
    }
    for d in &mut out.described {
        d.text = crate::people::apply(&d.text, map);
    }
    out.people = relabelled(&out.people, map);
}

/// Labels with the agreed ones in place, each once, in order of first use.
fn relabelled(labels: &[String], map: &[(String, String)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in labels {
        let to = map
            .iter()
            .find(|(f, _)| f.eq_ignore_ascii_case(l))
            .map_or(l, |(_, t)| t);
        if !out.iter().any(|c| c.eq_ignore_ascii_case(to)) {
            out.push(to.clone());
        }
    }
    out
}

fn same_schema() -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "same_people",
            "strict": true,
            "schema": {
                "type": "object",
                "properties": {
                    "people": { "type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "use": { "type": "string" },
                            "labels": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["use", "labels"],
                        "additionalProperties": false
                    }}
                },
                "required": ["people"],
                "additionalProperties": false
            }
        }
    })
}

/// The labels in every answer so far, each with the looks it was first
/// given: all a request sent at once is told of the others (log entry 31).
fn known(saved: &Checkpoint) -> Vec<String> {
    let (mut people, mut looks): (Vec<String>, Vec<(String, String)>) = (Vec::new(), Vec::new());
    for (text, _) in saved.replies.iter().flatten() {
        for name in parse_people(text) {
            if !people.iter().any(|c| c.eq_ignore_ascii_case(&name)) {
                people.push(name);
            }
        }
        for (name, look) in parse_looks(text) {
            if !looks.iter().any(|(c, _)| c.eq_ignore_ascii_case(&name)) {
                looks.push((name, look));
            }
        }
    }
    labelled(&people, &looks)
}

/// Everything a request sent at once needs: none of it depends on another
/// request's answer.
struct Together<'a> {
    batches: &'a [&'a [Gap]],
    brief: &'a Brief,
    system: &'a str,
    route: &'a Route,
    model: &'a str,
}

impl Together<'_> {
    /// Sends every request not answered yet, `brief.together` at a time. The
    /// pictures are read here, in turn, while earlier requests are out; each
    /// answer is kept as it comes back. False if stopped.
    fn send(
        &self,
        look: &Look,
        saved: &mut Checkpoint,
        out: &mut Outcome,
        stop: &AtomicBool,
        progress: &mut dyn FnMut(String),
    ) -> Result<bool, String> {
        use std::sync::{mpsc, Mutex};
        let total = self.batches.len();
        let todo: Vec<usize> = (0..total).filter(|&bi| saved.reply(bi).is_none()).collect();
        if todo.is_empty() {
            return Ok(true);
        }
        let mut done = total - todo.len();
        let schema = reply_schema(self.brief.hears, self.brief.faces);
        let (jobs, queue) = mpsc::channel::<(usize, Vec<Value>)>();
        let queue = Mutex::new(queue);
        let (answer, answers) = mpsc::channel();
        let mut failed: Option<String> = None;
        let mut stopped = false;
        std::thread::scope(|scope| {
            for _ in 0..self.brief.together.min(todo.len()) {
                let answer = answer.clone();
                let (queue, schema) = (&queue, &schema);
                scope.spawn(move || loop {
                    let next = queue.lock().map(|q| q.recv());
                    let Ok(Ok((bi, content))) = next else {
                        break;
                    };
                    let r = self.route.ask(
                        self.model,
                        self.system,
                        content,
                        Some(schema.clone()),
                        openrouter::LOOK,
                        stop,
                    );
                    if answer.send((bi, r)).is_err() {
                        break;
                    }
                });
            }
            drop(answer);
            let mut out_now = 0;
            let mut take = |r: (usize, Result<Option<openrouter::Reply>, String>),
                            saved: &mut Checkpoint,
                            out: &mut Outcome,
                            failed: &mut Option<String>,
                            stopped: &mut bool| {
                match r.1 {
                    Ok(Some(reply)) => {
                        out.requests += 1;
                        if let Err(why) = saved.keep(r.0, reply.text, reply.cost) {
                            failed.get_or_insert(why);
                        }
                        done += 1;
                        progress(format!(
                            "Describing, {} requests at a time: {done} of {total} answered.",
                            self.brief.together
                        ));
                    }
                    Ok(None) => *stopped = true,
                    Err(why) => {
                        failed.get_or_insert(why);
                    }
                }
            };
            for &bi in &todo {
                if failed.is_some() || stopped || stop.load(Ordering::Relaxed) {
                    break;
                }
                // The first request goes alone, so the people at the start
                // have labels the others can use.
                while out_now >= self.brief.together
                    || (out_now > 0 && saved.replies.iter().all(Option::is_none))
                {
                    let Ok(r) = answers.recv() else { break };
                    out_now -= 1;
                    take(r, saved, out, &mut failed, &mut stopped);
                }
                let first = bi * PLACES_PER_REQUEST + 1;
                let prompt =
                    request_prompt(self.batches[bi], first, self.brief, &[], &known(saved));
                match content(self.batches[bi], first, prompt, look, self.brief, stop, out) {
                    Ok(Some(c)) => {
                        if jobs.send((bi, c)).is_ok() {
                            out_now += 1;
                        }
                    }
                    Ok(None) => stopped = true,
                    Err(why) => failed = Some(why),
                }
            }
            drop(jobs);
            while out_now > 0 {
                let Ok(r) = answers.recv() else { break };
                out_now -= 1;
                take(r, saved, out, &mut failed, &mut stopped);
            }
        });
        if let Some(why) = failed {
            return Err(why);
        }
        Ok(!stopped && !stop.load(Ordering::Relaxed))
    }
}

/// The fewest words the editor may cut a description to, short of emptying it.
const MIN_CUT: usize = 3;

/// The words that fit a pause spoken at the fastest speed the mix uses.
fn fast_limit(words: usize) -> usize {
    (words as f64 * crate::mix::FASTEST).floor() as usize
}

/// Edits one part of the draft, places given by index. A reply that breaks
/// the rules is ignored line by line, so the draft can only get better.
fn edit(
    gaps: &[Gap],
    part: &[usize],
    out: &mut Outcome,
    route: &Route,
    model: &str,
    stop: &AtomicBool,
) -> Result<(), String> {
    let text_of = |out: &Outcome, place: usize| {
        out.described
            .iter()
            .find(|d| d.place == place)
            .map(|d| d.text.clone())
            .unwrap_or_default()
    };
    // Where each line happens, said only where it differs from the place
    // before: the editor tells the listener a new place (prompt v8).
    let mut before = out
        .described
        .iter()
        .filter(|d| d.place <= part[0] && !d.setting.is_empty())
        .max_by_key(|d| d.place)
        .map(|d| d.setting.clone())
        .unwrap_or_default();
    let mut moved: Vec<usize> = Vec::new();
    let lines: Vec<String> = part
        .iter()
        .map(|&i| {
            let setting = out
                .described
                .iter()
                .find(|d| d.place == i + 1 && !d.text.is_empty())
                .map(|d| d.setting.clone())
                .unwrap_or_default();
            let at = if !setting.is_empty() && !setting.eq_ignore_ascii_case(&before) {
                before = setting.clone();
                moved.push(i + 1);
                format!(" [place: {setting}]")
            } else {
                String::new()
            };
            let g = &gaps[i];
            let t = text_of(out, i + 1);
            let w = words(&t);
            // Counted for it: models count words badly (log entry 23). Only
            // what will not fit even spoken faster is marked (prompt v5).
            let over = if w > fast_limit(g.words) {
                format!(" [{w} words: CUT TO {} OR FEWER]", g.words.max(MIN_CUT))
            } else {
                String::new()
            };
            format!(
                "{}: {:.1} to {:.1}, up to {} words{}: {}{over}{at}",
                i + 1,
                g.start,
                g.end,
                g.words,
                if g.optional() { ", optional" } else { "" },
                if t.is_empty() { "(empty)" } else { &t }
            )
        })
        .collect();
    let first = part[0];
    let earlier: Vec<String> = out
        .described
        .iter()
        .filter(|d| d.place <= first && !d.text.is_empty())
        .rev()
        .take(EDIT_EARLIER)
        .map(|d| format!("{}: {}", d.place, d.text))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let mut prompt = section("EDIT");
    if !out.people.is_empty() {
        prompt += &format!("\n\nLabels:\n{}", out.people.join("\n"));
    }
    if !earlier.is_empty() {
        prompt += &format!(
            "\n\nAlready edited, for continuity:\n{}",
            earlier.join("\n")
        );
    }
    prompt += &format!("\n\nGaps:\n{}", lines.join("\n"));
    let content = vec![json!({ "type": "text", "text": prompt })];
    let Some(reply) = route.ask(
        model,
        "You edit audio description.",
        content,
        Some(edit_schema()),
        openrouter::TEXT,
        stop,
    )?
    else {
        return Ok(());
    };
    out.requests += 1;
    out.cost += reply.cost;
    let answers = parse_reply(&reply.text).unwrap_or_default();
    // By order when the counts agree: GLM has numbered answers wrongly before
    // (log entry 15).
    let by_order = answers.len() == part.len();
    for (n, a) in answers.into_iter().enumerate() {
        let place = if by_order { part[n] + 1 } else { a.place };
        if !part.contains(&(place.wrapping_sub(1))) {
            continue;
        }
        let old = text_of(out, place);
        let new = unmarked(&a.text);
        if new == old || (!by_order && !new.is_empty() && !same_subject(&old, &new)) {
            continue;
        }
        // Never padded: the editor only cuts and corrects, except to say a
        // new place in a few words (prompt v8). Nor cut to a scrap: "Wee."
        // told the listener nothing (prompt v5).
        let room = if moved.contains(&place) {
            NEW_PLACE_WORDS
        } else {
            0
        };
        if words(&new) > words(&old) + room
            || (!new.is_empty() && words(&new) < MIN_CUT.min(words(&old)))
        {
            continue;
        }
        let Some(d) = out.described.iter_mut().find(|d| d.place == place) else {
            continue;
        };
        if new.is_empty() {
            out.emptied += 1;
        } else {
            out.edited += 1;
        }
        d.text = new;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gap(start: f64, end: f64) -> Gap {
        Gap {
            start,
            end,
            words: crate::speech::word_limit(end - start),
            part: None,
        }
    }

    fn brief() -> Brief {
        Brief {
            family: true,
            language: "British English".into(),
            style: "Vivid.".into(),
            name: None,
            cues: vec![(9.0, 11.0, "Look at that!".into())],
            hears: false,
            who: String::new(),
            known: String::new(),
            together: 1,
            faces: false,
            tagged: None,
        }
    }

    #[test]
    fn the_prompt_has_every_section() {
        for s in ["SYSTEM", "REQUEST", "EDIT", "FAMILY RULES", "YOUTUBE RULES"] {
            assert!(!section(s).is_empty(), "{s}");
        }
        let sys = system_prompt(&brief());
        assert!(sys.contains("This is a home video"));
        assert!(!sys.contains("end credits"));
        assert!(!sys.contains("_rule"), "placeholders left");
    }

    #[test]
    fn a_request_numbers_its_places_and_carries_the_dialogue() {
        let gaps = [gap(4.0, 5.5), gap(12.0, 15.0)];
        let p = request_prompt(&gaps, 9, &brief(), &[], &[]);
        assert!(p.contains("9: 4.0 to 5.5, up to 3 words"), "{p}");
        assert!(p.contains("10: 12.0 to 15.0, up to 7 words"), "{p}");
        assert!(p.contains("- 9.0 to 11.0: Look at that!"), "{p}");
        assert!(p.contains("two or three still frames"), "{p}");
        assert!(!p.contains("heard nothing yet"));
        assert!(!p.contains("{gaps}") && !p.contains("{context}"));
    }

    #[test]
    fn names_are_sent_only_when_given() {
        let gaps = [gap(4.0, 5.5)];
        assert!(!request_prompt(&gaps, 1, &brief(), &[], &[]).contains("called"));
        let mut b = brief();
        b.name = Some("Party.mp4".into());
        assert!(
            request_prompt(&gaps, 1, &b, &[], &[]).contains("The file is called \"Party.mp4\".")
        );
    }

    #[test]
    fn replies_parse_whole_fenced_or_cut_off() {
        let whole = "```json\n{\"descriptions\":[{\"gap\":1,\"text\":\"She grins.\",\"seen_at\":4.2}],\"people\":[{\"name\":\"the girl\",\"new\":\"\",\"heard\":false}]}\n```";
        let d = parse_reply(whole).unwrap();
        assert_eq!(d[0].text, "She grins.");
        assert_eq!(d[0].seen_at, Some(4.2));
        assert_eq!(parse_people(whole), vec!["the girl"]);
        let cut =
            "{\"descriptions\":[{\"gap\":1,\"text\":\"A dog.\",\"seen_at\":1.0},{\"gap\":2,\"te";
        assert_eq!(parse_reply(cut).unwrap().len(), 1);
        assert!(parse_reply("Sorry, I cannot help.").is_err());
    }

    #[test]
    fn words_are_counted_as_heard() {
        assert_eq!(words("The bride - beaming - waves."), 4);
        assert_eq!(words("  "), 0);
    }

    #[test]
    fn short_places_get_two_pictures() {
        assert_eq!(picture_times(&gap(0.0, 1.5)).len(), 2);
        assert_eq!(picture_times(&gap(0.0, 4.0)), vec![0.3, 2.0, 3.7]);
    }

    #[test]
    fn a_shortening_must_keep_the_subject() {
        assert!(same_subject(
            "The bride hugs her father",
            "Bride hugs father"
        ));
        assert!(!same_subject("The bride hugs her father", "A dog runs"));
    }

    #[test]
    fn the_checkpoint_is_kept_only_for_the_same_job() {
        let path = std::env::temp_dir().join(format!("vs-ck-{}.json", std::process::id()));
        let gaps = [gap(4.0, 5.5)];
        let print = fingerprint(&gaps, &brief(), "m");
        let mut c = Checkpoint::load(&path, print.clone());
        c.replies.push(Some(("{}".into(), 0.01)));
        c.save().unwrap();
        assert_eq!(Checkpoint::load(&path, print).replies.len(), 1);
        let other = fingerprint(&gaps, &brief(), "another model");
        assert!(Checkpoint::load(&path, other).replies.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn named_people_are_known_with_their_labels_and_looks() {
        let names = [
            ("the groom", "Sam"),
            ("a bald man in a brown suit", "Ben"),
            ("the bearded groom", "Sam"),
            ("the choir", ""),
        ]
        .map(|(l, n)| (l.to_string(), n.to_string()));
        let looks = [
            ("the groom", "dark beard, grey suit"),
            ("a bald man in a brown suit", "in his sixties, bald"),
        ]
        .map(|(l, k)| (l.to_string(), k.to_string()));
        let known = known_lines(&names, &looks);
        assert_eq!(
            known,
            "- Sam, labelled \"the groom\" and \"the bearded groom\": dark beard, grey suit\n\
             - Ben, labelled \"a bald man in a brown suit\": in his sixties, bald"
        );
        let mut b = brief();
        assert!(!system_prompt(&b).contains("matter most to the family"));
        b.known = known;
        assert!(system_prompt(&b).contains("matter most to the family"));
        assert_ne!(fingerprint(&[], &b, "m"), fingerprint(&[], &brief(), "m"));
        assert!(known_lines(&names[3..], &looks).is_empty());
    }

    #[test]
    fn every_look_noted_is_kept_once() {
        let mut looks = Vec::new();
        add_look(&mut looks, "the groom", "dark beard, grey suit.");
        add_look(&mut looks, "The groom", "in his thirties, tall");
        add_look(&mut looks, "the groom", "Dark beard, grey suit");
        add_look(&mut looks, "the groom", " ");
        assert_eq!(
            looks,
            vec![(
                "the groom".to_string(),
                "dark beard, grey suit. Also: in his thirties, tall".to_string()
            )]
        );
    }

    #[test]
    fn unnumbered_or_misnumbered_descriptions_are_placed() {
        let batch = [gap(10.0, 14.0), gap(20.0, 25.0), gap(30.0, 33.0)];
        let d = |place, t| Described {
            place,
            text: format!("at {t}"),
            seen_at: Some(t),
            setting: String::new(),
        };
        // A full set without numbers goes by order.
        let p = placed(vec![d(0, 1.0), d(0, 2.0), d(0, 3.0)], &batch, 37);
        assert_eq!(
            p.iter().map(|d| d.place).collect::<Vec<_>>(),
            vec![37, 38, 39]
        );
        // Numbered from 1, one too many: by the time seen at.
        let p = placed(
            vec![d(1, 11.0), d(2, 12.0), d(3, 21.0), d(4, 31.0)],
            &batch,
            37,
        );
        assert_eq!(
            p.iter().map(|d| d.place).collect::<Vec<_>>(),
            vec![37, 38, 39]
        );
        // Numbered right: kept as it is.
        let p = placed(vec![d(38, 99.0)], &batch, 37);
        assert_eq!(p[0].place, 38);
    }

    #[test]
    fn speech_is_asked_about_only_of_a_service_that_hears() {
        let items = |h| {
            reply_schema(h, false)["json_schema"]["schema"]["properties"]["descriptions"]["items"]
                .clone()
        };
        assert!(items(true)["properties"]["speech"].is_object());
        assert!(items(false)["properties"]["speech"].is_null());
        let reply = r#"{"descriptions": [{"gap": 3, "text": "She nods.", "seen_at": 1.0, "speech": true},
            {"gap": 4, "text": "A cake.", "seen_at": 2.0, "speech": false}], "people": []}"#;
        assert_eq!(heard_speech(reply), vec![3]);
        let mut b = brief();
        b.hears = true;
        assert!(system_prompt(&b).contains("\"speech\" to true"));
        assert!(!system_prompt(&brief()).contains("\"speech\""));
    }

    #[test]
    fn a_finished_editor_pass_is_kept() {
        let path = std::env::temp_dir().join(format!("vs-ed-{}.json", std::process::id()));
        let gaps = [gap(4.0, 5.5)];
        let print = fingerprint(&gaps, &brief(), "m");
        let mut c = Checkpoint::load(&path, print.clone());
        c.replies.push(Some(("{}".into(), 0.01)));
        c.save().unwrap();
        assert!(Checkpoint::load(&path, print.clone()).edited.is_none());
        c.edited = Some((vec![(1, "A dog runs.".into())], 1, 0));
        c.save().unwrap();
        let back = Checkpoint::load(&path, print);
        assert_eq!(
            back.edited,
            Some((vec![(1, "A dog runs.".to_string())], 1, 0))
        );
        assert_eq!(back.replies.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn place_marks_copied_by_the_editor_are_taken_out() {
        assert_eq!(
            unmarked("The group walks along the path. [place: the church]"),
            "The group walks along the path."
        );
        assert_eq!(
            unmarked("At the hall, [Place: hall] they dance."),
            "At the hall, they dance."
        );
        assert_eq!(unmarked("They dance. [place: the hall"), "They dance.");
    }

    #[test]
    fn labels_are_joined_by_person_but_checked() {
        let labels: Vec<String> = [
            "the ponytailed girl",
            "the dark-haired girl",
            "the girl",
            "the blond man",
            "the man",
            "the groom",
            "a bald guest on the phone",
            "the bride",
            "a woman in a patterned dress",
        ]
        .map(String::from)
        .to_vec();
        let drafts: Vec<String> = [
            "The ponytailed girl waves.",
            "The dark-haired girl runs.",
            "The girl falls.",
            "The ponytailed girl smiles.",
            "The blond man grins at the man.",
            "The groom laughs.",
            "A bald guest on the phone kisses the bride.",
            "A bald guest on the phone signs.",
        ]
        .map(String::from)
        .to_vec();
        let reply = r#"{"people": [
            {"use": "the girl", "labels": ["the girl", "the Dark-haired girl", "the ponytailed girl"]},
            {"use": "the groom", "labels": ["the groom", "a bald guest on the phone"]},
            {"use": "the blond man", "labels": ["the blond man", "the man"]},
            {"use": "the ghost", "labels": ["the ghost", "the snail"]},
            {"use": "the bride", "labels": ["the bride", "a woman in a patterned dress"]}
        ]}"#;
        let looks = [(
            "a woman in a patterned dress".to_string(),
            "blonde, holding the bride's train".to_string(),
        )];
        let map = parse_same(reply, &labels, &drafts, &looks);
        // A bare "the girl" gives way to her label used most; the two men are
        // in one description, so they stay apart; unknown labels are ignored.
        assert_eq!(
            map,
            vec![
                ("the girl".to_string(), "the ponytailed girl".to_string()),
                (
                    "the dark-haired girl".to_string(),
                    "the ponytailed girl".to_string()
                ),
                // A role is kept, though the other label is longer and used
                // more.
                (
                    "a bald guest on the phone".to_string(),
                    "the groom".to_string()
                ),
            ]
        );
        assert_eq!(
            relabelled(&labels, &map),
            vec![
                "the ponytailed girl",
                "the blond man",
                "the man",
                "the groom",
                // Her looks name the bride, so she is someone else.
                "the bride",
                "a woman in a patterned dress"
            ]
        );
    }

    #[test]
    fn faces_add_their_rule_and_their_tag_and_a_new_job() {
        let mut b = brief();
        b.faces = true;
        assert!(system_prompt(&b).contains("FACES."));
        assert!(!system_prompt(&b).contains("# Faces"));
        assert!(!system_prompt(&brief()).contains("FACES."));
        assert_ne!(fingerprint(&[], &b, "m"), fingerprint(&[], &brief(), "m"));
        let person = |f| {
            reply_schema(false, f)["json_schema"]["schema"]["properties"]["people"]["items"].clone()
        };
        assert!(person(true)["properties"]["tag"].is_object());
        assert!(person(false)["properties"]["tag"].is_null());
        let reply = r#"{"descriptions": [], "people": [
            {"name": "the bride", "new": "", "heard": false, "tag": "Person 2"},
            {"name": "a boy", "new": "", "heard": false, "tag": ""}]}"#;
        assert_eq!(parse_tags(reply), vec![("the bride".to_string(), 2)]);
    }

    #[test]
    fn a_tag_is_sure_only_when_one_label_alone_claims_it() {
        let cast = Cast {
            tags: vec![
                ("the old man".into(), 2),
                ("the young man".into(), 2),
                ("the bride".into(), 1),
                ("the groom".into(), 3),
                ("the groom".into(), 4),
            ],
            ..Cast::default()
        };
        assert_eq!(cast.sure_tags(), vec![("the bride".to_string(), 1)]);
    }
}
