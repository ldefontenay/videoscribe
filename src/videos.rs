//! The list of videos in the main window (log entry 43): every video's
//! folder in the output folder, most recently made first, and what is done
//! to one from the list itself: renaming it, with its files, moving it to
//! the Recycle Bin, and its context menu. After NoteFetch's list.

use std::time::SystemTime;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, POINT, RECT, SYSTEMTIME, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::UI::Shell::{
    SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE,
    SHFILEOPSTRUCTW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::library::{self, Entry};
use crate::message;
use crate::wide;

/// When, in words with the month in letters and no colon, as NoteFetch's
/// names have it: "1 Nov 2026, 2130".
pub fn when(t: SystemTime) -> String {
    let Ok(since) = t.duration_since(SystemTime::UNIX_EPOCH) else {
        return String::new();
    };
    // 100-nanosecond steps since 1601.
    let ticks = since.as_nanos() as u64 / 100 + 116_444_736_000_000_000;
    let ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return String::new();
        }
    }
    const MO: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{} {} {}, {:02}{:02}",
        local.wDay,
        MO[(local.wMonth.clamp(1, 12) - 1) as usize],
        local.wYear,
        local.wHour,
        local.wMinute
    )
}

pub fn item_text(e: &Entry) -> String {
    format!("{}: {}, {}", e.name, e.holds(), when(e.made))
}

/// Fills the list box `lb` with `entries`, choosing the one with the key
/// `key`, else the one at `at`, else the last.
pub fn fill(lb: HWND, entries: &[Entry], key: Option<&str>, at: usize) {
    unsafe {
        SendMessageW(lb, LB_RESETCONTENT, None, None);
        for e in entries {
            let t = wide(&item_text(e));
            SendMessageW(lb, LB_ADDSTRING, None, Some(LPARAM(t.as_ptr() as isize)));
        }
        if entries.is_empty() {
            return;
        }
        let i = key
            .and_then(|k| entries.iter().position(|e| e.key == k))
            .unwrap_or(at.min(entries.len() - 1));
        SendMessageW(lb, LB_SETCURSEL, Some(WPARAM(i)), None);
    }
}

/// Which item of the list box is chosen.
pub fn chosen(lb: HWND) -> Option<usize> {
    usize::try_from(unsafe { SendMessageW(lb, LB_GETCURSEL, None, None) }.0).ok()
}

/// Asks for a new name, and renames the video's folder and files. What to
/// say, or None when the user cancelled or was told why nothing changed.
pub fn rename(owner: HWND, e: &Entry) -> Option<String> {
    let typed = crate::namebox::ask(
        owner,
        "Rename the video - Video Scribe",
        "&New name for the video's folder and its files:",
        &e.name,
        library::bad_name,
    )?;
    match library::rename(&e.folder, &typed) {
        Ok((_, files)) => Some(match files {
            0 => format!("Renamed to {typed}."),
            1 => format!("Renamed to {typed}, and the 1 file in it named after it."),
            n => format!("Renamed to {typed}, and the {n} files in it named after it."),
        }),
        Err(why) => {
            message::problem(owner, &why);
            None
        }
    }
}

/// Asks, then moves the video's folder to the Recycle Bin. What to say.
pub fn delete(owner: HWND, e: &Entry) -> String {
    let sure = message::ask(
        owner,
        &format!(
            "Move the folder {}, with everything in it, to the Recycle Bin? It can be restored \
             from there. The descriptions are kept, so describing the video again sends nothing.",
            e.name
        ),
        "&Delete",
        "&Keep it",
        false,
    );
    if !sure {
        return "Nothing was deleted.".into();
    }
    let mut from = wide(&e.folder.display().to_string());
    // The list of paths ends with two nulls.
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: owner,
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    let result = unsafe { SHFileOperationW(&mut op) };
    if result != 0 || op.fAnyOperationsAborted.as_bool() || e.folder.exists() {
        message::problem(
            owner,
            &format!(
                "{} could not be moved to the Recycle Bin. A file in it may be open in another \
                 program, such as a media player: close it and try again.",
                e.name
            ),
        );
        return "Nothing was deleted.".into();
    }
    format!("{} was moved to the Recycle Bin.", e.name)
}

/// A context menu of `items` (a command and its words; 0 for a line), the
/// first in bold as what Enter does, at the mouse or, from the keyboard,
/// under the chosen video. The command chosen, or None.
pub fn context_menu(
    owner: HWND,
    lb: HWND,
    at_mouse: Option<(i32, i32)>,
    items: &[(i32, &str)],
) -> Option<i32> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for (id, text) in items {
            if *id == 0 {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            } else {
                let t = wide(text);
                let _ = AppendMenuW(menu, MF_STRING, *id as usize, PCWSTR(t.as_ptr()));
            }
        }
        let _ = SetMenuDefaultItem(menu, items[0].0 as u32, 0);
        let (x, y) = match at_mouse {
            Some(p) => p,
            None => {
                let mut r = RECT::default();
                let i = chosen(lb).unwrap_or(0);
                SendMessageW(
                    lb,
                    LB_GETITEMRECT,
                    Some(WPARAM(i)),
                    Some(LPARAM(&mut r as *mut _ as isize)),
                );
                let mut p = POINT {
                    x: r.left + 16,
                    y: r.bottom,
                };
                let _ = ClientToScreen(lb, &mut p);
                (p.x, p.y)
            }
        };
        let id = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            x,
            y,
            None,
            owner,
            None,
        );
        let _ = DestroyMenu(menu);
        (id.0 != 0).then_some(id.0)
    }
}
