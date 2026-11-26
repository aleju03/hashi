use std::collections::VecDeque;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::Bridge;

// a map's results for a rate, as song select goes back and forth over a few
const KEEP: usize = 48;

#[derive(Default)]
pub(crate) struct Dans {
    done: VecDeque<(String, Value)>,
}

// one at a time, the newest selection is the one waited on
static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Wanted {
    md5: String,
    rate: f64,
    od: Option<&'static str>,
    // Invert and Hold Off make another chart, which the site doesn't rate
    converted: bool,
}

fn wanted(b: &Bridge) -> Option<Wanted> {
    let s = b.st();
    let map = s.reader.map.as_ref()?;
    let mods = map.get("mods").and_then(Value::as_array);
    Some(Wanted {
        md5: map.get("md5")?.as_str()?.to_string(),
        rate: map.get("rate").and_then(Value::as_f64).unwrap_or(1.0),
        od: match map.get("od").and_then(Value::as_str) {
            Some("HR") => Some("HR"),
            Some("EZ") => Some("EZ"),
            _ => None,
        },
        converted: mods.is_some_and(|m| m.iter().any(|m| m == "IN" || m == "HO")),
    })
}

// the selected map's dan and MSD at its rate. 409 when osu! has moved on from `md5`
pub(crate) async fn selected(b: &Arc<Bridge>, md5: &str) -> (u16, Value) {
    let _one = RUNNING.lock().await;
    let Some(w) = wanted(b).filter(|w| w.md5 == md5) else { return (409, json!({ "error": "Another map is selected now." })) };
    let head = json!({ "md5": w.md5, "rate": w.rate, "od": w.od });
    if w.converted {
        return (200, head);
    }
    let Some(path) = crate::skin::selected_chart(b) else { return (404, json!({ "error": "Couldn't find this map." })) };
    rated(b, head, &path, w.od).await
}

// the map the rate changer has open, at any rate it can make
pub(crate) async fn at_rate(b: &Arc<Bridge>, md5: &str, rate: f64) -> (u16, Value) {
    let Some(rate) = crate::rate::checked(rate) else { return (400, json!({ "error": crate::rate::RANGE_ERROR })) };
    let Some(src) = crate::cut::source(md5) else { return (404, json!({ "error": "This map isn't open here anymore." })) };
    let _one = RUNNING.lock().await;
    rated(b, json!({ "md5": md5, "rate": rate, "od": null }), &src.chart, None).await
}

// `head` names the md5 and rate, the file has to match it
async fn rated(b: &Arc<Bridge>, head: Value, path: &std::path::Path, od: Option<&str>) -> (u16, Value) {
    let md5 = head["md5"].as_str().unwrap_or_default().to_string();
    let rate = head["rate"].as_f64().unwrap_or(1.0);
    let key = format!("{md5}|{rate}|{}", od.unwrap_or(""));
    if let Some((_, v)) = b.st().dans.done.iter().find(|(k, _)| *k == key) {
        return (200, v.clone());
    }
    let Ok(bytes) = crate::read_capped(path, crate::MAX_CHART_BYTES).await else { return (404, json!({ "error": "Couldn't read this map." })) };
    if crate::replays::md5_hex(&bytes) != md5 {
        return (409, json!({ "error": "The map changed." }));
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let od = match od {
        Some("HR") => Some(dan_calc::PlayedOd::Hr),
        Some("EZ") => Some(dan_calc::PlayedOd::Ez),
        _ => None,
    };
    let options = dan_calc::Options { rate, od, ..Default::default() };
    let Some(Ok(e)) = estimate(text, options).await else { return (422, json!({ "error": "Couldn't rate this map." })) };

    let dan = |d: &dan_calc::Dan| json!({ "label": d.display_name, "ln": d.family == dan_calc::Family::Ln });
    // as the site: the other family's dan only on a hybrid
    let other = match e.primary.as_ref().map(|d| d.family).filter(|_| e.hybrid) {
        Some(dan_calc::Family::Rice) => e.ln.as_ref(),
        Some(dan_calc::Family::Ln) => e.rice.as_ref(),
        None => None,
    };
    let msd = e.msd.as_ref().and_then(|m| m.iter().find(|(k, _)| k == "Overall")).map(|(_, v)| *v).filter(|v| v.is_finite() && *v > 0.0);
    let mut v = head;
    v["dan"] = e.primary.as_ref().map(dan).into();
    v["other"] = other.map(dan).into();
    v["msd"] = msd.into();
    let mut s = b.st();
    s.dans.done.push_front((key, v.clone()));
    s.dans.done.truncate(KEEP);
    (200, v)
}

type Job = (String, dan_calc::Options, tokio::sync::oneshot::Sender<Result<dan_calc::Estimate, dan_calc::Error>>);

// always on the same thread: glibc gives each thread its own arena and keeps what a big chart
// freed in it, so a new blocking thread each time would hold on to tens of MB
async fn estimate(text: String, options: dan_calc::Options) -> Option<Result<dan_calc::Estimate, dan_calc::Error>> {
    static WORKER: std::sync::OnceLock<std::sync::mpsc::Sender<Job>> = std::sync::OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        keep_big_blocks_apart();
        let (send, jobs) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("dan".into())
            .spawn(move || {
                for (text, options, done) in jobs {
                    let e = dan_calc::estimate(&text, &options);
                    drop(text);
                    give_back();
                    let _ = done.send(e);
                }
            })
            .expect("dan thread");
        send
    });
    let (done, result) = tokio::sync::oneshot::channel();
    worker.send((text, options, done)).ok()?;
    result.await.ok()
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
extern "C" {
    fn malloc_trim(pad: usize) -> i32;
    fn mallopt(param: i32, value: i32) -> i32;
}

// glibc raises its mmap threshold each time a big block is freed, and MinaCalc's big blocks then
// stay in the heap after a chart: keeping it at its default 128 KB hands them back as they're freed
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn keep_big_blocks_apart() {
    const M_MMAP_THRESHOLD: i32 = -3;
    // SAFETY: a malloc setting, safe to change at any time
    unsafe { mallopt(M_MMAP_THRESHOLD, 128 * 1024) };
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn give_back() {
    // SAFETY: only hands free memory back to the system
    unsafe { malloc_trim(0) };
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn keep_big_blocks_apart() {}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn give_back() {}

#[cfg(test)]
mod tests {
    use serde_json::json;

    // a minute of 4K jumpstream at 170 bpm
    fn chart() -> String {
        let mut text = String::from(
            "osu file format v14\n\n[General]\nAudioFilename: audio.mp3\nMode: 3\n\n[Metadata]\nTitle:t\nArtist:a\nVersion:v\n\n[Difficulty]\nCircleSize:4\nOverallDifficulty:8\nHPDrainRate:8\n\n[TimingPoints]\n0,352.941176470588,4,1,0,100,1,0\n\n[HitObjects]\n",
        );
        let x = [64, 192, 320, 448];
        for i in 0..680 {
            let t = 1000 + i * 88;
            text += &format!("{},192,{t},1,0,0:0:0:0:\n", x[i % 4]);
            if i % 2 == 0 {
                text += &format!("{},192,{t},1,0,0:0:0:0:\n", x[(i + 2) % 4]);
            }
        }
        text
    }

    #[tokio::test]
    async fn selected_map() {
        let (b, dir) = crate::auth::tests::test_bridge("http://127.0.0.1:9");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("map.osu");
        std::fs::write(&file, chart()).unwrap();
        let md5 = crate::replays::md5_hex(chart().as_bytes());
        let select = |rate: f64, mods: &[&str]| {
            let mut s = b.st();
            s.reader.last_frame = Some(json!({ "client": "lazer", "files": { "beatmap": file.to_string_lossy() } }));
            s.reader.map = Some(json!({ "md5": md5, "rate": rate, "od": null, "mods": mods }));
        };

        select(1.0, &[]);
        assert_eq!(super::selected(&b, "0123456789abcdef0123456789abcdef").await.0, 409);
        let (status, one) = super::selected(&b, &md5).await;
        assert_eq!(status, 200, "{one}");
        assert!(one["msd"].as_f64().is_some_and(|m| m > 10.0), "{one}");
        assert!(one["dan"]["label"].is_string(), "{one}");

        select(1.3, &[]);
        let (_, faster) = super::selected(&b, &md5).await;
        assert_eq!(faster["rate"], 1.3);
        assert!(faster["msd"].as_f64() > one["msd"].as_f64(), "{faster}");

        // the same map at 1x again comes from what was kept
        select(1.0, &[]);
        assert_eq!(super::selected(&b, &md5).await.1, one);

        select(1.0, &["IN"]);
        let (_, inverted) = super::selected(&b, &md5).await;
        assert!(inverted["msd"].is_null() && inverted["dan"].is_null(), "{inverted}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
