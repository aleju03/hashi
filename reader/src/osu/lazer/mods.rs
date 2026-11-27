// mods.rs —— lazer 的 mod 自定义设置（自定义倍速 / DA 自定义 OD）的读取准备。
//
// 现状（2026-10）：`frame::read_lazer_mods` 从 `SelectedMods` 的 mod 对象上按固定
// 候选位移猜 bindable 值。倍速（DT/HT 的 `BindableNumber<double>`）能读；但 DA 的
// `OverallDifficulty` 是 `DifficultyBindable : Bindable<float?>`（可空 float），
// 按 f64 读 `+0x20` 既读不到 OD，也无法区分 `DrainRate` 与 `OverallDifficulty`
// （基类字段顺序里 DrainRate 在前）——所以 DA 自定义 OD 目前不可靠。
//
// 权威来源是 `ScoreInfo.ModsJson`：`Player.load` 开局时执行
// `Score.ScoreInfo.Mods = gameplayMods`，setter 调 `updateModsJson()`，把
// `APIMod[]` 序列化成 JSON；`APIMod(Mod)` 只收录**非默认**设置
// （`if (!bindable.IsDefault)`），所以自定义倍速/OD 一定在里面，默认值一定不在。
// 它名字自描述（`speed_change` / `overall_difficulty`）、与 lazer 版本解耦，
// 且能覆盖任意 ruleset 的 mod——比逐个解 bindable 稳。
//
// 本文件是准备工作，不改变现有读取行为：
// 1. `lazer_mods_from_mods_json`：JSON → `Vec<LazerMod>`（单元测试覆盖）；
//
// 下一步（拿到一次真机日志后）：把 Score/ScoreInfo 的位移固化进偏移表的 `play` 段
// （与 `results` 段同形），让 `read_lazer_mods` 优先读 `ModsJson`，`SelectedMods`
// 的启发式降为兜底。

use std::sync::Mutex;

use crate::osu::model::LazerMod;
use crate::osu::offsets::OffsetTable;

use super::screen::read_eetype;
use super::source::{read_ptr_field, read_string, Source, StringLayout};

/// `APIMod[]` 的 JSON 形状：
/// `[{"acronym":"DT","settings":{"speed_change":1.2}}, {"acronym":"DA","settings":{"overall_difficulty":8.5}}]`
///
/// 判据（任一不过 ⇒ `None`）：顶层是数组；每项是对象且 `acronym` 是非空字符串。
/// `settings` 缺失/不是对象都按"没有非默认设置"处理；未知键忽略。
/// 空数组是合法的"无 mod"（lazer 自身会写空串而不是 `[]`，这里只做容错）。
pub fn lazer_mods_from_mods_json(text: &str) -> Option<Vec<LazerMod>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let array = value.as_array()?;
    let mut mods = Vec::with_capacity(array.len());
    for item in array {
        let object = item.as_object()?;
        let acronym = object.get("acronym")?.as_str()?.trim();
        if acronym.is_empty() {
            return None;
        }
        let settings = object.get("settings").and_then(|value| value.as_object());
        let number = |key: &str| {
            settings
                .and_then(|settings| settings.get(key))
                .and_then(|value| value.as_f64())
                .filter(|value| value.is_finite())
        };
        let integer = |key: &str| {
            settings
                .and_then(|settings| settings.get(key))
                .and_then(|value| value.as_i64())
                .filter(|value| *value >= i32::MIN as i64 && *value <= i32::MAX as i64)
                .map(|value| value as i32)
        };
        let boolean = |key: &str| settings.and_then(|settings| settings.get(key)).and_then(|value| value.as_bool());
        mods.push(LazerMod {
            acronym: acronym.to_uppercase(),
            // 速率必须为正；0/负数不是有效倍速。
            speed_change: number("speed_change").filter(|value| *value > 0.0),
            adjust_pitch: boolean("adjust_pitch"),
            drain_rate: number("drain_rate"),
            // OD 0 合法。
            overall_difficulty: number("overall_difficulty"),
            seed: integer("seed"),
            extended_limits: boolean("extended_limits"),
        });
    }
    Some(mods)
}

// 屏幕对象上找 `<Score>k__BackingField`；Score 上找 `ScoreInfo`；ScoreInfo 上找
// `ModsJson`。三个字段的精确位移都依赖 lazer 构建，探针先用"逐个引用字段 + 类型名"
// 找到对象，再由字符串内容确认，不写死任何位移。
const SCAN_START: u64 = 0x8;
const SCREEN_SCAN_END: u64 = 0x800;
const SCORE_SCAN_END: u64 = 0x40;
const SCOREINFO_SCAN_END: u64 = 0x400;

static LAST_MODS_JSON: Mutex<Option<(u64, Option<Vec<LazerMod>>)>> = Mutex::new(None);

/// 正式读取当前 Player 的 `ScoreInfo.ModsJson`（含 RD seed）。同一屏幕缓存一次。
///
/// a miss is kept too: `ModsJson` is set once in `Player.load`, and with no mods it's `[]`,
/// which is never taken, so each tick would scan the whole screen again
pub fn read_play_mods(
    source: &dyn Source,
    table: &OffsetTable,
    screen_top: u64,
    layout: Option<StringLayout>,
    modules: &[(u64, String)],
) -> Option<Vec<LazerMod>> {
    if let Ok(cache) = LAST_MODS_JSON.lock() {
        if let Some((cached_screen, mods)) = cache.as_ref() {
            if *cached_screen == screen_top {
                return mods.clone();
            }
        }
    }
    let mods = read_play_mods_json(source, table, screen_top, layout, modules);
    if let Ok(mut cache) = LAST_MODS_JSON.lock() {
        *cache = Some((screen_top, mods.clone()));
    }
    mods
}

/// 结算屏直接从 `ResultsScreen.Score` 的 `ScoreInfo.ModsJson` 读 mods。
///
/// 回放播放时 `read_play_mods`（`Player.Score`）可能因分数对象是
/// `LegacyDatabasedScore` 子类而扫不到；结算屏的 `results.score_offset` 是已验证的
/// `ScoreInfo` 位移，可作为兜底。
pub fn read_results_mods(
    source: &dyn Source,
    table: &OffsetTable,
    screen_top: u64,
    layout: Option<StringLayout>,
) -> Option<Vec<LazerMod>> {
    let section = table.results()?;
    let layout = layout?;
    let score_info = read_ptr_field(source, screen_top, section.score_offset)?;
    let json = find_mods_json(source, layout, score_info, SCOREINFO_SCAN_END)?;
    lazer_mods_from_mods_json(&json)
}

fn read_play_mods_json(
    source: &dyn Source,
    table: &OffsetTable,
    screen_top: u64,
    layout: Option<StringLayout>,
    modules: &[(u64, String)],
) -> Option<Vec<LazerMod>> {
    let layout = layout?;
    let (_, score) = find_object_of_type(
        source,
        table,
        screen_top,
        SCAN_START,
        SCREEN_SCAN_END,
        "osu.Game.Scoring.Score",
        modules,
    )?;
    let (_, score_info) = find_object_of_type(
        source,
        table,
        score,
        SCAN_START,
        SCORE_SCAN_END,
        "osu.Game.Scoring.ScoreInfo",
        modules,
    )?;
    let json = find_mods_json(source, layout, score_info, SCOREINFO_SCAN_END)?;
    lazer_mods_from_mods_json(&json)
}

fn find_object_of_type(
    source: &dyn Source,
    table: &OffsetTable,
    base: u64,
    start: u64,
    end: u64,
    wanted: &str,
    modules: &[(u64, String)],
) -> Option<(u64, u64)> {
    for offset in (start..end).step_by(8) {
        let Some(candidate) = read_ptr_field(source, base, offset as i64) else {
            continue;
        };
        let Ok(info) = read_eetype(source, table, candidate, modules) else {
            continue;
        };
        let type_name = info.type_name.as_deref();
        let matched = match wanted {
            // `Player.Score` 在回放里是 `LegacyDatabasedScore` 这类子类，不能只认基类名。
            "osu.Game.Scoring.Score" => type_name.is_some_and(|name| {
                name == wanted || (name.starts_with("osu.Game.Scoring.") && name.ends_with("Score"))
            }),
            _ => type_name == Some(wanted),
        };
        if matched {
            return Some((offset, candidate));
        }
    }
    None
}

fn find_mods_json(
    source: &dyn Source,
    layout: StringLayout,
    score_info: u64,
    end: u64,
) -> Option<String> {
    for offset in (SCAN_START..end).step_by(8) {
        let Some(pointer) = read_ptr_field(source, score_info, offset as i64) else {
            continue;
        };
        let Some(text) = read_string(source, pointer, layout) else {
            continue;
        };
        if !text.trim_start().starts_with('[') || !text.contains("\"acronym\"") {
            continue;
        }
        if lazer_mods_from_mods_json(&text).is_some() {
            return Some(text);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mods_json_maps_custom_rate_and_od() {
        let mods = lazer_mods_from_mods_json(
            r#"[
                {"acronym":"DT","settings":{"speed_change":1.2,"adjust_pitch":false}},
                {"acronym":"DA","settings":{"drain_rate":5.5,"overall_difficulty":8.5,"extended_limits":true}},
                {"acronym":"MR"}
            ]"#,
        )
        .expect("有效 ModsJson");

        assert_eq!(mods.len(), 3);
        assert_eq!(mods[0].acronym, "DT");
        assert_eq!(mods[0].speed_change, Some(1.2));
        assert_eq!(mods[0].adjust_pitch, Some(false));
        assert_eq!(mods[0].overall_difficulty, None);
        assert_eq!(mods[1].acronym, "DA");
        assert_eq!(mods[1].speed_change, None);
        assert_eq!(mods[1].drain_rate, Some(5.5));
        assert_eq!(mods[1].overall_difficulty, Some(8.5));
        assert_eq!(mods[1].extended_limits, Some(true));
        assert_eq!(mods[2].acronym, "MR");
        assert_eq!(mods[2].speed_change, None);
        assert_eq!(mods[2].overall_difficulty, None);
    }

    #[test]
    fn default_mods_have_no_settings_and_od_zero_is_kept() {
        let mods = lazer_mods_from_mods_json(
            r#"[{"acronym":"DT"},{"acronym":"DA","settings":{"overall_difficulty":0}}]"#,
        )
        .expect("有效 ModsJson");
        assert_eq!(
            mods[0].speed_change, None,
            "默认 1.5x 不应产生 speed_change"
        );
        assert_eq!(mods[0].overall_difficulty, None);
        assert_eq!(mods[1].overall_difficulty, Some(0.0), "OD 0 是合法值");
    }

    #[test]
    fn invalid_settings_do_not_poison_the_rest() {
        let mods = lazer_mods_from_mods_json(
            r#"[
                {"acronym":"DT","settings":{"speed_change":0}},
                {"acronym":"HT","settings":{"speed_change":"0.75"}},
                {"acronym":"DA","settings":[]},
                {"acronym":"MR","settings":{"unknown":1}}
            ]"#,
        )
        .expect("结构仍合法");
        assert_eq!(mods[0].speed_change, None, "0 不是有效倍速");
        assert_eq!(mods[1].speed_change, None, "字符串不是数字");
        assert_eq!(mods[2].overall_difficulty, None, "settings 不是对象");
        assert_eq!(mods[3].acronym, "MR");
    }

    #[test]
    fn empty_array_is_no_mods() {
        assert_eq!(
            lazer_mods_from_mods_json("[]").unwrap(),
            Vec::<LazerMod>::new()
        );
    }

    #[test]
    fn malformed_payloads_are_rejected() {
        for text in [
            "",
            "not json",
            "{}",
            r#"[{"settings":{}}]"#,
            r#"[{"acronym":""}]"#,
            r#"[{"acronym":1}]"#,
            r#"[{"acronym":"DT"},]"#,
        ] {
            assert!(
                lazer_mods_from_mods_json(text).is_none(),
                "{text:?} 不该被当成 ModsJson"
            );
        }
    }
}
