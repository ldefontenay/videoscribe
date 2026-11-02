//! Claude Code as a describing service (milestone 6, PLAN.md §4.2): `claude -p`
//! on this computer, on the user's Claude subscription, given the same still
//! pictures and prompt as OpenRouter. Ported from Photo Scribe's describe.rs,
//! where it is proven: it refuses to run on anything but a subscription, so
//! nothing is billed to an API account; the child gets an empty working
//! folder, no tools, no hooks, no MCP servers and no session saved.
//!
//! The one other program Video Scribe may start besides yt-dlp (CLAUDE.md
//! constraint 6).

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::openrouter::Reply;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Four places' pictures take Claude well under a minute; five minutes means
/// something is wrong.
const TIMEOUT: Duration = Duration::from_secs(300);
/// Variables that would move the child off the subscription, or mark it as
/// nested inside another Claude Code session.
const STRIPPED_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDECODE",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
];

/// Where Claude Code is installed: on the PATH, or where its installer puts
/// it.
pub fn find() -> Result<PathBuf, String> {
    let on_path = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).find_map(|dir| {
            ["claude.exe", "claude.cmd"]
                .iter()
                .map(|n| dir.join(n))
                .find(|p| p.is_file())
        })
    });
    on_path
        .or_else(|| {
            let home = PathBuf::from(std::env::var_os("USERPROFILE")?);
            let p = home.join(".local").join("bin").join("claude.exe");
            p.is_file().then_some(p)
        })
        .ok_or_else(|| {
            "Claude Code is not installed on this computer. Install it from claude.com/code, \
             sign in with your Claude account, then try again. Or choose OpenRouter in the \
             Options menu, under Model provider."
                .to_string()
        })
}

fn command(claude: &Path) -> Command {
    let mut c = Command::new(claude);
    for v in STRIPPED_ENV {
        c.env_remove(v);
    }
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// Refuses unless Claude Code is signed in with a Claude subscription.
pub fn check_subscription(claude: &Path) -> Result<(), String> {
    let out = command(claude)
        .args(["auth", "status"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Video Scribe could not run Claude Code: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    // The CLI wraps its JSON in terminal escape codes.
    let json = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if b > a => &text[a..=b],
        _ => "",
    };
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    if v["loggedIn"] != true {
        return Err(
            "Claude Code is not signed in. Open a terminal, type claude auth login, \
                    press Enter, and sign in with your Claude account. Then try again."
                .into(),
        );
    }
    let method = v["authMethod"].as_str().unwrap_or("unknown");
    if method != "claude.ai" {
        return Err(format!(
            "Claude Code is signed in with \"{method}\", not with a Claude subscription, so \
             using it would be billed to an API account. Video Scribe refuses to do that. To use \
             your subscription: open a terminal, type claude auth login, and sign in with your \
             Claude account."
        ));
    }
    Ok(())
}

/// A chat request's content, as OpenRouter takes it, in the shape Claude
/// takes: text stays text, and each picture's data URL becomes a base64 image.
fn blocks(content: &[Value]) -> Result<Vec<Value>, String> {
    content
        .iter()
        .map(|c| match c["type"].as_str() {
            Some("text") => Ok(json!({ "type": "text", "text": c["text"] })),
            Some("image_url") => {
                let url = c["image_url"]["url"].as_str().unwrap_or_default();
                let (head, data) = url
                    .strip_prefix("data:")
                    .and_then(|u| u.split_once(";base64,"))
                    .ok_or("A picture was not in the form Claude takes.")?;
                Ok(json!({
                    "type": "image",
                    "source": { "type": "base64", "media_type": head, "data": data }
                }))
            }
            _ => Err("Claude Code takes still pictures, not video clips.".to_string()),
        })
        .collect()
}

/// Sends one request to Claude through Claude Code: `system` as its system
/// prompt, `content` as the message. None if `stop` was set. The reply costs
/// nothing beyond the subscription, so its cost is nought.
pub fn ask(
    claude: &Path,
    model: &str,
    system: &str,
    content: Vec<Value>,
    stop: &AtomicBool,
) -> Result<Option<Reply>, String> {
    let message = json!({
        "type": "user",
        "message": { "role": "user", "content": blocks(&content)? }
    });
    // An empty folder of its own as the working directory, so Claude Code
    // picks up no project and no CLAUDE.md; the system prompt travels in a
    // file, never on the command line. One folder per request, as several go
    // at once.
    let scratch = std::env::temp_dir().join(format!(
        "videoscribe-claude-{}-{}",
        std::process::id(),
        next_id()
    ));
    std::fs::create_dir_all(&scratch)
        .map_err(|e| format!("Could not make a temporary folder: {e}"))?;
    let result = run(claude, model, system, &message, &scratch, stop);
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn next_id() -> usize {
    use std::sync::atomic::AtomicUsize;
    static ID: AtomicUsize = AtomicUsize::new(0);
    ID.fetch_add(1, Ordering::Relaxed)
}

fn run(
    claude: &Path,
    model: &str,
    system: &str,
    message: &Value,
    scratch: &Path,
    stop: &AtomicBool,
) -> Result<Option<Reply>, String> {
    let system_file = scratch.join("system_prompt.txt");
    std::fs::write(&system_file, system)
        .map_err(|e| format!("Could not write a temporary file: {e}"))?;
    let mut child = command(claude)
        .args(["-p", "--model", model, "--system-prompt-file"])
        .arg(&system_file)
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
        .current_dir(scratch)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Video Scribe could not start Claude Code: {e}"))?;

    // Separate threads: a dozen pictures on stdin must not deadlock against
    // the child filling its stdout.
    let mut stdin = child.stdin.take().expect("piped");
    let payload = format!("{message}\n");
    std::thread::spawn(move || {
        let _ = stdin.write_all(payload.as_bytes());
    });
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut s);
        s
    });
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut result = None;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if v["type"] == "result" {
                    result = Some(v);
                }
            }
        }
        let _ = tx.send(result);
    });

    let started = Instant::now();
    let result = loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(r) => break r,
            Err(mpsc::RecvTimeoutError::Disconnected) => break None,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if stop.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(None);
                }
                if started.elapsed() > TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(
                        "Claude did not answer within 5 minutes. Start again later: \
                                what was already described is kept."
                            .into(),
                    );
                }
            }
        }
    };
    let _ = child.wait();
    let stderr_text = err_reader.join().unwrap_or_default();
    let Some(v) = result else {
        let detail: String = stderr_text.trim().chars().take(300).collect();
        return Err(if detail.is_empty() {
            "Claude Code stopped without giving an answer.".to_string()
        } else {
            format!("Claude Code stopped without giving an answer: {detail}")
        });
    };
    let text = v["result"].as_str().unwrap_or("").trim().to_string();
    if v["is_error"] == true || text.is_empty() {
        return Err(problem(&text));
    }
    Ok(Some(Reply { text, cost: 0.0 }))
}

/// What to tell the user when Claude answered with an error.
fn problem(text: &str) -> String {
    let lower = text.to_lowercase();
    if lower.contains("limit") {
        format!(
            "Your Claude plan's usage limit has been reached, so describing has stopped. Claude \
             said: \"{text}\" Nothing is lost: start again when the limit resets, and Video \
             Scribe carries on from where it stopped."
        )
    } else if ["login", "log in", "authenticat", "oauth"]
        .iter()
        .any(|w| lower.contains(w))
    {
        format!(
            "Claude Code needs you to sign in again. Claude said: \"{text}\" Open a terminal, \
             type claude auth login, sign in, and try again."
        )
    } else if text.is_empty() {
        "Claude gave no answer.".into()
    } else {
        format!("Claude said: {text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_become_claude_images() {
        let content = vec![
            json!({ "type": "text", "text": "Gap 1, 2.0 s:" }),
            json!({ "type": "image_url", "image_url": { "url": "data:image/jpeg;base64,QUJD" } }),
        ];
        let b = blocks(&content).unwrap();
        assert_eq!(b[0], json!({ "type": "text", "text": "Gap 1, 2.0 s:" }));
        assert_eq!(b[1]["source"]["media_type"], "image/jpeg");
        assert_eq!(b[1]["source"]["data"], "QUJD");
        let clip = vec![
            json!({ "type": "video_url", "video_url": { "url": "data:video/mp4;base64,AA" } }),
        ];
        assert!(blocks(&clip).is_err());
    }

    #[test]
    fn a_usage_limit_says_nothing_is_lost() {
        assert!(problem("Claude AI usage limit reached|1700000000").contains("Nothing is lost"));
        assert!(problem("Invalid API key · Please run /login").contains("sign in again"));
    }

    /// Needs Claude Code signed in with a subscription: `cargo test -- --ignored claude`.
    /// Uses a little of the plan.
    #[test]
    #[ignore]
    fn claude_answers_a_text_request() {
        let exe = find().unwrap();
        check_subscription(&exe).unwrap();
        let stop = AtomicBool::new(false);
        let r = ask(
            &exe,
            "haiku",
            "Reply with JSON only.",
            vec![json!({ "type": "text", "text": "Reply with {\"ok\": true} and nothing else." })],
            &stop,
        )
        .unwrap()
        .unwrap();
        assert!(r.text.contains("\"ok\""), "{}", r.text);
    }
}
