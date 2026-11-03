//! User 11-01: "minimize to the tray" (Settings, Minimize to the tray). Minimized, the launcher leaves the taskbar for
//! an icon by the clock: a click (or Enter) on it opens the launcher again, its right-click menu has Open and Quit.
//! The icon talks to a hidden window of its own on the event loop's thread, so everything here runs on that thread.
use std::cell::{Cell, OnceCell};
use windows_sys::Win32::{Foundation::{HWND, LPARAM, LRESULT, WPARAM}, System::LibraryLoader::GetModuleHandleW,
                         UI::{Shell::*, WindowsAndMessaging::*}};

const CALLBACK: u32 = WM_APP + 1;  // the icon's messages to its window
const KEY_SELECT: u32 = NIN_SELECT | NINF_KEY;  // Enter or Space on the icon
const OPEN: usize = 1;
const QUIT: usize = 2;

struct Tray { window: HWND, explorer_restarted: u32, shown: Cell<bool>, on: Box<dyn Fn(&'static str)> }

thread_local! { static TRAY: OnceCell<Tray> = const { OnceCell::new() }; }

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

/// Makes the icon's hidden window; `on` then hears "open" or "quit" from the icon. Once, on the event loop's thread.
pub fn init(on: impl Fn(&'static str) + 'static) {
    let class = wide("S3R tray");
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance, lpszClassName: class.as_ptr(), ..Default::default() });
        // top-level and never shown: a message-only window would miss Explorer's TaskbarCreated
        let window = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(),
                                     instance, std::ptr::null());
        if window.is_null() { return; }
        let explorer_restarted = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        TRAY.with(|t| { let _ = t.set(Tray { window, explorer_restarted, shown: Cell::new(false), on: Box::new(on) }); });
    }
}

/// Puts the icon by the clock. False when Windows has no tray for it (Explorer not running): stay on the taskbar.
pub fn show() -> bool {
    TRAY.with(|t| t.get().is_some_and(|tray| tray.shown.get() || { tray.shown.set(add(tray.window)); tray.shown.get() }))
}

/// Takes the icon away: the launcher is back, or closing.
pub fn hide() {
    TRAY.with(|t| if let Some(tray) = t.get() && tray.shown.replace(false) {
        unsafe { Shell_NotifyIconW(NIM_DELETE, &icon(tray.window)) };
    });
}

/// Out of the tray (or off the taskbar) and in front. Windows' own restore, as the taskbar button does it: tao's
/// set_minimized(false) rewrites the style from flags taken before the restore, which drops a maximized window's
/// WS_MAXIMIZE.
pub fn bring_back(window: &tao::window::Window) {
    use tao::platform::windows::WindowExtWindows;
    hide();
    window.set_visible(true);  // first: tao hides a window it has marked hidden again on every later change
    if window.is_minimized() { unsafe { SendMessageW(window.hwnd() as HWND, WM_SYSCOMMAND, SC_RESTORE as WPARAM, 0) }; }
    window.set_focus();
}

/// S3R's own icon (resource 1, build.rs) at the tray's size, named in its tooltip.
fn icon(window: HWND) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: window, uID: 1,
                                     uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP, uCallbackMessage: CALLBACK,
                                     ..Default::default() };
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    unsafe {
        data.hIcon = LoadImageW(GetModuleHandleW(std::ptr::null()), 1 as _, IMAGE_ICON, GetSystemMetrics(SM_CXSMICON),
                                GetSystemMetrics(SM_CYSMICON), LR_SHARED);
        if data.hIcon.is_null() { data.hIcon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION); }
    }
    for (to, c) in data.szTip.iter_mut().zip("S3R".encode_utf16()) { *to = c; }
    data
}

fn add(window: HWND) -> bool {
    let data = icon(window);
    unsafe {
        if Shell_NotifyIconW(NIM_ADD, &data) == 0 { return false; }
        // version 4: a click comes as NIN_SELECT, the keyboard's as KEY_SELECT, the menu as WM_CONTEXTMENU
        if Shell_NotifyIconW(NIM_SETVERSION, &data) == 0 { Shell_NotifyIconW(NIM_DELETE, &data); return false; }
    }
    true
}

/// The icon's menu where it was asked for (wParam: the click, or the icon for the keyboard), until a pick or a click
/// elsewhere.
fn menu(window: HWND, at: WPARAM) -> Option<&'static str> {
    let (x, y) = ((at & 0xffff) as i16 as i32, ((at >> 16) & 0xffff) as i16 as i32);
    let pick = unsafe {
        let menu = CreatePopupMenu();
        AppendMenuW(menu, MF_STRING, OPEN, wide("Open").as_ptr());
        AppendMenuW(menu, MF_STRING, QUIT, wide("Quit").as_ptr());
        SetForegroundWindow(window);  // or a click elsewhere would leave the menu open
        let pick = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, x, y, 0, window, std::ptr::null());
        DestroyMenu(menu);
        PostMessageW(window, WM_NULL, 0, 0);
        pick
    };
    match pick as usize { OPEN => Some("open"), QUIT => Some("quit"), _ => None }
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    TRAY.with(|t| {
        let Some(tray) = t.get() else { return unsafe { DefWindowProcW(window, message, wparam, lparam) } };
        match message {
            CALLBACK => match (lparam & 0xffff) as u32 {
                NIN_SELECT | KEY_SELECT => (tray.on)("open"),
                WM_CONTEXTMENU => if let Some(action) = menu(window, wparam) { (tray.on)(action) },
                _ => {}
            },
            // Explorer restarted with an empty tray: the icon again, or the launcher back if Windows won't take it
            m if m == tray.explorer_restarted && tray.shown.get() => if !add(window) { tray.shown.set(false); (tray.on)("open") },
            _ => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
        0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Puts a real icon in this PC's tray for a moment, so it is left out of the normal run (`--ignored`): it shows
    /// once, a click on it says "open", and it goes again.
    #[test]
    #[ignore]
    fn the_icon_shows_answers_a_click_and_goes() {
        let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = heard.clone();
        init(move |action| log.borrow_mut().push(action));
        assert!(show() && show(), "shown, and a second show is the same icon");
        let window = TRAY.with(|t| t.get().unwrap().window);
        unsafe { SendMessageW(window, CALLBACK, 0, NIN_SELECT as LPARAM) };
        assert_eq!(*heard.borrow(), ["open"]);
        hide();
        assert!(!TRAY.with(|t| t.get().unwrap().shown.get()));
    }
}
