// Saves Hashi's skin edits straight into one of lazer's skins, without lazer's "Edit externally".
// lazer keeps a skin as names pointing at files stored under their hashes, in its Realm database.
// The changed files go into its file store, then LazerWriter (lazer-writer/) points the names at
// them. It runs on the .NET and Realm that come with the player's lazer, so they match its database,
// and in a process of its own so a failure there can't take Hashi with it.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::Bridge;

// empty when the build had no .NET SDK to make it with
static WRITER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/LazerWriter.dll"));

// Hashi's own executable started with this runs LazerWriter instead of the app
pub const MODE: &str = "--lazer-write";
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) fn can_save() -> bool {
    !WRITER.is_empty()
}

fn work_dir(b: &Bridge) -> PathBuf {
    b.env.data_dir.join("lazer work")
}

// Changes the lazer skin Hashi read into `found` the way `change` changes a copy of it. The copy is
// made of real files: the folder Hashi reads lazer skins into links to lazer's own, and writing
// through a link would change lazer's stored file under its old hash.
pub(crate) fn save(b: &Bridge, found: &crate::skin::Found, change: impl FnOnce(&Path) -> Result<Value, String>) -> Result<Value, String> {
    if !can_save() {
        return Err("This copy of Hashi can't save to lazer skins. Save as a new skin instead.".into());
    }
    let root = b.st().tosu.lazer_files_dir.parent().map(Path::to_path_buf).ok_or("Hashi can't find lazer's folder.")?;
    let id = crate::skin::lazer_skin_id(&root).ok_or("Hashi can't tell which skin lazer has on.")?;
    let work = work_dir(b).join("skin");
    let _ = std::fs::remove_dir_all(&work);
    crate::skin_render::copy_tree(&found.dir, &work).map_err(|e| format!("Couldn't copy the skin: {e}"))?;
    let result = change(&work).and_then(|v| push(b, &root, id, &found.dir, &work).map(|_| v));
    let _ = std::fs::remove_dir_all(&work);
    result
}

// what differs between the skin as lazer has it (`was`) and `now`, written into lazer
fn push(b: &Bridge, root: &Path, id: uuid::Uuid, was: &Path, now: &Path) -> Result<usize, String> {
    let before = tree(was);
    let after = tree(now);
    let has: std::collections::HashSet<String> = after.keys().map(|k| k.to_lowercase()).collect();
    let mut set = vec![];
    for (name, path) in &after {
        let hash = sha256(path)?;
        let same = before.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).is_some_and(|(_, p)| sha256(p).is_ok_and(|h| h == hash));
        if !same {
            set.push((name.clone(), hash, path.clone()));
        }
    }
    let remove: Vec<String> = before.keys().filter(|n| !has.contains(&n.to_lowercase())).cloned().collect();
    if set.is_empty() && remove.is_empty() {
        return Ok(0);
    }
    let files = root.join("files");
    for (_, hash, path) in &set {
        let dest = files.join(&hash[..1]).join(&hash[..2]).join(hash);
        if dest.is_file() {
            continue;
        }
        std::fs::create_dir_all(dest.parent().ok_or("bad path")?).map_err(|e| e.to_string())?;
        // a part file renamed into place, so lazer never sees half of one
        let part = dest.with_extension("hashi-part");
        std::fs::copy(path, &part).map_err(|e| format!("Couldn't add a file to lazer's: {e}"))?;
        std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    }
    let dir = work_dir(b);
    let job = dir.join("job.json");
    let result = dir.join("result.json");
    let _ = std::fs::remove_file(&result);
    let body = json!({
        "realm": root.join("client.realm"),
        "skin": id.to_string(),
        "set": set.iter().map(|(name, hash, _)| json!({ "name": name, "hash": hash })).collect::<Vec<_>>(),
        "remove": remove,
        "result": result,
    });
    std::fs::write(&job, body.to_string()).map_err(|e| e.to_string())?;
    let ran = run_writer(b, &job);
    let answer = std::fs::read(&result).ok().and_then(|r| serde_json::from_slice::<Value>(&r).ok());
    let _ = std::fs::remove_file(&job);
    let _ = std::fs::remove_file(&result);
    ran?;
    match answer {
        Some(a) if a["ok"] == true => {
            crate::skin::forget_stored();
            crate::log!("lazer skin {id}: saved {} files, took out {}", set.len(), remove.len());
            Ok(set.len() + remove.len())
        }
        Some(a) => Err(a["error"].as_str().map_or_else(|| "lazer's database didn't take the change.".into(), |e| format!("Nothing was saved: {e}"))),
        None => Err("The writer stopped before it finished, nothing was saved.".into()),
    }
}

// every file under `dir` by its path with forward slashes, the way lazer names them
fn tree(dir: &Path) -> BTreeMap<String, PathBuf> {
    fn walk(at: &Path, prefix: &str, out: &mut BTreeMap<String, PathBuf>) {
        let Ok(entries) = std::fs::read_dir(at) else { return };
        for e in entries.flatten() {
            let name = format!("{prefix}{}", e.file_name().to_string_lossy());
            match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path(), &format!("{name}/"), out),
                Ok(_) => {
                    out.insert(name, e.path());
                }
                Err(_) => {}
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out);
    out
}

fn sha256(path: &Path) -> Result<String, String> {
    use sha2::Digest;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn run_writer(b: &Bridge, job: &Path) -> Result<(), String> {
    let (runtime, _mount) = runtime(b)?;
    let dll = work_dir(b).join("LazerWriter.dll");
    if std::fs::read(&dll).ok().as_deref() != Some(WRITER) {
        std::fs::write(&dll, WRITER).map_err(|e| e.to_string())?;
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = Command::new(exe)
        .arg(MODE)
        .arg(&runtime)
        .arg(&dll)
        .arg(job)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't start the writer: {e}"))?;
    let until = Instant::now() + WRITE_TIMEOUT;
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if Instant::now() > until {
            let _ = child.kill();
            let _ = child.wait();
            return Err("lazer's database didn't answer in time, nothing was saved.".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        crate::log!("lazer writer: {status} {}", err.trim());
    }
    Ok(())
}

// an AppImage mounted for the writer, unmounted again when it's dropped
struct Mount(Child);

impl Drop for Mount {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(windows)]
const CORECLR: &str = "coreclr.dll";
#[cfg(not(windows))]
const CORECLR: &str = "libcoreclr.so";

// The folder with lazer's .NET runtime and Realm. On Linux lazer is an AppImage: its files are only
// there while it's mounted, by a running lazer or by Hashi for the write.
fn runtime(b: &Bridge) -> Result<(PathBuf, Option<Mount>), String> {
    #[cfg(windows)]
    {
        let _ = b;
        let dir = std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("osulazer").join("current"));
        return match dir.filter(|d| d.join(CORECLR).is_file()) {
            Some(d) => Ok((d, None)),
            None => Err("Hashi can't find lazer's install to save with.".into()),
        };
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(dir) = running_lazer(b) {
            return Ok((dir, None));
        }
        let remembered = std::fs::read_to_string(b.env.data_dir.join("lazer appimage")).ok().map(|s| PathBuf::from(s.trim()));
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let candidates = remembered
            .into_iter()
            .chain([PathBuf::from("/opt/osu-lazer/osu.AppImage"), home.join("Applications/osu.AppImage"), home.join(".local/bin/osu.AppImage")]);
        for image in candidates.filter(|p| p.is_file()) {
            if let Some((dir, mount)) = mount_appimage(&image) {
                return Ok((dir, Some(mount)));
            }
        }
        Err("Hashi can't find lazer's files to save with. Open lazer and try again.".into())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = b;
        Err("Saving to lazer skins isn't supported here yet.".into())
    }
}

// a running lazer's AppImage is mounted, its executable sits next to the runtime. The AppImage's own
// path is kept for when lazer is closed.
#[cfg(target_os = "linux")]
fn running_lazer(b: &Bridge) -> Option<PathBuf> {
    for e in std::fs::read_dir("/proc").ok()?.flatten() {
        if !e.file_name().to_string_lossy().bytes().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(exe) = std::fs::read_link(e.path().join("exe")) else { continue };
        let Some(dir) = exe.parent().filter(|d| exe.file_name().is_some_and(|n| n == "osu!") && d.join(CORECLR).is_file()) else { continue };
        if let Ok(env) = std::fs::read(e.path().join("environ")) {
            if let Some(image) = env.split(|c| *c == 0).find_map(|v| v.strip_prefix(b"APPIMAGE=")) {
                let _ = std::fs::write(b.env.data_dir.join("lazer appimage"), image);
            }
        }
        return Some(dir.to_path_buf());
    }
    None
}

// `--appimage-mount` mounts it and prints where, and keeps it mounted until it's stopped
#[cfg(target_os = "linux")]
fn mount_appimage(image: &Path) -> Option<(PathBuf, Mount)> {
    let mut child = Command::new(image).arg("--appimage-mount").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(out), &mut line);
        let _ = tx.send(line);
    });
    let mount = Mount(child);
    let line = rx.recv_timeout(Duration::from_secs(15)).ok()?;
    let dir = PathBuf::from(line.trim()).join("usr").join("bin");
    dir.join(CORECLR).is_file().then_some((dir, mount))
}

// Hashi started in MODE: [runtime folder, LazerWriter.dll, job.json]. .NET is started from lazer's
// own folder with its assemblies trusted, the way its own launcher does, and runs LazerWriter
pub fn writer_main(args: &[String]) -> i32 {
    let [runtime, dll, job] = args else {
        eprintln!("{MODE} <runtime folder> <LazerWriter.dll> <job.json>");
        return 2;
    };
    match host(Path::new(runtime), Path::new(dll), job) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

type Initialize = unsafe extern "C" fn(
    exe_path: *const std::ffi::c_char,
    app_domain: *const std::ffi::c_char,
    count: i32,
    keys: *const *const std::ffi::c_char,
    values: *const *const std::ffi::c_char,
    handle: *mut *mut std::ffi::c_void,
    domain: *mut u32,
) -> i32;

type Execute = unsafe extern "C" fn(
    handle: *mut std::ffi::c_void,
    domain: u32,
    argc: i32,
    argv: *const *const std::ffi::c_char,
    assembly: *const std::ffi::c_char,
    exit_code: *mut u32,
) -> i32;

fn host(runtime: &Path, dll: &Path, job: &str) -> Result<i32, String> {
    use std::ffi::CString;
    let c = |s: &str| CString::new(s).map_err(|e| e.to_string());
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut trusted: Vec<String> = std::fs::read_dir(runtime)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")))
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    trusted.push(dll.to_string_lossy().into_owned());
    let app = dll.parent().unwrap_or(Path::new(".")).to_string_lossy().into_owned();
    let props = [
        ("TRUSTED_PLATFORM_ASSEMBLIES", trusted.join(sep)),
        ("APP_PATHS", app),
        ("NATIVE_DLL_SEARCH_DIRECTORIES", format!("{}{sep}", runtime.to_string_lossy())),
        ("System.Globalization.Invariant", "true".into()),
    ];
    let keys: Vec<CString> = props.iter().map(|(k, _)| c(k)).collect::<Result<_, _>>()?;
    let values: Vec<CString> = props.iter().map(|(_, v)| c(v)).collect::<Result<_, _>>()?;
    let key_ptrs: Vec<_> = keys.iter().map(|k| k.as_ptr()).collect();
    let value_ptrs: Vec<_> = values.iter().map(|v| v.as_ptr()).collect();
    let exe = c(&std::env::current_exe().map_err(|e| e.to_string())?.to_string_lossy())?;
    let name = c("Hashi")?;
    let assembly = c(&dll.to_string_lossy())?;
    let arg = c(job)?;
    // SAFETY: coreclr's hosting functions, called the way its docs and dotnet's own hosts call them,
    // with every string alive until the call returns. The library stays loaded until the process ends.
    unsafe {
        let lib = libloading::Library::new(runtime.join(CORECLR)).map_err(|e| format!("Couldn't load lazer's .NET: {e}"))?;
        let init: libloading::Symbol<Initialize> = lib.get(b"coreclr_initialize\0").map_err(|e| e.to_string())?;
        let execute: libloading::Symbol<Execute> = lib.get(b"coreclr_execute_assembly\0").map_err(|e| e.to_string())?;
        let mut handle = std::ptr::null_mut();
        let mut domain = 0u32;
        let rc = init(exe.as_ptr(), name.as_ptr(), props.len() as i32, key_ptrs.as_ptr(), value_ptrs.as_ptr(), &mut handle, &mut domain);
        if rc < 0 {
            return Err(format!("lazer's .NET didn't start ({rc:#x})"));
        }
        let mut code = 0u32;
        let argv = [arg.as_ptr()];
        let rc = execute(handle, domain, 1, argv.as_ptr(), assembly.as_ptr(), &mut code);
        std::mem::forget(lib);
        if rc < 0 {
            return Err(format!("LazerWriter didn't run ({rc:#x})"));
        }
        Ok(code as i32)
    }
}
