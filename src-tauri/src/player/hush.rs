use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

// one change at a time, from the window's thread or at its exit
static BUSY: Mutex<()> = Mutex::new(());

// Linux and Windows remember an app's mute for next time, so what was muted is written down
fn file() -> PathBuf {
    mania_bridge::Env::from_env().data_dir.join("muted-osu.json")
}

fn noted() -> BTreeSet<String> {
    std::fs::read(file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn note(keys: &BTreeSet<String>) {
    if keys.is_empty() {
        let _ = std::fs::remove_file(file());
    } else {
        let _ = std::fs::write(file(), serde_json::to_vec(keys).unwrap_or_default());
    }
}

fn mute() {
    let _busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    let mut keys = noted();
    let before = keys.len();
    for s in os::streams() {
        if !s.muted {
            os::set_mute(&s, true);
            keys.insert(s.key.clone());
        }
    }
    if keys.len() != before {
        note(&keys);
    }
}

// unmutes what was muted here, by this window or one before it that didn't get to. One whose osu!
// isn't running stays written down, as its mute comes back when osu! starts. True when none is left
pub fn restore() -> bool {
    let _busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    let mut keys = noted();
    if keys.is_empty() {
        return true;
    }
    let streams = os::streams();
    for s in streams.iter().filter(|s| s.muted && keys.contains(&s.key)) {
        os::set_mute(s, false);
    }
    keys.retain(|k| !streams.iter().any(|s| &s.key == k));
    note(&keys);
    keys.is_empty()
}

pub struct Hush {
    tx: Option<Sender<bool>>,
    worker: Option<JoinHandle<()>>,
    on: bool,
}

impl Hush {
    pub fn start() -> Hush {
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || keep(rx));
        Hush { tx: Some(tx), worker: Some(worker), on: false }
    }

    pub fn set(&mut self, on: bool) {
        if on != self.on {
            self.on = on;
            if let Some(tx) = &self.tx {
                let _ = tx.send(on);
            }
        }
    }

    // the worker unmutes on its way out; once it's gone nothing can mute osu! again
    pub fn stop(&mut self) {
        self.tx = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn keep(rx: Receiver<bool>) {
    let mut on = false;
    loop {
        // osu! restarted meanwhile has a new stream to mute
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(v) if v == on => continue,
            Ok(v) => on = v,
            Err(RecvTimeoutError::Timeout) if on => {}
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                restore();
                return;
            }
        }
        if on {
            mute();
        } else {
            restore();
        }
    }
}

fn is_osu(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "osu!" || name == "osu!.exe"
}

// a stream is known by its app's name, which outlives it
#[cfg(target_os = "linux")]
mod os {
    use std::process::{Command, Stdio};

    use serde_json::Value;

    pub struct Stream {
        pub key: String,
        pub muted: bool,
        index: u64,
    }

    pub fn streams() -> Vec<Stream> {
        let Ok(out) = Command::new("pactl").args(["-f", "json", "list", "sink-inputs"]).stderr(Stdio::null()).output() else { return vec![] };
        let list: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap_or_default();
        list.iter()
            .filter_map(|s| {
                let prop = |k: &str| s.get("properties").and_then(|p| p.get(k)).and_then(Value::as_str).unwrap_or("");
                let name = prop("application.name");
                let osu = super::is_osu(prop("application.process.binary")) || name.ends_with("[osu!]") || super::is_osu(name);
                let index = s.get("index").and_then(Value::as_u64).filter(|_| osu)?;
                Some(Stream { key: name.to_string(), muted: s.get("mute").and_then(Value::as_bool).unwrap_or(false), index })
            })
            .collect()
    }

    pub fn set_mute(s: &Stream, on: bool) {
        let _ = Command::new("pactl").args(["set-sink-input-mute", &s.index.to_string(), if on { "1" } else { "0" }]).stderr(Stdio::null()).status();
    }
}

// a session is known by its process's exe name
#[cfg(windows)]
mod os {
    use windows::core::{Interface, PWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Media::Audio::{
        eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};

    pub struct Stream {
        pub key: String,
        pub muted: bool,
        volume: ISimpleAudioVolume,
    }

    fn program(pid: u32) -> Option<String> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let named = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
            let _ = CloseHandle(h);
            named.ok()?;
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            Some(path.rsplit('\\').next()?.to_string())
        }
    }

    pub fn streams() -> Vec<Stream> {
        let mut out = vec![];
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(devices) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return out };
            let Ok(devices) = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return out };
            for i in 0..devices.GetCount().unwrap_or(0) {
                let Ok(device) = devices.Item(i) else { continue };
                let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
                let Ok(sessions) = manager.GetSessionEnumerator() else { continue };
                for j in 0..sessions.GetCount().unwrap_or(0) {
                    let Ok(session) = sessions.GetSession(j) else { continue };
                    let Some(name) = session.cast::<IAudioSessionControl2>().ok().and_then(|s| s.GetProcessId().ok()).and_then(program) else { continue };
                    if !super::is_osu(&name) {
                        continue;
                    }
                    let Ok(volume) = session.cast::<ISimpleAudioVolume>() else { continue };
                    let muted = volume.GetMute().is_ok_and(|m| m.as_bool());
                    out.push(Stream { key: name.to_ascii_lowercase(), muted, volume });
                }
            }
        }
        out
    }

    pub fn set_mute(s: &Stream, on: bool) {
        unsafe {
            let _ = s.volume.SetMute(on, std::ptr::null());
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod os {
    pub struct Stream {
        pub key: String,
        pub muted: bool,
    }

    pub fn streams() -> Vec<Stream> {
        vec![]
    }

    pub fn set_mute(_: &Stream, _: bool) {}
}
