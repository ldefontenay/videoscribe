//! Speaking through the screen reader with UI Automation notification events,
//! for what happens without focus moving: a job starting or finishing, a
//! choice made in a menu. The same text always goes into the Status box too.
//!
//! `[proven]` in NoteFetch (milestone 3, 18 October 2026) and Photo Scribe: JAWS speaks these
//! from a plain Win32 dialog, after its own speech.

use windows::core::BSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    NotificationKind_Other, NotificationProcessing_ImportantAll, UiaClientsAreListening,
    UiaHostProviderFromHwnd, UiaRaiseNotificationEvent,
};

/// Ask the screen reader to say `text`. False if nothing is listening.
pub fn say(hwnd: HWND, text: &str) -> bool {
    unsafe {
        if !UiaClientsAreListening().as_bool() {
            return false;
        }
        let Ok(provider) = UiaHostProviderFromHwnd(hwnd) else {
            return false;
        };
        UiaRaiseNotificationEvent(
            &provider,
            NotificationKind_Other,
            NotificationProcessing_ImportantAll,
            &BSTR::from(text),
            &BSTR::from("Video Scribe"),
        )
        .is_ok()
    }
}
