// lazer & stable 偏移表模块门面
//
// 纯数据 + 加载 + 结构校验，由子模块分别承担：
// - lookup: 字段与类型查找、canonical_type
// - lazer_table: Lazer 偏移表结构与就近回落
// - stable_table: Stable 锚点与拓扑定义及本地查找
// - validation: 纯数据 Schema 安全与合理性校验器

pub mod lookup;
pub mod lazer_table;
pub mod stable_table;
pub mod validation;

pub use lookup::*;
pub use lazer_table::*;
pub use stable_table::*;
pub use validation::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tables_are_valid() {
        let stable = default_stable_table();
        assert!(validate_stable_schema(&stable).is_ok());

        let ppysb = default_ppysb_table();
        assert_eq!(ppysb.client, "ppysb");
        assert_eq!(ppysb.arch, "x86");
        assert!(validate_stable_schema(&ppysb).is_ok());

        let lazer = default_lazer_table();
        assert!(validate_lazer_schema(&lazer).is_ok());
        let mods = lazer.mods().expect("default table carries the mods section");
        assert_eq!(
            mods.settings_for("DA")
                .and_then(|fields| fields.get("overall_difficulty")),
            Some(&0x28)
        );
        assert_eq!(
            mods.settings_for("DT")
                .and_then(|fields| fields.get("speed_change")),
            Some(&0x10)
        );
    }

    #[test]
    fn on_disk_stable_table_loads_and_validates() {
        let path = std::path::Path::new("offsets/stable/stable__x86.json");
        if path.exists() {
            let bytes = std::fs::read(path).expect("read stable__x86.json");
            let table = StableTable::load(&bytes).expect("load stable__x86.json");
            assert_eq!(table.client, "stable");
            assert_eq!(table.arch, "x86");
            assert!(table.anchors.contains_key("statusPtr"));
            assert!(table.anchors.contains_key("baseAddr"));
            assert_eq!(table.state_name(2), Some("play"));
            assert_eq!(table.state_name(5), Some("selectPlay"));
            assert_eq!(table.state_name(7), Some("resultScreen"));
        }
    }

    #[test]
    fn stable_schema_rejects_script_injection() {
        let mut table = default_stable_table();
        table.client = "<script>alert(1)</script>".to_string();
        assert!(matches!(
            validate_stable_schema(&table),
            Err(ValidationError::ExecutableContent(_))
        ));

        let mut table2 = default_stable_table();
        table2.evidence = "normal text with javascript:evil() inside".to_string();
        assert!(matches!(
            validate_stable_schema(&table2),
            Err(ValidationError::ExecutableContent(_))
        ));

        let mut table3 = default_stable_table();
        table3.version = "eval(foo)".to_string();
        assert!(matches!(
            validate_stable_schema(&table3),
            Err(ValidationError::ExecutableContent(_))
        ));
    }

    #[test]
    fn stable_schema_rejects_out_of_range_displacement() {
        let mut table = default_stable_table();
        table.topology.beatmap_from_base = 2_000_000;
        assert!(matches!(
            validate_stable_schema(&table),
            Err(ValidationError::DisplacementOutOfRange(..))
        ));

        let mut table2 = default_stable_table();
        table2.topology.hits_candidate_offsets[0] = 5_000_000;
        assert!(matches!(
            validate_stable_schema(&table2),
            Err(ValidationError::DisplacementOutOfRange(..))
        ));
    }

    #[test]
    fn stable_schema_rejects_unaligned_hits_offset() {
        let mut table = default_stable_table();
        table.topology.hits_candidate_offsets[0] = 0x89; // odd offset
        assert!(matches!(
            validate_stable_schema(&table),
            Err(ValidationError::InvalidAlignment(..))
        ));
    }

    #[test]
    fn stable_schema_rejects_hop_depth_exceeded() {
        let mut table = default_stable_table();
        for i in 0..20 {
            table.anchors.insert(
                format!("extra_anchor_{i}"),
                StableAnchorDef {
                    pattern: "90 90".to_string(),
                    offset: 0,
                    derivation: String::new(),
                    evidence: String::new(),
                },
            );
        }
        assert!(matches!(
            validate_stable_schema(&table),
            Err(ValidationError::HopDepthExceeded(_))
        ));
    }

    #[test]
    fn stable_schema_rejects_unknown_state_names() {
        let mut table = default_stable_table();
        table
            .mappings
            .states
            .insert("99".to_string(), "malicious_state".to_string());
        assert!(matches!(
            validate_stable_schema(&table),
            Err(ValidationError::UnknownStateName(_))
        ));
    }

    #[test]
    fn lazer_schema_rejects_out_of_range_displacement() {
        let mut table = default_lazer_table();
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("evil_field".to_string(), 100_000_000);
        table.types.insert("Some.Type".to_string(), fields);
        assert!(matches!(
            validate_lazer_schema(&table),
            Err(ValidationError::DisplacementOutOfRange(..))
        ));
    }

    #[test]
    fn find_stable_table_returns_valid_table() {
        let table = find_stable_table(None).expect("must find table or compile-time fallback");
        assert_eq!(table.client, "stable");
        assert!(validate_stable_schema(&table).is_ok());

        let ppysb = find_stable_table_variant(None, "ppysb").expect("must find ppysb table");
        assert_eq!(ppysb.client, "ppysb");
        assert_eq!(ppysb.arch, "x86");
        assert!(validate_stable_schema(&ppysb).is_ok());
    }
}
