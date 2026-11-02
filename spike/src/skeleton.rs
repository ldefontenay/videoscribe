//! Milestone 1, the walking skeleton, doubling as the bake-off's renderer (PLAN.md §8, spike 0(h)).
//!
//!   skeleton <video> <start_s> <end_s> <service> <out_dir> [captions.vtt] [--family]
//!
//! One clip, end to end: speech map (Silero, plus captions when given) -> gaps -> descriptions from one
//! service -> Windows speech -> schedule -> duck and mix -> MP3, plus a plain-text list of what was said
//! when, and what was dropped and why. Rough on purpose; thrown away after milestone 1.
//!
//! Services: gemini-lite and gemini-flash get the clip as video (picture and sound) through OpenRouter;
//! glm and gemma-free get three still frames per gap through OpenRouter; claude gets the same frames
//! through Claude Code on the user's subscription.

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Instant;

use serde_json::{json, Value};

use crate::{gaps, mf, net, sapi, vad, vtt};

const PROMPT: &str = include_str!("../prompts/describe.v3.txt");
const MIX_HZ: u32 = 48_000;
/// Leave this much air after speech ends and before it starts again. The reference description sits
/// 0 to 0.1 s from speech (log entry 15).
const MARGIN: f64 = 0.1;
const MIN_GAP: f64 = 1.0;
/// Long silences are split so a description comes about every five seconds, as in the reference.
const MAX_PART: f64 = 5.0;
/// Film level under a description: about -12 dB.
const DUCK: f32 = 0.25;
/// Description loudness, as RMS of full scale: about -16 dBFS.
const VOICE_RMS: f32 = 0.16;
const FRAMES_PER_REQUEST_GAPS: usize = 8;

#[derive(Clone, Debug)]
pub struct Gap {
    pub n: usize,
    pub start: f64,
    pub end: f64,
    pub words: usize,
    pub part: Option<(usize, usize)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Route {
    Video,
    Frames,
}

struct Service {
    name: &'static str,
    model: &'static str,
    route: Route,
    claude: bool,
}

const SERVICES: [Service; 6] = [
    Service {
        name: "gemini-lite",
        model: "google/gemini-3.5-flash-lite",
        route: Route::Video,
        claude: false,
    },
    Service {
        name: "gemini-flash",
        model: "google/gemini-3.8-flash",
        route: Route::Video,
        claude: false,
    },
    Service {
        name: "glm",
        model: "z-ai/glm-5.3-flash",
        route: Route::Frames,
        claude: false,
    },
    Service {
        name: "gemma-free",
        model: "google/gemma-4-31b-it:free",
        route: Route::Frames,
        claude: false,
    },
    Service {
        name: "gemma",
        model: "google/gemma-4-31b-it",
        route: Route::Frames,
        claude: false,
    },
    Service {
        name: "claude",
        model: "sonnet",
        route: Route::Frames,
        claude: true,
    },
];

/// Word limit for a gap (prompt VALUES): floor((seconds - 0.4) * 2.5), at least 1. Edge voices speak
/// 3.0 to 3.5 words a second (log entry 15) but pause at full stops, and may be sped up to fit.
pub fn word_limit(secs: f64) -> usize {
    // The small nudge stops 1.4 s landing on 2.4999... words in floating point.
    (((secs - 0.4) * 2.5 + 1e-9).floor() as i64).max(1) as usize
}

/// A tuning knob for the spike, from the environment.
fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Silences inside [start, end] at least MIN_GAP long, long ones split into equal parts of at most
/// MAX_PART seconds. Times stay on the video's clock.
pub fn plan_gaps(speech: &gaps::Spans, secs: f64, start: f64, end: f64) -> Vec<Gap> {
    let mut out = Vec::new();
    for (s, e) in gaps::invert(speech, secs) {
        let (s, e) = (s.max(start), e.min(end));
        if e - s < env_f64("SPIKE_MIN_GAP", MIN_GAP) {
            continue;
        }
        let parts = ((e - s) / env_f64("SPIKE_MAX_PART", MAX_PART))
            .ceil()
            .max(1.0) as usize;
        let len = (e - s) / parts as f64;
        for p in 0..parts {
            let (a, b) = (s + p as f64 * len, s + (p + 1) as f64 * len);
            out.push(Gap {
                n: out.len() + 1,
                start: a,
                end: b,
                words: word_limit(b - a),
                part: (parts > 1).then_some((p + 1, parts)),
            });
        }
    }
    out
}

/// The prompt file split into its `=== NAME ===` sections, notes (`#` lines) removed.
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

fn system_prompt(family: bool, hears: bool) -> String {
    let s = section("SYSTEM")
        .replace(
            "{heard_names_rule}",
            &if hears {
                section("HEARD NAMES RULE")
            } else {
                String::new()
            },
        )
        .replace(
            "{family_rules}",
            &if family {
                section("FAMILY RULES")
            } else {
                String::new()
            },
        )
        .replace(
            "{youtube_rules}",
            &if family {
                String::new()
            } else {
                section("YOUTUBE RULES")
            },
        );
    tidy(&s)
}

/// Experiment (log entry 14): Gemini labels video moments as MM:SS, so try giving it times that way.
fn mmss() -> bool {
    std::env::var_os("SPIKE_MMSS").is_some()
}

fn stamp(t: f64) -> String {
    if mmss() {
        gaps::clock(t)
    } else {
        format!("{t:.1}")
    }
}

fn gap_line(g: &Gap, offset: f64) -> String {
    let mut s = format!(
        "{}: {} to {}, up to {} words",
        g.n,
        stamp(g.start - offset),
        stamp(g.end - offset),
        g.words
    );
    if let Some((p, n)) = g.part {
        s.push_str(&format!(", part {p} of {n}"));
    }
    s
}

/// What a request knows besides its gaps.
#[derive(Default)]
struct Story<'a> {
    first: bool,
    previous: &'a [(f64, String)],
    characters: &'a [String],
    /// Caption cues (start, end, text) on the video's clock.
    cues: &'a [(f64, f64, String)],
}

fn request_prompt(gaps: &[Gap], offset: f64, story: &Story, evidence: &str) -> String {
    let prev = if story.previous.is_empty() {
        String::new()
    } else {
        let mut s = "Already said:".to_string();
        for (t, text) in story.previous.iter().rev().take(6).rev() {
            s.push_str(&format!("\n- {:.1}: {text}", t - offset));
        }
        s
    };
    let characters = if story.characters.is_empty() {
        String::new()
    } else {
        format!(
            "Characters so far (use these labels exactly):\n{}",
            story
                .characters
                .iter()
                .map(|c| format!("- {c}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let dialogue = match (gaps.first(), gaps.last()) {
        (Some(a), Some(b)) if !story.cues.is_empty() => {
            let lines: Vec<String> = story
                .cues
                .iter()
                .filter(|(s, e, _)| *e > a.start - 8.0 && *s < b.end + 8.0)
                .map(|(s, e, t)| format!("- {:.1} to {:.1}: {t}", s - offset, e - offset))
                .collect();
            if lines.is_empty() {
                String::new()
            } else {
                format!(
                    "What is said around these gaps, for your understanding only; never repeat it:\n{}",
                    lines.join("\n")
                )
            }
        }
        _ => String::new(),
    };
    let context = if story.first {
        "The listener has heard nothing yet: this is the start."
    } else {
        ""
    };
    let lines: Vec<String> = gaps.iter().map(|g| gap_line(g, offset)).collect();
    tidy(
        &section("REQUEST")
            .replace("{language}", "British English")
            .replace(
                "{style}",
                "Vivid and specific: name the action, the object and who does it. Fill each gap.",
            )
            .replace("{context}", context)
            .replace("{known_people}", "")
            .replace("{characters}", &characters)
            .replace("{dialogue}", &dialogue)
            .replace("{previous}", &prev)
            .replace("{gaps}", &lines.join("\n"))
            .replace("{evidence_note}", evidence),
    )
}

#[derive(Debug, Clone)]
pub struct Description {
    pub gap: usize,
    pub text: String,
    pub seen_at: Option<f64>,
}

/// The JSON object in a reply, tolerating code fences and chatter around it.
pub fn parse_reply(text: &str) -> Result<Vec<Description>, String> {
    let list = match first_object(text) {
        Some(v) => v["descriptions"]
            .as_array()
            .ok_or("no \"descriptions\" list")?
            .clone(),
        // A reply cut off before its end (Gemma, log entry 15): keep every complete description.
        None => {
            let found: Vec<Value> = text
                .match_indices('{')
                .filter_map(|(i, _)| {
                    serde_json::Deserializer::from_str(&text[i..])
                        .into_iter::<Value>()
                        .next()?
                        .ok()
                })
                .filter(|v| v.get("gap").is_some())
                .collect();
            if found.is_empty() {
                return Err(format!(
                    "no JSON in the reply: {}",
                    text.chars().take(200).collect::<String>()
                ));
            }
            eprintln!(
                "  reply was cut off; kept {} complete descriptions",
                found.len()
            );
            found
        }
    };
    Ok(list
        .iter()
        .filter_map(|d| {
            Some(Description {
                gap: d["gap"].as_u64()? as usize,
                text: d["text"].as_str().unwrap_or("").trim().to_string(),
                seen_at: d["seen_at"].as_f64().or_else(|| {
                    // "01:05.2" when the experiment asks for minutes and seconds.
                    let t = d["seen_at"].as_str()?;
                    let (m, sec) = t.split_once(':')?;
                    Some(m.trim().parse::<f64>().ok()? * 60.0 + sec.trim().parse::<f64>().ok()?)
                }),
            })
        })
        .collect())
}

/// The first JSON object in a reply, skipping code fences and chatter, and ignoring anything after it
/// (Claude once sent a second object, log entry 15).
fn first_object(text: &str) -> Option<Value> {
    let a = text.find('{')?;
    serde_json::Deserializer::from_str(&text[a..])
        .into_iter::<Value>()
        .next()?
        .ok()
}

/// The names and labels in a reply's "people" list.
pub fn parse_people(text: &str) -> Vec<String> {
    let Some(v) = first_object(text) else {
        return Vec::new();
    };
    v["people"]
        .as_array()
        .map(|l| {
            l.iter()
                .filter_map(|p| p["name"].as_str().map(|n| n.trim().to_string()))
                .filter(|n| !n.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub struct Reply {
    pub text: String,
    pub cost: f64,
}

/// The reply shape for description requests, as an OpenRouter structured-output schema: GLM once
/// answered with "g" for "gap" (log entry 15).
fn reply_schema() -> Value {
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
                        "properties": {
                            "gap": { "type": "integer" },
                            "text": { "type": "string" },
                            "seen_at": { "type": "number" }
                        },
                        "required": ["gap", "text", "seen_at"],
                        "additionalProperties": false
                    }},
                    "people": { "type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "new": { "type": "string" },
                            "heard": { "type": "boolean" }
                        },
                        "required": ["name", "new", "heard"],
                        "additionalProperties": false
                    }}
                },
                "required": ["descriptions", "people"],
                "additionalProperties": false
            }
        }
    })
}

pub fn openrouter(
    model: &str,
    system: &str,
    content: Vec<Value>,
    schema: Option<Value>,
) -> Result<Reply, String> {
    let key = net::read_key(net::OPENROUTER).ok_or("no OpenRouter key in Credential Manager")?;
    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": content }
        ],
        "max_tokens": 4000,
        "reasoning": { "effort": "low", "exclude": true },
        "provider": { "data_collection": "deny" },
        "usage": { "include": true }
    });
    let mut body = body;
    if let Some(schema) = schema {
        body["response_format"] = schema;
    }
    let body = body.to_string();
    let headers = vec![
        format!("Authorization: Bearer {key}"),
        "Content-Type: application/json".to_string(),
        "X-Title: Video Scribe spike".to_string(),
    ];
    for wait in [0u64, 10, 30] {
        std::thread::sleep(std::time::Duration::from_secs(wait));
        let (status, text) = net::post(
            "openrouter.ai",
            "/api/v1/chat/completions",
            &headers,
            &body,
            600_000,
        )?;
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status == 429 || status >= 500 {
            eprintln!(
                "  OpenRouter {status}, retrying: {}",
                v["error"]["message"].as_str().unwrap_or("")
            );
            continue;
        }
        if status != 200 {
            return Err(format!(
                "OpenRouter {status}: {}",
                text.chars().take(400).collect::<String>()
            ));
        }
        let reply = v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if reply.trim().is_empty() {
            // Seen once from Gemma on CoreWeave (log entry 15): whitespace and nothing else.
            eprintln!("  OpenRouter gave an empty reply, retrying");
            continue;
        }
        return Ok(Reply {
            text: reply,
            cost: v["usage"]["cost"].as_f64().unwrap_or(0.0),
        });
    }
    Err("OpenRouter stayed busy".into())
}

fn find_claude() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in ["claude.exe", "claude.cmd"] {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let p = std::path::PathBuf::from(std::env::var_os("USERPROFILE")?)
        .join(".local")
        .join("bin")
        .join("claude.exe");
    p.is_file().then_some(p)
}

/// Claude Code on the subscription, run as Photo Scribe runs it: no tools, hooks, MCP or session,
/// an empty working folder, the system prompt in a file, content on stdin.
fn claude(
    model: &str,
    system: &str,
    content: Vec<Value>,
    work: &std::path::Path,
) -> Result<Reply, String> {
    let exe = find_claude().ok_or("Claude Code not found")?;
    let dir = work.join("claude-empty");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let sys = dir.join("system_prompt.txt");
    std::fs::write(&sys, system).map_err(|e| e.to_string())?;
    let blocks: Vec<Value> = content
        .into_iter()
        .map(|c| {
            if c["type"] == "image_url" {
                let url = c["image_url"]["url"].as_str().unwrap_or("");
                let data = url.split_once(",").map(|x| x.1).unwrap_or("");
                json!({ "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": data } })
            } else {
                c
            }
        })
        .collect();
    let message = json!({ "type": "user", "message": { "role": "user", "content": blocks } });
    let mut cmd = Command::new(exe);
    for v in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_SSE_PORT",
    ] {
        cmd.env_remove(v);
    }
    let mut child = cmd
        .args(["-p", "--model", model, "--system-prompt-file"])
        .arg(&sys)
        .args([
            "--settings",
            r#"{"disableAllHooks": true}"#,
            "--setting-sources",
            "project,local",
            "--tools",
            "",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--no-session-persistence",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|e| format!("could not start Claude Code: {e}"))?;
    let mut stdin = child.stdin.take().unwrap();
    let payload = format!("{message}\n");
    std::thread::spawn(move || {
        let _ = stdin.write_all(payload.as_bytes());
    });
    let stderr = child.stderr.take().unwrap();
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut s);
        s
    });
    let mut result = None;
    for line in BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map_while(Result::ok)
    {
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            if v["type"] == "result" {
                result = Some(v);
            }
        }
    }
    let _ = child.wait();
    let errtext = err.join().unwrap_or_default();
    let v = result.ok_or_else(|| {
        format!(
            "Claude Code gave no answer: {}",
            errtext.chars().take(300).collect::<String>()
        )
    })?;
    let text = v["result"].as_str().unwrap_or("").to_string();
    if v["is_error"] == true || text.is_empty() {
        return Err(format!("Claude said: {text}"));
    }
    Ok(Reply { text, cost: 0.0 })
}

fn ask(
    svc: &Service,
    system: &str,
    content: Vec<Value>,
    work: &std::path::Path,
    structured: bool,
) -> Result<Reply, String> {
    if svc.claude {
        claude(svc.model, system, content, work)
    } else {
        openrouter(svc.model, system, content, structured.then(reply_schema))
    }
}

/// Frame times for a gap: just inside each end and the middle, two for a short gap.
fn frame_times(g: &Gap) -> Vec<f64> {
    let len = g.end - g.start;
    if len < 2.0 {
        vec![g.start + len * 0.3, g.start + len * 0.7]
    } else {
        vec![g.start + 0.3, (g.start + g.end) / 2.0, g.end - 0.3]
    }
}

struct Placed {
    gap: usize,
    text: String,
    at: f64,
    samples: Vec<i16>,
}

/// Whether a shortened description still shares a word of four letters or more with the original: a
/// guard against a reply filed under the wrong gap number (log entry 15).
fn same_subject(original: &str, short: &str) -> bool {
    let words = |t: &str| -> Vec<String> {
        t.split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() >= 4)
            .map(|w| w.to_lowercase())
            .collect()
    };
    let a = words(original);
    words(short).iter().any(|w| a.contains(w))
}

/// Scale a description to VOICE_RMS, without letting its peaks clip.
fn level(samples: &[i16]) -> Vec<i16> {
    let n = samples.len().max(1) as f64;
    let rms = (samples.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / n).sqrt() as f32;
    let peak = samples.iter().map(|&s| (s as i32).abs()).max().unwrap_or(0) as f32;
    if rms < 1.0 {
        return samples.to_vec();
    }
    let k = (VOICE_RMS * 32767.0 / rms).min(32000.0 / peak.max(1.0));
    samples.iter().map(|&s| (s as f32 * k) as i16).collect()
}

fn trim(samples: &[i16], hz: u32) -> Vec<i16> {
    let (lead, trail) = sapi::silence(samples, hz);
    let a = ((lead - 0.03).max(0.0) * hz as f64) as usize;
    let b = samples
        .len()
        .saturating_sub(((trail - 0.05).max(0.0) * hz as f64) as usize);
    samples[a.min(b)..b].to_vec()
}

/// Rule checks from rubric.txt part 2 that can be done by machine.
fn rule_failures(d: &Description, g: &Gap, offset: f64) -> Vec<String> {
    let mut out = Vec::new();
    let lower = format!(" {} ", d.text.to_lowercase());
    for w in ["we see", "the camera", "in this scene", "the video shows"] {
        if lower.contains(w) {
            out.push(format!("narrator \"{w}\""));
        }
    }
    for w in [
        " appears",
        " seems",
        " possibly",
        " likely",
        " perhaps",
        " maybe",
    ] {
        if lower.contains(w) {
            out.push(format!("hedging \"{}\"", w.trim()));
        }
    }
    let words = sapi::words(&d.text);
    if words > g.words {
        out.push(format!("over budget, {words} words for {}", g.words));
    }
    match d.seen_at {
        None => out.push("no seen_at".into()),
        Some(t) if t + offset < g.start - 0.5 || t + offset > g.end + 0.5 => {
            out.push(format!("seen_at {:.1} outside its gap", t))
        }
        _ => {}
    }
    out
}

pub fn run(args: &[&str], family: bool) -> Result<(), String> {
    let (video, start, end, service, out_dir, captions) = match args {
        [v, s, e, svc, o, rest @ ..] => (
            *v,
            s.parse::<f64>().map_err(|_| "start")?,
            e.parse::<f64>().map_err(|_| "end")?,
            *svc,
            *o,
            rest.first().copied(),
        ),
        _ => return Err("usage: spike skeleton <video> <start_s> <end_s> <service> <out_dir> [captions.vtt] [--family]".into()),
    };
    let svc = SERVICES.iter().find(|s| s.name == service).ok_or_else(|| {
        format!(
            "service is one of: {}",
            SERVICES
                .iter()
                .map(|s| s.name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let work = std::path::PathBuf::from(out_dir);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    mf::startup().map_err(|e| e.to_string())?;
    let t0 = Instant::now();

    // 1. Speech map, on this machine.
    let wav16 = work.join("speech16.wav");
    if !wav16.exists() {
        let pcm = mf::decode(video, 16_000, 1).map_err(|e| e.to_string())?;
        sapi::write_wav(wav16.to_str().unwrap(), &pcm.samples, 16_000)
            .map_err(|e| e.to_string())?;
    }
    let (mut speech, secs) = vad::detect(wav16.to_str().unwrap(), None)?;
    let cue_text = match captions {
        Some(c) => vtt::cues(c)?,
        None => Vec::new(),
    };
    if let Some(c) = captions {
        let cues = vtt::speech(c)?;
        // SPIKE_MAP: "union" (milestone 1's first run) or "led" (captions lead; the default).
        speech = match std::env::var("SPIKE_MAP").as_deref() {
            Ok("union") => gaps::merge(speech.into_iter().chain(cues).collect()),
            _ => gaps::captions_led(&speech, &cues, env_f64("SPIKE_KEEP_UNCUED", f64::MAX)),
        };
    }
    let end = end.min(secs);
    let plan = plan_gaps(&speech, secs, start, end);
    println!(
        "{} gaps in {} to {} ({:.0} s of {:.0} s silent enough)",
        plan.len(),
        gaps::clock(start),
        gaps::clock(end),
        plan.iter().map(|g| g.end - g.start).sum::<f64>(),
        end - start
    );

    for g in &plan {
        println!("  {}", gap_line(g, start));
    }
    if std::env::var_os("SPIKE_PLAN_ONLY").is_some() {
        return Ok(());
    }

    // 2. Descriptions.
    let system = system_prompt(family, svc.route == Route::Video || !cue_text.is_empty());
    let mut descriptions: Vec<Description> = Vec::new();
    let mut raw = Vec::new();
    let mut cost = 0.0;
    let ask_start = Instant::now();
    let mut requests = 0;
    match svc.route {
        Route::Video => {
            let clip = work.join("clip.mp4");
            let fps = std::env::var("SPIKE_CLIP_FPS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2);
            mf::clip(video, start, end, clip.to_str().unwrap(), 640, fps)
                .map_err(|e| e.to_string())?;
            let bytes = std::fs::read(&clip).map_err(|e| e.to_string())?;
            println!(
                "sending: a {:.0}-second clip (picture and sound, {:.1} MB) to {} through OpenRouter, providers that keep no data",
                end - start,
                bytes.len() as f64 / 1e6,
                svc.model
            );
            let evidence = "The video clip starts at 0.0 seconds; the gap times above are on the clip's clock. \
                You can hear the soundtrack: use it only to understand who is who and what is happening, never as \
                something to describe.";
            let story = Story {
                first: true,
                ..Default::default()
            };
            let prompt = request_prompt(&plan, start, &story, evidence);
            let content = vec![
                json!({ "type": "text", "text": prompt }),
                json!({ "type": "video_url", "video_url": { "url": format!("data:video/mp4;base64,{}", net::base64(&bytes)) } }),
            ];
            let reply = ask(svc, &system, content, &work, true)?;
            requests += 1;
            cost += reply.cost;
            raw.push(reply.text.clone());
            descriptions = parse_reply(&reply.text)?;
        }
        Route::Frames => {
            let total_frames: usize = plan.iter().map(|g| frame_times(g).len()).sum();
            println!(
                "sending: {total_frames} still frames, 512 pixels, in requests of {FRAMES_PER_REQUEST_GAPS} gaps, to {}{}",
                svc.model,
                if svc.claude { " through Claude Code on your subscription" } else { " through OpenRouter, providers that keep no data" }
            );
            let mut characters: Vec<String> = Vec::new();
            for (bi, batch) in plan.chunks(FRAMES_PER_REQUEST_GAPS).enumerate() {
                let evidence = format!(
                    "For each gap you are given {} still frames, labelled with their gap number and time in seconds. \
                     You cannot hear the soundtrack, so describe only the pictures.",
                    if batch.iter().all(|g| frame_times(g).len() == 3) { "three" } else { "two or three" }
                );
                let previous: Vec<(f64, String)> = descriptions
                    .iter()
                    .filter(|d| !d.text.is_empty())
                    .filter_map(|d| {
                        plan.iter()
                            .find(|g| g.n == d.gap)
                            .map(|g| (g.start, d.text.clone()))
                    })
                    .collect();
                let story = Story {
                    first: bi == 0,
                    previous: &previous,
                    characters: &characters,
                    cues: &cue_text,
                };
                let mut content = vec![
                    json!({ "type": "text", "text": request_prompt(batch, start, &story, &evidence) }),
                ];
                for g in batch {
                    for t in frame_times(g) {
                        let (jpg, _) = mf::frame(video, t, 512).map_err(|e| e.to_string())?;
                        content.push(json!({ "type": "text", "text": format!("Gap {}, {:.1} s:", g.n, t - start) }));
                        content.push(json!({ "type": "image_url", "image_url": { "url": format!("data:image/jpeg;base64,{}", net::base64(&jpg)) } }));
                    }
                }
                let reply = ask(svc, &system, content, &work, true)?;
                requests += 1;
                cost += reply.cost;
                raw.push(reply.text.clone());
                descriptions.extend(parse_reply(&reply.text)?);
                for name in parse_people(&reply.text) {
                    if !characters.iter().any(|c| c.eq_ignore_ascii_case(&name)) {
                        characters.push(name);
                    }
                }
            }
        }
    }
    // One text-only request to shorten anything over its word limit (log entry 15).
    let over: Vec<String> = plan
        .iter()
        .filter_map(|g| {
            let d = descriptions.iter().find(|d| d.gap == g.n)?;
            (sapi::words(&d.text) > g.words)
                .then(|| format!("{}: at most {} words: {}", g.n, g.words, d.text))
        })
        .collect();
    let mut shortened: Vec<usize> = Vec::new();
    if !over.is_empty() {
        let prompt = format!(
            "{}

{}",
            section("SHORTEN"),
            over.join(
                "
"
            )
        );
        let content = vec![json!({ "type": "text", "text": prompt })];
        let reply = ask(svc, "You shorten audio description.", content, &work, false)?;
        requests += 1;
        cost += reply.cost;
        raw.push(reply.text.clone());
        // Match by order when the counts agree: GLM numbered its answers wrongly (log entry 15).
        let asked: Vec<usize> = over
            .iter()
            .filter_map(|l| l.split(':').next()?.parse().ok())
            .collect();
        let answers = parse_reply(&reply.text)?;
        let by_order = answers.len() == asked.len();
        for (i, s) in answers.into_iter().enumerate() {
            let gap = if by_order { asked[i] } else { s.gap };
            if let Some(d) = descriptions.iter_mut().find(|d| d.gap == gap) {
                if !s.text.is_empty() && same_subject(&d.text, &s.text) {
                    d.text = s.text;
                    shortened.push(gap);
                }
            }
        }
    }
    let ask_secs = ask_start.elapsed().as_secs_f64();
    std::fs::write(
        work.join(format!("{service}.reply.txt")),
        raw.join("\n\n----\n\n"),
    )
    .map_err(|e| e.to_string())?;

    // 3. Speak, fit, place.
    sapi::com_init();
    let want = std::env::var("SPIKE_VOICE").unwrap_or("en-GB-SoniaNeural".into());
    let voices = sapi::voices().map_err(|e| e.to_string())?;
    let windows_voice = voices
        .iter()
        .find(|v| !want.contains("Neural") && v.name.contains(&want));
    let voice_name = windows_voice
        .map(|v| v.name.clone())
        .unwrap_or(want.clone());
    let speak = |text: &str, rate: i32| -> Result<Vec<i16>, String> {
        let s = match windows_voice {
            Some(v) => sapi::speak_hz(v, text, rate / 10, MIX_HZ).map_err(|e| e.to_string())?,
            None => {
                let (mp3, _) = crate::edge::speak(&want, rate, text)?;
                mf::decode_bytes(&mp3, MIX_HZ, 1)
                    .map_err(|e| e.to_string())?
                    .samples
            }
        };
        Ok(level(&trim(&s, MIX_HZ)))
    };
    let mut placed: Vec<Placed> = Vec::new();
    let mut report = Vec::new();
    let mut failures = 0;
    let mut empty = 0;
    for g in &plan {
        let Some(d) = descriptions.iter().find(|d| d.gap == g.n) else {
            report.push(format!(
                "{}  gap {} ({:.1} s): no description returned",
                gaps::clock(g.start - start),
                g.n,
                g.end - g.start
            ));
            empty += 1;
            continue;
        };
        let fails = rule_failures(d, g, start);
        failures += fails.len();
        let mut note = if fails.is_empty() {
            String::new()
        } else {
            format!("   [check: {}]", fails.join("; "))
        };
        if shortened.contains(&g.n) {
            note.push_str("   (shortened on request)");
        }
        if d.text.is_empty() {
            report.push(format!(
                "{}  gap {} ({:.1} s): left empty by the service",
                gaps::clock(g.start - start),
                g.n,
                g.end - g.start
            ));
            empty += 1;
            continue;
        }
        let room = (g.end - g.start) - 2.0 * MARGIN;
        let mut fitted = None;
        let mut text = d.text.clone();
        // Percent faster, for Edge; SAPI gets a tenth of it. The base rate is the user's (log entry 15).
        let base = env_f64("SPIKE_RATE", 10.0) as i32;
        for attempt in 0..2 {
            let mut fastest = 0.0;
            for rate in [base, base + 15, base + 30] {
                let s = speak(&text, rate)?;
                let dur = s.len() as f64 / MIX_HZ as f64;
                if dur <= room {
                    fitted = Some((s, rate, dur));
                    break;
                }
                fastest = dur;
            }
            if fitted.is_some() || attempt == 1 {
                break;
            }
            // Still too long once spoken: one more shortening, to the measured proportion.
            let words = sapi::words(&text);
            let target =
                ((words as f64 * room / fastest).floor() as usize).clamp(1, words.max(2) - 1);
            let prompt = format!(
                "{}\n\n{}: at most {target} words: {text}",
                section("SHORTEN"),
                g.n
            );
            let reply = ask(
                svc,
                "You shorten audio description.",
                vec![json!({ "type": "text", "text": prompt })],
                &work,
                false,
            )?;
            requests += 1;
            cost += reply.cost;
            raw.push(format!(
                "(cut to fit, gap {})
{}",
                g.n, reply.text
            ));
            // One gap was asked for, so take the answer whatever number it carries (GLM copied the
            // example's), as long as it is still about the same thing.
            match parse_reply(&reply.text)?
                .into_iter()
                .find(|s| !s.text.is_empty() && same_subject(&text, &s.text))
            {
                Some(s) => {
                    note.push_str(&format!("   (cut to fit, was: {text})"));
                    text = s.text;
                }
                None => break,
            }
        }
        let d = Description { text, ..d.clone() };
        match fitted {
            Some((samples, rate, dur)) => {
                let at = g.start + MARGIN;
                report.push(format!(
                    "{}  {}{}{}",
                    gaps::clock(at - start),
                    d.text,
                    if rate > base {
                        format!(
                            "   (spoken faster, {dur:.1} s in a {:.1} s gap)",
                            g.end - g.start
                        )
                    } else {
                        String::new()
                    },
                    note
                ));
                placed.push(Placed {
                    gap: g.n,
                    text: d.text.clone(),
                    at,
                    samples,
                });
            }
            None => report.push(format!(
                "{}  DROPPED, too long for its {:.1} s gap even spoken faster: {}{note}",
                gaps::clock(g.start - start),
                g.end - g.start,
                d.text
            )),
        }
    }

    // 4. Duck and mix the original sound under the descriptions.
    let orig = mf::decode(video, MIX_HZ, 2).map_err(|e| e.to_string())?;
    let a = (start * MIX_HZ as f64) as usize * 2;
    let b = ((end * MIX_HZ as f64) as usize * 2).min(orig.samples.len());
    let mut mix: Vec<f32> = orig.samples[a..b].iter().map(|&s| s as f32).collect();
    let frames = mix.len() / 2;
    let mut gain = vec![1.0f32; frames];
    let (fade_in, hold_after) = (0.15, 0.25);
    for p in &placed {
        let s = (p.at - start) as f32;
        let e = s + p.samples.len() as f32 / MIX_HZ as f32;
        let from = ((s - fade_in as f32).max(0.0) * MIX_HZ as f32) as usize;
        let to = (((e + hold_after as f32 + fade_in as f32) * MIX_HZ as f32) as usize).min(frames);
        for (i, g) in gain.iter_mut().enumerate().take(to).skip(from) {
            let t = i as f32 / MIX_HZ as f32;
            let k = if t < s {
                1.0 - (t - (s - fade_in as f32)) / fade_in as f32 * (1.0 - DUCK)
            } else if t > e + hold_after as f32 {
                DUCK + (t - e - hold_after as f32) / fade_in as f32 * (1.0 - DUCK)
            } else {
                DUCK
            };
            *g = g.min(k.clamp(DUCK, 1.0));
        }
    }
    for (i, g) in gain.iter().enumerate() {
        mix[2 * i] *= g;
        mix[2 * i + 1] *= g;
    }
    for p in &placed {
        let base = ((p.at - start) * MIX_HZ as f64) as usize;
        for (k, &s) in p.samples.iter().enumerate() {
            let i = base + k;
            if i >= frames {
                break;
            }
            mix[2 * i] += s as f32;
            mix[2 * i + 1] += s as f32;
        }
    }
    let pcm = mf::Pcm {
        rate: MIX_HZ,
        channels: 2,
        samples: mix
            .iter()
            .map(|&v| v.clamp(-32768.0, 32767.0) as i16)
            .collect(),
    };
    let mp3 = work.join(format!("{service}.mp3"));
    mf::mp3(mp3.to_str().unwrap(), &pcm, 192).map_err(|e| e.to_string())?;

    let summary = format!(
        "{service} ({}): {} gaps, {} spoken, {} empty, {} dropped, {} shortened on request; {} rule checks failed; {requests} requests, {:.1} s waiting, US${:.4}",
        svc.model,
        plan.len(),
        placed.len(),
        empty,
        plan.len() - placed.len() - empty,
        shortened.len(),
        failures,
        ask_secs,
        cost
    );
    let mut text = format!(
        "Described clip: {} to {} of the video, {} service, voice {}.\n{summary}\n\nTimes are from the start of the clip.\n\n",
        gaps::clock(start),
        gaps::clock(end),
        service,
        voice_name
    );
    for line in &report {
        text.push_str(line);
        text.push('\n');
    }
    std::fs::write(work.join(format!("{service}.txt")), text).map_err(|e| e.to_string())?;
    // Again, now with the cut-to-fit replies.
    std::fs::write(
        work.join(format!("{service}.reply.txt")),
        raw.join(
            "

----

",
        ),
    )
    .map_err(|e| e.to_string())?;
    let _ = placed.iter().map(|p| (p.gap, &p.text)).count();
    println!(
        "{summary}\nwrote {} in {:.0} s",
        mp3.display(),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_limits_follow_the_two_and_a_half_words_a_second_rule() {
        assert_eq!(word_limit(1.0), 1);
        assert_eq!(word_limit(1.4), 2);
        assert_eq!(word_limit(3.0), 6);
        assert_eq!(word_limit(0.5), 1);
    }

    #[test]
    fn long_silences_split_into_equal_parts() {
        let speech = vec![(0.0, 10.0), (22.0, 40.0)];
        let g = plan_gaps(&speech, 40.0, 0.0, 40.0);
        assert_eq!(g.len(), 3);
        assert_eq!(g[0].part, Some((1, 3)));
        assert!((g[2].end - 22.0).abs() < 1e-9);
        assert!(g.iter().all(|x| x.end - x.start <= MAX_PART));
    }

    #[test]
    fn gaps_are_cut_to_the_clip_and_short_ones_dropped() {
        let speech = vec![(0.0, 5.0), (5.5, 20.0), (23.0, 30.0)];
        let g = plan_gaps(&speech, 30.0, 10.0, 22.0);
        assert_eq!(g.len(), 1);
        assert_eq!((g[0].start, g[0].end), (20.0, 22.0));
    }

    #[test]
    fn the_prompt_fills_without_leftover_placeholders() {
        let sys = system_prompt(true, true);
        assert!(!sys.contains('{') || sys.contains("{\n"), "system: {sys}");
        assert!(sys.contains("Home video"));
        assert!(!sys.contains("Credits"));
        let g = vec![Gap {
            n: 1,
            start: 61.0,
            end: 64.0,
            words: 5,
            part: None,
        }];
        let cues = vec![(58.0, 60.5, "Hello, Mr Snail!".to_string())];
        let characters = vec!["the ponytailed girl".to_string()];
        let story = Story {
            first: true,
            previous: &[],
            characters: &characters,
            cues: &cues,
        };
        let r = request_prompt(&g, 60.0, &story, "note");
        assert!(r.contains("1: 1.0 to 4.0, up to 5 words"), "{r}");
        assert!(r.contains("- -2.0 to 0.5: Hello, Mr Snail!"), "{r}");
        assert!(r.contains("- the ponytailed girl"), "{r}");
        assert!(r.contains("heard nothing yet"), "{r}");
        for p in [
            "{language}",
            "{style}",
            "{gaps}",
            "{previous}",
            "{evidence_note}",
            "{characters}",
            "{dialogue}",
            "{context}",
        ] {
            assert!(!r.contains(p), "{p} left in {r}");
        }
    }

    #[test]
    fn replies_parse_through_fences() {
        let r = "```json\n{\"descriptions\":[{\"gap\":2,\"text\":\"A fox.\",\"seen_at\":3.5}],\"people\":[]}\n```";
        let d = parse_reply(r).unwrap();
        let two = "{\"descriptions\":[{\"gap\":1,\"text\":\"A.\"}]}
{\"descriptions\":[]}";
        assert_eq!(parse_reply(two).unwrap().len(), 1);
        let cut = "```json
{\"descriptions\": [{\"gap\": 1, \"text\": \"A.\"}, {\"gap\": 2, \"te";
        assert_eq!(parse_reply(cut).unwrap().len(), 1);
        assert!(same_subject(
            "Three sprites lug a bloody boot.",
            "Sprites carry a boot."
        ));
        assert!(!same_subject(
            "Trainers among red toadstools.",
            "Campers sleep."
        ));
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].gap, 2);
        assert_eq!(d[0].seen_at, Some(3.5));
        let p = parse_people("{\"descriptions\":[],\"people\":[{\"name\":\" the blond boy \"}]}");
        assert_eq!(p, vec!["the blond boy".to_string()]);
    }
}

/// `transcribe <audio.mp3> <model>`: a timed transcript of a soundtrack, to study another tool's
/// described film (log entry 15). Public material only.
pub fn transcribe(args: &[&str]) -> Result<(), String> {
    let [path, model] = args else {
        return Err("usage: spike transcribe <audio.mp3> <model>".into());
    };
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let prompt = "This soundtrack is a film with audio description added: a narrator's voice describes what \
        is on screen between the characters' lines. List every line the describing narrator speaks, in order, \
        each with its start time as MM:SS, one per line, like \"01:02 A girl peers into a tent.\" Leave out \
        the characters' dialogue. Write the narrator's words exactly.";
    let content = vec![
        json!({ "type": "text", "text": prompt }),
        json!({ "type": "input_audio", "input_audio": { "data": net::base64(&bytes), "format": "mp3" } }),
    ];
    let r = openrouter(model, "You transcribe audio precisely.", content, None)?;
    println!("{}\n\n(US${:.4})", r.text, r.cost);
    Ok(())
}
