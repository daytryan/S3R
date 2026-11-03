//! Small Windows calls the launcher needs: find and wait for a process, one launcher at a time, keep the PC awake,
//! the screen's work area and a drive's free space. All one-shot or blocking: nothing here polls.
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

fn wide(s: &std::ffi::OsStr) -> Vec<u16> { use std::os::windows::ffi::OsStrExt; s.encode_wide().chain(Some(0)).collect() }

/// The pid of a running process started from exactly `exe` (any parent: the launcher, Explorer, Steam).
pub fn find_process(exe: &Path) -> Option<u32> { find_process_except(exe, &[]) }

/// [`find_process`], skipping the pids in `except` (the launcher's own runs of that exe, such as a hero render).
pub fn find_process_except(exe: &Path, except: &[u32]) -> Option<u32> {
    use windows_sys::Win32::System::{Diagnostics::ToolHelp::*, Threading::*};
    let name = exe.file_name()?.to_string_lossy().to_lowercase();
    let want = std::fs::canonicalize(exe).ok()?;
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 { return None; }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = None;
        let mut more = Process32FirstW(snap, &mut entry) != 0;
        while more && found.is_none() {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase() == name && !except.contains(&entry.th32ProcessID) {
                let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID);
                if !process.is_null() {
                    let (mut buf, mut size) = ([0u16; 1024], 1024u32);
                    if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut size) != 0 {
                        let path = PathBuf::from(String::from_utf16_lossy(&buf[..size as usize]));
                        if std::fs::canonicalize(path).is_ok_and(|p| p == want) { found = Some(entry.th32ProcessID); }
                    }
                    CloseHandle(process);
                }
            }
            more = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        found
    }
}

/// Block until process `pid` ends or `ms` pass (u32::MAX: no limit). Returns at once for a pid that is gone.
pub fn wait_for_exit(pid: u32, ms: u32) {
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !process.is_null() {
            WaitForSingleObject(process, ms);
            CloseHandle(process);
        }
    }
}

const MUTEX: &str = "Local\\Skate3Remaster.Launcher";
const WAKE: &str = "Local\\Skate3Remaster.Launcher.Show";

/// One launcher per Windows session. `wait_ms` lets a self-updated launcher wait for the one it replaces.
/// Ok(handle) holds the lock for the process's life; Err(()) means another launcher has it (it was asked to show itself).
pub fn single_instance(wait_ms: u32) -> Result<HANDLE, ()> { single_instance_named(MUTEX, WAKE, wait_ms) }

fn single_instance_named(mutex_name: &str, wake: &str, wait_ms: u32) -> Result<HANDLE, ()> {
    use windows_sys::Win32::{Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0}, System::Threading::*,
                             UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY}};
    unsafe {
        let mutex = CreateMutexW(std::ptr::null(), 0, wide(mutex_name.as_ref()).as_ptr());
        if mutex.is_null() { return Ok(mutex); }  // ponytail: no lock at all is better than no launcher
        let got = WaitForSingleObject(mutex, wait_ms);
        if got == WAIT_OBJECT_0 || got == WAIT_ABANDONED { return Ok(mutex); }
        CloseHandle(mutex);
        AllowSetForegroundWindow(ASFW_ANY);  // this process was just opened by the player, so it may hand focus on
        let event = OpenEventW(EVENT_MODIFY_STATE, 0, wide(wake.as_ref()).as_ptr());
        if !event.is_null() { SetEvent(event); CloseHandle(event); }
        Err(())
    }
}

/// The running launcher's side: `show` runs each time a second launcher is opened. A thread blocked on an event.
pub fn on_second_open(show: impl Fn() + Send + 'static) { on_second_open_named(WAKE, show) }

fn on_second_open_named(wake: &str, show: impl Fn() + Send + 'static) {
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
    let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, wide(wake.as_ref()).as_ptr()) } as usize;
    if event == 0 { return; }
    std::thread::spawn(move || loop {
        unsafe { WaitForSingleObject(event as HANDLE, INFINITE) };
        show();
    });
}

/// Keep Windows from sleeping while this thread lives and `on` holds (a build's own thread calls it).
pub fn keep_awake(on: bool) {
    use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
    unsafe { SetThreadExecutionState(if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS }) };
}

/// The main screen's work area (the screen minus the taskbar), in physical pixels: (width, height).
pub fn work_area() -> Option<(u32, u32)> {
    use windows_sys::Win32::{Foundation::RECT, UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA}};
    let mut r: RECT = unsafe { std::mem::zeroed() };
    let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut _ as *mut _, 0) } != 0;
    (ok && r.right > r.left && r.bottom > r.top).then(|| ((r.right - r.left) as u32, (r.bottom - r.top) as u32))
}

/// (free, total) bytes on the drive holding `path` (the nearest folder that exists).
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let existing = path.ancestors().find(|p| p.exists())?;
    let (mut free, mut total) = (0u64, 0u64);
    let ok = unsafe { GetDiskFreeSpaceExW(wide(existing.as_os_str()).as_ptr(), &mut free, &mut total, std::ptr::null_mut()) } != 0;
    ok.then_some((free, total))
}

/// "D:" for `D:\Games\...`, the share for a UNC path.
pub fn drive(path: &Path) -> String {
    match path.components().next() {
        Some(std::path::Component::Prefix(p)) => p.as_os_str().to_string_lossy().into_owned(),
        _ => String::new(),
    }
}

/// "16 GB" (installed memory), for the support bundle.
pub fn memory_text() -> String {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 { return "-".into(); }
    format!("{} GB ({} GB free)", (m.ullTotalPhys + (1 << 29)) >> 30, m.ullAvailPhys >> 30)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_is_found_by_its_exact_exe_and_waited_for() {
        let dir = std::env::temp_dir().join(format!("s3r-find-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("PING.EXE");
        std::fs::copy(r"C:\Windows\System32\PING.EXE", &exe).unwrap();
        let mut child = std::process::Command::new(&exe).args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        assert_eq!(find_process(&exe), Some(child.id()));
        assert_eq!(find_process_except(&exe, &[child.id()]), None, "a pid the launcher started itself is skipped");
        assert_ne!(find_process(Path::new(r"C:\Windows\System32\PING.EXE")), Some(child.id()), "same name, another folder");
        child.kill().unwrap();
        wait_for_exit(child.id(), 5000);
        assert!(child.try_wait().unwrap().is_some());
        assert_eq!(find_process(&exe), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_second_launcher_is_refused_and_wakes_the_first() {
        let (mutex, wake) = (format!(r"Local\s3r-test-{}", std::process::id()), format!(r"Local\s3r-test-wake-{}", std::process::id()));
        let (tx, rx) = std::sync::mpsc::channel();
        on_second_open_named(&wake, move || { let _ = tx.send(()); });
        let first = { let (m, w) = (mutex.clone(), wake.clone()); std::thread::spawn(move || {
            let held = single_instance_named(&m, &w, 0).map(|h| h as usize);
            std::thread::sleep(std::time::Duration::from_millis(800));
            held.map(|h| unsafe { CloseHandle(h as HANDLE) }).is_ok()
        }) };
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(single_instance_named(&mutex, &wake, 0).is_err(), "the first one holds it");
        rx.recv_timeout(std::time::Duration::from_secs(5)).expect("the first launcher was asked to show itself");
        assert!(first.join().unwrap());
        assert!(single_instance_named(&mutex, &wake, 2000).is_ok(), "free once the first one closed (an update's relaunch waits)");
    }

    #[test]
    fn disk_space_and_drive_name() {
        let (free, total) = disk_space(&std::env::temp_dir().join("not/made/yet")).unwrap();
        assert!(total > 0 && free <= total);
        assert_eq!(drive(Path::new(r"D:\Games\S3R")), "D:");
        assert!(work_area().is_some());
        assert!(memory_text().ends_with("free)"));
    }
}
