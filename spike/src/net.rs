//! HTTPS through WinHTTP and keys from Credential Manager, as in Photo Scribe's `net.rs` and
//! `credential.rs` (same author, reused by design; CLAUDE.md). Keys are read, never written.

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::Security::Credentials::*;

pub const OPENROUTER: &str = "Photo Scribe:OpenRouter";

/// A generic credential's secret, UTF-16 (Credential Manager's own dialog) or UTF-8.
pub fn read_key(target: &str) -> Option<String> {
    unsafe {
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        CredReadW(&HSTRING::from(target), CRED_TYPE_GENERIC, None, &mut cred).ok()?;
        let c = &*cred;
        let blob = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let text = if blob.len() % 2 == 0 && blob.len() >= 2 && blob[1] == 0 {
            let units: Vec<u16> = blob
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            String::from_utf8_lossy(blob).into_owned()
        };
        CredFree(cred as _);
        let text = text.trim().to_string();
        (!text.is_empty()).then_some(text)
    }
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

/// POST `body` to `https://{host}{path}`; returns (status, body). Waits up to `wait_ms` for the answer.
pub fn post(
    host: &str,
    path: &str,
    headers: &[String],
    body: &str,
    wait_ms: i32,
) -> Result<(u32, String), String> {
    unsafe {
        let session = Handle(WinHttpOpen(
            &HSTRING::from("VideoScribe-spike/0"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return Err(last_error("could not start WinHTTP"));
        }
        let _ = WinHttpSetTimeouts(session.0, 30_000, 30_000, 120_000, wait_ms);
        let conn = Handle(WinHttpConnect(session.0, &HSTRING::from(host), 443, 0));
        if conn.0.is_null() {
            return Err(last_error("could not connect"));
        }
        let req = Handle(WinHttpOpenRequest(
            conn.0,
            &HSTRING::from("POST"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return Err(last_error("could not create the request"));
        }
        let h: Vec<u16> = headers.join("\r\n").encode_utf16().collect();
        let b = body.as_bytes();
        WinHttpSendRequest(
            req.0,
            Some(&h),
            Some(b.as_ptr() as _),
            b.len() as u32,
            b.len() as u32,
            0,
        )
        .map_err(|_| last_error("could not send the request"))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|_| last_error("no answer"))?;
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
            let mut avail = 0u32;
            WinHttpQueryDataAvailable(req.0, &mut avail)
                .map_err(|_| last_error("the answer was cut off"))?;
            if avail == 0 {
                break;
            }
            let start = data.len();
            data.resize(start + avail as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(req.0, data[start..].as_mut_ptr() as _, avail, &mut read)
                .map_err(|_| last_error("the answer was cut off"))?;
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
