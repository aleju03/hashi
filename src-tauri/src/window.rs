// the window, started by the background process. it says what happens on stdout and does what
// it's told on stdin, and leaves when stdin closes

use std::io::{BufRead, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_updater::UpdaterExt;

use crate::background::TELL;

const MAIN: &str = "main";
const OPENING: &str = "opening";
const SKIN: &str = "skin";

static HELPER_PORT: OnceLock<u16> = OnceLock::new();
static COUNTER_URL: OnceLock<String> = OnceLock::new();
// windows: the background goes first, this one has to stay until the installer is started
static INSTALLING: AtomicBool = AtomicBool::new(false);

struct Args {
    url: String,
    theme: (u16, u8),
    place: Option<Place>,
    opening: bool,
    on_top: bool,
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
    }
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
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let handle = app.handle().clone();
            build(&handle, &args, !args.opening)?;
            if args.opening {
                opening(&handle, &args.url);
            }
            std::thread::spawn(move || commands(handle));
            Ok(())
        })
        .on_window_event(move |w, e| {
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
        // links that would open a new window (target=_blank) go to the browser instead
        .on_new_window(|url, _| {
            if matches!(url.scheme(), "http" | "https") {
                mania_bridge::open_url(url.as_str());
            }
            tauri::webview::NewWindowResponse::Deny
        });
    if let Some(p) = args.place {
        window = window.position(p.x, p.y).inner_size(p.width, p.height).maximized(p.maximized);
    }
    let _window = window.build()?;
    #[cfg(windows)]
    trim_context_menu(&_window);
    Ok(())
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

// The skin preview at the size osu! draws it, pixel for pixel: `1440x900`, or `screen` when the
// game fills the screen. One that doesn't fit goes fullscreen, the closest there is to the game.
fn skin_window(app: &AppHandle, size: &str) {
    if let Some(w) = app.get_webview_window(SKIN) {
        let _ = w.destroy();
    }
    let Some(base) = COUNTER_URL.get() else { return };
    let monitor = app.get_webview_window(MAIN).and_then(|w| w.current_monitor().ok().flatten()).or_else(|| app.primary_monitor().ok().flatten());
    let screen = monitor.as_ref().map(|m| *m.size());
    let scale = monitor.as_ref().map_or(1.0, |m| m.scale_factor());
    let wanted = size.split_once('x').and_then(|(w, h)| Some(PhysicalSize::new(w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)));
    let Some(px) = wanted.or(screen) else { return };
    let full = wanted.is_none() || screen.is_some_and(|s| px.width >= s.width || px.height >= s.height);
    let Ok(url) = format!("{base}?skin-window={}x{}", px.width, px.height).parse() else { return };
    let window = WebviewWindowBuilder::new(app, SKIN, WebviewUrl::External(url))
        .title(format!("Hashi · your skin at {}×{}", px.width, px.height))
        .inner_size(f64::from(px.width) / scale, f64::from(px.height) / scale)
        // fixed in size, so tiling window managers float it at that size
        .resizable(false)
        .fullscreen(full)
        .center()
        .background_color(tauri::window::Color(0, 0, 0, 255))
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .build();
    match window {
        Ok(w) => {
            if !full {
                let _ = w.set_size(px);
            }
            let _ = w.set_focus();
        }
        Err(e) => eprintln!("no skin window: {e}"),
    }
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
            if let Some(w) = app.get_webview_window(MAIN) {
                let _ = w.start_dragging();
            }
        }
        "skin-window" => skin_window(app, rest),
        "skin-close" => {
            if let Some(w) = app.get_webview_window(SKIN) {
                let _ = w.destroy();
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
