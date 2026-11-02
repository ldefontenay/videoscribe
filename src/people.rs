//! The people in one video (milestone 5, log entry 28): names the user gives
//! to the labels the descriptions used ("the bearded groom" = "Sam"), put
//! in on this computer before speaking, and a note on who is in the video,
//! written before describing and sent with the pictures.
//!
//! Kept in %LOCALAPPDATA%\Video Scribe\people, one plain text file per video,
//! so it can be read and corrected in Notepad. Names apply to that video only:
//! carrying them to other videos waits for face recognition (PLAN.md §5.7).

use crate::choices::Source;
use std::path::PathBuf;

/// The people in one video.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct People {
    /// Each label the descriptions used, and the name to say instead; an
    /// empty name keeps the label.
    pub names: Vec<(String, String)>,
    /// Who is in the video, in the user's words, sent with the pictures.
    pub who: String,
}

const NAMES: &str = "== Names ==";
const WHO: &str = "== Who is in this video ==";

/// The name a video's files go by: a file's name without its extension, or
/// a downloaded video's key (`ytdlp::key`), as the job saves them.
pub fn key(source: &Source) -> Option<String> {
    match source {
        Source::File(p) => p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty()),
        Source::Web(url) => crate::ytdlp::key(url),
    }
}

fn path(key: &str) -> PathBuf {
    crate::ytdlp::local_dir()
        .join("people")
        .join(format!("{key}.txt"))
}

pub fn load(key: &str) -> People {
    std::fs::read_to_string(path(key))
        .map(|t| parse(&t))
        .unwrap_or_default()
}

/// Written beside and renamed, so a half-written file never replaces a whole one.
pub fn save(key: &str, people: &People) -> Result<(), String> {
    let path = path(key);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let part = path.with_extension("txt.part");
    std::fs::write(&part, format!("\u{feff}{}", text(people)))
        .and_then(|_| std::fs::rename(&part, &path))
        .map_err(|e| format!("The names could not be saved in {}: {e}", path.display()))
}

fn text(people: &People) -> String {
    let mut out = vec![
        "# The people in one video, for Video Scribe. Safe to edit in Notepad while Video Scribe"
            .to_string(),
        "# is closed. Under Names, each line is a label the descriptions used, then = and the"
            .to_string(),
        "# name to say instead.".to_string(),
        String::new(),
        NAMES.to_string(),
    ];
    for (label, name) in &people.names {
        out.push(format!("{label} = {name}"));
    }
    out.push(String::new());
    out.push(WHO.to_string());
    out.extend(people.who.lines().map(str::to_string));
    out.join("\r\n") + "\r\n"
}

fn parse(text: &str) -> People {
    let mut people = People::default();
    let mut section = "";
    let mut who = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let t = line.trim();
        if t == NAMES || t == WHO {
            section = if t == NAMES { NAMES } else { WHO };
        } else if section == WHO {
            who.push(line.trim_end());
        } else if section == NAMES && !t.is_empty() && !t.starts_with('#') {
            if let Some((label, name)) = t.split_once('=') {
                if !label.trim().is_empty() {
                    people
                        .names
                        .push((label.trim().to_string(), name.trim().to_string()));
                }
            }
        }
    }
    people.who = who.join("\r\n").trim().to_string();
    people
}

/// The labels in `labels`, each with its name if one is given, then any
/// named label the descriptions no longer use: nothing typed is lost.
pub fn merged(people: &People, labels: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = labels
        .iter()
        .map(|l| {
            let name = people
                .names
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(l))
                .map(|(_, n)| n.clone())
                .unwrap_or_default();
            (l.clone(), name)
        })
        .collect();
    for (k, n) in &people.names {
        if !out.iter().any(|(l, _)| l.eq_ignore_ascii_case(k)) {
            out.push((k.clone(), n.clone()));
        }
    }
    out
}

/// `text` with each named label replaced by its name. The model and the
/// editor do not always repeat a label word for word, so "the groom" also
/// catches "The groom", "a groom", "her groom" and "the bearded groom": up to
/// two describing words between a determiner and the label are taken with
/// it. A label that ends in what someone wears or holds is also caught
/// without it, "a bald man in a brown suit" as "a bald man", when no other
/// label has that short form. Longer labels first, so "the bearded groom" is
/// not caught by "the groom".
pub fn apply(text: &str, names: &[(String, String)]) -> String {
    let labels: Vec<&str> = names.iter().map(|(l, _)| l.as_str()).collect();
    let mut named: Vec<(&str, &str)> = Vec::new();
    for (l, n) in names {
        let l = strip_determiner(l);
        if l.is_empty() || n.is_empty() {
            continue;
        }
        named.push((l, n));
        if let Some(short) = short_form(l, &labels) {
            named.push((short, n));
        }
    }
    named.sort_by_key(|(l, _)| std::cmp::Reverse(l.len()));
    let mut out = text.to_string();
    for (label, name) in named {
        out = replace_label(&out, label, name);
    }
    out
}

/// Words that start what someone wears or holds, at the end of a label: in
/// English, French and Afrikaans, the languages tested (log entry 47).
const TRAILING: &[&str] = &[
    "in", "with", "wearing", "holding", "carrying", // English
    "en", "avec", "portant", "tenant", "vêtu", "vêtue", // French
    "met", "wat", "dra", // Afrikaans
];

/// The label without what someone wears or holds ("bald man" for "bald man
/// in a brown suit"), when that leaves two words or more and no other label
/// in `labels` is, or shortens to, the same.
fn short_form<'a>(label: &'a str, labels: &[&str]) -> Option<&'a str> {
    fn cut(l: &str) -> Option<&str> {
        let ws: Vec<&str> = l.split_whitespace().collect();
        let at = ws
            .iter()
            .position(|w| TRAILING.contains(&w.to_lowercase().as_str()))?;
        if at < 2 {
            return None;
        }
        let end = l.find(&format!(" {} ", ws[at]))?;
        Some(l[..end].trim())
    }
    let short = cut(label)?;
    let clash = labels.iter().map(|o| strip_determiner(o)).any(|o| {
        !o.eq_ignore_ascii_case(label)
            && (o.eq_ignore_ascii_case(short)
                || cut(o).is_some_and(|c| c.eq_ignore_ascii_case(short)))
    });
    (!clash).then_some(short)
}

/// Words that may come before a label and go with it, in English, French
/// and Afrikaans together: a description is in one language, and none of
/// these is a word that could come before a label in another. French "l'"
/// and Afrikaans "'n" are the words "l" and "n" beside an apostrophe.
const DETERMINERS: &[&str] = &[
    "the", "a", "an", "her", "his", "their", // English
    "le", "la", "les", "l", "un", "une", "son", "sa", "ses", "leur", "ce", "cet", "cette", "du",
    "au", // French; "du" and "au" keep their "de" and "à"
    "die", "n", "haar", "sy", "hul", // Afrikaans
];
/// Words that end the describing words before a label: "the man beside the
/// groom" keeps "the man beside".
const STOP: &[&str] = &[
    "and",
    "or",
    "but",
    "of",
    "with",
    "beside",
    "behind",
    "near",
    "at",
    "in",
    "on",
    "to",
    "from",
    "by",
    "while",
    "as",
    "than",
    "then",
    "who",
    "that",
    "which",
    "is",
    "are",
    "was",
    "were",
    // French
    "et",
    "ou",
    "mais",
    "de",
    "avec",
    "à",
    "près",
    "derrière",
    "dans",
    "sur",
    "par",
    "pendant",
    "comme",
    "que",
    "qui",
    "est",
    "sont",
    "était",
    // Afrikaans
    "en",
    "of",
    "maar",
    "met",
    "langs",
    "agter",
    "naby",
    "by",
    "op",
    "na",
    "van",
    "deur",
    "terwyl",
    "as",
    "wat",
    "is",
    "was",
];

/// What stays of a determiner that also holds a preposition, when a name
/// takes its place: French "du marié" becomes "de Marc", "au marié" "à Marc".
fn kept_of(determiner: &str) -> &'static str {
    match determiner {
        "du" => "de ",
        "au" => "à ",
        _ => "",
    }
}

fn apostrophe(c: char) -> bool {
    c == '\'' || c == '\u{2019}'
}

/// Whether `text` mentions `label`, in any of the ways `apply` would catch.
pub fn mentions(text: &str, label: &str) -> bool {
    !strip_determiner(label).is_empty()
        && apply(text, &[(label.to_string(), "\u{1}".to_string())]) != text
}

fn strip_determiner(label: &str) -> &str {
    let label = label.trim();
    // French "l'homme", Afrikaans "'n man".
    for elided in ["l'", "l\u{2019}", "L'", "L\u{2019}", "'n ", "\u{2019}n "] {
        if let Some(rest) = label.strip_prefix(elided) {
            return rest.trim();
        }
    }
    match label.split_once(' ') {
        Some((first, rest)) if DETERMINERS.contains(&first.to_lowercase().as_str()) => rest.trim(),
        _ => label,
    }
}

/// French elision where a name meets "de" or "d'": "de Anne" is "d'Anne",
/// and "d'Marc" is "de Marc". `before` is what is already written.
fn elide(before: &mut String, name: &str) {
    let vowel = name
        .chars()
        .next()
        .is_some_and(|c| "aeiouyAEIOUYéÉèÈêÊàÀâÂîÎôÔûÛ".contains(c));
    let word_start = |s: &str, n: usize| {
        s.len() == n
            || s[..s.len() - n]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_alphanumeric())
    };
    if vowel {
        for (de, d) in [("de ", "d'"), ("De ", "D'")] {
            if before.ends_with(de) && word_start(before, de.len()) {
                before.truncate(before.len() - de.len());
                before.push_str(d);
                return;
            }
        }
    } else {
        for (d, de) in [
            ("d'", "de "),
            ("d\u{2019}", "de "),
            ("D'", "De "),
            ("D\u{2019}", "De "),
        ] {
            if before.ends_with(d) && word_start(before, d.len()) {
                before.truncate(before.len() - d.len());
                before.push_str(de);
                return;
            }
        }
    }
}

/// The words of `text`, as byte ranges: letters, digits, and hyphens inside a
/// word, so "lace-sleeved" is one word and "bride's" is "bride" then "s".
fn words(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (k, &(i, c)) in chars.iter().enumerate() {
        let inner_hyphen = c == '-'
            && start.is_some()
            && chars.get(k + 1).is_some_and(|&(_, n)| n.is_alphanumeric());
        if c.is_alphanumeric() || inner_hyphen {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            out.push((s, i));
        }
    }
    if let Some(s) = start {
        out.push((s, text.len()));
    }
    out
}

fn replace_label(text: &str, label: &str, name: &str) -> String {
    let want: Vec<String> = words(label)
        .into_iter()
        .map(|(a, b)| label[a..b].to_lowercase())
        .collect();
    if want.is_empty() {
        return text.to_string();
    }
    let ws = words(text);
    let word = |k: usize| text[ws[k].0..ws[k].1].to_lowercase();
    // Only spaces between two words, or French "l'" and "d'": no comma or
    // full stop.
    let joined = |a: usize, b: usize| {
        let between = &text[ws[a].1..ws[b].0];
        between.chars().all(|c| c == ' ')
            || (between.chars().count() == 1 && between.chars().all(apostrophe))
    };
    let mut out = String::new();
    let mut copied = 0;
    let mut k = 0;
    while k + want.len() <= ws.len() {
        let hit =
            (0..want.len()).all(|n| word(k + n) == want[n] && (n == 0 || joined(k + n - 1, k + n)));
        if !hit {
            k += 1;
            continue;
        }
        let last = k + want.len() - 1;
        // Back over at most two describing words to a determiner.
        let mut from = k;
        let mut j = k;
        for _ in 0..3 {
            if j == 0 || !joined(j - 1, j) {
                break;
            }
            let w = word(j - 1);
            if DETERMINERS.contains(&w.as_str()) {
                from = j - 1;
                break;
            }
            if STOP.contains(&w.as_str()) || j < k.saturating_sub(1) {
                break;
            }
            j -= 1;
        }
        if ws[from].0 < copied {
            from = k;
        }
        // Afrikaans "'n": the apostrophe goes with it.
        let mut start = ws[from].0;
        if from < k && word(from) == "n" {
            if let Some(c) = text[copied..start]
                .chars()
                .next_back()
                .filter(|c| apostrophe(*c))
            {
                start -= c.len_utf8();
            }
        }
        out.push_str(&text[copied..start]);
        // A label put in for another starts a sentence as that one did.
        let capital = text[start..].starts_with(char::is_uppercase);
        let mut put = String::new();
        if from < k {
            put.push_str(kept_of(&word(from)));
        }
        put.push_str(name);
        elide(&mut out, &put);
        let mut first = put.chars();
        match first.next() {
            Some(c) if capital => {
                out.extend(c.to_uppercase());
                out.push_str(first.as_str());
            }
            _ => out.push_str(&put),
        }
        copied = ws[last].1;
        k = last + 1;
    }
    out.push_str(&text[copied..]);
    out
}

/// Whether `path` is inside the people folder (a guard for tests).
#[cfg(test)]
fn inside(path: &std::path::Path) -> bool {
    path.starts_with(crate::ytdlp::local_dir().join("people"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn names_replace_labels_in_every_form() {
        let names = n(&[("the bearded groom", "Sam"), ("the bride", "Alex")]);
        assert_eq!(
            apply("The bearded groom smiles at the bride.", &names),
            "Sam smiles at Alex."
        );
        assert_eq!(
            apply("A bearded groom waits; the bride's veil lifts.", &names),
            "Sam waits; Alex's veil lifts."
        );
        // Whole words only, and a label with no name stays.
        assert_eq!(
            apply("The brides and the groom.", &names),
            "The brides and the groom."
        );
    }

    #[test]
    fn a_label_put_in_starts_a_sentence_with_a_capital() {
        let to = n(&[("the dark-haired girl", "the ponytailed girl")]);
        assert_eq!(
            apply(
                "The dark-haired girl runs. She sees the dark-haired girl.",
                &to
            ),
            "The ponytailed girl runs. She sees the ponytailed girl."
        );
    }

    #[test]
    fn describing_words_before_a_label_go_with_it() {
        let names = n(&[("the groom", "Sam"), ("the bride", "Alex")]);
        assert_eq!(
            apply(
                "The bearded groom holds the lace-sleeved bride's hand.",
                &names
            ),
            "Sam holds Alex's hand."
        );
        assert_eq!(
            apply("She smiles at her groom.", &names),
            "She smiles at Sam."
        );
        // But not past a word like "beside", nor across a comma.
        assert_eq!(
            apply("The man beside the groom waves.", &names),
            "The man beside Sam waves."
        );
        assert_eq!(
            apply("Guests, groom and all.", &names),
            "Guests, Sam and all."
        );
        // Nor three describing words: then only the label goes.
        assert_eq!(
            apply("The tall dark bearded groom.", &names),
            "The tall dark bearded Sam."
        );
    }

    #[test]
    fn a_label_is_caught_without_what_they_wear() {
        let names = n(&[
            ("a bald man in a brown suit", "Ben"),
            ("an older woman in grey", ""),
            ("the girl with a ponytail", "Ann"),
        ]);
        assert_eq!(
            apply("Kate walks on a bald man's arm. A bald man reads.", &names),
            "Kate walks on Ben's arm. Ben reads."
        );
        assert_eq!(
            apply("A bald man in a brown suit claps.", &names),
            "Ben claps."
        );
        // One word left is too little to be sure: "the girl".
        assert_eq!(apply("The girl runs.", &names), "The girl runs.");
        assert!(mentions("A bald man reads.", "a bald man in a brown suit"));
        // Not when another label has the same short form.
        let two = n(&[
            ("a bald man in a brown suit", "Ben"),
            ("a bald man in a blue suit", "Tom"),
        ]);
        assert_eq!(apply("A bald man reads.", &two), "A bald man reads.");
        let also = n(&[("a bald man in a brown suit", "Ben"), ("the bald man", "")]);
        assert_eq!(apply("A bald man reads.", &also), "A bald man reads.");
    }

    #[test]
    fn french_labels_take_their_articles_with_them() {
        let names = n(&[
            ("la femme aux cheveux gris", "Anne"),
            ("l'homme barbu", "Marc"),
            ("le marié", "Paul"),
        ]);
        assert_eq!(
            apply("La femme aux cheveux gris sourit à l'homme barbu.", &names),
            "Anne sourit à Marc."
        );
        assert_eq!(
            apply(
                "L'homme barbu prend la main de la femme aux cheveux gris.",
                &names
            ),
            "Marc prend la main d'Anne."
        );
        // "du" and "au" keep their "de" and "à".
        assert_eq!(
            apply("La cravate du marié; elle parle au marié.", &names),
            "La cravate de Paul; elle parle à Paul."
        );
        assert_eq!(
            apply("Le bouquet d'un homme barbu.", &names),
            "Le bouquet de Marc."
        );
        // A describing word before the noun goes too.
        assert_eq!(apply("Le jeune marié rit.", &names), "Paul rit.");
    }

    #[test]
    fn afrikaans_labels_take_their_articles_with_them() {
        let names = n(&[("die bruid", "Marie"), ("'n grysharige vrou", "Anne")]);
        assert_eq!(
            apply(
                "Die bruid lag. 'n Grysharige vrou soen die bruid se wang.",
                &names
            ),
            "Marie lag. Anne soen Marie se wang."
        );
        assert_eq!(
            apply("Sy kyk na die mooi bruid.", &names),
            "Sy kyk na Marie."
        );
    }

    #[test]
    fn the_longer_label_wins() {
        let names = n(&[("the groom", "X"), ("the bearded groom", "Sam")]);
        assert_eq!(
            apply("The bearded groom and the groom.", &names),
            "Sam and X."
        );
    }

    #[test]
    fn the_file_reads_back_what_it_wrote() {
        let p = People {
            names: n(&[("the bride", "Alex"), ("the celebrant", "")]),
            who: "The groom has a beard.\r\nThe celebrant wears a white robe.".into(),
        };
        assert_eq!(parse(&text(&p)), p);
        // Hand edits: spaces, comments and a line with no name.
        let t = "# note\r\n== Names ==\r\n  the bride =Alex \r\nno equals sign\r\n\r\n== Who is in this video ==\r\nHello\r\n";
        let back = parse(t);
        assert_eq!(back.names, n(&[("the bride", "Alex")]));
        assert_eq!(back.who, "Hello");
    }

    #[test]
    fn names_typed_before_are_kept_when_labels_change() {
        let p = People {
            names: n(&[("the bride", "Alex"), ("the old label", "Anne")]),
            who: String::new(),
        };
        let m = merged(&p, &["the groom".into(), "The bride".into()]);
        assert_eq!(
            m,
            n(&[
                ("the groom", ""),
                ("The bride", "Alex"),
                ("the old label", "Anne")
            ])
        );
    }

    #[test]
    fn a_video_is_known_by_its_file_name_or_youtube_id() {
        let f = Source::File(PathBuf::from(r"C:\Videos\wedding.mp4"));
        assert_eq!(key(&f).as_deref(), Some("wedding"));
        let y = Source::Web("https://www.youtube.com/watch?v=_cMxraX_5RE".into());
        assert_eq!(key(&y).as_deref(), Some("_cMxraX_5RE"));
        assert!(inside(&path("wedding")));
    }
}
