//! The People in this video window (milestone 5, log entry 28): a name for
//! each label the descriptions used, put in on this computer, and a note on
//! who is in the video, sent with the pictures. Opened with Ctrl+P, or People
//! in this video in the Describe menu, before or after describing.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::describe::Cast;
use crate::people::People;
use crate::ui::{focus, get_text, set_text};
use crate::wide;

const IDD_PEOPLE: u16 = 103;
const IDC_P_LIST: i32 = 1040;
const IDC_P_NAME: i32 = 1041;
const IDC_P_WHO: i32 = 1042;
const IDC_P_USES: i32 = 1043;

const LBN_SELCHANGE: u32 = 1;
const EN_CHANGE: u32 = 0x0300;

/// Shown when there is nothing to name yet. Never greyed out (PLAN.md §5.10).
const NOTHING_YET: &str =
    "No labels yet: describe this video first, then come back here to name the people in it.";

struct State {
    /// Each label, with its name, or an empty name.
    names: Vec<(String, String)>,
    who: String,
    /// Labels that are names the model heard spoken (log entry 28, Q4).
    heard: Vec<String>,
    /// How each label's person looked, and the descriptions, so the user
    /// can tell who "a bald man" is before naming him (log entry 31).
    looks: Vec<(String, String)>,
    texts: Vec<(usize, String)>,
    /// Set while the list is being refilled, so its own changes are ignored.
    filling: bool,
}

/// Opens the window for one video, with what its descriptions said about
/// its people. The changed people if the user chose OK, None if they
/// cancelled.
pub fn show(owner: HWND, people: &People, cast: &Cast) -> Option<People> {
    // Old labels last (round 3 test, 4.1); otherwise in order of first use.
    let mut names = crate::people::merged(people, &cast.labels);
    names.sort_by_key(|(l, _)| {
        !cast
            .texts
            .iter()
            .any(|(_, t)| crate::people::mentions(t, l))
    });
    let mut state = State {
        names,
        who: people.who.clone(),
        heard: cast.heard.clone(),
        looks: cast.looks.clone(),
        texts: cast.texts.clone(),
        filling: false,
    };
    let ok = unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        DialogBoxParamW(
            Some(instance.into()),
            PCWSTR(IDD_PEOPLE as usize as *const u16),
            Some(owner),
            Some(proc),
            LPARAM(&mut state as *mut State as isize),
        ) == IDOK.0 as isize
    };
    ok.then(|| People {
        names: state.names,
        who: state.who.trim().to_string(),
    })
}

/// A label's line in the list. A label no description uses says so: a name
/// for it is never said (the wedding's "a grey-haired mother of the bride",
/// milestone 8 test, 2.3).
fn item(label: &str, name: &str, heard: bool, used: bool) -> String {
    let label = if heard {
        format!("{label}, a name the model heard spoken")
    } else {
        label.to_string()
    };
    let line = if name.is_empty() {
        format!("{label}: unnamed, the label is spoken")
    } else {
        format!("{label}: {name}")
    };
    if used {
        line
    } else {
        // Said first, as "old label" (round 2 test, 3.4).
        format!("Old label, {line}. No description uses it now, so a name here is not said")
    }
}

unsafe fn state<'a>(hwnd: HWND) -> &'a mut State {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State)
}

fn selected(hwnd: HWND) -> Option<usize> {
    let i = unsafe { SendDlgItemMessageW(hwnd, IDC_P_LIST, LB_GETCURSEL, WPARAM(0), LPARAM(0)) };
    (i.0 >= 0).then_some(i.0 as usize)
}

/// Fills the list, choosing `at`.
fn fill(hwnd: HWND, s: &mut State, at: usize) {
    s.filling = true;
    unsafe {
        SendDlgItemMessageW(hwnd, IDC_P_LIST, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        let lines: Vec<String> = if s.names.is_empty() {
            vec![NOTHING_YET.to_string()]
        } else {
            s.names
                .iter()
                .map(|(l, n)| {
                    let heard = s.heard.iter().any(|h| h.eq_ignore_ascii_case(l));
                    let used = s.texts.iter().any(|(_, t)| crate::people::mentions(t, l));
                    item(l, n, heard, used)
                })
                .collect()
        };
        for line in &lines {
            let w = wide(line);
            SendDlgItemMessageW(
                hwnd,
                IDC_P_LIST,
                LB_ADDSTRING,
                WPARAM(0),
                LPARAM(w.as_ptr() as isize),
            );
        }
        SendDlgItemMessageW(hwnd, IDC_P_LIST, LB_SETCURSEL, WPARAM(at), LPARAM(0));
    }
    s.filling = false;
}

/// What the descriptions say about one label: how that person looked, then
/// each description that mentions them.
pub fn uses(label: &str, looks: &[(String, String)], texts: &[(usize, String)]) -> String {
    let mut lines = Vec::new();
    if let Some((_, look)) = looks.iter().find(|(l, _)| l.eq_ignore_ascii_case(label)) {
        lines.push(format!("Looks: {look}"));
    }
    let used: Vec<String> = texts
        .iter()
        .filter(|(_, t)| crate::people::mentions(t, label))
        .map(|(p, t)| format!("Place {p}: {t}"))
        .collect();
    if used.is_empty() {
        lines.push("An old label: no description uses it now.".to_string());
    } else {
        lines.extend(used);
    }
    lines.join("\r\n")
}

/// Puts the chosen label's name in the name box, and its uses in theirs.
fn show_name(hwnd: HWND, s: &mut State) {
    let chosen = selected(hwnd).and_then(|i| s.names.get(i));
    let name = chosen.map(|(_, n)| n.clone()).unwrap_or_default();
    let about = match chosen {
        Some((l, _)) => uses(l, &s.looks, &s.texts),
        None => NOTHING_YET.to_string(),
    };
    s.filling = true;
    set_text(hwnd, IDC_P_NAME, &name);
    set_text(hwnd, IDC_P_USES, &about);
    s.filling = false;
}

unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match message {
        WM_INITDIALOG => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
            let s = state(hwnd);
            fill(hwnd, s, 0);
            show_name(hwnd, s);
            set_text(hwnd, IDC_P_WHO, &s.who);
            // Before describing there is nothing to name: start on the note.
            focus(
                hwnd,
                if s.names.is_empty() {
                    IDC_P_WHO
                } else {
                    IDC_P_LIST
                },
            );
            0
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            let s = state(hwnd);
            match (id, code) {
                (IDC_P_LIST, LBN_SELCHANGE) if !s.filling => show_name(hwnd, s),
                (IDC_P_NAME, EN_CHANGE) if !s.filling => {
                    // Typed with "no labels yet" chosen: there is nothing to name.
                    if let Some(i) = selected(hwnd).filter(|&i| i < s.names.len()) {
                        s.names[i].1 = get_text(hwnd, IDC_P_NAME).trim().to_string();
                        fill(hwnd, s, i);
                    }
                }
                (IDC_P_WHO, EN_CHANGE) => s.who = get_text(hwnd, IDC_P_WHO),
                // Read from the top each time (round 3 test, 4.2).
                (IDC_P_USES, EN_SETFOCUS) => crate::ui::caret_to_start(hwnd, IDC_P_USES),
                _ if id == IDOK.0 || id == IDCANCEL.0 => {
                    let _ = EndDialog(hwnd, id as isize);
                    return 1;
                }
                _ => {}
            }
            0
        }
        _ => 0,
    }
}
