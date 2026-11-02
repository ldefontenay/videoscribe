//! OpenRouter, the default model provider (PLAN.md §7 decisions 15 and 19):
//! one chat request at a time, through WinHTTP. Only providers that neither
//! keep nor train on what they are sent are used.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use crate::net;

/// Video Scribe's own key, when there is one; Photo Scribe's otherwise, so a
/// user of both enters it once.
const TARGETS: &[&str] = &["Video Scribe:OpenRouter", "Photo Scribe:OpenRouter"];

pub fn key() -> Result<String, String> {
    TARGETS.iter().find_map(|t| net::read_key(t)).ok_or_else(|| {
        "There is no OpenRouter key on this computer. Make one at openrouter.ai, then add it in \
         Windows Credential Manager: Windows Credentials, Add a generic credential, with \
         \"Video Scribe:OpenRouter\" as the Internet or network address, any user name, and \
         the key as the password."
            .into()
    })
}

pub struct Reply {
    pub text: String,
    /// US dollars, as OpenRouter counts it.
    pub cost: f64,
}

/// How hard the model thinks before answering. GLM 5.3 Flash cannot be told
/// not to think at all.
#[derive(Clone, Copy)]
pub struct Effort(&'static str);

/// For looking at pictures.
pub const LOOK: Effort = Effort("low");
/// For text-only work: at "low", GLM thought for 7,500 tokens over 20 lines
/// of editing, 44 seconds, and sometimes ran out of room; at "minimal", 2,700
/// tokens and 11 seconds (log entry 31).
pub const TEXT: Effort = Effort("minimal");

/// Sends one request. Busy answers are retried twice, after 10 and 30
/// seconds. None if `stop` was set while waiting to retry.
pub fn ask(
    key: &str,
    model: &str,
    system: &str,
    content: Vec<Value>,
    schema: Option<Value>,
    effort: Effort,
    stop: &AtomicBool,
) -> Result<Option<Reply>, String> {
    let mut body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": content }
        ],
        "max_tokens": 8000,
        "reasoning": { "effort": effort.0, "exclude": true },
        "provider": { "data_collection": "deny" },
        "usage": { "include": true }
    });
    if let Some(schema) = schema {
        body["response_format"] = schema;
    }
    let body = body.to_string();
    let headers = vec![
        format!("Authorization: Bearer {key}"),
        "Content-Type: application/json".to_string(),
        "X-Title: Video Scribe".to_string(),
    ];
    let mut last = String::new();
    for wait in [0u64, 10, 30] {
        for _ in 0..wait * 10 {
            if stop.load(Ordering::Relaxed) {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let (status, text) = net::post(
            "openrouter.ai",
            "/api/v1/chat/completions",
            &headers,
            body.as_bytes(),
            300_000,
        )?;
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let said = v["error"]["message"].as_str().unwrap_or("").to_string();
        match status {
            200 => {}
            401 => {
                return Err(
                    "OpenRouter did not accept the key. Check it in Windows Credential Manager, \
                     under \"Video Scribe:OpenRouter\" or \"Photo Scribe:OpenRouter\"."
                        .into(),
                )
            }
            402 => return Err(
                "OpenRouter says the account has run out of credit. Add some at openrouter.ai, \
                     then start again: what was already described is kept."
                    .into(),
            ),
            429 | 500.. => {
                last = format!("OpenRouter was busy ({status}). {said}");
                continue;
            }
            _ => return Err(format!("OpenRouter refused the request ({status}). {said}")),
        }
        let reply = v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if reply.trim().is_empty() {
            // Seen once from Gemma (log entry 15): whitespace and nothing else.
            last = match v["choices"][0]["finish_reason"].as_str() {
                Some("length") => "OpenRouter's answer was empty: it ran out of room.".into(),
                _ => "OpenRouter's answer was empty.".into(),
            };
            continue;
        }
        return Ok(Some(Reply {
            text: reply,
            cost: v["usage"]["cost"].as_f64().unwrap_or(0.0),
        }));
    }
    Err(format!(
        "{} It was tried three times; start again later, and what was already described is kept.",
        last.trim()
    ))
}
