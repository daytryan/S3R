//! F16: can this PC run the game? The game draws with Vulkan, so the check is: does Windows have a Vulkan driver
//! with a real GPU (integrated, discrete or virtual) on Vulkan 1.1 or newer. It runs in a child launcher
//! (`--pc-check`) so the GPU driver's DLLs never load into the launcher itself.
use serde_json::{json, Value};
use std::ffi::c_void;

type RawFn = unsafe extern "system" fn();
type GetProc = unsafe extern "system" fn(*mut c_void, *const u8) -> Option<RawFn>;
type CreateInstance = unsafe extern "system" fn(*const InstanceInfo, *const c_void, *mut *mut c_void) -> i32;
type Enumerate = unsafe extern "system" fn(*mut c_void, *mut u32, *mut *mut c_void) -> i32;
type Properties = unsafe extern "system" fn(*mut c_void, *mut u8);
type Destroy = unsafe extern "system" fn(*mut c_void, *const c_void);

#[repr(C)]
struct AppInfo { s_type: u32, next: *const c_void, app_name: *const u8, app_version: u32, engine_name: *const u8, engine_version: u32, api_version: u32 }
#[repr(C)]
struct InstanceInfo { s_type: u32, next: *const c_void, flags: u32, app: *const AppInfo, layers: u32, layer_names: *const c_void,
                      extensions: u32, extension_names: *const c_void }

/// (device type, Vulkan version, name) of each GPU Vulkan sees, or why there are none.
fn devices() -> Result<Vec<(u32, u32, String)>, &'static str> {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let name: Vec<u16> = "vulkan-1.dll\0".encode_utf16().collect();
    let lib = unsafe { LoadLibraryW(name.as_ptr()) };
    if lib.is_null() { return Err("No Vulkan driver is installed."); }
    unsafe {
        let get: GetProc = std::mem::transmute(GetProcAddress(lib, c"vkGetInstanceProcAddr".as_ptr().cast()).ok_or("No Vulkan driver is installed.")?);
        let create: CreateInstance = std::mem::transmute(get(std::ptr::null_mut(), c"vkCreateInstance".as_ptr().cast()).ok_or("Vulkan is broken.")?);
        let app = AppInfo { s_type: 0, next: std::ptr::null(), app_name: c"S3R".as_ptr().cast(), app_version: 0, engine_name: std::ptr::null(),
                            engine_version: 0, api_version: 1 << 22 | 1 << 12 };  // VK_API_VERSION_1_1
        let info = InstanceInfo { s_type: 1, next: std::ptr::null(), flags: 0, app: &app, layers: 0, layer_names: std::ptr::null(),
                                  extensions: 0, extension_names: std::ptr::null() };
        let mut instance = std::ptr::null_mut();
        if create(&info, std::ptr::null(), &mut instance) != 0 { return Err("The Vulkan driver refused to start."); }
        let enumerate: Enumerate = std::mem::transmute(get(instance, c"vkEnumeratePhysicalDevices".as_ptr().cast()).ok_or("Vulkan is broken.")?);
        let props: Properties = std::mem::transmute(get(instance, c"vkGetPhysicalDeviceProperties".as_ptr().cast()).ok_or("Vulkan is broken.")?);
        let destroy: Destroy = std::mem::transmute(get(instance, c"vkDestroyInstance".as_ptr().cast()).ok_or("Vulkan is broken.")?);
        let mut count = 0u32;
        enumerate(instance, &mut count, std::ptr::null_mut());
        let mut handles = vec![std::ptr::null_mut(); count as usize];
        enumerate(instance, &mut count, handles.as_mut_ptr());
        let found = handles.iter().take(count as usize).map(|&device| {
            let mut p = [0u8; 1024];  // VkPhysicalDeviceProperties is 824 bytes
            props(device, p.as_mut_ptr());
            let word = |at: usize| u32::from_le_bytes(p[at..at + 4].try_into().unwrap());
            let name = &p[20..276];
            (word(16), word(0), String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(256)]).into_owned())
        }).collect();
        destroy(instance, std::ptr::null());
        Ok(found)
    }
}

/// The verdict on a list of devices: the first real GPU on Vulkan 1.1+ passes.
pub fn verdict(found: Result<Vec<(u32, u32, String)>, &str>) -> Value {
    const UPDATE: &str = "Update your graphics driver from your GPU maker's website, then try again.";
    let message = |text: &str| format!("{text} {UPDATE}");
    match found {
        Err(why) => json!({"type": "pc", "ok": false, "gpu": "", "message": message(why)}),
        Ok(list) => {
            let real = |t: u32| (1..=3).contains(&t);
            match list.iter().find(|(t, v, _)| real(*t) && *v >= (1 << 22 | 1 << 12)).or(list.iter().find(|(t, _, _)| real(*t))) {
                Some((_, v, gpu)) if *v >= (1 << 22 | 1 << 12) => json!({"type": "pc", "ok": true, "gpu": gpu, "message": ""}),
                Some((_, _, gpu)) => json!({"type": "pc", "ok": false, "gpu": gpu, "message": message(&format!("{gpu} has an old Vulkan driver."))}),
                None => json!({"type": "pc", "ok": false, "gpu": list.first().map_or("", |d| d.2.as_str()), "message": message("No graphics card that can run the game was found.")}),
            }
        }
    }
}

/// `--pc-check`: print the verdict and exit.
pub fn run() { println!("{}", verdict(devices())); }

/// Run `--pc-check` in a child launcher; its one line is the `pc` event. A child that crashes (a broken driver) or
/// says nothing is a failed check too, and one that hangs in the driver is stopped after 15 s (A11).
pub fn check_in_child() -> Value {
    let Ok(exe) = std::env::current_exe() else { return verdict(Err("Checking the graphics driver crashed.")) };
    check_with(std::process::Command::new(exe).arg("--pc-check"), std::time::Duration::from_secs(15))
}

fn check_with(command: &mut std::process::Command, limit: std::time::Duration) -> Value {
    use std::os::windows::process::CommandExt;
    let Ok(mut child) = command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).creation_flags(0x0800_0000).spawn()
        else { return verdict(Err("Checking the graphics driver crashed.")) };
    crate::pipeline::dies_with_launcher(&child);  // never a launcher left behind in Task Manager
    match crate::pipeline::stdout_within(&mut child, limit) {
        None => verdict(Err("The graphics driver did not answer.")),
        Some(out) => serde_json::from_slice::<Value>(&out).ok().filter(|v| v["type"] == "pc")
            .unwrap_or_else(|| verdict(Err("Checking the graphics driver crashed."))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verdict_wants_a_real_gpu_on_vulkan_1_1() {
        let v11 = 1 << 22 | 1 << 12;
        let ok = verdict(Ok(vec![(4, v11, "llvmpipe".into()), (2, v11 | 200, "NVIDIA GeForce GTX 1050".into())]));
        assert_eq!((ok["ok"].as_bool(), ok["gpu"].as_str()), (Some(true), Some("NVIDIA GeForce GTX 1050")));
        let old = verdict(Ok(vec![(1, 1 << 22, "Intel HD 4000".into())]));
        assert_eq!(old["ok"], false);
        assert!(old["message"].as_str().unwrap().contains("Intel HD 4000 has an old Vulkan driver"));
        let cpu = verdict(Ok(vec![(4, v11, "llvmpipe".into())]));
        assert_eq!((cpu["ok"].as_bool(), cpu["gpu"].as_str()), (Some(false), Some("llvmpipe")));
        assert_eq!(verdict(Err("No Vulkan driver is installed."))["ok"], false);
    }

    /// A11: a check that hangs is stopped and reported, not waited on forever.
    #[test]
    fn a_hung_driver_check_is_stopped() {
        let start = std::time::Instant::now();
        let v = check_with(std::process::Command::new(r"C:\Windows\System32\PING.EXE").args(["-n", "30", "127.0.0.1"]), std::time::Duration::from_millis(500));
        assert!(v["message"].as_str().unwrap().starts_with("The graphics driver did not answer."), "{v}");
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    /// The real check on this PC: it must answer either way, and not crash the test process.
    #[test]
    fn the_real_check_answers() {
        let v = verdict(devices());
        assert_eq!(v["type"], "pc");
        assert!(v["ok"].is_boolean());
    }
}
