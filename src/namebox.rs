//! A window with one box for a name: a preset's, or a video's when it is
//! renamed. A name that cannot be a file name is refused with what is wrong
//! and how to write it, and focus goes back to the box
//! (accessible-windows-apps checklist).

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::message;
use crate::ui::{focus, get_text, set_text};

const EM_SETSEL: u32 = 0x00B1;

const IDD_NAME: u16 = 106;
const IDC_N_NAME: i32 = 1070;
const IDC_N_LABEL: i32 = 1071;

struct Asked<'a> {
    /// The window's title and the box's label; None keeps the preset's.
    words: Option<(&'a str, &'a str)>,
    name: String,
    check: fn(&str) -> Option<String>,
}

/// A preset's name, or None if the user cancelled.
pub fn show(owner: HWND) -> Option<String> {
    run(owner, None, "", crate::presets::bad_name)
}

/// A name, starting from `name`, with `title` and `label` (which holds the
/// box's letter); None if the user cancelled.
pub fn ask(
    owner: HWND,
    title: &str,
    label: &str,
    name: &str,
    check: fn(&str) -> Option<String>,
) -> Option<String> {
    run(owner, Some((title, label)), name, check)
}

fn run(
    owner: HWND,
    words: Option<(&str, &str)>,
    name: &str,
    check: fn(&str) -> Option<String>,
) -> Option<String> {
    let mut asked = Asked {
        words,
        name: name.to_string(),
        check,
    };
    let ok = unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        DialogBoxParamW(
            Some(instance.into()),
            PCWSTR(IDD_NAME as usize as *const u16),
            Some(owner),
            Some(proc),
            LPARAM(&mut asked as *mut Asked as isize),
        ) == IDOK.0 as isize
    };
    ok.then_some(asked.name)
}

unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match message {
        WM_INITDIALOG => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
            let asked = &*(lparam.0 as *const Asked);
            if let Some((title, label)) = asked.words {
                let t = crate::wide(title);
                let _ = SetWindowTextW(hwnd, PCWSTR(t.as_ptr()));
                set_text(hwnd, IDC_N_LABEL, label);
                set_text(hwnd, IDOK.0, "OK");
            }
            set_text(hwnd, IDC_N_NAME, &asked.name);
            focus(hwnd, IDC_N_NAME);
            // Every letter chosen, so typing replaces the old name.
            let _ = SendDlgItemMessageW(hwnd, IDC_N_NAME, EM_SETSEL, WPARAM(0), LPARAM(-1));
            0
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            if id == IDOK.0 {
                let typed = get_text(hwnd, IDC_N_NAME).trim().to_string();
                let asked = &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Asked);
                if let Some(why) = (asked.check)(&typed) {
                    message::problem(hwnd, &why);
                    focus(hwnd, IDC_N_NAME);
                    return 1;
                }
                asked.name = typed;
                let _ = EndDialog(hwnd, IDOK.0 as isize);
                return 1;
            }
            if id == IDCANCEL.0 {
                let _ = EndDialog(hwnd, IDCANCEL.0 as isize);
                return 1;
            }
            0
        }
        _ => 0,
    }
}
