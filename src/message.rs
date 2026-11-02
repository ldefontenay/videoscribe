//! The window that replaces MessageBox: a read-only edit the arrow keys can
//! review, headed by what kind of message it is. See the accessible-windows-apps
//! skill, announcements.md.
//!
//! Taken from Photo Scribe as it stands; the "round" notes refer to Photo
//! Scribe's test rounds, where each behaviour was settled.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::wide;

const IDD_MESSAGE: u16 = 101;
const IDC_MESSAGE: i32 = 1017;
const IDC_MESSAGE_LABEL: i32 = 1018;

// The heading's letter takes focus back to the text; nothing else in the
// window uses a letter, so any first letter will do.
const PROBLEM_HEADING: &str = "&What went wrong, and how to fix it:";
const WELCOME_HEADING: &str = "&Welcome:";
const REPORT_HEADING: &str = "&What was found:";
const ABOUT_HEADING: &str = "&About Video Scribe:";

pub fn problem(owner: HWND, text: &str) {
    show(owner, text, MB_ICONEXCLAMATION, PROBLEM_HEADING);
}

/// The first-run welcome: a note, under its own heading.
pub fn welcome(owner: HWND, text: &str) {
    show(owner, text, MB_ICONINFORMATION, WELCOME_HEADING);
}

/// Help, About. "What happened" sounded as if something had gone wrong
/// (test round 1, 4.3).
pub fn about(owner: HWND, text: &str) {
    show(owner, text, MB_ICONINFORMATION, ABOUT_HEADING);
}

/// A finished job's report, to read with the arrow keys.
pub fn report(owner: HWND, text: &str) {
    show(owner, text, MB_ICONINFORMATION, REPORT_HEADING);
}

struct Shown {
    heading: Vec<u16>,
    body: Vec<u16>,
}

fn show(owner: HWND, text: &str, sound: MESSAGEBOX_STYLE, heading: &str) {
    unsafe {
        // The ding arrives ahead of any speech and says which kind of message this is.
        let _ = MessageBeep(sound);
    }
    let shown = Shown {
        heading: wide(heading),
        // Edit controls want CRLF; a bare newline is read as a stray character.
        body: wide(&text.replace("\r\n", "\n").replace('\n', "\r\n")),
    };
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return;
        };
        DialogBoxParamW(
            Some(instance.into()),
            PCWSTR(IDD_MESSAGE as usize as *const u16),
            Some(owner),
            Some(proc),
            LPARAM(&shown as *const Shown as isize),
        );
    }
}

unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match message {
        WM_INITDIALOG => {
            let shown = &*(lparam.0 as *const Shown);
            let _ = SetDlgItemTextW(hwnd, IDC_MESSAGE_LABEL, PCWSTR(shown.heading.as_ptr()));
            let _ = SetDlgItemTextW(hwnd, IDC_MESSAGE, PCWSTR(shown.body.as_ptr()));
            focus_text_at_start(hwnd, IDC_MESSAGE);
            0 // focus set: on the message itself, that is the point
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            if id == IDOK.0 || id == IDCANCEL.0 {
                let _ = EndDialog(hwnd, id as isize);
                return 1;
            }
            0
        }
        _ => 0,
    }
}

/// Focus on the text with the caret at its start, so the arrow keys read from
/// the top (round 4, 2.1). Letting the dialog choose the focus would select all
/// the text and leave the caret at the end.
unsafe fn focus_text_at_start(hwnd: HWND, id: i32) {
    if let Ok(edit) = GetDlgItem(Some(hwnd), id) {
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(edit));
        SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(0)));
        SendMessageW(edit, EM_SCROLLCARET, None, None);
    }
}

const EM_SETSEL: u32 = 0x00B1;
const EM_SCROLLCARET: u32 = 0x00B7;

/// When the safe answer is "no", it becomes the first button, on the left and
/// first after the text in tab order: Tab from the question lands on it, so
/// the dangerous answer is never the first one heard (round 4, 8.2).
unsafe fn safe_answer_first(hwnd: HWND) {
    let (Ok(text), Ok(yes), Ok(no)) = (
        GetDlgItem(Some(hwnd), IDC_ASK_TEXT),
        GetDlgItem(Some(hwnd), IDOK.0),
        GetDlgItem(Some(hwnd), IDCANCEL.0),
    ) else {
        return;
    };
    let place = |w: HWND| {
        let mut r = windows::Win32::Foundation::RECT::default();
        let _ = GetWindowRect(w, &mut r);
        let mut pts = [
            windows::Win32::Foundation::POINT {
                x: r.left,
                y: r.top,
            },
            windows::Win32::Foundation::POINT {
                x: r.right,
                y: r.bottom,
            },
        ];
        windows::Win32::Graphics::Gdi::MapWindowPoints(None, Some(hwnd), &mut pts);
        pts[0]
    };
    let (yes_at, no_at) = (place(yes), place(no));
    let _ = SetWindowPos(
        no,
        Some(text),
        no_at.x.min(yes_at.x),
        yes_at.y,
        0,
        0,
        SWP_NOSIZE,
    );
    let _ = SetWindowPos(
        yes,
        Some(no),
        yes_at.x.max(no_at.x),
        yes_at.y,
        0,
        0,
        SWP_NOSIZE,
    );
    // The group, for the arrow keys, now starts at the first button.
    let style = |w: HWND, on: bool| {
        let s = GetWindowLongPtrW(w, GWL_STYLE);
        let g = WS_GROUP.0 as isize;
        SetWindowLongPtrW(w, GWL_STYLE, if on { s | g } else { s & !g });
    };
    style(no, true);
    style(yes, false);
}

const IDD_ASK: u16 = 102;
const IDC_ASK_TEXT: i32 = 1020;
// DM_GETDEFID is WM_USER; sending that by mistake left yes as the default
// (round 6, 1.1).
const DM_SETDEFID: u32 = WM_USER + 1;

struct Asked {
    body: Vec<u16>,
    yes: Vec<u16>,
    no: Vec<u16>,
    yes_is_default: bool,
}

/// A question with two answers, labelled by what they do ("&Download",
/// "&Not now"). True for `yes`. Enter gives the default answer, Escape `no`;
/// make the default the answer that cannot lose anything.
pub fn ask(owner: HWND, text: &str, yes: &str, no: &str, yes_is_default: bool) -> bool {
    unsafe {
        let _ = MessageBeep(MB_ICONQUESTION);
    }
    let asked = Asked {
        body: wide(&text.replace("\r\n", "\n").replace('\n', "\r\n")),
        yes: wide(yes),
        no: wide(no),
        yes_is_default,
    };
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return false;
        };
        DialogBoxParamW(
            Some(instance.into()),
            PCWSTR(IDD_ASK as usize as *const u16),
            Some(owner),
            Some(ask_proc),
            LPARAM(&asked as *const Asked as isize),
        ) == IDOK.0 as isize
    }
}

unsafe extern "system" fn ask_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match message {
        WM_INITDIALOG => {
            let asked = &*(lparam.0 as *const Asked);
            let _ = SetDlgItemTextW(hwnd, IDC_ASK_TEXT, PCWSTR(asked.body.as_ptr()));
            let _ = SetDlgItemTextW(hwnd, IDOK.0, PCWSTR(asked.yes.as_ptr()));
            let _ = SetDlgItemTextW(hwnd, IDCANCEL.0, PCWSTR(asked.no.as_ptr()));
            let default = if asked.yes_is_default { IDOK } else { IDCANCEL };
            SendMessageW(hwnd, DM_SETDEFID, Some(WPARAM(default.0 as usize)), None);
            // The styles too, so the default is also marked on the button.
            for (id, style) in [
                (default.0, BS_DEFPUSHBUTTON),
                (
                    if asked.yes_is_default {
                        IDCANCEL.0
                    } else {
                        IDOK.0
                    },
                    BS_PUSHBUTTON,
                ),
            ] {
                let _ =
                    SendDlgItemMessageW(hwnd, id, BM_SETSTYLE, WPARAM(style as usize), LPARAM(1));
            }
            if !asked.yes_is_default {
                safe_answer_first(hwnd);
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, asked.yes_is_default as isize);
            focus_text_at_start(hwnd, IDC_ASK_TEXT);
            0 // focus set: the question itself first, as in a message
        }
        WM_COMMAND => {
            let mut id = (wparam.0 & 0xFFFF) as i32;
            // Enter in the multi-line question box arrives as IDOK whatever the
            // default button is (round 5, section 1): only a real press of the
            // yes button, which names itself in lParam, counts as yes.
            if id == IDOK.0 {
                let pressed =
                    GetDlgItem(Some(hwnd), IDOK.0).is_ok_and(|b| b.0 as isize == lparam.0);
                if !pressed && GetWindowLongPtrW(hwnd, GWLP_USERDATA) == 0 {
                    id = IDCANCEL.0;
                }
            }
            if id == IDOK.0 || id == IDCANCEL.0 {
                let _ = EndDialog(hwnd, id as isize);
                return 1;
            }
            0
        }
        _ => 0,
    }
}
