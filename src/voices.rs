//! Edge's whole voice list, for the Language and voice window (PLAN.md
//! milestone 11, log entry 46): about 320 voices in over 100 languages,
//! fetched from Edge now and then and kept in
//! `%LOCALAPPDATA%\Video Scribe\voices.json`. Until it has been fetched, or
//! when Edge cannot be reached, the short list in `choices::VOICES` stands in.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::choices;

#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    /// Such as "fr-CA-SylvieNeural".
    pub id: String,
    /// "Sylvie".
    pub name: String,
    /// "French".
    pub language: String,
    /// "Canada"; empty when Edge names no region, as for Catalan.
    pub region: String,
    /// What a list says after the name: "Canada, woman".
    pub kind: String,
}

/// How long a fetched list is kept before it is fetched again.
const KEEP: Duration = Duration::from_secs(7 * 24 * 3600);

static LIST: Mutex<Option<Arc<Vec<Voice>>>> = Mutex::new(None);

fn cache_path() -> PathBuf {
    crate::ytdlp::local_dir().join("voices.json")
}

/// Every voice: Edge's list as last fetched, or the built-in one.
pub fn all() -> Arc<Vec<Voice>> {
    let mut list = LIST.lock().unwrap_or_else(|e| e.into_inner());
    list.get_or_insert_with(|| {
        let saved = std::fs::read_to_string(cache_path())
            .ok()
            .map(|j| parse(&j))
            .filter(|v| !v.is_empty());
        Arc::new(saved.unwrap_or_else(built_in))
    })
    .clone()
}

/// Fetches Edge's list again when the saved one is missing or a week old.
/// A failure keeps what there is: the reason, for the window to say.
pub fn refresh() -> Result<(), String> {
    let fresh = std::fs::metadata(cache_path())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < KEEP);
    if fresh && all().len() > choices::VOICES.len() {
        return Ok(());
    }
    let json = crate::edge::voices_json()?;
    let list = parse(&json);
    if list.is_empty() {
        return Err("Edge's voice list could not be read.".into());
    }
    let path = cache_path();
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, &json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
    *LIST.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(list));
    Ok(())
}

/// The voices in Edge's JSON list, with the built-in list's wording kept for
/// the voices it has ("Sonia (British, woman)", "Maisie (British, girl)").
pub fn parse(json: &str) -> Vec<Voice> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(json)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let text = |k: &str| {
            item.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let id = text("ShortName");
        let friendly = text("FriendlyName");
        if id.is_empty() {
            continue;
        }
        let (language, region) = language_and_region(
            friendly
                .split_once(" - ")
                .map_or(text("LocaleName").as_str(), |(_, l)| l),
        );
        let (name, several) = voice_name(&id);
        let kind = match choices::voice(&id) {
            Some(v) => v.kind.to_string(),
            None => {
                let who = match text("Gender").as_str() {
                    "Female" => "woman",
                    "Male" => "man",
                    _ => "",
                };
                [
                    region.as_str(),
                    who,
                    if several {
                        "speaks several languages"
                    } else {
                        ""
                    },
                ]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
            }
        };
        if language.is_empty() || out.iter().any(|v: &Voice| v.id == id) {
            continue;
        }
        out.push(Voice {
            id,
            name,
            language,
            region,
            kind,
        });
    }
    out
}

/// "French (Canada)" as ("French", "Canada"); "English (India) (Preview)" as
/// ("English", "India"); "Catalan" as ("Catalan", "").
fn language_and_region(text: &str) -> (String, String) {
    match text.split_once(" (") {
        Some((l, rest)) => (
            l.trim().to_string(),
            rest.split(')').next().unwrap_or("").trim().to_string(),
        ),
        None => (text.trim().to_string(), String::new()),
    }
}

/// "fr-FR-VivienneMultilingualNeural" as ("Vivienne", true).
fn voice_name(id: &str) -> (String, bool) {
    let last = id.rsplit('-').next().unwrap_or(id);
    let last = last.strip_suffix("Neural").unwrap_or(last);
    match last.strip_suffix("Multilingual") {
        Some(n) => (n.to_string(), true),
        None => (last.to_string(), false),
    }
}

fn built_in() -> Vec<Voice> {
    choices::VOICES
        .iter()
        .map(|v| Voice {
            id: v.id.into(),
            name: v.name.into(),
            language: v.language.into(),
            region: String::new(),
            kind: v.kind.into(),
        })
        .collect()
}

/// The languages, in alphabetical order.
pub fn languages(list: &[Voice]) -> Vec<String> {
    let mut out: Vec<String> = list.iter().map(|v| v.language.clone()).collect();
    out.sort();
    out.dedup();
    out
}

/// A language's voices, by region and then name.
pub fn of_language<'a>(list: &'a [Voice], language: &str) -> Vec<&'a Voice> {
    let mut out: Vec<&Voice> = list.iter().filter(|v| v.language == language).collect();
    out.sort_by(|a, b| (&a.region, &a.name).cmp(&(&b.region, &b.name)));
    out
}

pub fn find(id: &str) -> Option<Voice> {
    all().iter().find(|v| v.id == id).cloned()
}

/// "Sylvie (Canada, woman)"; the bare id for one Edge does not list.
pub fn label(id: &str) -> String {
    find(id).map_or_else(|| id.to_string(), |v| format!("{} ({})", v.name, v.kind))
}

/// The language a voice speaks, or None for one Edge does not list.
pub fn language_of(id: &str) -> Option<String> {
    find(id).map(|v| v.language)
}

/// The language code of a voice: "fr" for "fr-CA-SylvieNeural", "fil" for
/// Filipino.
pub fn code(id: &str) -> &str {
    id.split('-')
        .next()
        .filter(|c| !c.is_empty())
        .unwrap_or("en")
}

/// What a voice says as its sample, in its own language where there is a
/// sentence for it, else only its name, which every voice can say.
pub fn sample(v: &Voice) -> String {
    let n = &v.name;
    match code(&v.id) {
        "en" => format!("Hello, I'm {n}. A woman in a red coat walks to the door and opens it."),
        "fr" => format!(
            "Bonjour, je suis {n}. Une femme en manteau rouge marche vers la porte et l'ouvre."
        ),
        "af" => {
            format!("Hallo, ek is {n}. 'n Vrou in 'n rooi jas loop na die deur en maak dit oop.")
        }
        "de" => format!(
            "Hallo, ich bin {n}. Eine Frau in einem roten Mantel geht zur Tür und öffnet sie."
        ),
        "nl" => format!(
            "Hallo, ik ben {n}. Een vrouw in een rode jas loopt naar de deur en doet hem open."
        ),
        "es" => {
            format!("Hola, soy {n}. Una mujer con un abrigo rojo camina hacia la puerta y la abre.")
        }
        "it" => {
            format!("Ciao, sono {n}. Una donna con un cappotto rosso va verso la porta e la apre.")
        }
        "pt" => {
            format!("Olá, sou {n}. Uma mulher de casaco vermelho caminha até à porta e abre-a.")
        }
        _ => format!("{n}."),
    }
}

/// The language as the model is asked to write it: "British English" for a
/// British voice, "French (Canada)" for a Canadian one, "Afrikaans" when the
/// language has one region only.
pub fn writing_language(voice: &str, language: &str) -> String {
    match voice.get(..5) {
        Some("en-GB") => return "British English".into(),
        Some("en-US") => return "American English".into(),
        Some("pt-BR") => return "Brazilian Portuguese".into(),
        Some("pt-PT") => return "European Portuguese".into(),
        _ => {}
    }
    let list = all();
    let Some(v) = list
        .iter()
        .find(|v| v.id == voice && v.language == language)
    else {
        return language.to_string();
    };
    let regions = {
        let mut r: Vec<&str> = of_language(&list, language)
            .iter()
            .map(|v| v.region.as_str())
            .collect();
        r.dedup();
        r.len()
    };
    if regions > 1 && !v.region.is_empty() {
        format!("{language} ({})", v.region)
    } else {
        language.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"[
      {"ShortName":"af-ZA-AdriNeural","Gender":"Female","Locale":"af-ZA",
       "FriendlyName":"Microsoft Adri Online (Natural) - Afrikaans (South Africa)"},
      {"ShortName":"fr-CA-SylvieNeural","Gender":"Female",
       "FriendlyName":"Microsoft Sylvie Online (Natural) - French (Canada)"},
      {"ShortName":"fr-FR-RemyMultilingualNeural","Gender":"Male",
       "FriendlyName":"Microsoft RemyMultilingual Online (Natural) - French (France)"},
      {"ShortName":"en-GB-SoniaNeural","Gender":"Female",
       "FriendlyName":"Microsoft Sonia Online (Natural) - English (United Kingdom)"},
      {"ShortName":"en-IN-AaravNeural","Gender":"Male",
       "FriendlyName":"Microsoft Aarav Online (Natural) - English (India) (Preview)"},
      {"ShortName":"ca-ES-JoanaNeural","Gender":"Female",
       "FriendlyName":"Microsoft Joana Online (Natural) - Catalan"}
    ]"#;

    #[test]
    fn edges_list_is_read_into_languages_and_voices() {
        let list = parse(JSON);
        assert_eq!(list.len(), 6);
        assert_eq!(
            languages(&list),
            ["Afrikaans", "Catalan", "English", "French"]
        );
        let sylvie = &list[1];
        assert_eq!(
            (
                sylvie.name.as_str(),
                sylvie.language.as_str(),
                sylvie.kind.as_str()
            ),
            ("Sylvie", "French", "Canada, woman")
        );
        assert_eq!(list[2].name, "Remy");
        assert_eq!(list[2].kind, "France, man, speaks several languages");
        // The built-in wording stays for the voices it has.
        assert_eq!(list[3].kind, "British, woman");
        assert_eq!(list[4].region, "India");
        assert_eq!(
            (list[5].language.as_str(), list[5].kind.as_str()),
            ("Catalan", "woman")
        );
    }

    #[test]
    fn a_languages_voices_go_by_region_then_name() {
        let list = parse(JSON);
        let names: Vec<&str> = of_language(&list, "French")
            .iter()
            .map(|v| v.name.as_str())
            .collect();
        assert_eq!(names, ["Sylvie", "Remy"]);
    }

    #[test]
    fn bad_json_gives_no_voices() {
        assert!(parse("not json").is_empty());
        assert!(parse("{}").is_empty());
    }

    #[test]
    fn codes_and_samples() {
        assert_eq!(code("fil-PH-AngeloNeural"), "fil");
        let list = parse(JSON);
        assert!(sample(&list[0]).starts_with("Hallo, ek is Adri."));
        assert_eq!(sample(&list[5]), "Joana.");
    }

    #[test]
    fn every_built_in_voice_has_a_sample_in_its_language() {
        for v in built_in() {
            assert_ne!(sample(&v), format!("{}.", v.name), "{}", v.id);
        }
    }
}
