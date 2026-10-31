// what stays running: the helper and the tray icon, no web view. the window is a child process,
// started when it's opened and gone when it's hidden

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use futures_util::future::BoxFuture;
use mania_bridge::{log, Bridge, Options, Progress, Updater};
use tokio::runtime::Handle;
use tokio::sync::oneshot;

use crate::{autostart, tray, RELAUNCHED, WINDOW};

pub const TELL: &str = "mania-bridge:";

static BRIDGE: OnceLock<Arc<Bridge>> = OnceLock::new();
static RUNTIME: OnceLock<Handle> = OnceLock::new();
static SKIN_WINDOW: Mutex<Option<std::process::Child>> = Mutex::new(None);
static WINDOW_PROCESS: Mutex<Window> = Mutex::new(Window { stdin: None, generation: 0, place: None, update: None });

struct Window {
    stdin: Option<ChildStdin>,
    generation: u64,
    place: Option<String>,
    update: Option<PendingUpdate>,
}

struct PendingUpdate {
    progress: Progress,
    done: oneshot::Sender<Result<(), String>>,
}

fn window() -> MutexGuard<'static, Window> {
    WINDOW_PROCESS.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn run(args: &[String]) {
    let hidden = args.iter().any(|a| a == autostart::HIDDEN);
    let port = std::env::var("BRIDGE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(mania_bridge::DEFAULT_PORT);
    // after an update the old copy can still be answering on the port
    if !args.iter().any(|a| a == RELAUNCHED) && already_running(port, !hidden) {
        return;
    }

    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
    let _ = RUNTIME.set(rt.handle().clone());
    let has_tray = tray::start(&rt);

    autostart::refresh();
    // a game size window that went down with Hashi left osu! muted. osu! may not be running yet, so
    // this waits for its stream for a while, leaving it to any game size window open meanwhile
    std::thread::spawn(|| {
        for _ in 0..360 {
            let open = SKIN_WINDOW.lock().unwrap_or_else(|e| e.into_inner()).is_some();
            if !open && crate::player::hush::restore() {
                return;
            }
            std::thread::sleep(Duration::from_secs(10));
        }
    });
    let opts = Options {
        tray: has_tray,
        titlebar: cfg!(windows),
        on_window: Some(Arc::new(on_window)),
        log_file: true,
        updater: Some(Arc::new(WindowUpdater)),
        autostart: Some(Arc::new(autostart::LoginItem)),
    };
    let bridge = match rt.block_on(mania_bridge::start(opts)) {
        Ok(b) => b,
        Err(e) => {
            log!("{e}");
            #[cfg(windows)]
            fatal(&format!("Hashi couldn't start: {e}"));
            std::process::exit(1);
        }
    };
    let in_tray = has_tray && (hidden || bridge.starts_in_tray());
    let _ = BRIDGE.set(bridge);
    if !has_tray {
        log!("no tray on this system");
    }
    if !in_tray {
        open(true);
    }

    // ctrl+c, logout, pkill: quit the normal way so the helper still cleans up
    rt.spawn(async {
        #[cfg(unix)]
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut s) => drop(s.recv().await),
                Err(_) => std::future::pending().await,
            }
        };
        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate => {}
        }
        quit();
    });
    tray::park(&rt);
}

// windows has no console to print it to
#[cfg(windows)]
fn fatal(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (text, title) = (wide(message), wide("Hashi"));
    // SAFETY: both are nul-terminated and outlive the call
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

// a second launch shows the window of the first one and goes away
fn already_running(port: u16, show: bool) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else { return false };
    if !show {
        return true;
    }
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let req = format!(
        "POST /window/show HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let mut reply = String::new();
    if s.write_all(req.as_bytes()).is_err() || (s.read_to_string(&mut reply).is_err() && reply.is_empty()) {
        return false;
    }
    reply.starts_with("HTTP/1.1 204")
}

fn on_window(name: &str) {
    match name {
        "show" => show(),
        "hide" => hide(),
        "close" => quit(),
        _ => {
            if let Some(size) = name.strip_prefix("skin-window ") {
                return skin_window(size);
            }
            tell_window(name);
        }
    }
}

// the game size window, a process of its own: `size` is 1440x900, or `screen` for the whole screen.
// Opening it again replaces the one that's open
fn skin_window(size: &str) {
    let Some(bridge) = BRIDGE.get() else { return };
    let exe = match tauri::utils::platform::current_exe() {
        Ok(exe) => exe,
        Err(e) => return log!("could not open the game size window: {e}"),
    };
    let mut open = SKIN_WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut old) = open.take() {
        let _ = old.kill();
    }
    let mut cmd = Command::new(exe);
    let (hue, sat) = bridge.theme();
    cmd.arg(crate::player::MODE).arg(format!("--bridge={}", bridge.env.port)).arg(format!("--theme={hue},{sat}"));
    if size != "screen" {
        cmd.arg(format!("--size={size}"));
    }
    // the main window's middle, to open on its screen
    let near = window().place.as_deref().and_then(|p| {
        let n: Vec<f64> = p.split(',').filter_map(|n| n.parse().ok()).collect();
        (n.len() >= 4).then(|| (n[0] + n[2] / 2.0, n[1] + n[3] / 2.0))
    });
    if let Some((x, y)) = near {
        cmd.arg(format!("--near={x},{y}"));
    }
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return log!("could not open the game size window: {e}"),
    };
    let id = child.id();
    *open = Some(child);
    drop(open);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(250));
        let mut open = SKIN_WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let Some(child) = open.as_mut().filter(|c| c.id() == id) else { return };
        if !matches!(child.try_wait(), Ok(None)) {
            *open = None;
            // its own unmuting of osu! doesn't happen if it crashed
            crate::player::hush::restore();
            if let Some(b) = BRIDGE.get() {
                mania_bridge::player_status(b, r#"{"open":false}"#);
            }
            return;
        }
    });
}

pub fn show() {
    open(false);
}

pub fn hide() {
    // the window exits once its stdin closes
    window().stdin = None;
}

pub fn toggle() {
    let open = window().stdin.is_some();
    if open {
        hide();
    } else {
        show();
    }
}

fn tell_window(line: &str) -> bool {
    let mut w = window();
    let Some(stdin) = &mut w.stdin else { return false };
    if writeln!(stdin, "{line}").is_ok() {
        return true;
    }
    w.stdin = None;
    false
}

fn open(opening: bool) {
    if tell_window("show") {
        return;
    }
    let Some(bridge) = BRIDGE.get() else { return };
    let exe = match tauri::utils::platform::current_exe() {
        Ok(exe) => exe,
        Err(e) => return log!("could not open the window: {e}"),
    };
    let (hue, sat) = bridge.theme();
    let mut w = window();
    let mut cmd = Command::new(exe);
    cmd.arg(WINDOW)
        .arg(format!("--url={}", bridge.env.counter_url()))
        .arg(format!("--theme={hue},{sat}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    if opening {
        cmd.arg("--opening");
    }
    if bridge.on_top() {
        cmd.arg("--on-top");
    }
    if let Some(place) = &w.place {
        cmd.arg(format!("--place={place}"));
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return log!("could not open the window: {e}"),
    };
    w.generation += 1;
    let generation = w.generation;
    w.stdin = child.stdin.take();
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || listen(out));
    }
    std::thread::spawn(move || {
        let _ = child.wait();
        let mut w = window();
        if w.generation == generation {
            w.stdin = None;
            w.update = None;
        }
    });
}

fn listen(out: ChildStdout) {
    for line in BufReader::new(out).lines().map_while(Result::ok) {
        let Some(line) = line.strip_prefix(TELL) else { continue };
        let (what, rest) = line.split_once(' ').unwrap_or((line, ""));
        match what {
            "place" => window().place = Some(rest.to_string()),
            "quit" => quit(),
            "progress" => {
                let mut parts = rest.split(' ');
                let done = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let total = parts.next().and_then(|n| n.parse().ok());
                if let Some(u) = &window().update {
                    (u.progress)(done, total);
                }
            }
            "update-failed" => {
                if let Some(u) = window().update.take() {
                    let _ = u.done.send(Err(rest.to_string()));
                }
            }
            // linux: the new copy is in place
            "relaunch" => leave(true),
            // windows: the installer is about to replace this copy
            "shutdown" => leave(false),
            _ => {}
        }
    }
}

pub fn quit() {
    leave(false);
}

// on a thread of its own, whoever asked may be one the shutdown needs
fn leave(relaunch: bool) {
    std::thread::spawn(move || {
        hide();
        if let (Some(bridge), Some(rt)) = (BRIDGE.get().cloned(), RUNTIME.get()) {
            rt.block_on(async {
                let _ = tokio::time::timeout(Duration::from_secs(2), bridge.shutdown()).await;
            });
        }
        if relaunch {
            start_again();
        }
        std::process::exit(0);
    });
}

struct WindowUpdater;

impl Updater for WindowUpdater {
    // dev builds and copies run outside an installer can't replace themselves
    fn can_install(&self) -> bool {
        tauri::utils::platform::bundle_type().is_some()
    }

    // tauri's updater lives in the window, which is open: that's where the button is
    fn install(&self, url: String, progress: Progress) -> BoxFuture<'static, Result<(), String>> {
        let (done, result) = oneshot::channel();
        window().update = Some(PendingUpdate { progress, done });
        let sent = tell_window(&format!("update {url}"));
        if !sent {
            window().update = None;
        }
        Box::pin(async move {
            if !sent {
                return Err("Open the window to update.".into());
            }
            result.await.unwrap_or_else(|_| Err("The window closed before the update was done.".into()))
        })
    }
}

fn start_again() {
    let exe = match std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()) {
        Some(p) => std::path::PathBuf::from(p),
        // the one it started from: once a .deb or .rpm replaced it, linux's own answer ends in " (deleted)"
        None => match tauri::utils::platform::current_exe() {
            Ok(exe) => exe,
            Err(e) => return log!("could not start the new version: {e}"),
        },
    };
    let mut cmd = Command::new(exe);
    cmd.arg(RELAUNCHED);
    // an appimage puts its own folders in front of PATH, LD_LIBRARY_PATH and so on. the new
    // copy brings its own, these would load the old copy's libraries and keep it mounted
    #[cfg(target_os = "linux")]
    if let Some(appdir) = std::env::var("APPDIR").ok().filter(|d| !d.is_empty()) {
        let inside = |p: &str| p == appdir || p.starts_with(&format!("{appdir}/"));
        cmd.env_clear();
        for (k, v) in std::env::vars_os() {
            match v.to_str() {
                Some(v) if v.contains(&appdir) => {
                    let kept: Vec<&str> = v.split(':').filter(|p| !p.is_empty() && !inside(p)).collect();
                    if !kept.is_empty() {
                        cmd.env(&k, kept.join(":"));
                    }
                }
                _ => {
                    cmd.env(&k, &v);
                }
            }
        }
        cmd.current_dir(std::env::var("OWD").ok().filter(|d| !inside(d)).unwrap_or_else(|| "/".into()));
        // and doesn't take along the handle this copy got on its mount, which would keep
        // the old one around for as long as the new one runs
        use std::os::unix::process::CommandExt;
        // close-on-exec rather than closed, std's own pipe for exec errors has to stay.
        // SAFETY: only close_range(2)/fcntl(2) between fork and exec, both async-signal-safe
        unsafe {
            cmd.pre_exec(|| {
                const CLOSE_RANGE_CLOEXEC: u32 = 1 << 2;
                if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, CLOSE_RANGE_CLOEXEC) != 0 {
                    for fd in 3..4096 {
                        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                    }
                }
                Ok(())
            });
        }
    }
    if let Err(e) = cmd.spawn() {
        log!("could not start the new version: {e}");
    }
}
