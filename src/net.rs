//! HTTPS through WinHTTP, which is part of Windows: no TLS crate to ship.
//! Synchronous; call it from the worker thread, never the UI thread.
//!
//! A file download (yt-dlp), and the POST requests to the describing
//! services, as in Photo Scribe's net.rs and credential.rs.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

const AGENT: &str = concat!(
    "VideoScribe/",
    env!("CARGO_PKG_VERSION"),
    " (Windows app that adds audio description to videos)"
);

/// Closes a WinHTTP handle when dropped.
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
    let e = windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code));
    let plain = match code {
        12002 => "the server took too long to answer".to_string(),
        12007 => {
            "the server's name could not be found; is the internet connection working?".to_string()
        }
        12029 | 12030 => "the connection to the server failed or was dropped".to_string(),
        12175 => "the secure connection could not be set up".to_string(),
        _ => e.message().trim().to_string(),
    };
    format!("{what}: {plain} (WinHTTP error {code})")
}

/// A file from `https://{host}{path}`, following redirects, reporting (bytes
/// so far, total or 0) as it arrives. None if `stop` is set on the way.
pub fn download(
    host: &str,
    path: &str,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Option<Vec<u8>>, String> {
    unsafe {
        let session = Handle(WinHttpOpen(
            &HSTRING::from(AGENT),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return Err(last_error("Could not start WinHTTP"));
        }
        let _ = WinHttpSetTimeouts(session.0, 30_000, 30_000, 60_000, 60_000);
        let connect = Handle(WinHttpConnect(session.0, &HSTRING::from(host), 443, 0));
        if connect.0.is_null() {
            return Err(last_error(&format!("Could not connect to {host}")));
        }
        let req = Handle(WinHttpOpenRequest(
            connect.0,
            &HSTRING::from("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return Err(last_error("Could not create the request"));
        }
        WinHttpSendRequest(req.0, None, None, 0, 0, 0)
            .map_err(|_| last_error(&format!("Could not send the request to {host}")))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut())
            .map_err(|_| last_error(&format!("No answer from {host}")))?;
        let number = |query| {
            let mut n = 0u32;
            let mut size = 4u32;
            let _ = WinHttpQueryHeaders(
                req.0,
                query | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some(&mut n as *mut u32 as _),
                &mut size,
                std::ptr::null_mut(),
            );
            n
        };
        let status = number(WINHTTP_QUERY_STATUS_CODE);
        if status != 200 {
            return Err(format!("{host} answered with status {status}"));
        }
        let total = number(WINHTTP_QUERY_CONTENT_LENGTH) as u64;
        let mut data = Vec::with_capacity(total as usize);
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let mut available = 0u32;
            WinHttpQueryDataAvailable(req.0, &mut available)
                .map_err(|_| last_error(&format!("The download from {host} was cut off")))?;
            if available == 0 {
                break;
            }
            let start = data.len();
            data.resize(start + available as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(req.0, data[start..].as_mut_ptr() as _, available, &mut read)
                .map_err(|_| last_error(&format!("The download from {host} was cut off")))?;
            data.truncate(start + read as usize);
            progress(data.len() as u64, total);
        }
        Ok(Some(data))
    }
}

/// POSTs `body` to `https://{host}{path}` and returns (status, answer).
/// Waits up to `wait_ms` for the answer: a describing request can take a minute.
pub fn post(
    host: &str,
    path: &str,
    headers: &[String],
    body: &[u8],
    wait_ms: i32,
) -> Result<(u32, String), String> {
    unsafe {
        let session = Handle(WinHttpOpen(
            &HSTRING::from(AGENT),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return Err(last_error("Could not start WinHTTP"));
        }
        let _ = WinHttpSetTimeouts(session.0, 30_000, 30_000, 120_000, wait_ms);
        let connect = Handle(WinHttpConnect(session.0, &HSTRING::from(host), 443, 0));
        if connect.0.is_null() {
            return Err(last_error(&format!("Could not connect to {host}")));
        }
        let req = Handle(WinHttpOpenRequest(
            connect.0,
            &HSTRING::from("POST"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return Err(last_error("Could not create the request"));
        }
        let h: Vec<u16> = headers.join("\r\n").encode_utf16().collect();
        WinHttpSendRequest(
            req.0,
            Some(&h),
            Some(body.as_ptr() as _),
            body.len() as u32,
            body.len() as u32,
            0,
        )
        .map_err(|_| last_error(&format!("Could not send the request to {host}")))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut())
            .map_err(|_| last_error(&format!("No answer from {host}")))?;
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
        let mut data = Vec::new();
        loop {
            let mut available = 0u32;
            WinHttpQueryDataAvailable(req.0, &mut available)
                .map_err(|_| last_error(&format!("The answer from {host} was cut off")))?;
            if available == 0 {
                break;
            }
            let start = data.len();
            data.resize(start + available as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(req.0, data[start..].as_mut_ptr() as _, available, &mut read)
                .map_err(|_| last_error(&format!("The answer from {host} was cut off")))?;
            data.truncate(start + read as usize);
        }
        Ok((status, String::from_utf8_lossy(&data).into_owned()))
    }
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

/// A key from Windows Credential Manager, never from a file. Read only:
/// Video Scribe does not write keys yet.
pub fn read_key(target: &str) -> Option<String> {
    use windows::Win32::Security::Credentials::*;
    unsafe {
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        CredReadW(&HSTRING::from(target), CRED_TYPE_GENERIC, None, &mut cred).ok()?;
        let c = &*cred;
        let blob = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let text = decode_key(blob);
        CredFree(cred as _);
        let text = text.trim().to_string();
        (!text.is_empty()).then_some(text)
    }
}

/// Credential Manager's own dialog stores the password as UTF-16; other tools
/// may store UTF-8. Accept either (Photo Scribe's credential.rs).
fn decode_key(blob: &[u8]) -> String {
    if blob.len().is_multiple_of(2) && blob.len() >= 2 && blob[1] == 0 {
        let units: Vec<u16> = blob
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(blob).into_owned()
    }
}

/// SHA-256 through CNG, which is part of Windows.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    use windows::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
    let mut out = [0u8; 32];
    unsafe {
        let _ = BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out);
    }
    out
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_standard() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn keys_decode_in_both_encodings() {
        let utf16: Vec<u8> = "sk-or-abc"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(decode_key(&utf16), "sk-or-abc");
        assert_eq!(decode_key(b"sk-or-abc"), "sk-or-abc");
    }
}
