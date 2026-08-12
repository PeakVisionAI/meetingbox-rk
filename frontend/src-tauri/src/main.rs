#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use log;
use env_logger;

fn main() {
    std::env::set_var("RUST_LOG", "info");
    #[cfg(target_os = "linux")]
    {
        // Mali blob provides GLES/EGL only. GTK defaults to GLX, which falls
        // back to Mesa llvmpipe on this board (AIGLX/rockchip_dri fails).
        std::env::set_var("GDK_GL", "gles");
        std::env::set_var("GST_GL_API", "gles2");
        std::env::set_var("GST_GL_PLATFORM", "egl");
        std::env::set_var("WEBKIT_FORCE_COMPOSITING_MODE", "1");
        // Mali DMA-BUF often paints black tiles. Do not disable compositing:
        // that leaves framer-motion opacity-0 layers invisible.
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    env_logger::init();

    // Async logger will be initialized lazily when first needed (after Tauri runtime starts)
    log::info!("Starting application...");
    app_lib::run();
}
