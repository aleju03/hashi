// osu!'s memory, read by the hashi-reader program next to this one. it's a program of its own so that on linux only it needs cap_sys_ptrace, not the app with its webview
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::{log, tosu, Bridge};

const RETRY_BLOCKED: Duration = Duration::from_secs(5);

const NAME: &str = if cfg!(windows) { "hashi-reader.exe" } else { "hashi-reader" };

// the one next to the app, or OSU_READER for development. an AppImage's files are in a read-only
// mount that can't carry a capability, so from one it runs a copy in the data folder, rewritten
// only when the app brings a different one, so the capability given to it stays
fn reader_exe(data_dir: &Path) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("OSU_READER").filter(|p| !p.is_empty()) {
        return Some(p.into());
    }
    let bundled = std::env::current_exe().ok()?.parent()?.join(NAME);
    if !bundled.is_file() {
        return None;
    }
    if cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some() {
        return copied_out(&bundled, data_dir);
    }
    Some(bundled)
}

fn copied_out(bundled: &Path, data_dir: &Path) -> Option<PathBuf> {
    let copy = data_dir.join(NAME);
    let bytes = std::fs::read(bundled).ok()?;
    if std::fs::read(&copy).ok().as_deref() != Some(&bytes[..]) {
        // written aside and moved over, a running copy keeps its file
        let aside = data_dir.join(format!("{NAME}.new"));
        std::fs::create_dir_all(data_dir).ok()?;
        std::fs::write(&aside, &bytes).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&aside, std::fs::Permissions::from_mode(0o755)).ok()?;
        }
        std::fs::rename(&aside, &copy).ok()?;
    }
    Some(copy)
}

pub(crate) fn start(b: &Arc<Bridge>) {
    let b2 = b.clone();
    b.spawn(async move {
        let b = b2;
        let mut warned = false;
        loop {
            let Some(exe) = reader_exe(&b.env.data_dir) else {
                if !warned {
                    log!("{NAME} isn't next to the app, nothing is read from osu!");
                    warned = true;
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            };
            let mut cmd = Command::new(&exe);
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(if cfg!(debug_assertions) { Stdio::inherit() } else { Stdio::null() })
                .kill_on_drop(true);
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            match cmd.spawn() {
                Ok(mut child) => {
                    b.st().tosu.connected = true;
                    b.broadcast();
                    // it leaves once this closes, with the app
                    let _stdin = child.stdin.take();
                    let mut blocked_since = None;
                    if let Some(out) = child.stdout.take() {
                        let mut lines = BufReader::new(out).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                            // a capability only counts from a program's start, so a fresh one picks up
                            // the fix as soon as it's run
                            if !on_line(&b, &exe, msg) {
                                blocked_since = None;
                            } else if blocked_since.get_or_insert_with(Instant::now).elapsed() >= RETRY_BLOCKED {
                                break;
                            }
                        }
                    }
                    let _ = child.kill().await;
                    if blocked_since.is_none() {
                        log!("{NAME} stopped, starting it again");
                    }
                }
                Err(e) => log!("could not start {NAME}: {e}"),
            }
            tosu::reader_gone(&b);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

// { phase, reason, pid, frame }. true while linux keeps it out
fn on_line(b: &Arc<Bridge>, exe: &Path, msg: Value) -> bool {
    let denied = msg.get("reason").and_then(Value::as_str) == Some("access-denied");
    let blocked = (denied && cfg!(target_os = "linux")).then(|| format!("sudo setcap cap_sys_ptrace=eip {}", shell_quote(&exe.display().to_string())));
    tosu::set_blocked(b, blocked);
    match msg.get("frame").filter(|f| f.is_object()) {
        Some(frame) => tosu::on_tosu_message(b, frame.clone()),
        None => tosu::osu_gone(b),
    }
    denied
}

// for a command that gets copied into a terminal
fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@%,=".contains(c)) {
        return s.into();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_is_kept_while_the_same_and_replaced_when_not() {
        let dir = std::env::temp_dir().join(format!("hashi-reader-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bundled = dir.join("bundled");
        let data = dir.join("data");
        std::fs::write(&bundled, b"one").unwrap();
        let copy = copied_out(&bundled, &data).unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), b"one");
        let written = std::fs::metadata(&copy).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        copied_out(&bundled, &data).unwrap();
        assert_eq!(std::fs::metadata(&copy).unwrap().modified().unwrap(), written, "an unchanged reader isn't rewritten");
        std::fs::write(&bundled, b"two").unwrap();
        copied_out(&bundled, &data).unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("/home/a/.local/bin/hashi-reader"), "/home/a/.local/bin/hashi-reader");
        assert_eq!(shell_quote("/home/a/My Apps/hashi-reader"), "'/home/a/My Apps/hashi-reader'");
        assert_eq!(shell_quote("/x/it's/$hashi-reader"), r"'/x/it'\''s/$hashi-reader'");
    }
}
