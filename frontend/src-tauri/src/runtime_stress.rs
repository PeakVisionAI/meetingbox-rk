//! Opt-in diagnostic against the actual window/runtime handle clone path.
use tauri::Manager;

pub fn run(app: &tauri::AppHandle) {
    if !cfg!(debug_assertions) || std::env::var("MEETINGBOX_RUNTIME_CLONE_TEST").unwrap_or_default() != "1" { return; }
    let workers = (0..8).map(|_| {
        let app = app.clone();
        std::thread::spawn(move || {
            for i in 0..20_000 {
                let handle = app.clone();
                if let Some(window) = handle.get_webview_window("main") {
                    let window = window.clone();
                    if i % 500 == 0 { let _ = window.eval("void 0"); }
                }
            }
        })
    }).collect::<Vec<_>>();
    std::thread::spawn(move || {
        for worker in workers { if worker.join().is_err() { log::error!("Runtime clone stress failed"); return; } }
        log::info!("Runtime clone stress PASS: 8 threads, 160000 actual handle/window clone-drop iterations");
    });
}
