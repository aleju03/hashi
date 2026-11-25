// prints what the reader sees, each time it changes: cargo run -p osu-reader --example dump
use std::sync::Arc;
use std::time::Duration;

use osu_reader::osu::lazer::score::{HitEventLog, ReplayFrameLog};

fn main() {
    let reader = osu_reader::spawn_osu_reader(Arc::new(HitEventLog::new()), Arc::new(ReplayFrameLog::default()));
    let mut last = String::new();
    loop {
        let s = reader.state();
        let line = format!(
            "phase={} health={} reason={:?} client={:?} pid={:?} gaps={:?} chain={:?}\n{}",
            s.phase,
            s.health,
            s.reason,
            s.client,
            s.pid,
            s.lazer.as_ref().map(|l| l.gaps.clone()),
            s.lazer.as_ref().and_then(|l| l.chain.clone()),
            s.packet.as_ref().map(|p| serde_json::to_string_pretty(p).unwrap_or_default()).unwrap_or_else(|| "(no packet)".into()),
        );
        if line != last {
            println!("---\n{line}");
            last = line;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
