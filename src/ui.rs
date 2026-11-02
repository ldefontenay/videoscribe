//! The main window: a modeless dialog from the IDD_MAIN template, so every
//! control is a real Win32 control that a screen reader already knows.
//!
//! Milestone 2 (PLAN.md §8): the window, its choices and the summary of what
//! will be sent. Milestone 3: Start finds the speech and the pauses, on this
//! computer, and shows them as a report. Describing arrives with milestones 4
//! to 7; until then nothing is sent to any describing service.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_DELETE, VK_ESCAPE, VK_F2, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, FOS_FILEMUSTEXIST,
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::choices::{self, Source};
use crate::config::{Config, Service};
use crate::job::{self, Event};
use crate::library;
use crate::message;
use crate::videos;
use crate::wide;

const IDD_MAIN: u16 = 100;
const IDC_STATUS: i32 = 1004;
const IDC_LIST: i32 = 1005;
const IDC_LISTEN: i32 = 1006;
const IDC_REVIEW: i32 = 1007;
const IDC_PEOPLE: i32 = 1008;
const IDC_ADD: i32 = 1009;
const IDC_RENAME: i32 = 1010;
const IDC_SHOW: i32 = 1011;
const IDC_DELETE: i32 = 1012;

// Menu commands, in step with videoscribe.rc.
const IDM_OPEN: i32 = 2001;
const IDM_FOLDER: i32 = 2002;
const IDM_EXPLORER: i32 = 2003;
const IDM_EXIT: i32 = 2004;
const IDM_UPDATE_YTDLP: i32 = 2005;
const IDM_START: i32 = 2010;
const IDM_STOP: i32 = 2011;
const IDM_SEND_NAMES: i32 = 2012;
const IDM_KEEP_DOWNLOADS: i32 = 2013;
const IDM_PEOPLE: i32 = 2014;
const IDM_SHORT_TOO: i32 = 2015;
const IDM_AT_ONCE: i32 = 2016;
const IDM_REVIEW: i32 = 2017;
const IDM_LISTEN: i32 = 2018;
const IDM_KNOWING: i32 = 2019;
const IDM_PRESET_SAVE: i32 = 2020;
const IDM_PRESET_FOLDER: i32 = 2021;
const IDM_ADD: i32 = 2023;
const IDM_RENAME: i32 = 2024;
const IDM_DELETE: i32 = 2025;
const IDM_SHOW: i32 = 2026;
const IDM_FACES: i32 = 2027;
const IDM_VOICES: i32 = 2028;
const IDM_HELP: i32 = 2030;
const IDM_ABOUT: i32 = 2031;
// The first number of each list of choices, filled in as its menu opens.
const IDM_SERVICE: i32 = 2100;
const IDM_MODEL: i32 = 2200;
const IDM_STYLE: i32 = 2300;
const IDM_ROOM: i32 = 2500;
const IDM_OUTPUT: i32 = 2600;
const IDM_RATE: i32 = 2800;
const IDM_PRESET: i32 = 2900;
const IDM_QUALITY: i32 = 3000;

// Where each menu sits, in step with videoscribe.rc.
const OPTIONS_MENU: i32 = 2;
const PROVIDER_MENU: i32 = 0;
const MODEL_MENU: i32 = 1;
const STYLE_MENU: i32 = 2;
#[cfg(test)]
const VOICES_ITEM: i32 = 3;
const SPEED_MENU: i32 = 4;
const ROOM_MENU: i32 = 5;
const OUTPUT_MENU: i32 = 6;
const QUALITY_MENU: i32 = 7;
/// After the five ticked items.
const PRESETS_MENU: i32 = 13;

const EM_SETSEL: u32 = 0x00B1;
const EM_SCROLLCARET: u32 = 0x00B7;
const EN_SETFOCUS: u32 = 0x0100;
const LBN_DBLCLK: u32 = 2;

const WM_WELCOME: u32 = WM_APP + 13;

const WELCOME: &str =
    "Welcome to Video Scribe. It adds audio descriptions to web videos, such as YouTube's, or your own \
videos: spoken descriptions of what can be seen, in the pauses between speech. The result is saved \
as a new audio file; your video is never changed.\n\n\
Press Ctrl+N to add a video: type or paste a web address, or choose a file, then press Enter \
to start. Before anything is sent, Video Scribe shows you exactly what will go to the describing \
service. Each video you add gets a folder of its own, and a place in the list of videos, where \
Enter listens to it and the Applications key opens a menu of what else can be done.\n\n\
The Options menu (Alt+O) chooses the model, the style, the language, the voice and its speed. F1 \
opens the help.\n\n\
This is a test version. The result is an MP3 of the film's sound with the descriptions spoken in it, and a text list of what is said when.";

struct App {
    cfg: Config,
    /// No settings file yet: say hello and explain the choices.
    welcome: bool,
    /// The job running now, by its stop flag.
    job: Option<Arc<AtomicBool>>,
    /// The running job is the one that sends pictures.
    describing: bool,
    /// Describe again knowing these people: the known people to send, until
    /// the job that was started for it takes them.
    knowing: Option<String>,
    /// The key of the video the job running now is on.
    running: Option<String>,
    /// The videos in the list, most recent first.
    entries: Vec<library::Entry>,
    /// The key of the video to choose in the list once its folder is there:
    /// the one a job was started on.
    select: Option<String>,
}

pub fn run(cfg: Config, welcome: bool) {
    let app = Box::new(App {
        cfg,
        welcome,
        job: None,
        describing: false,
        knowing: None,
        running: None,
        entries: Vec::new(),
        select: None,
    });
    let hwnd = unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return;
        };
        match CreateDialogParamW(
            Some(instance.into()),
            PCWSTR(IDD_MAIN as usize as *const u16),
            None,
            Some(dialog_proc),
            LPARAM(Box::into_raw(app) as isize),
        ) {
            Ok(h) => h,
            Err(_) => return,
        }
    };
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    let mut msg = MSG::default();
    loop {
        // Zero is WM_QUIT and -1 an error; as_bool would spin forever on -1.
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        // The menus' shortcuts, for this window only: the other windows are
        // modal and run their own loops.
        if msg.message == WM_KEYDOWN {
            if let Some(id) = shortcut(msg.wParam.0 as u16) {
                unsafe {
                    SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(id as usize)), None);
                }
                continue;
            }
        }
        // The whole keyboard behaviour of the window: Tab, Enter for the
        // default button, Escape, mnemonics.
        if unsafe { IsDialogMessageW(hwnd, &msg) }.as_bool() {
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn app<'a>(hwnd: HWND) -> Option<&'a mut App> {
    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App).as_mut()
}

unsafe extern "system" fn dialog_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match message {
        WM_INITDIALOG => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
            if let Some(app) = app(hwnd) {
                refresh(hwnd, app);
                let ready = match app.entries.len() {
                    0 => "Ready. No videos yet: press Ctrl+N to add one.".to_string(),
                    1 => "Ready. 1 video. Enter listens to it; Ctrl+N adds another.".to_string(),
                    n => format!(
                        "Ready. {n} videos. Enter listens to the chosen one; Ctrl+N adds another."
                    ),
                };
                set_text(hwnd, IDC_STATUS, &ready);
                focus(hwnd, IDC_LIST);
            }
            if app(hwnd).is_some_and(|a| a.welcome) {
                let _ = PostMessageW(Some(hwnd), WM_WELCOME, WPARAM(0), LPARAM(0));
            }
            0 // zero: we set the focus ourselves
        }
        WM_WELCOME => {
            message::welcome(hwnd, WELCOME);
            if let Some(app) = app(hwnd) {
                // Written now, so the welcome comes only once.
                app.cfg.save();
            }
            focus(hwnd, IDC_LIST);
            1
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            let Some(app) = app(hwnd) else { return 0 };
            match (id, code) {
                (IDC_STATUS, EN_SETFOCUS) => caret_to_start(hwnd, IDC_STATUS),
                (IDC_LIST, LBN_DBLCLK) => {
                    command(hwnd, app, IDM_LISTEN);
                }
                (IDC_LIST, _) => return 0,
                // The buttons do what their menu items do. Enter is Listen
                // anywhere it is the default.
                (IDC_LISTEN, _) => {
                    command(hwnd, app, IDM_LISTEN);
                }
                (id, _) if id == IDOK.0 => {
                    command(hwnd, app, IDM_LISTEN);
                }
                (IDC_REVIEW, _) => {
                    command(hwnd, app, IDM_REVIEW);
                }
                (IDC_PEOPLE, _) => {
                    command(hwnd, app, IDM_PEOPLE);
                }
                (IDC_ADD, _) => {
                    command(hwnd, app, IDM_ADD);
                }
                (IDC_RENAME, _) => {
                    command(hwnd, app, IDM_RENAME);
                }
                (IDC_SHOW, _) => {
                    command(hwnd, app, IDM_SHOW);
                }
                (IDC_DELETE, _) => {
                    command(hwnd, app, IDM_DELETE);
                }
                (id, _) if id == IDCANCEL.0 => stop(hwnd),
                // Menus and shortcuts: code 0 or 1, no control.
                (id, 0 | 1) if lparam.0 == 0 => {
                    if !command(hwnd, app, id) {
                        return 0;
                    }
                }
                _ => return 0,
            }
            1
        }
        // Delete in the list (LBS_WANTKEYBOARDINPUT); F2 is a shortcut
        // everywhere in the window.
        WM_VKEYTOITEM => {
            let key = (wparam.0 & 0xFFFF) as u16;
            if key != VK_DELETE.0 {
                return -1;
            }
            if let Some(app) = app(hwnd) {
                command(hwnd, app, IDM_DELETE);
            }
            -2
        }
        // Shift+F10, the Applications key or a right click on the list.
        WM_CONTEXTMENU => {
            let lb = list(hwnd);
            if wparam.0 != lb.0 as usize {
                return 0;
            }
            if let Some(app) = app(hwnd) {
                context_menu(hwnd, app, lparam);
            }
            1
        }
        job::WM_JOB => {
            let event = job::take(lparam);
            if let Some(app) = app(hwnd) {
                job_event(hwnd, app, *event);
            }
            1
        }
        WM_INITMENUPOPUP => {
            if let Some(app) = app(hwnd) {
                init_menu(hwnd, app, HMENU(wparam.0 as *mut _));
            }
            0
        }
        WM_HELP => {
            help(hwnd);
            1
        }
        WM_CLOSE => {
            // A multi-line edit turns Escape into WM_CLOSE on its dialog; only
            // Alt+F4 and the close button should end the app.
            if GetKeyState(VK_ESCAPE.0 as i32) < 0 {
                stop(hwnd);
                return 1;
            }
            if let Some(app) = app(hwnd) {
                app.cfg.save();
                // yt-dlp ends with Video Scribe in any case (ytdlp.rs, Job).
                if let Some(stop) = &app.job {
                    stop.store(true, Ordering::Relaxed);
                }
            }
            let _ = DestroyWindow(hwnd);
            1
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        WM_NCDESTROY => {
            let ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if ptr != 0 {
                drop(Box::from_raw(ptr as *mut App));
            }
            0
        }
        _ => 0,
    }
}

fn list(hwnd: HWND) -> HWND {
    unsafe { GetDlgItem(Some(hwnd), IDC_LIST).unwrap_or_default() }
}

/// Fills the list of videos again from the output folder, keeping the one
/// chosen, or choosing the one a job was just started on.
fn refresh(hwnd: HWND, app: &mut App) {
    let lb = list(hwnd);
    let at = videos::chosen(lb);
    let key = app
        .select
        .take()
        .or_else(|| at.and_then(|i| app.entries.get(i)).map(|e| e.key.clone()));
    app.entries = library::list(&app.cfg.output_folder());
    videos::fill(lb, &app.entries, key.as_deref(), at.unwrap_or(0));
}

/// The video chosen in the list; when there is none, that is said.
fn chosen(hwnd: HWND, app: &App) -> Option<library::Entry> {
    let e = videos::chosen(list(hwnd))
        .and_then(|i| app.entries.get(i))
        .cloned();
    if e.is_none() {
        say(
            hwnd,
            "There is no video in the list. Press Ctrl+N to add one.",
        );
    }
    e
}

/// What the chosen video was made from, to describe it again.
fn source_of(hwnd: HWND, e: &library::Entry) -> Option<Source> {
    let lower = e.source.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(Source::Web(e.source.clone()));
    }
    let path = PathBuf::from(&e.source);
    if e.source.is_empty() {
        message::problem(
            hwnd,
            &format!(
                "Video Scribe does not know where {} came from, so it cannot describe it again. \
                 Add it again with Ctrl+N.",
                e.name
            ),
        );
        return None;
    }
    if !path.is_file() {
        message::problem(
            hwnd,
            &format!(
                "The video {} was made from is no longer at {}, so it cannot be described again. \
                 If it was moved, add it again from where it is now with Ctrl+O.",
                e.name, e.source
            ),
        );
        return None;
    }
    Some(Source::File(path))
}

/// The running job is on this video, so its folder must stay as it is.
fn busy_with(hwnd: HWND, app: &App, e: &library::Entry) -> bool {
    let busy = app.job.is_some() && app.running.as_deref() == Some(e.key.as_str());
    if busy {
        say(
            hwnd,
            "Video Scribe is working on this video now. Wait for it to finish, or press Escape \
             to stop it first.",
        );
    }
    busy
}

/// The context menu of the list, for the video chosen, or the one under
/// the mouse.
fn context_menu(hwnd: HWND, app: &mut App, at: LPARAM) {
    let lb = list(hwnd);
    let (x, y) = (
        (at.0 & 0xFFFF) as i16 as i32,
        ((at.0 >> 16) & 0xFFFF) as i16 as i32,
    );
    let mouse = (x, y) != (-1, -1);
    if mouse {
        // A right click chooses the video it is on, as in File Explorer.
        let mut p = windows::Win32::Foundation::POINT { x, y };
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(lb, &mut p);
            let r = SendMessageW(
                lb,
                LB_ITEMFROMPOINT,
                None,
                Some(LPARAM(
                    ((p.y as isize & 0xFFFF) << 16) | (p.x as isize & 0xFFFF),
                )),
            )
            .0;
            if (r >> 16) & 0xFFFF == 0 {
                SendMessageW(lb, LB_SETCURSEL, Some(WPARAM((r & 0xFFFF) as usize)), None);
            }
        }
    }
    if chosen(hwnd, app).is_none() {
        return;
    }
    let items = [
        (IDM_LISTEN, "&Listen to the described sound\tEnter"),
        (IDM_REVIEW, "&Review the descriptions\tCtrl+R"),
        (IDM_PEOPLE, "&People in this video\tCtrl+P"),
        (0, ""),
        (IDM_START, "Describe &again\tCtrl+D"),
        (
            IDM_KNOWING,
            "Describe again &knowing these people\tCtrl+Shift+D",
        ),
        (0, ""),
        (IDM_RENAME, "Re&name\tF2"),
        (IDM_SHOW, "Show in File E&xplorer"),
        (IDM_DELETE, "&Delete\tDelete"),
    ];
    if let Some(id) = videos::context_menu(hwnd, lb, mouse.then_some((x, y)), &items) {
        command(hwnd, app, id);
    }
}

/// Add a video: the Add a video window, starting from `text`, then
/// describing it.
fn add(hwnd: HWND, app: &mut App, text: &str) {
    if app.job.is_some() {
        say(
            hwnd,
            "Already working. Press Escape to stop, or wait for the report.",
        );
        return;
    }
    match crate::addvideo::show(hwnd, &app.cfg, text) {
        Some(source) => {
            app.knowing = None;
            start(hwnd, app, source);
        }
        None => {
            say(hwnd, "No video added.");
            focus(hwnd, IDC_LIST);
        }
    }
}

fn start(hwnd: HWND, app: &mut App, source: Source) {
    if app.job.is_some() {
        say(
            hwnd,
            "Already working. Press Escape to stop, or wait for the report.",
        );
        return;
    }
    // No "Start?" question here: the user asked for it to go (test round 6,
    // 10.3). Nothing is sent before the "what will be sent" question anyway.
    let mut fetch_ytdlp = false;
    if matches!(source, Source::Web(_)) && crate::ytdlp::find().is_none() {
        fetch_ytdlp = message::ask(
            hwnd,
            "Video Scribe downloads videos with yt-dlp, a free program that is not on this \
             computer yet. Download it now from its official page on GitHub? It is about 18 MB, \
             is checked against its published checksum, and is kept in Video Scribe's own folder.",
            "&Download",
            "&Not now",
            true,
        );
        if !fetch_ytdlp {
            say(
                hwnd,
                "Not started: downloading needs yt-dlp. Nothing was sent.",
            );
            focus(hwnd, IDC_LIST);
            return;
        }
    }
    app.select = crate::people::key(&source);
    app.running = app.select.clone();
    let settings = job::Settings {
        source,
        lang: crate::voices::code(&app.cfg.voice).to_string(),
        output_folder: app.cfg.output_folder(),
        fetch_ytdlp,
        // Sharper pictures only for a described video (log entry 39, Q2).
        lines: if app.cfg.output == crate::config::Output::Video {
            app.cfg.lines
        } else {
            480
        },
    };
    app.job = Some(job::start(hwnd, settings));
    say(hwnd, "Started. Press Escape to stop.");
    focus(hwnd, IDC_STATUS);
}

/// The name of the folder `path` is in: the video's folder.
fn folder_name(path: &Path) -> String {
    path.parent().map_or_else(String::new, file_name)
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn stop(hwnd: HWND) {
    let Some(app) = (unsafe { app(hwnd) }) else {
        return;
    };
    match &app.job {
        Some(flag) => {
            flag.store(true, Ordering::Relaxed);
            say(
                hwnd,
                if app.describing {
                    "Stopping once the request being answered now is back."
                } else {
                    "Stopping."
                },
            );
        }
        None => say(hwnd, "Nothing is running, so there is nothing to stop."),
    }
}

/// News from the job.
fn job_event(hwnd: HWND, app: &mut App, event: Event) {
    match event {
        Event::Step(text) => say(hwnd, &text),
        Event::Progress(text) => set_text(hwnd, IDC_STATUS, &text),
        Event::Mapped(m) => {
            app.job = None;
            refresh(hwnd, app);
            mapped(hwnd, app, m);
        }
        Event::Described { report, path, made } => {
            app.job = None;
            app.running = None;
            app.describing = false;
            refresh(hwnd, app);
            if made {
                set_text(
                    hwnd,
                    IDC_STATUS,
                    &format!(
                        "Finished. The described sound and its list are saved in the folder {}; \
                         the list is {}.",
                        folder_name(&path),
                        file_name(&path)
                    ),
                );
                message::report(
                    hwnd,
                    &format!(
                        "Saved in the folder {}, in the output folder, as {}.\n\n{report}",
                        folder_name(&path),
                        file_name(&path)
                    ),
                );
            } else {
                set_text(
                    hwnd,
                    IDC_STATUS,
                    &format!(
                        "Nothing changed, so nothing was made again. The list is {}.",
                        file_name(&path)
                    ),
                );
                message::report(hwnd, &report);
            }
            focus(hwnd, IDC_LIST);
        }
        Event::Failed(why) => {
            app.job = None;
            app.running = None;
            refresh(hwnd, app);
            let after = if app.describing {
                "What was already described is kept: Describe again, Ctrl+D, with the same \
                 choices, carries on from there."
            } else {
                "Nothing was sent to any describing service."
            };
            set_text(hwnd, IDC_STATUS, &format!("Stopped by a problem. {after}"));
            message::problem(hwnd, &format!("{why}\n\n{after}"));
            app.describing = false;
            focus(hwnd, IDC_LIST);
        }
        Event::Said(text) => {
            app.job = None;
            app.running = None;
            say(hwnd, &text);
        }
        Event::Stopped => {
            app.job = None;
            app.running = None;
            refresh(hwnd, app);
            say(
                hwnd,
                if app.describing {
                    "Stopped. What was already described is kept: Describe again, Ctrl+D, with \
                     the same choices, carries on from there."
                } else {
                    "Stopped. Nothing was sent."
                },
            );
            app.describing = false;
            focus(hwnd, IDC_LIST);
        }
    }
}

/// The pauses are found: say what describing would send, and ask. Without a
/// yes, the speech map report is shown instead and nothing is sent.
fn mapped(hwnd: HWND, app: &mut App, m: Box<job::Mapped>) {
    let check = match &m.check {
        Ok(c) => format!(
            "To check the pauses by ear, play {} from the same folder: it is the video's own \
             sound, with a short high beep where each pause starts and a low beep where it ends.",
            file_name(c)
        ),
        Err(why) => format!("The pause check file could not be made. {why}"),
    };
    let map_report = format!(
        "Saved in the folder {}, in the output folder, as {}.\n\n{check}\n\n{}",
        folder_name(&m.path),
        file_name(&m.path),
        m.report
    );
    let show_map = |why: &str| {
        say(hwnd, &format!("{why} Nothing was sent."));
        message::report(hwnd, &map_report);
        focus(hwnd, IDC_LIST);
    };
    let places = m.gaps.len();
    if places == 0 || m.video.is_none() {
        app.running = None;
    }
    if places == 0 {
        return show_map("No pauses long enough for a description were found.");
    }
    if m.video.is_none() {
        return show_map(
            "Not described: the site offered no picture that Windows can read for this video.",
        );
    }
    let claude = app.cfg.service == Service::Claude;
    let found = if claude {
        crate::claude::find().and_then(|exe| {
            crate::claude::check_subscription(&exe)?;
            Ok((
                crate::describe::Route::Claude { exe },
                app.cfg.claude_model.clone(),
            ))
        })
    } else {
        crate::openrouter::key().map(|key| {
            (
                crate::describe::Route::OpenRouter { key },
                app.cfg.openrouter_model.clone(),
            )
        })
    };
    let (route, model) = match found {
        Ok(f) => f,
        Err(why) => {
            app.running = None;
            message::problem(hwnd, &why);
            return show_map("Not described.");
        }
    };
    // Claude Code takes still pictures only.
    let clips = !claude && choices::watches_video(&model);
    let mut d = job::Describe {
        route,
        model: model.clone(),
        model_label: choices::service_label(&app.cfg),
        language: choices::writing_language(&app.cfg),
        style: choices::style_line(&app.cfg.style).to_string(),
        send_name: app.cfg.send_names,
        clips,
        output_folder: app.cfg.output_folder(),
        voice: app.cfg.voice.clone(),
        rate: app.cfg.rate,
        room: app.cfg.room,
        short_too: app.cfg.short_too,
        wants_video: app.cfg.output == crate::config::Output::Video,
        keep_downloads: app.cfg.keep_downloads,
        people: crate::people::load(&m.saved_as),
        known: app.knowing.take(),
        together: app.cfg.together,
        faces: false,
        fetch_faces: false,
    };
    // Faces, for family videos only (log entry 44). Descriptions kept from
    // before faces stay as they are: only describing afresh tags faces.
    let kept = job::left(&m, &d).len() < places.div_ceil(crate::describe::PLACES_PER_REQUEST);
    if m.family && app.cfg.faces && !kept {
        if crate::faces::models_present() {
            d.faces = true;
        } else {
            d.fetch_faces = message::ask(
                hwnd,
                "Video Scribe can know the people in this video by their faces, on this \
                 computer, so that a name you gave someone, in this video or an earlier one, is \
                 said for them. It needs two small face models first: 39 MB, once, from the \
                 OpenCV project's page on GitHub, checked against their published fingerprints. \
                 Download them now?\n\nNot now describes this video without faces. Options, \
                 \"Know people by their faces in family videos\", turns faces off for good.",
                "&Download",
                "&Not now",
                true,
            );
            d.faces = d.fetch_faces;
        }
    }
    // A stopped job carries on: count only what is still to send (test
    // round 3, 3.3).
    let per = crate::describe::PLACES_PER_REQUEST;
    let requests = places.div_ceil(per);
    let left = job::left(&m, &d);
    let done = requests - left.len().min(requests);
    let rest: Vec<_> = left.into_iter().flatten().collect();
    let rest = &rest[..];
    let pictures = crate::describe::picture_count(rest);
    let cost = match choices::cost_per_hour(&model).filter(|_| !claude) {
        _ if claude => "nothing beyond your Claude plan, but it uses some of the plan's usage, \
                        about one request for every four places"
            .to_string(),
        Some(per_hour) => {
            let c = per_hour * m.secs / 3600.0 * rest.len() as f64 / places as f64;
            if c < 0.01 {
                "under 1 US cent".to_string()
            } else {
                format!("about {:.0} US cents", (c * 100.0).ceil())
            }
        }
        None => "not measured yet for this model".to_string(),
    };
    let mut sent = if clips {
        // Exactly as describe.rs cuts them: per request, then per pause.
        let list: Vec<_> = rest
            .chunks(per)
            .flat_map(|batch| crate::clips::plan(batch, 1))
            .collect();
        let secs: f64 = list.iter().map(|c| c.3 - c.2).sum();
        format!(
            "{} short clips, picture and sound, for {} places: places that follow one another \
             share a clip. They come to {} of the video's {}, because that much of it is pause. \
             A clip holds only pauses, never the speech around them",
            list.len(),
            rest.len(),
            crate::speech::clock(secs),
            crate::speech::clock(m.secs)
        )
    } else {
        format!("{pictures} still pictures, 512 pixels wide, two or three from each place")
    };
    if !m.cues.is_empty() {
        sent += ", and the caption lines around each place";
    }
    if !d.people.who.is_empty() {
        sent += ", and your note on who is in this video";
    }
    if d.faces {
        sent += ", and beside each picture a tag such as \"Person 3\" for each face in it: the \
                 faces are found and compared on this computer, and no name and no measurement of \
                 a face is sent";
    }
    if d.known.is_some() {
        sent += ", and the names you gave people in People in this video, each with how they \
                 looked and the label they had";
    }
    if app.cfg.send_names {
        sent += if m.family {
            ", and the file name"
        } else {
            ", and the video's title"
        };
    }
    let resume = if done > 0 {
        format!(
            " Last time, {done} of {requests} requests were answered before it was stopped. \
             Those answers are kept and not sent again, so this sends only the rest:"
        )
    } else {
        " This sends".to_string()
    };
    let question = if done == requests {
        // Described before with the same choices: only the voice and the
        // mix are new.
        format!(
            "Found {places} places for a description in {}. They were all described last time, \
             with the same model and choices, and those descriptions are kept.\n\nMake the \
             described sound now? {}",
            crate::speech::clock(m.secs),
            if job::edited(&m, &d) {
                "Nothing is sent: only the speaking and mixing are done again."
            } else {
                "No pictures and no sound are sent. Only the descriptions' text goes to the model \
                 once more, to finish the editor pass that did not finish last time."
            }
        )
    } else {
        format!(
            "Found {places} places for a description in {}. The speech map and the pause check \
             file are saved in the folder {}.\n\nDescribe them now?{resume} {sent}. They \
             go to {}. Nothing else is sent: not {}{}. Estimated cost: {cost}.",
            crate::speech::clock(m.secs),
            folder_name(&m.path),
            if claude {
                format!(
                    "{}, to Anthropic, under the privacy settings of your own Claude account",
                    choices::service_label(&app.cfg)
                )
            } else {
                format!(
                    "{}, using only providers that do not keep or train on what they are sent",
                    choices::service_label(&app.cfg)
                )
            },
            if clips {
                "the rest of the sound"
            } else {
                "the sound"
            },
            if app.cfg.send_names {
                ""
            } else if m.family {
                ", and not the file or folder name"
            } else {
                ", the address or the title"
            }
        )
    };
    let go = message::ask(hwnd, &question, "&Describe", "&Not now", false);
    if !go {
        app.running = None;
        return show_map("Not described.");
    }
    app.job = Some(job::describe(hwnd, m, d));
    app.describing = true;
    say(
        hwnd,
        "Describing. Press Escape to stop; what is already described is kept for next time.",
    );
    focus(hwnd, IDC_STATUS);
}

fn submenu(menu: HMENU, pos: i32) -> HMENU {
    unsafe { GetSubMenu(menu, pos) }
}

/// A menu is about to open: tick what is chosen, and fill the lists of
/// choices. Nothing is greyed out (PLAN.md §5.10).
fn init_menu(hwnd: HWND, app: &mut App, menu: HMENU) {
    let bar = unsafe { GetMenu(hwnd) };
    let options = submenu(bar, OPTIONS_MENU);
    let cfg = &app.cfg;
    if menu == submenu(options, PROVIDER_MENU) {
        let items: Vec<String> = choices::SERVICES.iter().map(|s| s.1.to_string()).collect();
        let at = choices::SERVICES.iter().position(|s| s.0 == cfg.service);
        fill_radio(menu, IDM_SERVICE, &items, at);
    } else if menu == submenu(options, MODEL_MENU) {
        let (list, at) = models(cfg);
        let items: Vec<String> = list.into_iter().map(|m| m.1).collect();
        fill_radio(menu, IDM_MODEL, &items, Some(at));
    } else if menu == submenu(options, STYLE_MENU) {
        let items: Vec<String> = choices::STYLES.iter().map(|s| s.1.to_string()).collect();
        let at = choices::STYLES.iter().position(|s| s.0 == cfg.style);
        fill_radio(menu, IDM_STYLE, &items, at);
    } else if menu == submenu(options, ROOM_MENU) {
        let items: Vec<String> = choices::ROOMS.iter().map(|r| r.1.to_string()).collect();
        let at = choices::ROOMS.iter().position(|r| r.0 == cfg.room);
        fill_radio(menu, IDM_ROOM, &items, at);
    } else if menu == submenu(options, OUTPUT_MENU) {
        let items: Vec<String> = choices::OUTPUTS.iter().map(|o| o.1.to_string()).collect();
        let at = choices::OUTPUTS.iter().position(|o| o.0 == cfg.output);
        fill_radio(menu, IDM_OUTPUT, &items, at);
    } else if menu == submenu(options, QUALITY_MENU) {
        let items: Vec<String> = choices::QUALITIES.iter().map(|q| q.1.to_string()).collect();
        let at = choices::QUALITIES.iter().position(|q| q.0 == cfg.lines);
        fill_radio(menu, IDM_QUALITY, &items, at);
    } else if menu == submenu(options, SPEED_MENU) {
        let items: Vec<String> = choices::RATES
            .iter()
            .map(|r| choices::rate_label(*r))
            .map(|l| l[..1].to_uppercase() + &l[1..])
            .collect();
        let at = choices::RATES.iter().position(|r| *r == cfg.rate);
        fill_radio(menu, IDM_RATE, &items, at);
    } else if menu == submenu(options, PRESETS_MENU) {
        fill_presets(menu, &cfg.preset);
    }
    unsafe {
        CheckMenuItem(
            menu,
            IDM_KEEP_DOWNLOADS as u32,
            (MF_BYCOMMAND
                | if cfg.keep_downloads {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                })
            .0,
        );
        CheckMenuItem(
            menu,
            IDM_AT_ONCE as u32,
            (MF_BYCOMMAND
                | if cfg.together > 1 {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                })
            .0,
        );
        CheckMenuItem(
            menu,
            IDM_SHORT_TOO as u32,
            (MF_BYCOMMAND
                | if cfg.short_too {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                })
            .0,
        );
        CheckMenuItem(
            menu,
            IDM_FACES as u32,
            (MF_BYCOMMAND | if cfg.faces { MF_CHECKED } else { MF_UNCHECKED }).0,
        );
        CheckMenuItem(
            menu,
            IDM_SEND_NAMES as u32,
            (MF_BYCOMMAND
                | if cfg.send_names {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                })
            .0,
        );
    }
}

/// The models offered for the chosen service, as (model, what the menu
/// says), and which is chosen now. A model typed into settings.txt by hand
/// is offered too, under its own name.
fn models(cfg: &Config) -> (Vec<(String, String)>, usize) {
    let (fixed, current) = match cfg.service {
        Service::OpenRouter => (choices::OPENROUTER_MODELS, &cfg.openrouter_model),
        Service::Claude => (choices::CLAUDE_MODELS, &cfg.claude_model),
    };
    let mut list: Vec<(String, String)> = fixed
        .iter()
        .map(|(m, t)| (m.to_string(), t.to_string()))
        .collect();
    let at = match list.iter().position(|m| &m.0 == current) {
        Some(i) => i,
        None => {
            list.push((current.clone(), format!("{current}: from settings.txt")));
            list.len() - 1
        }
    };
    (list, at)
}

/// Replace a submenu's items with `items`, numbered from `first`, the chosen
/// one marked with a radio dot, which a screen reader reads as "checked".
fn fill_radio(menu: HMENU, first: i32, items: &[String], chosen: Option<usize>) {
    unsafe {
        while GetMenuItemCount(Some(menu)) > 0 {
            let _ = DeleteMenu(menu, 0, MF_BYPOSITION);
        }
        for (i, text) in items.iter().enumerate() {
            // A literal ampersand would underline the next letter.
            let t = wide(&text.replace('&', "&&"));
            let _ = AppendMenuW(menu, MF_STRING, first as usize + i, PCWSTR(t.as_ptr()));
        }
        if let Some(c) = chosen.filter(|&c| c < items.len()) {
            let _ = CheckMenuRadioItem(
                menu,
                first as u32,
                (first as usize + items.len() - 1) as u32,
                (first as usize + c) as u32,
                MF_BYCOMMAND.0,
            );
        }
    }
}

/// The Presets menu: save, then each preset, the one in use ticked, then
/// the folder they are kept in.
fn fill_presets(menu: HMENU, current: &str) {
    let add = |flags: MENU_ITEM_FLAGS, id: i32, text: &str| unsafe {
        let t = wide(&text.replace('&', "&&"));
        let _ = AppendMenuW(menu, flags, id as usize, PCWSTR(t.as_ptr()));
    };
    unsafe {
        while GetMenuItemCount(Some(menu)) > 0 {
            let _ = DeleteMenu(menu, 0, MF_BYPOSITION);
        }
    }
    add(
        MF_STRING,
        IDM_PRESET_SAVE,
        "Save these choices as a preset...",
    );
    let names = crate::presets::list();
    if !names.is_empty() {
        add(MF_SEPARATOR, 0, "");
    }
    for (i, name) in names.iter().enumerate() {
        let ticked = if name.eq_ignore_ascii_case(current) {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        add(MF_STRING | ticked, IDM_PRESET + i as i32, name);
    }
    add(MF_SEPARATOR, 0, "");
    add(
        MF_STRING,
        IDM_PRESET_FOLDER,
        "Show the presets folder in File Explorer",
    );
}

/// Saves the choices now made as a preset, under a name the user types.
fn save_preset(hwnd: HWND, app: &mut App) {
    let Some(name) = crate::namebox::show(hwnd) else {
        say(hwnd, "No preset saved.");
        return;
    };
    if crate::presets::exists(&name)
        && !message::ask(
            hwnd,
            &format!("There is already a preset called {name}. Replace it with these choices?"),
            "&Replace",
            "&Keep the old one",
            false,
        )
    {
        say(hwnd, "No preset saved.");
        return;
    }
    if let Err(why) = crate::presets::save(&name, &app.cfg) {
        message::problem(hwnd, &why);
        return;
    }
    app.cfg.preset = name.clone();
    app.cfg.save();
    say(
        hwnd,
        &format!(
            "Saved the preset {name}: the model, style, language, voice, speed, what happens when \
             a description does not fit, the output, the picture quality and describing four at \
             once, as they are now. Choose it in Options, Presets."
        ),
    );
}

/// Chooses the preset `name`, and says what it changed.
fn choose_preset(hwnd: HWND, app: &mut App, name: &str) {
    let next = match crate::presets::choose(name, &app.cfg) {
        Ok(n) => n,
        Err(why) => {
            message::problem(hwnd, &why);
            return;
        }
    };
    let changed = crate::presets::changed(&app.cfg, &next);
    app.cfg = next;
    app.cfg.save();
    let what = match changed.len() {
        0 => "Nothing changed: these were the choices already.".to_string(),
        1 => format!("It changed {}.", changed[0]),
        n => format!(
            "It changed {} and {}.",
            changed[..n - 1].join(", "),
            changed[n - 1]
        ),
    };
    say(hwnd, &format!("Preset {name} chosen. {what}"));
}

/// The shortcuts shown in the menus, in step with videoscribe.rc. Enter,
/// Escape and F1 are the dialog's own.
fn shortcut(key: u16) -> Option<i32> {
    let down = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) } < 0;
    if key == VK_F2.0 && !down(VK_MENU) && !down(VK_CONTROL) && !down(VK_SHIFT) {
        return Some(IDM_RENAME);
    }
    if down(VK_MENU) || !down(VK_CONTROL) {
        return None;
    }
    if down(VK_SHIFT) {
        return match key as u8 {
            b'D' => Some(IDM_KNOWING),
            b'V' => Some(IDM_VOICES),
            _ => None,
        };
    }
    match key as u8 {
        b'N' => Some(IDM_ADD),
        b'O' => Some(IDM_OPEN),
        b'D' => Some(IDM_START),
        b'P' => Some(IDM_PEOPLE),
        b'R' => Some(IDM_REVIEW),
        b'L' => Some(IDM_LISTEN),
        _ => None,
    }
}

/// Commands from the menus and their shortcuts. Every choice is saved at once
/// and said out loud, as the menu closes without a word.
fn command(hwnd: HWND, app: &mut App, id: i32) -> bool {
    let in_list = |first: i32, len: usize| (first..first + len as i32).contains(&id);
    let said = match id {
        IDM_ADD => {
            add(hwnd, app, "");
            return true;
        }
        IDM_OPEN => {
            open_video(hwnd, app);
            return true;
        }
        IDM_FOLDER => {
            choose_folder(hwnd, app);
            return true;
        }
        IDM_EXPLORER => {
            let folder = app.cfg.output_folder();
            let _ = std::fs::create_dir_all(&folder);
            open_in_explorer(&folder);
            return true;
        }
        IDM_UPDATE_YTDLP => {
            if app.job.is_some() {
                say(
                    hwnd,
                    "Already working. Update yt-dlp once this job has finished.",
                );
            } else {
                app.job = Some(job::update_ytdlp(hwnd));
            }
            return true;
        }
        IDM_EXIT => {
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            return true;
        }
        IDM_START => {
            app.knowing = None;
            if let Some(source) = chosen(hwnd, app).and_then(|e| source_of(hwnd, &e)) {
                start(hwnd, app, source);
            }
            return true;
        }
        IDM_KNOWING => {
            knowing(hwnd, app);
            return true;
        }
        IDM_PRESET_SAVE => {
            save_preset(hwnd, app);
            return true;
        }
        IDM_PRESET_FOLDER => {
            if let Some(dir) = crate::presets::dir() {
                let _ = std::fs::create_dir_all(&dir);
                open_in_explorer(&dir);
            }
            return true;
        }
        id if in_list(IDM_PRESET, crate::presets::list().len()) => {
            let name = crate::presets::list()[(id - IDM_PRESET) as usize].clone();
            choose_preset(hwnd, app, &name);
            return true;
        }
        IDM_STOP => {
            stop(hwnd);
            return true;
        }
        IDM_PEOPLE => {
            if let Some(e) = chosen(hwnd, app) {
                people(hwnd, &e.key);
            }
            return true;
        }
        IDM_REVIEW => {
            if let Some(e) = chosen(hwnd, app) {
                review_of(hwnd, hwnd, &e.key, &app.cfg.voice, app.cfg.rate);
            }
            return true;
        }
        IDM_LISTEN => {
            if let Some(e) = chosen(hwnd, app) {
                listen_to(hwnd, &e.key);
            }
            focus(hwnd, IDC_LIST);
            return true;
        }
        IDM_RENAME => {
            if let Some(e) = chosen(hwnd, app).filter(|e| !busy_with(hwnd, app, e)) {
                if let Some(said) = videos::rename(hwnd, &e) {
                    refresh(hwnd, app);
                    say(hwnd, &said);
                }
                focus(hwnd, IDC_LIST);
            }
            return true;
        }
        IDM_SHOW => {
            if let Some(e) = chosen(hwnd, app) {
                open_in_explorer(&e.folder);
            }
            return true;
        }
        IDM_DELETE => {
            if let Some(e) = chosen(hwnd, app).filter(|e| !busy_with(hwnd, app, e)) {
                let mut said = videos::delete(hwnd, &e);
                refresh(hwnd, app);
                if app.entries.is_empty() {
                    said += " No videos are left: press Ctrl+N to add one.";
                }
                say(hwnd, &said);
                focus(hwnd, IDC_LIST);
            }
            return true;
        }
        IDM_HELP => {
            help(hwnd);
            return true;
        }
        IDM_ABOUT => {
            message::about(hwnd, &about());
            return true;
        }
        IDM_KEEP_DOWNLOADS => {
            app.cfg.keep_downloads = !app.cfg.keep_downloads;
            if app.cfg.keep_downloads {
                "Video downloads are now kept for a day, so describing the same video again \
                 starts at once."
                    .to_string()
            } else {
                "Video downloads are now deleted once the described sound is made.".to_string()
            }
        }
        IDM_AT_ONCE => {
            app.cfg.together = if app.cfg.together > 1 {
                1
            } else {
                crate::config::AT_ONCE
            };
            if app.cfg.together > 1 {
                format!(
                    "Describing now sends {} requests at once: faster, each one told only the \
                     labels found so far.",
                    app.cfg.together
                )
            } else {
                "Describing now sends one request at a time, each one told what the earlier ones \
                 said: slower."
                    .to_string()
            }
        }
        IDM_SHORT_TOO => {
            app.cfg.short_too = !app.cfg.short_too;
            if app.cfg.short_too {
                "Short pauses now follow the same choice as the others when a description does \
                 not fit."
                    .to_string()
            } else {
                "A description too long for a short pause is now always left out.".to_string()
            }
        }
        IDM_FACES => {
            app.cfg.faces = !app.cfg.faces;
            if app.cfg.faces {
                "Video Scribe now finds the faces in a family video's pictures, on this computer, \
                 and says the name of anyone you named before. Only a tag such as Person 3 is \
                 sent with each picture."
                    .to_string()
            } else {
                "Faces are no longer found in family videos, and no tags are sent.".to_string()
            }
        }
        IDM_SEND_NAMES => {
            app.cfg.send_names = !app.cfg.send_names;
            if app.cfg.send_names {
                "The file name, or the web video's title, is now sent with each description request."
                    .to_string()
            } else {
                "The file name and the web video's title are no longer sent.".to_string()
            }
        }
        id if in_list(IDM_SERVICE, choices::SERVICES.len()) => {
            let service = choices::SERVICES[(id - IDM_SERVICE) as usize].0;
            choices::choose_service(&mut app.cfg, service)
        }
        id if in_list(IDM_MODEL, models(&app.cfg).0.len()) => {
            let model = models(&app.cfg).0[(id - IDM_MODEL) as usize].0.clone();
            choices::choose_model(&mut app.cfg, &model)
        }
        id if in_list(IDM_STYLE, choices::STYLES.len()) => {
            choices::choose_style(&mut app.cfg, choices::STYLES[(id - IDM_STYLE) as usize].0)
        }
        id if in_list(IDM_ROOM, choices::ROOMS.len()) => {
            choices::choose_room(&mut app.cfg, choices::ROOMS[(id - IDM_ROOM) as usize].0)
        }
        id if in_list(IDM_OUTPUT, choices::OUTPUTS.len()) => {
            choices::choose_output(&mut app.cfg, choices::OUTPUTS[(id - IDM_OUTPUT) as usize].0)
        }
        id if in_list(IDM_QUALITY, choices::QUALITIES.len()) => choices::choose_quality(
            &mut app.cfg,
            choices::QUALITIES[(id - IDM_QUALITY) as usize].0,
        ),
        IDM_VOICES => {
            let Some((language, voice)) =
                crate::voicebox::show(hwnd, &app.cfg.language, &app.cfg.voice, app.cfg.rate)
            else {
                say(hwnd, "Language and voice unchanged.");
                return true;
            };
            app.cfg.language = language;
            choices::choose_voice(&mut app.cfg, &voice);
            format!(
                "Descriptions in {}, spoken by {}.",
                app.cfg.language,
                choices::voice_label(&voice)
            )
        }
        id if in_list(IDM_RATE, choices::RATES.len()) => {
            choices::choose_rate(&mut app.cfg, choices::RATES[(id - IDM_RATE) as usize])
        }
        _ => return false,
    };
    // A preset is a starting point, not a lock (PLAN.md §5.12).
    let mut said = said;
    if !app.cfg.preset.is_empty() && !crate::presets::still(&app.cfg.preset, &app.cfg) {
        said += &format!(" No longer the {} preset.", app.cfg.preset);
        app.cfg.preset.clear();
    }
    app.cfg.save();
    say(hwnd, &said);
    true
}

const VIDEO_TYPES: &str = "*.mp4;*.m4v;*.mov;*.mkv;*.avi;*.wmv;*.mts;*.m2ts;*.3gp;*.webm";

/// The People in this video window, for the video `key`.
fn people(hwnd: HWND, key: &str) {
    let before = crate::people::load(key);
    let Some(after) = crate::peopleui::show(hwnd, &before, &job::cast(key)) else {
        say(hwnd, "Nothing changed.");
        return;
    };
    if after == before {
        say(hwnd, "Nothing changed.");
        return;
    }
    if let Err(why) = crate::people::save(key, &after) {
        message::problem(hwnd, &why);
        return;
    }
    let named = after.names.iter().filter(|(_, n)| !n.is_empty()).count();
    let mut said = match named {
        0 => "Saved. No label has a name.".to_string(),
        1 => "Saved: 1 label has a name.".to_string(),
        n => format!("Saved: {n} labels have names."),
    };
    if after.who != before.who {
        said += " Who is in this video changed, so it will be described again, with your note.";
    } else if named > 0 {
        said += " Describe again, Ctrl+D, to hear them: nothing is sent. Or describe again \
                  knowing these people, Ctrl+Shift+D, so the model names them itself.";
    }
    match learn(key, &before, &after) {
        Ok(learned) if !learned.is_empty() => {
            said += &format!(
                " Video Scribe will know {} by face in later videos.",
                and_list(&learned)
            );
        }
        Ok(_) => {}
        Err(why) => message::problem(hwnd, &why),
    }
    say(hwnd, &said);
}

/// "Ruth", "Ruth and Ben", "Ruth, Ben and Ann".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The names given or changed in People in this video, for people whose
/// faces were tagged: their faces are kept under the name, for later videos
/// (log entry 42, Q3). Returns the names now known by their faces.
fn learn(
    key: &str,
    before: &crate::people::People,
    after: &crate::people::People,
) -> Result<Vec<String>, String> {
    let Some(tagged) = crate::faces::tagged(key) else {
        return Ok(Vec::new());
    };
    let name_in = |p: &crate::people::People, label: &str| {
        p.names
            .iter()
            .find(|(l, _)| l.eq_ignore_ascii_case(label))
            .map(|(_, n)| n.trim().to_string())
            .unwrap_or_default()
    };
    let mut known = crate::known::Known::load();
    let mut changed = false;
    let mut learned: Vec<String> = Vec::new();
    for (label, tag) in job::cast(key).sure_tags() {
        let (was, now) = (name_in(before, &label), name_in(after, &label));
        if was == now {
            continue;
        }
        let Some(faces) = tag.checked_sub(1).and_then(|i| tagged.people.get(i)) else {
            continue;
        };
        changed |= known.learn(key, tag, &now, faces);
        if !now.is_empty() && !learned.iter().any(|n| n.eq_ignore_ascii_case(&now)) {
            learned.push(now);
        }
    }
    if changed {
        known.save()?;
    }
    Ok(learned)
}

/// Describe again knowing these people (log entry 37): the names the user
/// gave go with the pictures, so the model can say who is who at the
/// moments that matter. A fresh describing, asked about as any other.
fn knowing(hwnd: HWND, app: &mut App) {
    if app.job.is_some() {
        say(
            hwnd,
            "Already working. Press Escape to stop, or wait for the report.",
        );
        return;
    }
    let Some(e) = chosen(hwnd, app) else { return };
    let people = crate::people::load(&e.key);
    let known = crate::describe::known_lines(&people.names, &job::cast(&e.key).looks);
    if known.is_empty() {
        message::problem(
            hwnd,
            "Nobody in this video has a name yet. Describe it first with Ctrl+D, then open \
             People in this video with Ctrl+P and type a name for each label you are sure of. \
             Then choose Describe again knowing these people.",
        );
        return;
    }
    let Some(source) = source_of(hwnd, &e) else {
        return;
    };
    app.knowing = Some(known);
    start(hwnd, app, source);
}

/// The Review the descriptions window for the video `key`, over `owner`;
/// what changed is said there, and in the main window's Status box.
pub fn review_of(owner: HWND, main: HWND, key: &str, voice: &str, rate: i32) {
    let Some(result) = job::review(key) else {
        message::problem(
            owner,
            "This video has no described sound to review yet. Describe it first, with Ctrl+D, \
             then open Review the descriptions again.",
        );
        return;
    };
    let before = job::overrides(key);
    let after = crate::review::show(owner, result, before.clone(), voice, rate);
    let tell = |text: &str| {
        set_text(main, IDC_STATUS, text);
        crate::announce::say(owner, text);
    };
    if after == before {
        tell("Nothing changed.");
        return;
    }
    if let Err(why) = job::set_overrides(key, &after) {
        message::problem(owner, &why);
        return;
    }
    tell(&match after.len() {
        0 => "Your changes are kept: every place has its text as described. Press Ctrl+D to \
                  make the described sound again: nothing is sent."
            .to_string(),
        1 => "Your changes are kept: 1 place has your own text. Press Ctrl+D to make the \
                  described sound again: nothing is sent."
            .to_string(),
        n => format!(
            "Your changes are kept: {n} places have your own text. Press Ctrl+D to make the \
                 described sound again: nothing is sent."
        ),
    });
}

/// The Listen to the described sound window for the video `key`.
pub fn listen_to(hwnd: HWND, key: &str) {
    let Some(result) = job::review(key) else {
        message::problem(
            hwnd,
            "This video has no described sound to listen to yet. Describe it first, with \
             Ctrl+D, then open Listen to the described sound again.",
        );
        return;
    };
    if !result.output.exists() {
        message::problem(
            hwnd,
            &format!(
                "The described sound is no longer where it was saved, so it cannot be played: \
                 {}. Describe the video again with Ctrl+D to make it again: nothing is sent.",
                result.output.display()
            ),
        );
        return;
    }
    if let Err(why) = crate::listen::show(hwnd, &result.output, 0.0, false) {
        message::problem(
            hwnd,
            &format!("The described sound could not be played. {why}"),
        );
    }
}

/// Add a video file: the file picker, then the Add a video window with the
/// file in it, to see what will be sent before starting.
fn open_video(hwnd: HWND, app: &mut App) {
    // Start where the chosen video's file was.
    let start = videos::chosen(list(hwnd))
        .and_then(|i| app.entries.get(i))
        .map(|e| PathBuf::from(&e.source))
        .and_then(|p| p.parent().map(Path::to_path_buf));
    match pick(hwnd, start.as_deref(), "Choose a video to describe", false) {
        Some(path) => add(hwnd, app, &path.display().to_string()),
        // Windows would otherwise restore an older focus.
        None => focus(hwnd, IDC_LIST),
    }
}

fn choose_folder(hwnd: HWND, app: &mut App) {
    let current = app.cfg.output_folder();
    if let Some(folder) = pick(
        hwnd,
        Some(&current),
        "Choose the folder for described files",
        true,
    ) {
        app.cfg.output_folder = Some(folder.clone());
        app.cfg.save();
        refresh(hwnd, app);
        say(
            hwnd,
            &format!(
                "Described files will be written to {}. The list now shows the {} videos there.",
                folder.display(),
                app.entries.len()
            ),
        );
    }
    focus(hwnd, IDC_LIST);
}

/// A file or folder picker. None when cancelled, which is not a problem to report.
pub fn pick(owner: HWND, start: Option<&Path>, title: &str, folder: bool) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let options = dialog.GetOptions().ok()?;
        let extra = if folder {
            FOS_PICKFOLDERS
        } else {
            FOS_FILEMUSTEXIST
        };
        dialog
            .SetOptions(options | extra | FOS_FORCEFILESYSTEM)
            .ok()?;
        let title = wide(title);
        let _ = dialog.SetTitle(PCWSTR(title.as_ptr()));
        if !folder {
            let (name, spec) = (wide("Videos"), wide(VIDEO_TYPES));
            let (all, star) = (wide("All files"), wide("*.*"));
            let filters = [
                COMDLG_FILTERSPEC {
                    pszName: PCWSTR(name.as_ptr()),
                    pszSpec: PCWSTR(spec.as_ptr()),
                },
                COMDLG_FILTERSPEC {
                    pszName: PCWSTR(all.as_ptr()),
                    pszSpec: PCWSTR(star.as_ptr()),
                },
            ];
            let _ = dialog.SetFileTypes(&filters);
        }
        if let Some(start) = start.filter(|p| p.is_dir()) {
            let path = wide(&start.display().to_string());
            if let Ok(item) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(path.as_ptr()), None)
            {
                let _ = dialog.SetFolder(&item);
            }
        }
        dialog.Show(Some(owner)).ok()?;
        let item = dialog.GetResult().ok()?;
        let name: PWSTR = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as _));
        path.map(PathBuf::from)
    }
}

fn about() -> String {
    format!(
        "Video Scribe, version {}.\n\nAdds spoken descriptions of what can be seen to web \
         videos and family videos, with a screen reader first. Free and open source, under the MIT \
         licence.\n\nBy Laurent Cadet de Fontenay, with assistance from Claude. Voices: Microsoft \
         Edge's Read Aloud. Speech found with Silero VAD (MIT).\n\nThe help (F1) has the details.",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn open_in_explorer(folder: &Path) {
    unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            None,
            &windows::core::HSTRING::from("open"),
            &windows::core::HSTRING::from(folder.as_os_str()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Say it in the Status box and out loud, for things that happen without
/// focus moving: a choice from a menu, a job starting or ending.
fn say(hwnd: HWND, text: &str) {
    set_text(hwnd, IDC_STATUS, text);
    crate::announce::say(hwnd, text);
}

const HELP: &str = include_str!("help.html");

/// F1, or Help in the Help menu: the help page in the default browser, where a screen
/// reader can move from heading to heading.
fn help(hwnd: HWND) {
    let written = Config::dir().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("help.html");
        std::fs::write(
            &path,
            HELP.replace("{{VERSION}}", env!("CARGO_PKG_VERSION")),
        )
        .ok()?;
        Some(path)
    });
    let Some(path) = written else {
        message::problem(
            hwnd,
            "The help page could not be written to the settings folder, so it cannot be opened.",
        );
        return;
    };
    // A file:/// address goes to the default browser; a bare path can bring
    // up an "Open with" chooser instead, which ShellExecute calls success.
    let url = file_url(&path);
    let r = unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            Some(hwnd),
            &windows::core::HSTRING::from("open"),
            &windows::core::HSTRING::from(url.as_str()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if r.0 as isize <= 32 {
        message::problem(
            hwnd,
            &format!(
                "Windows could not open the help page in a browser. It is saved at {}; you can open it \
                 from Explorer.",
                path.display()
            ),
        );
    }
}

/// `C:\Users\Ann Lee\help.html` as `file:///C:/Users/Ann%20Lee/help.html`.
fn file_url(path: &Path) -> String {
    let mut url = String::from("file:///");
    for b in path.to_string_lossy().replace('\\', "/").bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'.' | b'-' | b'_' | b'~' => {
                url.push(b as char)
            }
            _ => url += &format!("%{b:02X}"),
        }
    }
    url
}

// ---------------------------------------------------------------- control helpers

pub fn set_text(hwnd: HWND, id: i32, text: &str) {
    let t = wide(&text.replace("\r\n", "\n").replace('\n', "\r\n"));
    unsafe {
        let _ = SetDlgItemTextW(hwnd, id, PCWSTR(t.as_ptr()));
    }
    // Leave the caret where it is in the box being typed in.
    let typing = unsafe { GetDlgItem(Some(hwnd), id).is_ok_and(|c| GetFocus() == c) };
    if !typing {
        caret_to_start(hwnd, id);
    }
}

pub fn get_text(hwnd: HWND, id: i32) -> String {
    unsafe {
        let Ok(control) = GetDlgItem(Some(hwnd), id) else {
            return String::new();
        };
        let len = GetWindowTextLengthW(control).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let n = GetWindowTextW(control, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..n]).replace("\r\n", "\n")
    }
}

/// Put the caret at the start, so review with the arrow keys begins at the top.
pub fn caret_to_start(hwnd: HWND, id: i32) {
    unsafe {
        if let Ok(edit) = GetDlgItem(Some(hwnd), id) {
            SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(0)));
            SendMessageW(edit, EM_SCROLLCARET, None, None);
        }
    }
}

/// Move focus the way a dialog expects (WM_NEXTDLGCTL), so the dialog manager
/// also moves the default-button highlight. A bare SetFocus leaves it behind.
pub fn focus(hwnd: HWND, id: i32) {
    unsafe {
        if let Ok(control) = GetDlgItem(Some(hwnd), id) {
            SendMessageW(
                hwnd,
                WM_NEXTDLGCTL,
                Some(WPARAM(control.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Menu titles and control labels both answer Alt+letter in the main
    /// window, so no letter may be used twice (as in Photo Scribe).
    #[test]
    fn main_window_alt_letters_are_unique() {
        let rc = include_str!("../videoscribe.rc");
        let block = |start: &str| {
            let b = &rc[rc.find(start).unwrap()..];
            b[..b.find("\nEND").unwrap()].to_string()
        };
        let (main, bar) = (block("IDD_MAIN DIALOGEX"), block("IDR_MENU MENU"));
        let letter = |line: &str| {
            let q = line.split('"').nth(1)?;
            let at = q.find('&')?;
            q[at + 1..].chars().next().map(|c| c.to_ascii_lowercase())
        };
        let mut letters: Vec<char> = main
            .lines()
            .filter(|l| {
                let l = l.trim_start();
                l.starts_with("LTEXT")
                    || l.starts_with("DEFPUSHBUTTON")
                    || l.starts_with("PUSHBUTTON")
            })
            .filter_map(letter)
            .collect();
        // Top-level menus only: indented by exactly four spaces.
        letters.extend(
            bar.lines()
                .filter(|l| l.starts_with("    POPUP"))
                .filter_map(letter),
        );
        assert_eq!(letters.len(), 13, "{letters:?}");
        let mut sorted = letters.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), letters.len(), "{letters:?}");
    }

    /// Within each menu, every item's letter is unique.
    #[test]
    fn menu_letters_are_unique_within_each_menu() {
        let rc = include_str!("../videoscribe.rc");
        let bar = &rc[rc.find("IDR_MENU MENU").unwrap()..];
        let bar = &bar[..bar.find("\nEND").unwrap()];
        let mut menu: Vec<char> = Vec::new();
        for line in bar.lines() {
            if line.starts_with("    POPUP") {
                menu.clear();
            } else if line.starts_with("        MENUITEM \"") || line.starts_with("        POPUP") {
                let q = line.split('"').nth(1).unwrap();
                if let Some(at) = q.find('&') {
                    let c = q[at + 1..].chars().next().unwrap().to_ascii_lowercase();
                    assert!(!menu.contains(&c), "{c} twice: {line}");
                    menu.push(c);
                }
            }
        }
    }

    /// The menu positions in ui.rs match the template.
    #[test]
    fn menu_positions_match_the_template() {
        let rc = include_str!("../videoscribe.rc");
        let bar = &rc[rc.find("IDR_MENU MENU").unwrap()..];
        let tops: Vec<&str> = bar.lines().filter(|l| l.starts_with("    POPUP")).collect();
        assert!(tops[OPTIONS_MENU as usize].contains("&Options"));
        let options = &bar[bar.find("\"&Options\"").unwrap()..];
        let options = &options[..options.find("\n    END").unwrap()];
        let items: Vec<&str> = options
            .lines()
            .filter(|l| l.starts_with("        MENUITEM") || l.starts_with("        POPUP"))
            .collect();
        assert!(items[PROVIDER_MENU as usize].contains("Model &provider"));
        assert!(items[MODEL_MENU as usize].contains("&Model"));
        assert!(items[STYLE_MENU as usize].contains("St&yle"));
        assert!(items[VOICES_ITEM as usize].contains("&Language and voice"));
        assert!(items[SPEED_MENU as usize].contains("&Speed"));
        assert!(items[ROOM_MENU as usize].contains("&fit"));
        assert!(items[OUTPUT_MENU as usize].contains("O&utput"));
        assert!(items[QUALITY_MENU as usize].contains("&quality"));
        assert!(items[PRESETS_MENU as usize].contains("P&resets"));
    }

    #[test]
    fn help_addresses_are_percent_encoded() {
        assert_eq!(
            file_url(Path::new(r"C:\Users\Ann Lee\Video Scribe\help.html")),
            "file:///C:/Users/Ann%20Lee/Video%20Scribe/help.html"
        );
    }
}
