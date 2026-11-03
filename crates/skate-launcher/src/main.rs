#![cfg_attr(not(test), windows_subsystem = "windows")]  // the test harness needs its console
mod app;
mod crash;
mod credentials;
mod hero;
mod maintenance;
mod pc;
mod pipeline;
mod playtime;
mod system;
mod tray;
mod update;

use serde_json::Value;
use wry::WebViewBuilderExtWindows;
use tao::{event::{Event, WindowEvent}, event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget},
          platform::windows::WindowBuilderExtWindows, window::{ResizeDirection, Window, WindowBuilder}};

// Both pages carry their fonts (ui/fonts.html), so nothing is fetched from the internet to draw them. The launcher's page
// also carries this version's change log (ui/changelog.md): What's new shows it until the update check brings the notes.
const PAGE: &str = concat!(include_str!("../ui/fonts.html"), "<script type=\"text/plain\" id=\"changelog\">\n",
                           include_str!("../ui/changelog.md"), "</script>\n", include_str!("../ui/launcher.html"));
const CRASH_PAGE: &str = concat!(include_str!("../ui/fonts.html"), include_str!("../ui/crash.html"));

/// The page draws its own title bar (no Windows frame), so it asks for the frame's jobs: {"cmd":"window","op":..}.
/// They go through the event loop because only the loop owns the window. Returns false when `op` is unknown.
fn window_op(window: &Window, op: &str, dir: &str) -> bool {
    match op {
        "drag" => { let _ = window.drag_window(); }
        "minimize" => window.set_minimized(true),
        "maximize" => window.set_maximized(!window.is_maximized()),
        "resize" => {
            let dir = match dir {
                "n" => ResizeDirection::North, "s" => ResizeDirection::South, "e" => ResizeDirection::East, "w" => ResizeDirection::West,
                "ne" => ResizeDirection::NorthEast, "nw" => ResizeDirection::NorthWest,
                "se" => ResizeDirection::SouthEast, "sw" => ResizeDirection::SouthWest, _ => return false,
            };
            if !window.is_maximized() { let _ = window.drag_resize_window(dir); }
        }
        _ => return false,
    }
    true
}

/// The window shows its own page only. WebView2 loads `with_html` as a `data:` page, so that (and `about:`) is the
/// initial navigation; a link, redirect or script that points anywhere else is cancelled.
/// Feature #2 (user 10-14): unfocused or minimized, the page stops its clip and lane (asleep()); minimized, WebView2
/// also trims its memory. A9: `last` is what was sent last time; only a change reaches the page and WebView2 (a drag
/// at the window edge sends many Resized events a second).
/// E1: a build gets WebView2's memory too (Low while it runs); the page already holds the hero clip during a build.
fn away(webview: &wry::WebView, window: &Window, building: bool, last: &mut Option<(bool, bool, bool)>) {
    use wry::WebViewExtWindows;
    let (minimized, focused) = (window.is_minimized(), window.is_focused());
    let was = last.replace((minimized, focused, building));
    if was == Some((minimized, focused, building)) { return; }
    if was.is_none_or(|(m, f, _)| (m, f) != (minimized, focused)) {
        let _ = webview.evaluate_script(&format!("window.s3r&&s3r.on({{\"type\":\"away\",\"on\":{}}})", minimized || !focused));
    }
    let _ = webview.set_memory_usage_level(if minimized || building { wry::MemoryUsageLevel::Low } else { wry::MemoryUsageLevel::Normal });
}

fn allowed_navigation(url: &str) -> bool { url.starts_with("data:") || url.starts_with("about:") }

/// The crash report's own small window (ui/crash.html), centred on the main screen and on top once. `report` is crash::message.
/// Its frame ops come back as {"crash_window": op}; Send and Open folder go to App::handle like the launcher page's.
/// The webview comes first in the pair so it drops before its window.
fn crash_window(target: &EventLoopWindowTarget<Value>, app: &std::sync::Arc<app::App>, frame: EventLoopProxy<Value>, report: &Value,
                context: &mut wry::WebContext) -> Option<(wry::WebView, Window)> {
    let window = WindowBuilder::new()
        .with_title("S3R crash report")
        .with_inner_size(tao::dpi::LogicalSize::new(720.0, 600.0))
        .with_resizable(false)
        .with_decorations(false)
        .with_undecorated_shadow(true)
        .with_visible(false)
        .with_always_on_top(true)  // over whatever took the screen when the game closed; dropped back once shown
        .build(target).ok()?;
    if let Some(monitor) = target.primary_monitor().or_else(|| window.current_monitor()) {  // where the game ran
        let (screen, size, at) = (monitor.size(), window.outer_size(), monitor.position());
        window.set_outer_position(tao::dpi::PhysicalPosition::new(at.x + (screen.width as i32 - size.width as i32) / 2,
                                                                 at.y + (screen.height as i32 - size.height as i32) / 2));
    }
    let handler = app.clone();
    let webview = wry::WebViewBuilder::new_with_web_context(context)
        .with_browser_accelerator_keys(false)
        .with_html(CRASH_PAGE)
        .with_initialization_script(format!("window.S3R_CRASH = {report};"))
        .with_background_color((0x1b, 0x1d, 0x21, 255))  // the page's --deck, not a white flash
        .with_navigation_handler(|url| allowed_navigation(&url))
        .with_ipc_handler(move |req: wry::http::Request<String>| {
            match serde_json::from_str::<Value>(req.body()) {
                Ok(m) if m["cmd"] == "window" => { let _ = frame.send_event(serde_json::json!({"crash_window": m["op"]})); }
                _ => handler.handle(req.body()),
            }
        })
        .build(&window).ok()?;
    window.set_visible(true);
    window.set_focus();
    window.set_always_on_top(false);
    Some((webview, window))
}

/// P7: WebView2's profile in the launcher's local data, never beside the exe (a Program Files install can't write there).
fn web_context() -> wry::WebContext { wry::WebContext::new(Some(maintenance::local_data().join("webview"))) }

/// F15: the first window size in logical pixels: 1180×760, shrunk to the screen's work area, never below the minimum.
fn first_size(work_area: Option<(u32, u32)>, scale: f64) -> (f64, f64) {
    let (w, h) = work_area.map_or((f64::MAX, f64::MAX), |(w, h)| (w as f64 / scale, h as f64 / scale));
    ((w - 24.0).min(1180.0).max(760.0), (h - 24.0).min(760.0).max(540.0))
}

/// F14: a panic anywhere in the launcher is logged (#12) and shown, instead of the window just vanishing.
fn catch_panics() {
    std::panic::set_hook(Box::new(|info| {
        let at = info.location().map_or(String::new(), |l| format!(" at {}:{}", l.file(), l.line()));
        let what = info.payload().downcast_ref::<&str>().map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        maintenance::log(&format!("panic{at}: {what}"));
        app::fatal(&format!("The launcher hit a problem and has to close.\n\n{what}{at}\n\nThe details are in {}",
                            maintenance::local_data().join("launcher.log").display()));
        std::process::exit(1);
    }));
}

fn main() {
    let exe = std::env::current_exe().unwrap_or_default();  // before any swap renames the running file
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--pc-check") { return pc::run(); }  // F16, in a child (pc::check_in_child)
    catch_panics();
    let (after, rebuild) = update::relaunched(&args);
    // the game crashed with no launcher open (crash_report.rs launcher_shows): only the crash window, and only for a
    // report name from the game's own report folder
    let crash_only = args.iter().position(|a| a == "--crash").and_then(|i| args.get(i + 1)).map(std::path::Path::new)
        .and_then(|p| crash::find(p.file_name()?.to_str()?)).as_deref().and_then(crash::message);
    if let Some(pid) = after { update::wait_for_exit(pid); }  // started by the launcher this one replaces
    // one launcher at a time; a second one wakes the first and closes. The crash window is not a launcher.
    // The handle lives until the process ends (event_loop.run never returns).
    let _instance = if crash_only.is_some() { None } else {
        match system::single_instance(if after.is_some() { 10_000 } else { 0 }) { Ok(h) => Some(h as usize), Err(()) => return }
    };
    update::finish_self_update();
    let event_loop = EventLoopBuilder::<serde_json::Value>::with_user_event().build();
    let app = std::sync::Arc::new(app::App::new(event_loop.create_proxy()));
    app.build_after_check.store(rebuild, std::sync::atomic::Ordering::SeqCst);  // the old launcher's automatic update
    let crash_frame = event_loop.create_proxy();
    let mut context = web_context();
    if let Some(report) = crash_only {
        let mut crash = crash_window(&event_loop, &app, crash_frame, &report, &mut context);
        event_loop.run(move |event, _, flow| {
            *flow = ControlFlow::Wait;
            let close = match &event {
                Event::UserEvent(m) if m.get("crash_window").is_some() => {
                    if let (Some((_, window)), Some(op)) = (&crash, m["crash_window"].as_str()) { window_op(window, op, ""); }
                    m["crash_window"] == "close"
                }
                Event::UserEvent(m) => {
                    if let Some((webview, _)) = &crash { let _ = webview.evaluate_script(&format!("window.s3r&&s3r.on({m})")); }
                    false
                }
                Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => true,
                _ => crash.is_none(),
            };
            if close { crash = None; *flow = ControlFlow::Exit; }
        });
    }
    let scale = event_loop.primary_monitor().map_or(1.0, |m| m.scale_factor());
    let (width, height) = first_size(system::work_area(), scale);
    let window = WindowBuilder::new()
        .with_title("S3R")
        .with_inner_size(tao::dpi::LogicalSize::new(width, height))
        .with_min_inner_size(tao::dpi::LogicalSize::new(760.0, 540.0))
        .with_decorations(false)  // the page's title bar is the window's (window_op)
        .with_undecorated_shadow(true)  // keeps Windows' shadow and rounded corners
        .build(&event_loop)
        .expect("window");
    let show = std::sync::Mutex::new(event_loop.create_proxy());
    system::on_second_open(move || { let _ = show.lock().unwrap().send_event(serde_json::json!({"show": true})); });
    let tray_events = event_loop.create_proxy();
    tray::init(move |action| { let _ = tray_events.send_event(serde_json::json!({"tray": action})); });
    let handler = app.clone();
    let frame = event_loop.create_proxy();
    let webview = match wry::WebViewBuilder::new_with_web_context(&mut context)
        .with_browser_accelerator_keys(false)  // no F5 / Ctrl+R: a reload mid-install would rerun `ready`
        .with_background_color((0x1b, 0x1d, 0x21, 255))  // the page's --deck, not a white flash (feature #2)
        .with_html(PAGE)
        .with_navigation_handler(|url| allowed_navigation(&url))
        .with_ipc_handler(move |req: wry::http::Request<String>| {
            match serde_json::from_str::<serde_json::Value>(req.body()) {
                Ok(m) if m["cmd"] == "window" => { let _ = frame.send_event(serde_json::json!({"window": m["op"], "dir": m["dir"]})); }
                _ => handler.handle(req.body()),
            }
        })
        .build(&window)
    {
        Ok(webview) => webview,
        Err(error) => return app::fatal(&format!(
            "S3R needs Microsoft Edge WebView2.\n\nInstall it from \
             https://developer.microsoft.com/microsoft-edge/webview2/ and open the launcher again.\n\n({error})")),
    };
    let mut crash: Option<(wry::WebView, Window)> = None;
    let (mut shown, mut maximized) = (None, None);  // A9: the away and maximized state the page last got
    event_loop.run(move |event, target, flow| {
        *flow = ControlFlow::Wait;
        match event {
            // the game this launcher started crashed (crash::watch): its report in the crash window, a newer one replacing it
            Event::UserEvent(message) if message.get("crash").is_some() => {
                crash = crash_window(target, &app, crash_frame.clone(), &message["crash"], &mut context).or(crash.take());
            }
            Event::UserEvent(message) if message.get("crash_window").is_some() => {
                let op = message["crash_window"].as_str().unwrap_or("");
                if op == "close" { crash = None; } else if let Some((_, w)) = &crash { window_op(w, op, ""); }
            }
            Event::UserEvent(message) if message["type"] == "crash_sent" => {
                if let Some((view, _)) = &crash { let _ = view.evaluate_script(&format!("window.s3r&&s3r.on({message})")); }
            }
            Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. }
                if crash.as_ref().is_some_and(|(_, w)| w.id() == window_id) => { crash = None; }
            // a self-update is in place: open the new launcher and close this one (not while a build runs: it would stop)
            Event::UserEvent(message) if message.get("relaunch").is_some() => {
                let rebuild = message["relaunch"] == true;
                if app.running.lock().unwrap().is_none() && update::relaunch(&exe, rebuild).is_ok() {
                    app.shutdown();
                    *flow = ControlFlow::Exit;
                } else {
                    app.build_after_check.store(rebuild, std::sync::atomic::Ordering::SeqCst);
                    app.send(serde_json::json!({"type": "toast", "text": "Launcher updated. It uses the new version next time you open it."}));
                    app.refresh();
                }
            }
            // a second launcher was opened (single_instance), or the tray icon clicked: this one comes to the front
            Event::UserEvent(message) if message.get("show").is_some() || message["tray"] == "open" => tray::bring_back(&window),
            Event::UserEvent(message) if message["tray"] == "quit" => { app.shutdown(); *flow = ControlFlow::Exit; }
            // F6: the build's progress on the taskbar button, red on an error, a flash when it ends
            Event::UserEvent(message) if message.get("building").is_some() =>  // E1: from pipeline::spawn
                away(&webview, &window, message["building"] == true, &mut shown),
            Event::UserEvent(message) if message.get("taskbar").is_some() => {
                use tao::window::{ProgressBarState, ProgressState};
                let (state, progress) = match &message["taskbar"] {
                    Value::Number(n) => (ProgressState::Normal, Some(n.as_f64().unwrap_or(0.0).clamp(0.0, 100.0) as u64)),
                    Value::String(s) if s == "error" => (ProgressState::Error, Some(100)),
                    _ => (ProgressState::None, None),
                };
                window.set_progress_bar(ProgressBarState { state: Some(state), progress, desktop_filename: None });
                if message["flash"] == true && !window.is_focused() {
                    window.request_user_attention(Some(tao::window::UserAttentionType::Informational));
                }
            }
            // F5: what the launcher does when Play started the game (default: out of the way, and nothing drawn)
            Event::UserEvent(message) if message.get("game_started").is_some() => match message["game_started"].as_str() {
                Some("close") if app.running.lock().unwrap().is_none() => { app.shutdown(); *flow = ControlFlow::Exit; }
                Some("open") => {}
                _ => window.set_minimized(true),
            },
            Event::UserEvent(message) if message.get("window").is_some() => {
                let op = message["window"].as_str().unwrap_or("");
                if op == "close" { app.shutdown(); *flow = ControlFlow::Exit; }  // same as Windows' own close
                else { window_op(&window, op, message["dir"].as_str().unwrap_or("")); }
            }
            Event::UserEvent(message) => {
                let _ = webview.evaluate_script(&format!("window.s3r&&s3r.on({message})"));
            }
            Event::WindowEvent { window_id, event: WindowEvent::Resized(_), .. } if window_id == window.id() => {
                // user 11-01: minimized with Minimize to the tray on, the taskbar button becomes the tray icon
                if window.is_minimized() && app.settings.lock().unwrap().minimize_to_tray && tray::show() { window.set_visible(false); }
                if maximized.replace(window.is_maximized()) != Some(window.is_maximized()) {
                    let _ = webview.evaluate_script(&format!("window.s3rMax&&s3rMax({})", window.is_maximized()));
                }
                away(&webview, &window, app.running.lock().unwrap().is_some(), &mut shown);
            }
            Event::WindowEvent { window_id, event: WindowEvent::Focused(_), .. } if window_id == window.id() => away(&webview, &window, app.running.lock().unwrap().is_some(), &mut shown),
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                app.shutdown();
                *flow = ControlFlow::Exit;
            }
            Event::LoopDestroyed => tray::hide(),  // every way out: no icon left behind
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{allowed_navigation, first_size, CRASH_PAGE, PAGE};

    #[test]
    fn the_first_window_fits_the_screen() {
        assert_eq!(first_size(Some((1920, 1040)), 1.0), (1180.0, 760.0));
        let (w, h) = first_size(Some((1366, 728)), 1.25);
        assert!((w - 1068.8).abs() < 0.01 && (h - 558.4).abs() < 0.01, "1366x768 at 125%: the bottom stays above the taskbar");
        assert_eq!(first_size(Some((700, 500)), 1.0), (760.0, 540.0), "never below the minimum");
        assert_eq!(first_size(None, 1.0), (1180.0, 760.0));
    }

    /// The crash window's buttons are ops crash_window's loop carries out, and its two commands are ones App::handle takes.
    #[test]
    fn the_crash_window_asks_only_for_what_exists() {
        let ops: Vec<&str> = CRASH_PAGE.split("data-op=\"").skip(1).map(|s| s.split('"').next().unwrap()).collect();
        assert_eq!(ops, ["minimize", "close", "close"]);
        for cmd in ["send_crash", "open_crash_folder"] {
            assert!(CRASH_PAGE.contains(&format!("cmd: '{cmd}'")), "{cmd}");
            assert!(crate::app::Command::parse(&format!(r#"{{"cmd":"{cmd}","name":"report-1-2.txt"}}"#)).is_ok(), "{cmd}");
        }
        assert!(CRASH_PAGE.contains("window.S3R_CRASH") && CRASH_PAGE.contains("'crash_sent'"));
    }

    /// The pages draw with their own fonts and load nothing from anywhere else.
    #[test]
    fn the_pages_load_nothing_from_the_internet() {
        for page in [PAGE, CRASH_PAGE] {
            assert!(!page.contains("<link") && !page.contains("src=\"http") && !page.contains("url(http"));
            for family in ["Big Shoulders Display", "Barlow", "IBM Plex Mono"] {
                assert!(page.contains(&format!("font-family: '{family}'")), "{family} is not embedded");
            }
        }
    }

    /// The mockup's decoration (fake window buttons, channels, history, switches that do nothing) must not ship. (The
    /// Discord button is real since 10-22: app.rs open_takes_only_the_projects_github_pages checks its link.)
    #[test]
    fn the_page_claims_no_feature_that_does_not_exist() {
        for word in ["Template", "Release channel", "Build history", "Display mode", "Vibration", "Close the launcher",
                     "Download updates automatically", "1.0.0", "winctl", "localStorage."] {
            assert!(!PAGE.contains(word), "the page still has mockup text: {word}");
        }
        assert!(PAGE.contains("id=\"ver\"") && PAGE.contains("set_animations") && PAGE.contains("fresh:"));
    }

    /// The window has no Windows frame, so every button the page's title bar shows must be an op main.rs carries out.
    #[test]
    fn every_title_bar_button_is_a_real_window_op() {
        let ops: Vec<&str> = PAGE.split("data-op=\"").skip(1).map(|s| s.split('"').next().unwrap()).collect();
        assert_eq!(ops, ["minimize", "maximize", "maximize", "close"]);
        assert!(PAGE.contains("frame('resize', dir)") && PAGE.contains("'drag'"));
    }

    /// Every switch the Settings page saves with `set` is a setting Settings::set takes (user 11-01: Minimize to the tray).
    #[test]
    fn every_settings_switch_is_a_real_setting() {
        let keys: Vec<&str> = PAGE.split("class=\"switch\"").skip(1)
            .filter_map(|s| s.split('>').next()?.split("data-key=\"").nth(1)?.split('"').next()).collect();
        assert!(keys.contains(&"minimize_to_tray"), "{keys:?}");
        for key in keys { assert!(crate::app::Settings::default().set(key, &serde_json::json!(true)), "{key}"); }
    }

    #[test]
    fn the_hero_clip_loops_across_the_lane_and_falls_back_to_the_board() {
        assert!(PAGE.contains("id=\"hero-clip\"") && PAGE.contains("muted") && PAGE.contains("playsinline"));
        let tag = PAGE.split("<video").nth(1).unwrap().split('>').next().unwrap();
        assert!(tag.contains(" loop"), "the game renders a seamless loop; it wraps off-screen");
        assert!(PAGE.contains("clip.currentTime % d) / d"), "his x follows the clip's time, once across per loop");
        assert!(!PAGE.contains("joints"), "he moves, so the coping stays still");
        assert!(PAGE.contains("m.type === 'hero'") && PAGE.contains("heroFail"));
    }

    #[test]
    fn only_the_initial_page_may_load() {
        assert!(allowed_navigation("data:text/html;charset=utf-8;base64,PGh0bWw+") && allowed_navigation("about:blank"));
        for url in ["https://example.com/", "http://127.0.0.1/", "file:///C:/Windows/win.ini", "javascript:alert(1)"] {
            assert!(!allowed_navigation(url), "{url}");
        }
    }

    /// The page's item table is copied from the pipeline's ITEMS (R6). This fails when either side drifts.
    #[test]
    fn page_lists_exactly_the_pipelines_items() {
        let sources = include_str!("../../../tools/pipeline/sources.py");
        let table = sources.split("ITEMS = [").nth(1).and_then(|s| s.split("\n]").next()).expect("ITEMS table");
        let quoted = |line: &str| {
            let (mut out, mut chars) = (Vec::new(), line.chars());
            while let Some(open) = chars.find(|c| *c == '\'' || *c == '"') {
                out.push(chars.by_ref().take_while(|c| *c != open).collect::<String>());
            }
            out
        };
        let rows: Vec<Vec<String>> = table.lines().filter(|l| l.trim_start().starts_with('(')).map(quoted).collect();
        assert_eq!(rows.len(), 18, "the pipeline lists 18 items");
        for row in &rows {
            let (id, label) = (&row[0], &row[2]);
            assert!(PAGE.contains(&format!("id: '{id}', label: ")), "page is missing item {id}");
            assert!(PAGE.contains(label.as_str()), "page is missing the label {label:?}");
        }
        assert_eq!(PAGE.matches("{ id: '").count(), rows.len(), "page has an item the pipeline does not");
    }
}
