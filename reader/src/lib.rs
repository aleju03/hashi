// osu!stable and osu!lazer's memory, read for what's selected and played. the reader in osu/ is
// LeoBlack's (github.com/LeoBlackMT/osumania_map_analyser, MIT, see LICENSE-LEOBLACK), taken from its
// desktop shell with a Linux side added next to its Windows one

pub mod osu;

pub use osu::{spawn_osu_reader, Reader};

// what the shell around the reader gave it
pub(crate) mod server {
    pub mod log {
        pub fn log_at(level: &str, msg: &str) {
            if level != "debug" && level != "trace" {
                eprintln!("[reader] {level}: {msg}");
            }
        }
    }

    pub fn now_ms() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }

    pub fn md5_hex_bytes(bytes: &[u8]) -> String {
        use md5::{Digest, Md5};
        Md5::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn md5_hex(input: &str) -> String {
        md5_hex_bytes(input.as_bytes())
    }
}

pub(crate) mod config {
    use std::path::PathBuf;

    // lazer's data folder when it isn't where lazer says: LAZER_DIR, as the app has it
    pub fn lazer_storage_root() -> Option<PathBuf> {
        std::env::var_os("LAZER_DIR").filter(|v| !v.is_empty()).map(PathBuf::from)
    }
}
