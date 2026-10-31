use tokio::runtime::Runtime;

use crate::background::{hide, quit, show, toggle};

pub(crate) fn rgba(png: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let mut decoder = png::Decoder::new(png);
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    (info.color_type == png::ColorType::Rgba && info.bit_depth == png::BitDepth::Eight).then(|| {
        buf.truncate(info.buffer_size());
        (buf, info.width, info.height)
    })
}

// tauri's linux tray (libappindicator) only has the menu, a left click never reaches the app
#[cfg(target_os = "linux")]
mod imp {
    use std::sync::OnceLock;

    use ksni::TrayMethods;
    use mania_bridge::log;

    use super::*;

    struct Tray;

    static HANDLE: OnceLock<ksni::Handle<Tray>> = OnceLock::new();

    impl ksni::Tray for Tray {
        fn id(&self) -> String {
            "mania-bridge".into()
        }

        fn title(&self) -> String {
            "Hashi".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            [&include_bytes!("../icons/32x32.png")[..], &include_bytes!("../icons/64x64.png")[..]]
                .into_iter()
                .filter_map(rgba)
                .map(|(data, width, height)| ksni::Icon {
                    width: width as i32,
                    height: height as i32,
                    data: data.chunks_exact(4).flat_map(|p| [p[3], p[0], p[1], p[2]]).collect(),
                })
                .collect()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip { title: "Hashi".into(), ..Default::default() }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            toggle();
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            let item = |label: &str, run: fn()| ksni::menu::StandardItem { label: label.into(), activate: Box::new(move |_: &mut Self| run()), ..Default::default() }.into();
            vec![item("Show", show), item("Hide to tray", hide), ksni::MenuItem::Separator, item("Quit", quit)]
        }
    }

    pub fn start(rt: &Runtime) -> bool {
        match rt.block_on(Tray.spawn()) {
            Ok(handle) => {
                let _ = HANDLE.set(handle);
                true
            }
            Err(e) => {
                log!("no tray: {e}");
                false
            }
        }
    }

    pub fn park(rt: &Runtime) {
        rt.block_on(std::future::pending::<()>());
    }
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;

    use mania_bridge::log;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
    use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG};

    use super::*;

    thread_local! {
        static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
    }

    fn build() -> Result<TrayIcon, String> {
        let menu = Menu::new();
        menu.append_items(&[
            &MenuItem::with_id("show", "Show", true, None),
            &MenuItem::with_id("hide", "Hide to tray", true, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id("quit", "Quit", true, None),
        ])
        .map_err(|e| e.to_string())?;
        let (data, width, height) = rgba(include_bytes!("../icons/32x32.png")).ok_or("bad icon")?;
        TrayIconBuilder::new()
            .with_tooltip("Hashi")
            .with_icon(Icon::from_rgba(data, width, height).map_err(|e| e.to_string())?)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| e.to_string())
    }

    // made on the main thread, which then runs its message loop in park()
    pub fn start(_: &Runtime) -> bool {
        MenuEvent::set_event_handler(Some(|e: MenuEvent| match e.id.0.as_str() {
            "show" => show(),
            "hide" => hide(),
            "quit" => quit(),
            _ => {}
        }));
        TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                toggle();
            }
        }));
        match build() {
            Ok(tray) => {
                TRAY.with(|t| *t.borrow_mut() = Some(tray));
                true
            }
            Err(e) => {
                log!("no tray: {e}");
                false
            }
        }
    }

    pub fn park(_: &Runtime) {
        // SAFETY: the usual win32 message loop over a zeroed MSG, on the thread that made the tray
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    use super::*;

    pub fn start(_: &Runtime) -> bool {
        false
    }

    pub fn park(rt: &Runtime) {
        rt.block_on(std::future::pending::<()>());
    }
}

pub use imp::{park, start};
