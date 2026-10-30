#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod background;
mod tray;
mod window;

// the window runs as its own process and talks to the background one over its stdin and stdout
pub const WINDOW: &str = "--window";
pub const RELAUNCHED: &str = "--relaunched";

fn main() {
    // webkitgtk's dma-buf renderer dies on nvidia under wayland (gdk "Error 71 (Protocol error)"),
    // set before gtk starts. everyone else keeps it, it's the faster one
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() && std::path::Path::new("/proc/driver/nvidia").exists() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    // saving to a lazer skin, run from the background process
    if args.first().map(String::as_str) == Some(mania_bridge::lazer::MODE) {
        std::process::exit(mania_bridge::lazer::writer_main(&args[1..]));
    }
    if args.iter().any(|a| a == WINDOW) {
        window::run(&args);
    } else {
        background::run(&args);
    }
}
