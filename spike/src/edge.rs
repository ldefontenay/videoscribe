//! Spike 0(f): Edge's Read Aloud voices over WinHTTP WebSockets (PLAN.md §2.9, §4.4).
//!
//! Written from the protocol facts in PLAN.md §2.9, not from SonarPad's code. The pieces:
//!   - the `Sec-MS-GEC` token: SHA-256 (CNG) of the 5-minute Windows tick count plus the public client token;
//!   - a WebSocket upgrade with Edge's `Origin` and `User-Agent`, through WinHTTP, so no TLS crate;
//!   - two text frames out (`speech.config`, then `ssml`), binary audio frames back;
//!   - **the turn rule**: audio counts only when a `Path:turn.end` text frame arrives. A close, a
//!     dropped connection or an error before it discards everything received, and the caller retries.
//!     A clip cut short would be measured short, scheduled into a gap it does not fit, and talk over
//!     the film. `collect` holds the rule and the tests below pin it.

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::Security::Cryptography::*;

/// The public client token every Read Aloud client sends; not a secret and not ours.
const CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
const ORIGIN: &str = "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold";
const OUTPUT_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";
/// Seconds between 1601-01-01 (Windows) and 1970-01-01 (Unix).
const WINDOWS_EPOCH: u64 = 11_644_473_600;

struct Route {
    /// Which `Sec-MS-GEC` the route sends: 1 (SonarPad's, refused since October 2026) or 2.
    gec: u8,
    host: &'static str,
    path: &'static str,
    key: &'static str,
    version: &'static str,
    agent: &'static str,
}

/// Tried in this order (PLAN.md §2.9). The v2 route is what edge-tts 7.3.1 sends (log entry 15).
const ROUTES: [Route; 3] = [
    Route {
        gec: 2,
        host: "speech.platform.bing.com",
        path: "/consumer/speech/synthesize/readaloud/edge/v1",
        key: "TrustedClientToken",
        version: "2-143.0.3650.75",
        agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36 Edg/143.0.0.0",
    },
    Route {
        gec: 1,
        host: "api.msedgeservices.com",
        path: "/tts/cognitiveservices/websocket/v1",
        key: "Ocp-Apim-Subscription-Key",
        version: "1-155.0.4283.45",
        agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36 Edg/155.0.0.0",
    },
    Route {
        gec: 1,
        host: "speech.platform.bing.com",
        path: "/consumer/speech/synthesize/readaloud/edge/v1",
        key: "TrustedClientToken",
        version: "1-132.0.2917.39",
        agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36 Edg/132.0.0.0",
    },
];

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    unsafe {
        let _ = BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out);
    }
    out
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    unsafe {
        let _ = BCryptGenRandom(None, &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
    }
    hex_upper(&b)
}

/// The rolling token: Windows ticks (100 ns since 1601) floored to 5 minutes, then the client token,
/// hashed and written as upper-case hex.
pub fn gec(unix_seconds: u64) -> String {
    let secs = unix_seconds + WINDOWS_EPOCH;
    let ticks = (secs - secs % 300) as u128 * 10_000_000;
    hex_upper(&sha256(format!("{ticks}{CLIENT_TOKEN}").as_bytes()))
}

/// The v2 token over the same text as v1 (see gec2.rs).
pub fn gec_v2(unix_seconds: u64) -> String {
    let secs = unix_seconds + WINDOWS_EPOCH;
    let ticks = (secs - secs % 300) as u128 * 10_000_000;
    crate::gec2::token_for(format!("{ticks}{CLIENT_TOKEN}").as_bytes())
}

/// JavaScript's `Date.toString()` in UTC, which is what the service expects in `X-Timestamp`.
pub fn js_date(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let rem = unix_seconds % 86_400;
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + (month <= 2) as i64;
    const WD: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MO: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{} {} {:02} {} {:02}:{:02}:{:02} GMT+0000 (Coordinated Universal Time)",
        WD[days.rem_euclid(7) as usize],
        MO[(month - 1) as usize],
        day,
        year,
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Text is spoken as text: the five XML specials are escaped, so a description can never become markup.
pub fn escape(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => s.push_str("&amp;"),
            '<' => s.push_str("&lt;"),
            '>' => s.push_str("&gt;"),
            '"' => s.push_str("&quot;"),
            '\'' => s.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => s.push(' '),
            c => s.push(c),
        }
    }
    s
}

/// `en-GB-SoniaNeural` -> `en-GB`.
fn voice_language(voice: &str) -> String {
    voice.split('-').take(2).collect::<Vec<_>>().join("-")
}

pub fn ssml(voice: &str, rate_percent: i32, text: &str) -> String {
    format!(
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='{}'><voice name='{}'><prosody pitch='+0Hz' rate='{:+}%' volume='+0%'>{}</prosody></voice></speak>",
        voice_language(voice),
        escape(voice),
        rate_percent,
        escape(text)
    )
}

/// One message from the socket.
#[derive(Debug)]
pub enum Frame {
    Text(String),
    Binary(Vec<u8>),
    /// The server closed the socket, or the connection simply ended.
    Closed,
}

/// The `Path:` header of a text frame or a binary frame's header block.
fn path_of(header: &str) -> Option<&str> {
    header
        .split("\r\n")
        .find_map(|l| l.strip_prefix("Path:"))
        .map(str::trim)
}

/// A binary frame: 2-byte big-endian header length, the header text, then audio. Returns the audio
/// if the header says `Path:audio`, `None` for any other path, and an error if the frame is malformed.
pub fn audio_payload(frame: &[u8]) -> Result<Option<&[u8]>, String> {
    if frame.len() < 2 {
        return Err("binary frame shorter than its length prefix".into());
    }
    let n = u16::from_be_bytes([frame[0], frame[1]]) as usize;
    let Some(header) = frame.get(2..2 + n) else {
        return Err("binary frame shorter than its header".into());
    };
    let header = std::str::from_utf8(header).map_err(|_| "binary frame header is not text")?;
    Ok((path_of(header) == Some("audio")).then(|| &frame[2 + n..]))
}

/// Gathers one turn's audio. Only `turn.end` makes it a success; anything else that ends the stream
/// (a close, the iterator running dry, an error) throws the partial audio away.
pub fn collect(frames: impl IntoIterator<Item = Result<Frame, String>>) -> Result<Vec<u8>, String> {
    let mut audio = Vec::new();
    for f in frames {
        match f {
            Err(e) => {
                return Err(format!(
                    "{e}; discarded {} bytes of partial audio",
                    audio.len()
                ))
            }
            Ok(Frame::Closed) => {
                return Err(format!(
                    "closed before turn.end; discarded {} bytes of partial audio",
                    audio.len()
                ))
            }
            Ok(Frame::Binary(b)) => {
                if let Some(a) = audio_payload(&b)? {
                    audio.extend_from_slice(a);
                }
            }
            Ok(Frame::Text(t)) => {
                if path_of(&t) == Some("turn.end") {
                    return if audio.is_empty() {
                        Err("turn.end with no audio".into())
                    } else {
                        Ok(audio)
                    };
                }
            }
        }
    }
    Err(format!(
        "connection ended before turn.end; discarded {} bytes of partial audio",
        audio.len()
    ))
}

struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn last_error(what: &str) -> String {
    let code = unsafe { windows::Win32::Foundation::GetLastError() }.0;
    format!("{what} (WinHTTP error {code})")
}

/// Reads frames off a WinHTTP WebSocket, joining fragments.
struct Socket {
    ws: Handle,
    _req: Handle,
    _conn: Handle,
    _session: Handle,
    done: bool,
}

impl Iterator for Socket {
    type Item = Result<Frame, String>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut msg = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            let mut kind = WINHTTP_WEB_SOCKET_BUFFER_TYPE(0);
            let err = unsafe {
                WinHttpWebSocketReceive(
                    self.ws.0,
                    buf.as_mut_ptr() as _,
                    buf.len() as u32,
                    &mut read,
                    &mut kind,
                )
            };
            if err != 0 {
                self.done = true;
                return Some(Err(format!("receive failed (WinHTTP error {err})")));
            }
            msg.extend_from_slice(&buf[..read as usize]);
            match kind {
                WINHTTP_WEB_SOCKET_BINARY_FRAGMENT_BUFFER_TYPE
                | WINHTTP_WEB_SOCKET_UTF8_FRAGMENT_BUFFER_TYPE => continue,
                WINHTTP_WEB_SOCKET_BINARY_MESSAGE_BUFFER_TYPE => {
                    return Some(Ok(Frame::Binary(msg)))
                }
                WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE => {
                    return Some(Ok(Frame::Text(String::from_utf8_lossy(&msg).into_owned())))
                }
                _ => {
                    self.done = true;
                    return Some(Ok(Frame::Closed));
                }
            }
        }
    }
}

impl Socket {
    fn send(&self, text: &str) -> Result<(), String> {
        let err = unsafe {
            WinHttpWebSocketSend(
                self.ws.0,
                WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE,
                Some(text.as_bytes()),
            )
        };
        if err == 0 {
            Ok(())
        } else {
            Err(format!("send failed (WinHTTP error {err})"))
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpWebSocketClose(
                self.ws.0,
                WINHTTP_WEB_SOCKET_SUCCESS_CLOSE_STATUS.0 as u16,
                None,
                0,
            );
        }
    }
}

fn connect(route: &Route, connection_id: &str) -> Result<Socket, String> {
    unsafe {
        let session = Handle(WinHttpOpen(
            &HSTRING::from(route.agent),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return Err(last_error("could not start WinHTTP"));
        }
        let _ = WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 30_000);
        let conn = Handle(WinHttpConnect(
            session.0,
            &HSTRING::from(route.host),
            443,
            0,
        ));
        if conn.0.is_null() {
            return Err(last_error("could not connect"));
        }
        // The token and connection id are part of the URL; never print it.
        let path = format!(
            "{}?{}={}&ConnectionId={}&Sec-MS-GEC={}&Sec-MS-GEC-Version={}",
            route.path,
            route.key,
            CLIENT_TOKEN,
            connection_id,
            if route.gec == 2 {
                gec_v2(unix_now())
            } else {
                gec(unix_now())
            },
            route.version
        );
        let req = Handle(WinHttpOpenRequest(
            conn.0,
            &HSTRING::from("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return Err(last_error("could not create the request"));
        }
        WinHttpSetOption(Some(req.0), WINHTTP_OPTION_UPGRADE_TO_WEB_SOCKET, None)
            .map_err(|_| last_error("could not ask for a WebSocket"))?;
        let headers = [
            "Pragma: no-cache".to_string(),
            "Cache-Control: no-cache".to_string(),
            format!("Origin: {ORIGIN}"),
            "Accept-Language: en-US,en;q=0.9".to_string(),
            "Accept-Encoding: gzip, deflate, br, zstd".to_string(),
            format!("Cookie: muid={};", random_hex(16)),
        ]
        .join("\r\n");
        let h: Vec<u16> = headers.encode_utf16().collect();
        WinHttpSendRequest(req.0, Some(&h), None, 0, 0, 0)
            .map_err(|_| last_error("could not send the upgrade request"))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut())
            .map_err(|_| last_error("no answer to the upgrade request"))?;
        let mut status = 0u32;
        let mut size = 4u32;
        let _ = WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as _),
            &mut size,
            std::ptr::null_mut(),
        );
        if status != 101 {
            if std::env::var_os("SPIKE_EDGE_DEBUG").is_some() {
                let mut raw = vec![0u16; 8192];
                let mut size = (raw.len() * 2) as u32;
                let _ = WinHttpQueryHeaders(
                    req.0,
                    WINHTTP_QUERY_RAW_HEADERS_CRLF,
                    PCWSTR::null(),
                    Some(raw.as_mut_ptr() as _),
                    &mut size,
                    std::ptr::null_mut(),
                );
                eprintln!(
                    "--- {} headers\n{}",
                    route.host,
                    String::from_utf16_lossy(&raw[..size as usize / 2])
                );
                let mut body = vec![0u8; 2048];
                let mut read = 0u32;
                let _ =
                    WinHttpReadData(req.0, body.as_mut_ptr() as _, body.len() as u32, &mut read);
                eprintln!(
                    "--- body\n{}",
                    String::from_utf8_lossy(&body[..read as usize])
                );
            }
            return Err(format!(
                "{} refused the WebSocket with status {status}",
                route.host
            ));
        }
        let ws = Handle(WinHttpWebSocketCompleteUpgrade(req.0, None));
        if ws.0.is_null() {
            return Err(last_error("WebSocket upgrade failed"));
        }
        Ok(Socket {
            ws,
            _req: req,
            _conn: conn,
            _session: session,
            done: false,
        })
    }
}

/// One attempt on one route: config, SSML, then frames until `turn.end`.
fn speak_once(route: &Route, voice: &str, rate: i32, text: &str) -> Result<Vec<u8>, String> {
    let id = random_hex(16).to_lowercase();
    let socket = connect(route, &id)?;
    let now = js_date(unix_now());
    socket.send(&format!(
        "X-Timestamp:{now}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n{{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\"sentenceBoundaryEnabled\":\"false\",\"wordBoundaryEnabled\":\"false\"}},\"outputFormat\":\"{OUTPUT_FORMAT}\"}}}}}}}}"
    ))?;
    socket.send(&format!(
        "X-RequestId:{id}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{now}Z\r\nPath:ssml\r\n\r\n{}",
        ssml(voice, rate, text)
    ))?;
    collect(socket)
}

/// Speaks `text`, trying each route, three rounds in all. Returns MP3 bytes and a note of what happened.
pub fn speak(voice: &str, rate: i32, text: &str) -> Result<(Vec<u8>, String), String> {
    let mut log = Vec::new();
    for round in 0..3 {
        for route in &ROUTES {
            match speak_once(route, voice, rate, text) {
                Ok(mp3) => {
                    log.push(format!("ok via {}", route.host));
                    return Ok((mp3, log.join("; ")));
                }
                Err(e) => log.push(format!("{}: {e}", route.host)),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500 * (round + 1)));
    }
    Err(log.join("; "))
}

/// The public voice list, as JSON text.
pub fn voices_json() -> Result<String, String> {
    unsafe {
        let session = Handle(WinHttpOpen(
            &HSTRING::from(ROUTES[0].agent),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        let conn = Handle(WinHttpConnect(
            session.0,
            &HSTRING::from("speech.platform.bing.com"),
            443,
            0,
        ));
        let path = format!(
            "/consumer/speech/synthesize/readaloud/voices/list?trustedclienttoken={CLIENT_TOKEN}&Sec-MS-GEC={}&Sec-MS-GEC-Version={}",
            gec_v2(unix_now()),
            ROUTES[0].version
        );
        let req = Handle(WinHttpOpenRequest(
            conn.0,
            &HSTRING::from("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return Err(last_error("could not create the request"));
        }
        WinHttpSendRequest(req.0, None, None, 0, 0, 0).map_err(|_| last_error("send"))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|_| last_error("receive"))?;
        let mut data = Vec::new();
        loop {
            let mut avail = 0u32;
            WinHttpQueryDataAvailable(req.0, &mut avail).map_err(|_| last_error("read"))?;
            if avail == 0 {
                break;
            }
            let start = data.len();
            data.resize(start + avail as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(req.0, data[start..].as_mut_ptr() as _, avail, &mut read)
                .map_err(|_| last_error("read"))?;
            data.truncate(start + read as usize);
        }
        Ok(String::from_utf8_lossy(&data).into_owned())
    }
}

/// `edge voices [filter]` | `edge say <voice> <rate%> <out.mp3> <text...>` | `edge measure <out_dir> <voice>...`
pub fn run(args: &[&str]) -> Result<(), String> {
    match args {
        ["voices", rest @ ..] => {
            let json = voices_json()?;
            if std::env::var_os("SPIKE_EDGE_DEBUG").is_some() {
                eprintln!("{}", &json[..json.len().min(400)]);
            }
            let filter = rest.first().copied().unwrap_or("");
            let names: Vec<&str> = json
                .split("\"ShortName\":")
                .skip(1)
                .filter_map(|s| s.trim_start().strip_prefix('"')?.split('"').next())
                .collect();
            println!("{} voices", names.len());
            for n in names.iter().filter(|n| n.contains(filter)) {
                println!("  {n}");
            }
        }
        ["say", voice, rate, out, words @ ..] => {
            let text = words.join(" ");
            let t = std::time::Instant::now();
            let (mp3, note) = speak(voice, rate.parse().map_err(|_| "rate is a whole percentage")?, &text)?;
            println!("{} bytes in {} ms ({note})", mp3.len(), t.elapsed().as_millis());
            std::fs::write(out, mp3).map_err(|e| e.to_string())?;
        }
        ["measure", out_dir, voices @ ..] => {
            crate::mf::startup().map_err(|e| e.to_string())?;
            println!("voice | words | total s | lead s | trail s | speech s | words/s | ms");
            for (vi, v) in voices.iter().enumerate() {
                let (mut tw, mut ts) = (0, 0.0);
                for (si, s) in crate::sapi::SAMPLES.iter().enumerate() {
                    let t = std::time::Instant::now();
                    let (mp3, _) = speak(v, 0, s)?;
                    let ms = t.elapsed().as_millis();
                    std::fs::write(format!("{out_dir}\\edge-{vi}-{si}.mp3"), &mp3).map_err(|e| e.to_string())?;
                    let pcm = crate::mf::decode_bytes(&mp3, 24_000, 1).map_err(|e| e.to_string())?;
                    let total = pcm.samples.len() as f64 / 24_000.0;
                    let (lead, trail) = crate::sapi::silence(&pcm.samples, 24_000);
                    let speech = total - lead - trail;
                    let w = crate::sapi::words(s);
                    tw += w;
                    ts += speech;
                    println!("{v} | {w} | {total:.2} | {lead:.2} | {trail:.2} | {speech:.2} | {:.2} | {ms}", w as f64 / speech);
                }
                println!("{v} | overall {:.2} words/s", tw as f64 / ts);
            }
        }
        _ => return Err("usage: spike edge voices [filter] | say <voice> <rate%> <out.mp3> <text...> | measure <out_dir> <voice>...".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio_frame(bytes: &[u8]) -> Frame {
        let header = b"X-RequestId:1\r\nContent-Type:audio/mpeg\r\nPath:audio\r\n";
        let mut f = (header.len() as u16).to_be_bytes().to_vec();
        f.extend_from_slice(header);
        f.extend_from_slice(bytes);
        Frame::Binary(f)
    }

    fn text(path: &str) -> Frame {
        Frame::Text(format!("X-RequestId:1\r\nPath:{path}\r\n\r\n{{}}"))
    }

    #[test]
    fn turn_end_returns_all_the_audio() {
        let frames = vec![
            Ok(text("turn.start")),
            Ok(audio_frame(b"abc")),
            Ok(audio_frame(b"de")),
            Ok(text("turn.end")),
        ];
        assert_eq!(collect(frames).unwrap(), b"abcde");
    }

    #[test]
    fn a_close_before_turn_end_throws_the_audio_away() {
        let frames = vec![
            Ok(audio_frame(b"abc")),
            Ok(Frame::Closed),
            Ok(text("turn.end")),
        ];
        let e = collect(frames).unwrap_err();
        assert!(e.contains("closed before turn.end"), "{e}");
        assert!(e.contains("3 bytes"), "{e}");
    }

    #[test]
    fn the_stream_ending_before_turn_end_is_a_failure_not_a_short_clip() {
        let frames = vec![Ok(audio_frame(b"abc"))];
        assert!(collect(frames)
            .unwrap_err()
            .contains("ended before turn.end"));
    }

    #[test]
    fn a_socket_error_throws_the_audio_away() {
        let frames = vec![
            Ok(audio_frame(b"abc")),
            Err("receive failed".to_string()),
            Ok(text("turn.end")),
        ];
        assert!(collect(frames)
            .unwrap_err()
            .starts_with("receive failed; discarded 3 bytes"));
    }

    #[test]
    fn turn_end_without_audio_is_a_failure() {
        assert!(collect(vec![Ok(text("turn.start")), Ok(text("turn.end"))]).is_err());
    }

    #[test]
    fn binary_frames_for_other_paths_are_ignored() {
        let header = b"Path:audio.metadata\r\n";
        let mut f = (header.len() as u16).to_be_bytes().to_vec();
        f.extend_from_slice(header);
        f.extend_from_slice(b"xyz");
        assert_eq!(audio_payload(&f).unwrap(), None);
        let frames = vec![
            Ok(Frame::Binary(f)),
            Ok(audio_frame(b"a")),
            Ok(text("turn.end")),
        ];
        assert_eq!(collect(frames).unwrap(), b"a");
    }

    #[test]
    fn malformed_binary_frames_are_errors() {
        assert!(audio_payload(&[0]).is_err());
        assert!(audio_payload(&[0, 50, b'P']).is_err());
    }

    #[test]
    fn the_token_is_stable_within_five_minutes_and_changes_after() {
        let t = 1_793_000_100;
        let base = t - (t + WINDOWS_EPOCH) % 300; // the start of a 5-minute window
        assert_eq!(gec(base), gec(base + 299));
        assert_ne!(gec(base), gec(base + 300));
        assert_eq!(gec(base).len(), 64);
        assert!(gec(base)
            .chars()
            .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c)));
    }

    #[test]
    fn the_token_hashes_the_ticks_then_the_client_token() {
        // 13,299,999,900 s after 1601 is on a 5-minute boundary; the expected value was computed
        // separately with Python's hashlib over "132999999000000000" followed by the client token.
        let unix = 13_299_999_900 - WINDOWS_EPOCH;
        let expect = "C97F235FE3E00DCE5E10CFEFF69CF0A379F0A82EB0E73F56BC612371DB1DB2F9";
        assert_eq!(gec(unix), expect);
        assert_eq!(gec(unix + 299), expect);
    }

    #[test]
    fn sha256_matches_the_standard() {
        assert_eq!(
            hex_upper(&sha256(b"abc")),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
    }

    #[test]
    fn dates_read_like_javascript() {
        assert_eq!(
            js_date(0),
            "Thu Jan 01 1970 00:00:00 GMT+0000 (Coordinated Universal Time)"
        );
        assert_eq!(
            js_date(1_792_933_509),
            "Sun Oct 25 2026 13:05:09 GMT+0000 (Coordinated Universal Time)"
        );
        assert_eq!(
            js_date(1_709_251_199),
            "Thu Feb 29 2024 23:59:59 GMT+0000 (Coordinated Universal Time)"
        );
    }

    #[test]
    fn text_cannot_become_markup() {
        let s = ssml("en-GB-SoniaNeural", 0, "Tom & Jerry <break time='9s'/>");
        assert!(s.contains("Tom &amp; Jerry &lt;break time=&apos;9s&apos;/&gt;"));
        assert!(s.contains("xml:lang='en-GB'"));
        assert!(s.contains("rate='+0%'"));
        assert!(ssml("en-US-GuyNeural", -10, "x").contains("rate='-10%'"));
    }
}
