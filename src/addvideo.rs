//! The Add a video window (File, Add a video, Ctrl+N): a web address or a
//! file to describe, and what will be sent (log entry 43). What was the main
//! window's Video box and summary, before the list of videos took its place.

use std::path::Path;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::choices::{self, Source};
use crate::config::Config;
use crate::message;
use crate::ui::{caret_to_start, focus, get_text, set_text};

const IDD_ADD: u16 = 108;
const IDC_A_SOURCE: i32 = 1090;
const IDC_A_FILE: i32 = 1091;
const IDC_A_PLAN: i32 = 1092;

const EN_SETFOCUS: u32 = 0x0100;
const EN_CHANGE: u32 = 0x0300;

struct State<'a> {
    cfg: &'a Config,
    /// What the Video box starts with.
    text: String,
    chosen: Option<Source>,
}

/// Asks for a video, starting with `text` in the Video box. The video to
/// describe, or None when the user cancelled.
pub fn show(owner: HWND, cfg: &Config, text: &str) -> Option<Source> {
    let mut state = State {
        cfg,
        text: text.to_string(),
        chosen: None,
    };
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        // Ctrl+O chooses a file here too, as in the main window (test round
        // 3, section 3).
        let hook = SetWindowsHookExW(WH_MSGFILTER, Some(filter), None, GetCurrentThreadId()).ok();
        DialogBoxParamW(
            Some(instance.into()),
            PCWSTR(IDD_ADD as usize as *const u16),
            Some(owner),
            Some(proc),
            LPARAM(&mut state as *mut State as isize),
        );
        if let Some(h) = hook {
            let _ = UnhookWindowsHookEx(h);
        }
        WINDOW.store(0, Ordering::Relaxed);
    }
    state.chosen
}

/// The Add a video window while it is open, for the Ctrl+O filter.
static WINDOW: AtomicIsize = AtomicIsize::new(0);

/// Ctrl+O in the Add a video window, and nowhere else: not in the file
/// picker it opens, which runs its own dialog on this thread.
unsafe extern "system" fn filter(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let down =
        |k: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| GetKeyState(k.0 as i32) < 0;
    if code == MSGF_DIALOGBOX as i32 {
        if let Some(msg) = (lparam.0 as *const MSG).as_ref() {
            let window = WINDOW.load(Ordering::Relaxed);
            if msg.message == WM_KEYDOWN
                && msg.wParam.0 == b'O' as usize
                && down(VK_CONTROL)
                && !down(VK_MENU)
                && window != 0
                && GetAncestor(msg.hwnd, GA_ROOT).0 as isize == window
            {
                let _ = PostMessageW(
                    Some(HWND(window as *mut _)),
                    WM_COMMAND,
                    WPARAM(IDC_A_FILE as usize),
                    LPARAM(0),
                );
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe fn state<'a>(hwnd: HWND) -> Option<&'a mut State<'a>> {
    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut()
}

fn update_plan(hwnd: HWND, cfg: &Config) {
    let source = choices::parse_source(&get_text(hwnd, IDC_A_SOURCE)).ok();
    set_text(hwnd, IDC_A_PLAN, &choices::plan(cfg, source.as_ref()));
}

unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match message {
        WM_INITDIALOG => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
            WINDOW.store(hwnd.0 as isize, Ordering::Relaxed);
            if let Some(s) = state(hwnd) {
                set_text(hwnd, IDC_A_SOURCE, &s.text);
                update_plan(hwnd, s.cfg);
            }
            focus(hwnd, IDC_A_SOURCE);
            0
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            let Some(s) = state(hwnd) else { return 0 };
            match (id, code) {
                (IDC_A_SOURCE, EN_CHANGE) => update_plan(hwnd, s.cfg),
                (IDC_A_PLAN, EN_SETFOCUS) => caret_to_start(hwnd, IDC_A_PLAN),
                (IDC_A_FILE, _) => choose_file(hwnd, s),
                (id, _) if id == IDOK.0 => {
                    match choices::parse_source(&get_text(hwnd, IDC_A_SOURCE)) {
                        Ok(source) => {
                            s.chosen = Some(source);
                            let _ = EndDialog(hwnd, IDOK.0 as isize);
                        }
                        Err(why) => {
                            message::problem(hwnd, &why);
                            focus(hwnd, IDC_A_SOURCE);
                        }
                    }
                }
                (id, _) if id == IDCANCEL.0 => {
                    let _ = EndDialog(hwnd, IDCANCEL.0 as isize);
                }
                _ => return 0,
            }
            1
        }
        _ => 0,
    }
}

fn choose_file(hwnd: HWND, s: &mut State) {
    let current = choices::parse_source(&get_text(hwnd, IDC_A_SOURCE)).ok();
    let start = match &current {
        Some(Source::File(p)) => p.parent().map(Path::to_path_buf),
        _ => None,
    };
    if let Some(path) = crate::ui::pick(hwnd, start.as_deref(), "Choose a video to describe", false)
    {
        set_text(hwnd, IDC_A_SOURCE, &path.display().to_string());
        update_plan(hwnd, s.cfg);
        crate::announce::say(
            hwnd,
            &format!(
                "Chosen: {}. Press Enter to start.",
                crate::ui::file_name(&path)
            ),
        );
    }
    // Windows would otherwise restore an older focus.
    focus(hwnd, IDC_A_SOURCE);
}
