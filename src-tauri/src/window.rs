// the window, started by the background process. it says what happens on stdout and does what
// it's told on stdin, and leaves when stdin closes

use std::io::{BufRead, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_updater::UpdaterExt;

use crate::background::TELL;

const MAIN: &str = "main";
const OPENING: &str = "opening";
const OVERLAY: &str = "overlay";

static HELPER_PORT: OnceLock<u16> = OnceLock::new();
static COUNTER_URL: OnceLock<String> = OnceLock::new();
static OVERLAY_AT: Mutex<Option<(f64, f64)>> = Mutex::new(None);
// the card's size in the page's pixels, which its zoom makes bigger on screen
static OVERLAY_SIZE: Mutex<(f64, f64)> = Mutex::new((320.0, 90.0));
static OVERLAY_ZOOM: Mutex<f64> = Mutex::new(1.0);
// windows: the background goes first, this one has to stay until the installer is started
static INSTALLING: AtomicBool = AtomicBool::new(false);

struct Args {
    url: String,
    theme: (u16, u8),
    place: Option<Place>,
    opening: bool,
    on_top: bool,
    overlay: bool,
    overlay_at: Option<(f64, f64)>,
}

#[derive(Clone, Copy)]
struct Place {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    maximized: bool,
}

fn parse(args: &[String]) -> Args {
    let value = |name: &str| args.iter().find_map(|a| a.strip_prefix(&format!("--{name}=")).map(str::to_string));
    let theme = value("theme")
        .and_then(|t| {
            let (h, s) = t.split_once(',')?;
            Some((h.parse().ok()?, s.parse().ok()?))
        })
        .unwrap_or((333, 100));
    let place = value("place").and_then(|p| {
        let n: Vec<f64> = p.split(',').filter_map(|n| n.parse().ok()).collect();
        (n.len() == 5).then(|| Place { x: n[0], y: n[1], width: n[2], height: n[3], maximized: n[4] != 0.0 })
    });
    Args {
        url: value("url").unwrap_or_else(|| format!("http://127.0.0.1:{}{}", mania_bridge::DEFAULT_PORT, mania_bridge::COUNTER_PATH)),
        theme,
        place,
        opening: args.iter().any(|a| a == "--opening"),
        on_top: args.iter().any(|a| a == "--on-top"),
        overlay: args.iter().any(|a| a == "--overlay"),
        overlay_at: value("overlay-at").and_then(|p| {
            let (x, y) = p.split_once(',')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        }),
    }
}

fn get<T: Copy>(m: &Mutex<T>) -> T {
    *m.lock().unwrap_or_else(|e| e.into_inner())
}

fn set<T>(m: &Mutex<T>, v: T) {
    *m.lock().unwrap_or_else(|e| e.into_inner()) = v;
}

fn tell(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{TELL}{line}");
    let _ = out.flush();
}

pub fn run(args: &[String]) {
    let args = parse(args);
    if let Some(port) = port_of(&args.url) {
        let _ = HELPER_PORT.set(port);
    }
    let _ = COUNTER_URL.set(args.url.clone());
    set(&OVERLAY_AT, args.overlay_at);
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let handle = app.handle().clone();
            build(&handle, &args, !args.opening && !args.overlay)?;
            if args.overlay {
                overlay(&handle, true);
            } else if args.opening {
                opening(&handle, &args.url);
            }
            std::thread::spawn(move || commands(handle));
            Ok(())
        })
        .on_window_event(move |w, e| {
            if w.label() == OVERLAY {
                match e {
                    // alt+f4 and the like put the window back
                    WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        overlay(w.app_handle(), false);
                    }
                    WindowEvent::Moved(at) if !cfg!(target_os = "linux") || std::env::var_os("WAYLAND_DISPLAY").is_none() => {
                        let Ok(scale) = w.scale_factor() else { return };
                        let at = at.to_logical::<f64>(scale);
                        set(&OVERLAY_AT, Some((at.x, at.y)));
                        tell(&format!("overlay-place {},{}", at.x, at.y));
                    }
                    _ => {}
                }
                return;
            }
            if w.label() != MAIN {
                return;
            }
            match e {
                // closing the window any other way than to the tray quits
                WindowEvent::CloseRequested { .. } => tell("quit"),
                WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                    if let Some(w) = w.app_handle().get_webview_window(MAIN) {
                        report_place(&w);
                    }
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("could not start the window")
        .run(|_, _| {});
}

fn report_place(w: &WebviewWindow) {
    let Ok(scale) = w.scale_factor() else { return };
    let (Ok(at), Ok(size)) = (w.outer_position(), w.inner_size()) else { return };
    if w.is_minimized().unwrap_or(false) {
        return;
    }
    let (at, size) = (at.to_logical::<f64>(scale), size.to_logical::<f64>(scale));
    let maximized = u8::from(w.is_maximized().unwrap_or(false));
    tell(&format!("place {},{},{},{},{maximized}", at.x, at.y, size.width, size.height));
}

fn build(app: &AppHandle, args: &Args, visible: bool) -> tauri::Result<()> {
    let mut window = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(args.url.parse().expect("counter url")))
        .title("Hashi")
        .visible(visible)
        .inner_size(480.0, 640.0)
        .min_inner_size(360.0, 400.0)
        .center()
        .decorations(!cfg!(windows))
        .always_on_top(args.on_top)
        // the theme's --osu-b5, no white flash before the page paints
        .background_color(backdrop(args.theme))
        // the page takes dropped files itself, a picture onto the skin preview
        .disable_drag_drop_handler()
        .on_new_window(to_browser);
    if let Some(p) = args.place {
        window = window.position(p.x, p.y).inner_size(p.width, p.height).maximized(p.maximized);
    }
    let _window = window.build()?;
    #[cfg(windows)]
    trim_context_menu(&_window);
    Ok(())
}

// links that would open a new window (target=_blank) go to the browser instead
fn to_browser(url: tauri::Url, _: tauri::webview::NewWindowFeatures) -> tauri::webview::NewWindowResponse<tauri::Wry> {
    if matches!(url.scheme(), "http" | "https") {
        mania_bridge::open_url(url.as_str());
    }
    tauri::webview::NewWindowResponse::Deny
}

// a small see-through card that stays over the game, in the main window's place while it's on
fn overlay(app: &AppHandle, on: bool) {
    if !on {
        return show(app);
    }
    let main = app.get_webview_window(MAIN);
    if let Some(w) = app.get_webview_window(OPENING) {
        let _ = w.destroy();
    }
    if app.get_webview_window(OVERLAY).is_none() {
        let url = format!("{}?overlay", COUNTER_URL.get().map_or("", String::as_str));
        let Ok(url) = url.parse() else { return };
        let (width, height) = overlay_size();
        let mut window = WebviewWindowBuilder::new(app, OVERLAY, WebviewUrl::External(url))
            .title("Hashi overlay")
            .inner_size(width, height)
            .resizable(!hyprland())
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            // it comes up over the game without taking the keyboard from it
            .focused(false)
            // nor when it's clicked or moved mid-play. windows only: no linux desktop's been tried
            .focusable(!cfg!(windows))
            .disable_drag_drop_handler()
            .on_new_window(to_browser);
        let at = get(&OVERLAY_AT);
        let near = main.as_ref().and_then(|w| Some(w.outer_position().ok()?.to_logical::<f64>(w.scale_factor().ok()?)));
        if let Some((x, y)) = at.or(near.map(|p| (p.x, p.y))) {
            window = window.position(x, y);
        }
        if let Err(e) = window.build() {
            eprintln!("no overlay window: {e}");
            return show(app);
        }
        #[cfg(target_os = "linux")]
        if hyprland() {
            hypr::settle(app.clone(), at);
        }
    }
    if let Some(w) = main {
        let _ = w.hide();
    }
}

// the box the player dragged it to. the page fits the card inside at its own zoom
fn overlay_size() -> (f64, f64) {
    let (w, h) = get(&OVERLAY_SIZE);
    (w.round(), h.round())
}

fn size_overlay(app: &AppHandle) {
    #[cfg(target_os = "linux")]
    if hyprland() {
        let (w, h) = overlay_size();
        return hypr::resize(w, h);
    }
    if let Some(w) = app.get_webview_window(OVERLAY) {
        let (width, height) = overlay_size();
        // a drag usually stops where the card already fits, and a resize can flash the see-through window
        let now = w.inner_size().ok().zip(w.scale_factor().ok()).map(|(s, f)| s.to_logical::<f64>(f));
        if now.is_some_and(|s| (s.width - width).abs() < 1.5 && (s.height - height).abs() < 1.5) {
            return;
        }
        let _ = w.set_size(tauri::LogicalSize::new(width, height));
    }
}

fn hyprland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|s| !s.is_empty())
}

// hyprland floats a window at half the screen whatever size it asks for, ignores where it asks to
// be, and draws its own border around it. hyprctl does all of that instead. not pinned to every
// workspace: dragging a pinned window crashed hyprland 0.56 in its snapping
#[cfg(target_os = "linux")]
mod hypr {
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use tauri::{AppHandle, Manager};

    use super::{get, overlay_size, set, OVERLAY, OVERLAY_ZOOM};

    static ADDRESS: Mutex<Option<String>> = Mutex::new(None);
    // blur behind it, but not behind a card with next to no background
    static BLUR: Mutex<bool> = Mutex::new(true);

    // its own corners, so the blur and the window stop where the card does
    fn rounding(a: &str) -> String {
        let zoom = get(&OVERLAY_ZOOM);
        format!("dispatch setprop {a} rounding {}", (14.0 * zoom).round())
    }

    fn blur(a: &str) -> String {
        format!("dispatch setprop {a} no_blur {}", u8::from(!get(&BLUR)))
    }

    // hyprland keeps no window above the others for an app, and a click in the game raises it over the
    // card. so the card goes back on top whenever another window takes the focus, and a few times a
    // second for clicks in one that has it already, which say nothing. through hyprland's socket, not a
    // hyprctl a time. it stops once the card is gone
    const RAISE_EVERY: Duration = Duration::from_millis(100);

    fn keep_on_top(address: String) {
        use std::io::{BufRead, BufReader};
        let (Some(runtime), Some(sig)) = (std::env::var_os("XDG_RUNTIME_DIR"), std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")) else { return };
        let dir = std::path::PathBuf::from(runtime).join("hypr").join(sig);
        let alive = {
            let address = address.clone();
            move || ADDRESS.lock().unwrap_or_else(|e| e.into_inner()).as_deref() == Some(address.as_str())
        };
        let raise = {
            let control = dir.join(".socket.sock");
            let command = format!("dispatch alterzorder top,address:{address}");
            move || {
                use std::io::{Read, Write};
                if let Ok(mut s) = std::os::unix::net::UnixStream::connect(&control) {
                    let _ = s.write_all(command.as_bytes());
                    let _ = s.read_to_end(&mut Vec::new());
                }
            }
        };
        {
            let (alive, raise) = (alive.clone(), raise.clone());
            std::thread::spawn(move || {
                while alive() {
                    raise();
                    std::thread::sleep(RAISE_EVERY);
                }
            });
        }
        let events = dir.join(".socket2.sock");
        std::thread::spawn(move || {
            let Ok(stream) = std::os::unix::net::UnixStream::connect(&events) else { return };
            let ours = address.trim_start_matches("0x").to_string();
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if !alive() {
                    return;
                }
                let Some(focused) = line.strip_prefix("activewindowv2>>") else { continue };
                if !focused.is_empty() && focused.trim_start_matches("0x") != ours {
                    raise();
                    // and once hyprland is done raising what was clicked
                    std::thread::sleep(Duration::from_millis(120));
                    raise();
                }
            }
        });
    }

    fn hyprctl(args: &[&str]) -> Option<String> {
        let out = Command::new("hyprctl").args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    // this process's window with that title: its address and where it is
    fn find(title: &str) -> Option<(String, (f64, f64))> {
        let clients: serde_json::Value = serde_json::from_str(&hyprctl(&["clients", "-j"])?).ok()?;
        clients.as_array()?.iter().find_map(|c| {
            if c["title"] != title || c["pid"].as_u64() != Some(u64::from(std::process::id())) {
                return None;
            }
            let at = (c["at"][0].as_f64()?, c["at"][1].as_f64()?);
            Some((c["address"].as_str()?.to_string(), at))
        })
    }

    // the overlay's gone: its keep-on-top threads stop with this
    pub fn forget() {
        set(&ADDRESS, None);
    }

    // once it's mapped: floating, sized, and where it was left or where the window was
    pub fn settle(app: AppHandle, at: Option<(f64, f64)>) {
        set(&ADDRESS, None);
        let near = find("Hashi").map(|(_, at)| at);
        std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(3);
            let address = loop {
                if let Some((address, _)) = find("Hashi overlay") {
                    break address;
                }
                if Instant::now() > until {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            // a window that can't be resized gets floated, and then can't be sized
            let (done, resizable) = std::sync::mpsc::channel();
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Some(w) = handle.get_webview_window(OVERLAY) {
                    let _ = w.set_resizable(true);
                }
                let _ = done.send(());
            });
            let _ = resizable.recv_timeout(Duration::from_secs(1));
            let (width, height) = overlay_size();
            let a = format!("address:{address}");
            // the border first, or taking it away moves the window by its width
            let mut batch = vec![
                format!("dispatch setprop {a} border_size 0"),
                format!("dispatch setprop {a} no_shadow 1"),
                rounding(&a),
                blur(&a),
                format!("dispatch resizewindowpixel exact {width} {height},{a}"),
            ];
            if let Some((x, y)) = at.or(near) {
                batch.push(format!("dispatch movewindowpixel exact {x} {y},{a}"));
            }
            set(&ADDRESS, Some(address.clone()));
            hyprctl(&["--batch", &batch.join(" ; ")]);
            keep_on_top(address);
        });
    }

    // it keeps a floating window's middle where it was, the card keeps its top
    pub fn resize(w: f64, h: f64) {
        if ADDRESS.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
            return;
        }
        std::thread::spawn(move || {
            let Some((address, (x, y))) = find("Hashi overlay") else { return };
            let a = format!("address:{address}");
            hyprctl(&["--batch", &format!("dispatch resizewindowpixel exact {w} {h},{a} ; dispatch movewindowpixel exact {x} {y},{a} ; {}", rounding(&a))]);
        });
    }

    pub fn set_blur(on: bool) {
        set(&BLUR, on);
        let Some(address) = ADDRESS.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
        std::thread::spawn(move || hyprctl(&["--batch", &blur(&format!("address:{address}"))]));
    }

    // its window never hears where it was moved to
    pub fn place() -> Option<(f64, f64)> {
        find("Hashi overlay").map(|(_, at)| at)
    }
}

// webview2's right-click menu is a browser's, with back, refresh, save as, print and share. only
// the editing items are kept, so it shows on inputs and links and nowhere else
#[cfg(windows)]
fn trim_context_menu(w: &WebviewWindow) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2_11, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR};
    use webview2_com::{take_pwstr, ContextMenuRequestedEventHandler};
    use windows_core::{Interface, PWSTR};

    const KEEP: [&str; 7] = ["undo", "redo", "cut", "copy", "paste", "selectAll", "copyLinkLocation"];
    let _ = w.with_webview(|v| {
        // SAFETY: webview2 COM calls on the thread that owns the webview, which is where
        // with_webview and the event handler run
        unsafe {
            let Ok(webview) = v.controller().CoreWebView2().and_then(|w| w.cast::<ICoreWebView2_11>()) else { return };
            let handler = ContextMenuRequestedEventHandler::create(Box::new(|_, args| {
                let Some(args) = args else { return Ok(()) };
                let items = args.MenuItems()?;
                let mut count = 0;
                items.Count(&mut count)?;
                // from the end, so a separator knows whether anything kept comes after it
                let mut item_after = false;
                for i in (0..count).rev() {
                    let item = items.GetValueAtIndex(i)?;
                    let mut kind = COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND::default();
                    item.Kind(&mut kind)?;
                    let keep = if kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR {
                        item_after
                    } else {
                        let mut name = PWSTR::null();
                        item.Name(&mut name)?;
                        KEEP.contains(&take_pwstr(name).as_str())
                    };
                    if keep {
                        item_after = kind != COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR;
                    } else {
                        items.RemoveValueAtIndex(i)?;
                    }
                }
                items.Count(&mut count)?;
                if count > 0 && !item_after {
                    items.RemoveValueAtIndex(0)?;
                    count -= 1;
                }
                if count == 0 {
                    args.SetHandled(true)?;
                }
                Ok(())
            }));
            let mut token = 0;
            let _ = webview.add_ContextMenuRequested(&handler, &mut token);
        }
    });
}

// the main window loads hidden behind it and comes up once the animation is done
fn opening(app: &AppHandle, url: &str) {
    let splash = WebviewWindowBuilder::new(app, OPENING, WebviewUrl::External(format!("{url}?opening").parse().expect("counter url")))
        .title("Hashi")
        .inner_size(200.0, 200.0)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .center()
        .build();
    let splash = match splash {
        Ok(w) => w,
        Err(e) => {
            eprintln!("no opening window: {e}");
            return opened(app);
        }
    };
    // hyprland floats it at half the screen whatever size it asks for, and all of that would
    // catch clicks
    let _ = splash.set_ignore_cursor_events(true);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        let ready = app.clone();
        let _ = app.run_on_main_thread(move || {
            if ready.get_webview_window(OPENING).is_some() {
                opened(&ready);
            }
        });
    });
}

fn opened(app: &AppHandle) {
    show(app);
    if let Some(w) = app.get_webview_window(OPENING) {
        let _ = w.destroy();
    }
}

fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(OVERLAY) {
        #[cfg(target_os = "linux")]
        if let Some((x, y)) = hyprland().then(hypr::place).flatten() {
            set(&OVERLAY_AT, Some((x, y)));
            tell(&format!("overlay-place {x},{y}"));
        }
        #[cfg(target_os = "linux")]
        hypr::forget();
        let _ = w.destroy();
    }
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn commands(app: AppHandle) {
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let run = app.clone();
        let _ = app.run_on_main_thread(move || command(&run, &line));
    }
    // the updater exits by itself once the installer runs
    if INSTALLING.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_secs(30));
    }
    app.exit(0);
}

fn command(app: &AppHandle, line: &str) {
    let (what, rest) = line.split_once(' ').unwrap_or((line, ""));
    match what {
        "show" if app.get_webview_window(OPENING).is_some() => opened(app),
        "show" => show(app),
        "opened" => opened(app),
        "minimize" => {
            if let Some(w) = app.get_webview_window(MAIN) {
                let _ = w.minimize();
            }
        }
        "maximize" => {
            if let Some(w) = app.get_webview_window(MAIN) {
                let _ = if w.is_maximized().unwrap_or(false) { w.unmaximize() } else { w.maximize() };
            }
        }
        "on-top" => {
            if let Some(w) = app.get_webview_window(MAIN) {
                let _ = w.set_always_on_top(rest == "on");
            }
        }
        "drag" => {
            if let Some(w) = app.get_webview_window(OVERLAY).or_else(|| app.get_webview_window(MAIN)) {
                let _ = w.start_dragging();
            }
        }
        "overlay" => overlay(app, rest == "on"),
        "resize" => {
            if let Some(w) = app.get_webview_window(OVERLAY) {
                let _ = w.as_ref().window().start_resize_dragging(tauri_runtime::ResizeDirection::SouthEast);
            }
        }
        "overlay-size" => {
            let mut n = rest.split(' ').map(str::parse::<f64>);
            let (Some(Ok(w)), Some(Ok(h)), Some(Ok(percent))) = (n.next(), n.next(), n.next()) else { return };
            set(&OVERLAY_SIZE, (w, h));
            // the card's, for hyprland's corners to match its own
            set(&OVERLAY_ZOOM, percent / 100.0);
            size_overlay(app);
        }
        "overlay-blur" => {
            #[cfg(target_os = "linux")]
            if hyprland() {
                hypr::set_blur(rest == "on");
            }
        }
        // while a map is played, clicks go to the game
        "overlay-through" => {
            if let Some(w) = app.get_webview_window(OVERLAY) {
                let _ = w.set_ignore_cursor_events(rest == "on");
            }
        }
        "update" => {
            let (app, url) = (app.clone(), rest.to_string());
            tauri::async_runtime::spawn(async move {
                if let Err(e) = update(&app, &url).await {
                    tell(&format!("update-failed {e}"));
                }
            });
        }
        _ => {}
    }
}

async fn update(app: &AppHandle, url: &str) -> Result<(), String> {
    let endpoint = url.parse().map_err(|e| format!("{e}"))?;
    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|e| e.to_string())?
        // windows: the installer closes the app itself, the helper finishes up first
        .on_before_exit(move || {
            tell("shutdown");
            if let Some(port) = HELPER_PORT.get() {
                wait_until_closed(*port);
            }
        })
        // it would start this window again, without the background. the installer starts the app
        .restart_after_install(false)
        .build()
        .map_err(|e| e.to_string())?;
    let update = updater.check().await.map_err(|e| e.to_string())?.ok_or("There's no update for this copy.")?;
    let mut done = 0u64;
    let mut said = Instant::now() - Duration::from_secs(1);
    let bytes = update
        .download(
            |chunk, total| {
                done += chunk as u64;
                if said.elapsed() >= Duration::from_millis(100) {
                    said = Instant::now();
                    tell(&format!("progress {done} {}", total.map(|t| t.to_string()).unwrap_or_default()));
                }
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    // .deb and .rpm ask for the password here
    INSTALLING.store(cfg!(windows), Ordering::SeqCst);
    let installed = tauri::async_runtime::spawn_blocking(move || update.install(bytes)).await;
    INSTALLING.store(false, Ordering::SeqCst);
    installed.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    tell("relaunch");
    Ok(())
}

fn port_of(url: &str) -> Option<u16> {
    let rest = url.strip_prefix("http://127.0.0.1:")?;
    rest.split(['/', '?']).next()?.parse().ok()
}

fn wait_until_closed(port: u16) {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until && TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
        std::thread::sleep(Duration::from_millis(100));
    }
}

// --osu-b5 of the theme: hsl(hue, 10% * sat, 15% - 8% * (1 - sat)), as rgb
fn backdrop((hue, sat): (u16, u8)) -> tauri::window::Color {
    let f = f32::from(sat) / 100.0;
    let (h, s, l) = (f32::from(hue) / 360.0, 0.1 * f, 0.15 - 0.08 * (1.0 - f));
    let a = s * l.min(1.0 - l);
    let channel = |n: f32| {
        let k = (n + h * 12.0) % 12.0;
        let v = l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0);
        (v * 255.0).round() as u8
    };
    tauri::window::Color(channel(0.0), channel(8.0), channel(4.0), 255)
}
