// where lazer keeps each field, read from the running game instead of a table made for one lazer
// version. .NET keeps every loaded type's field list next to the type (FieldDescs, each with the
// field's metadata row and its offset) and the field names in the assembly's metadata, so the same
// numbers a table generator pulls out of a build are here for whatever build is running
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use super::resolve::plausible_ptr;
use super::screen::found_image_base;
use super::source::{read_i32, read_u32, read_u64, Source};
use crate::osu::offsets::{ModsSection, OffsetTable, ResultsSection, ScoreSection};

// coreclr's MethodTable and EEClass, 64-bit, the same on windows and linux since .NET 9
const MT_BASE_SIZE: u64 = 0x04;
const MT_TOKEN: u64 = 0x08;
const MT_PARENT: u64 = 0x10;
const MT_MODULE: u64 = 0x18;
const MT_CLASS: u64 = 0x28;
const CLASS_METHOD_TABLE: u64 = 0x10;
const CLASS_FIELDS: u64 = 0x18;
const FIELD_DESC: u64 = 16;
const FIELD_STATIC: u32 = 1 << 24;
// a boxed value type's size counts the object header and the MethodTable pointer
const BOX_OVERHEAD: i64 = 16;
const OBJECT_HEADER: i64 = 8;
const MAX_FIELDS: u64 = 4096;
const MAX_PARENTS: usize = 64;
const MODULE_SPAN: usize = super::screen::MODULE_SPAN;
const MAX_DICT_ENTRIES: i32 = 64;
// osu.Game's is about 3 MB
const MAX_METADATA: usize = 64 << 20;

const FIELD_LITERAL: u16 = 0x40;

struct Field {
    name: String,
    offset: i64,
}

pub struct Layout<'a> {
    source: &'a dyn Source,
    modules: &'a [(u64, String)],
    metadata: HashMap<u64, Option<Arc<Metadata>>>,
    fields: HashMap<u64, Option<Arc<Vec<Field>>>>,
    // where a Module keeps its row -> MethodTable list for the types it defines
    typedef_map_at: Option<u64>,
}

impl<'a> Layout<'a> {
    pub fn new(source: &'a dyn Source, modules: &'a [(u64, String)]) -> Self {
        Layout { source, modules, metadata: HashMap::new(), fields: HashMap::new(), typedef_map_at: None }
    }

    fn u64(&self, addr: u64) -> Option<u64> {
        read_u64(self.source, addr)
    }

    // the MethodTable holding the type's EEClass, which generic instantiations over reference types share
    fn canonical(&self, mt: u64) -> Option<(u64, u64)> {
        if !plausible_ptr(mt) {
            return None;
        }
        let union = self.u64(mt + MT_CLASS)?;
        let canonical = if union & 1 == 1 { union & !1 } else { mt };
        let class = self.u64(canonical + MT_CLASS)?;
        (plausible_ptr(class) && class & 1 == 0 && self.u64(class + CLASS_METHOD_TABLE)? == canonical).then_some((canonical, class))
    }

    fn row(&self, mt: u64) -> Option<u32> {
        Some(read_u32(self.source, mt + MT_TOKEN)? >> 8).filter(|row| *row != 0)
    }

    fn module_of(&self, mt: u64) -> Option<u64> {
        self.u64(mt + MT_MODULE).filter(|m| plausible_ptr(*m))
    }

    fn parent(&self, mt: u64) -> Option<u64> {
        self.u64(mt + MT_PARENT).filter(|p| plausible_ptr(*p))
    }

    fn metadata(&mut self, module: u64) -> Option<Arc<Metadata>> {
        if let Some(known) = self.metadata.get(&module) {
            return known.clone();
        }
        let found = found_image_base(self.source, module, self.modules).and_then(|(base, _)| metadata_at(self.source, base));
        self.metadata.insert(module, found.clone());
        found
    }

    fn type_name(&mut self, mt: u64) -> Option<String> {
        let (canonical, _) = self.canonical(mt)?;
        let metadata = self.metadata(self.module_of(canonical)?)?;
        metadata.type_name(self.row(canonical)?)
    }

    // every instance field, the base type's first, offsets from the object's start
    fn fields(&mut self, mt: u64) -> Option<Arc<Vec<Field>>> {
        if !plausible_ptr(mt) {
            return None;
        }
        if let Some(known) = self.fields.get(&mt) {
            return known.clone();
        }
        let found = self.read_fields(mt).map(Arc::new);
        self.fields.insert(mt, found.clone());
        found
    }

    fn read_fields(&mut self, mt: u64) -> Option<Vec<Field>> {
        let mut chain = vec![mt];
        while let Some(parent) = self.parent(*chain.last()?) {
            if chain.len() >= MAX_PARENTS {
                return None;
            }
            chain.push(parent);
        }
        let mut out = Vec::new();
        for mt in chain.into_iter().rev() {
            let (canonical, class) = self.canonical(mt)?;
            let row = self.row(canonical)?;
            let metadata = self.metadata(self.module_of(canonical)?)?;
            let (first, end) = metadata.field_rows(row)?;
            // literals (consts) get no FieldDesc
            let count = (first..end).filter(|r| metadata.field(*r).is_some_and(|(flags, _)| flags & FIELD_LITERAL == 0)).count() as u64;
            if count == 0 {
                continue;
            }
            if count > MAX_FIELDS {
                return None;
            }
            let list = self.u64(class + CLASS_FIELDS).filter(|p| plausible_ptr(*p))?;
            let bytes = self.source.read(list, (count * FIELD_DESC) as usize)?;
            for desc in bytes.as_chunks::<{ FIELD_DESC as usize }>().0.iter() {
                let owner = u64::from_le_bytes(desc[0..8].try_into().ok()?);
                let word = u32::from_le_bytes(desc[8..12].try_into().ok()?);
                let place = u32::from_le_bytes(desc[12..16].try_into().ok()?);
                let field_row = word & 0xFF_FFFF;
                if owner != canonical || !(first..end).contains(&field_row) {
                    return None;
                }
                if word & FIELD_STATIC != 0 {
                    continue;
                }
                let (_, name) = metadata.field(field_row)?;
                out.push(Field { name: name.to_string(), offset: i64::from(place & 0x7FF_FFFF) + OBJECT_HEADER });
            }
        }
        Some(out)
    }

    // the most derived field of that name
    fn offset(&mut self, mt: u64, name: &str) -> Option<i64> {
        self.fields(mt)?.iter().rev().find(|f| f.name == name).map(|f| f.offset)
    }

    pub fn object_offset(&mut self, object: u64, name: &str) -> Option<i64> {
        let mt = self.u64(object)?;
        self.offset(mt, name)
    }

    fn follow(&mut self, object: u64, name: &str) -> Option<u64> {
        let offset = self.object_offset(object, name)?;
        self.u64(object.checked_add(offset as u64)?).filter(|p| plausible_ptr(*p))
    }

    // a value type's size inline, in an array or another object
    fn value_size(&self, mt: u64) -> Option<i64> {
        Some(i64::from(read_u32(self.source, mt + MT_BASE_SIZE)?) - BOX_OVERHEAD).filter(|s| *s > 0)
    }

    // the MethodTable of a type the module defines, if it's loaded
    fn find_type(&mut self, module: u64, full_name: &str) -> Option<u64> {
        let metadata = self.metadata(module)?;
        let row = metadata.find_type(full_name)?;
        let map = self.u64(module + self.typedef_map_at?)?;
        self.u64(map + 8 * u64::from(row)).map(|mt| mt & !7).filter(|mt| plausible_ptr(*mt))
    }

    // found once from a type whose MethodTable is known: the Module word pointing at a list holding it at its row
    fn learn_typedef_map(&mut self, mt: u64) -> bool {
        if self.typedef_map_at.is_some() {
            return true;
        }
        let (Some(module), Some(row)) = (self.module_of(mt), self.row(mt)) else { return false };
        let Some(words) = self.source.read(module, MODULE_SPAN) else { return false };
        for (i, word) in words.as_chunks::<8>().0.iter().enumerate() {
            let pointer = u64::from_le_bytes(*word);
            if plausible_ptr(pointer) && self.u64(pointer + 8 * u64::from(row)).is_some_and(|v| v & !7 == mt) {
                self.typedef_map_at = Some((i * 8) as u64);
                return true;
            }
        }
        false
    }
}

// read once per module while attached; forgotten on the next attach, the game may have restarted
static METADATA: Mutex<Option<HashMap<u64, Option<Arc<Metadata>>>>> = Mutex::new(None);

pub fn metadata_at(source: &dyn Source, base: u64) -> Option<Arc<Metadata>> {
    let mut cache = METADATA.lock().unwrap_or_else(|e| e.into_inner());
    cache.get_or_insert_with(HashMap::new).entry(base).or_insert_with(|| Metadata::read(source, base).map(Arc::new)).clone()
}

// also where Modules keep their image's base, learned on one game and maybe not another's
pub fn forget() {
    *METADATA.lock().unwrap_or_else(|e| e.into_inner()) = None;
    super::screen::forget_image_base();
}

// the parts of an assembly's metadata needed here: type and field names, which fields a type declares
pub struct Metadata {
    data: Vec<u8>,
    strings: usize,
    string_index: usize,
    typedef_at: usize,
    typedef_row: usize,
    typedefs: u32,
    typedef_fields: usize,
    field_index: usize,
    field_at: usize,
    field_row: usize,
    fields: u32,
    by_name: OnceLock<HashMap<String, Option<u32>>>,
}

impl Metadata {
    fn read(source: &dyn Source, base: u64) -> Option<Metadata> {
        let head = source.read(base, 0x400)?;
        let u16_at = |b: &[u8], at: usize| Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?));
        let u32_at = |b: &[u8], at: usize| Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?));
        let pe = u32_at(&head, 0x3C)? as usize;
        if u32_at(&head, pe)? != 0x4550 {
            return None;
        }
        let sections = u16_at(&head, pe + 6)? as usize;
        let optional = u16_at(&head, pe + 20)? as usize;
        let directories = match u16_at(&head, pe + 24)? {
            0x20B => 112,
            0x10B => 96,
            _ => return None,
        };
        let cli_rva = u32_at(&head, pe + 24 + directories + 14 * 8)?;
        let table = pe + 24 + optional;
        let spans: Vec<(u32, u32, u32)> = (0..sections)
            .filter_map(|i| {
                let at = table + i * 40;
                Some((u32_at(&head, at + 12)?, u32_at(&head, at + 8)?.max(u32_at(&head, at + 16)?), u32_at(&head, at + 20)?))
            })
            .collect();
        // mapped as the loader lays it out, or as the file is
        let places = |rva: u32| {
            let mut out = vec![base + u64::from(rva)];
            out.extend(spans.iter().filter(|(va, size, _)| (*va..va + size).contains(&rva)).map(|(va, _, raw)| base + u64::from(rva - va + raw)));
            out
        };
        let data = places(cli_rva).into_iter().find_map(|cli| {
            let header = source.read(cli, 16)?;
            let (rva, size) = (u32_at(&header, 8)?, u32_at(&header, 12)? as usize);
            places(rva).into_iter().find_map(|at| read_large(source, at, size).filter(|d| d.starts_with(b"BSJB")))
        })?;
        Metadata::parse(data)
    }

    fn parse(data: Vec<u8>) -> Option<Metadata> {
        let u16_at = |at: usize| Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?));
        let u32_at = |at: usize| Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?));
        let version = u32_at(12)? as usize;
        let header = 16 + ((version + 3) & !3);
        let count = u16_at(header + 2)?;
        let mut at = header + 4;
        let (mut tables, mut strings) = (None, None);
        for _ in 0..count {
            let offset = u32_at(at)? as usize;
            let end = data.get(at + 8..)?.iter().position(|b| *b == 0)? + at + 8;
            match data.get(at + 8..end)? {
                b"#~" => tables = Some(offset),
                b"#Strings" => strings = Some(offset),
                _ => {}
            }
            at = (end + 4) & !3;
        }
        let (tables, strings) = (tables?, strings?);
        let heaps = *data.get(tables + 6)?;
        let valid = u64::from_le_bytes(data.get(tables + 8..tables + 16)?.try_into().ok()?);
        let mut rows = [0u32; 64];
        let mut at = tables + 24;
        for (table, count) in rows.iter_mut().enumerate() {
            if valid >> table & 1 == 1 {
                *count = u32_at(at)?;
                at += 4;
            }
        }
        let wide = |tables: &[usize], tag_bits: u32| if tables.iter().map(|t| rows[*t]).max().unwrap_or(0) < (1 << (16 - tag_bits)) { 2 } else { 4 };
        let string_index = if heaps & 1 == 1 { 4 } else { 2 };
        let guid_index = if heaps & 2 == 2 { 4 } else { 2 };
        let blob_index = if heaps & 4 == 4 { 4 } else { 2 };
        let field_index = wide(&[4], 0);
        let typedef_fields = 4 + 2 * string_index + wide(&[2, 1, 0x1B], 2);
        let sizes = [
            2 + string_index + 3 * guid_index,
            wide(&[0, 0x1A, 0x23, 1], 2) + 2 * string_index,
            typedef_fields + field_index + wide(&[6], 0),
            field_index,
            2 + string_index + blob_index,
        ];
        let mut starts = [0usize; 5];
        for (table, size) in sizes.iter().enumerate() {
            starts[table] = at;
            at += rows[table] as usize * size;
        }
        // field rows reached through FieldPtr only in unoptimized metadata, which release builds don't have
        if rows[3] != 0 || at > data.len() {
            return None;
        }
        Some(Metadata {
            strings,
            string_index,
            typedef_at: starts[2],
            typedef_row: sizes[2],
            typedefs: rows[2],
            typedef_fields,
            field_index,
            field_at: starts[4],
            field_row: sizes[4],
            fields: rows[4],
            data,
            by_name: OnceLock::new(),
        })
    }

    fn index(&self, at: usize, size: usize) -> Option<u32> {
        let bytes = self.data.get(at..at + size)?;
        Some(if size == 4 { u32::from_le_bytes(bytes.try_into().ok()?) } else { u32::from(u16::from_le_bytes(bytes.try_into().ok()?)) })
    }

    fn string(&self, index: u32) -> Option<&str> {
        let start = self.strings + index as usize;
        let end = start + self.data.get(start..)?.iter().position(|b| *b == 0)?;
        std::str::from_utf8(&self.data[start..end]).ok()
    }

    fn typedef(&self, row: u32) -> Option<usize> {
        (1..=self.typedefs).contains(&row).then(|| self.typedef_at + (row as usize - 1) * self.typedef_row)
    }

    pub fn type_name(&self, row: u32) -> Option<String> {
        let at = self.typedef(row)?;
        let name = self.string(self.index(at + 4, self.string_index)?)?;
        let namespace = self.string(self.index(at + 4 + self.string_index, self.string_index)?)?;
        Some(if namespace.is_empty() { name.to_string() } else { format!("{namespace}.{name}") })
    }

    fn field_rows(&self, row: u32) -> Option<(u32, u32)> {
        let first = self.index(self.typedef(row)? + self.typedef_fields, self.field_index)?;
        let end = match self.typedef(row + 1) {
            Some(next) => self.index(next + self.typedef_fields, self.field_index)?,
            None => self.fields + 1,
        };
        (first <= end && end <= self.fields + 1).then_some((first, end))
    }

    fn field(&self, row: u32) -> Option<(u16, &str)> {
        if !(1..=self.fields).contains(&row) {
            return None;
        }
        let at = self.field_at + (row as usize - 1) * self.field_row;
        let flags = self.index(at, 2)? as u16;
        Some((flags, self.string(self.index(at + 2, self.string_index)?)?))
    }

    // "Namespace.Name", or "Outer+Inner" for a nested type (matched by its own name, which has to be unique)
    pub fn find_type(&self, full_name: &str) -> Option<u32> {
        let by_name = self.by_name.get_or_init(|| {
            let mut map: HashMap<String, Option<u32>> = HashMap::new();
            for row in 1..=self.typedefs {
                let Some(at) = self.typedef(row) else { continue };
                let nested = self.index(at, 4).is_some_and(|flags| flags & 0x7 >= 2);
                let Some(name) = self.type_name(row) else { continue };
                let key = if nested { format!("+{name}") } else { name };
                map.entry(key).and_modify(|seen| *seen = None).or_insert(Some(row));
            }
            map
        });
        let key = match full_name.rsplit_once('+') {
            Some((_, inner)) => format!("+{inner}"),
            None => full_name.to_string(),
        };
        by_name.get(&key).copied().flatten()
    }
}

// one read can only be so big
fn read_large(source: &dyn Source, addr: u64, len: usize) -> Option<Vec<u8>> {
    const PIECE: usize = 1 << 16;
    if len > MAX_METADATA {
        return None;
    }
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let size = PIECE.min(len - out.len());
        out.extend(source.read(addr + out.len() as u64, size)?);
    }
    Some(out)
}

// a type name as tables write it, without its generic arguments
fn open_name(key: &str) -> &str {
    key.split('<').next().unwrap_or(key)
}

pub struct LiveTable {
    pub table: OffsetTable,
    pub missing: Vec<String>,
}

struct Lookup<'a, 'b> {
    layout: &'b mut Layout<'a>,
    search: Vec<u64>,
    missing: Vec<String>,
}

impl Lookup<'_, '_> {
    fn find(&mut self, name: &str) -> Option<u64> {
        let found = self.search.iter().find_map(|module| self.layout.find_type(*module, open_name(name)));
        if found.is_none() {
            self.miss(format!("type {name}"));
        }
        found
    }

    fn field(&mut self, type_name: &str, name: &str) -> Option<i64> {
        let mt = self.find(type_name)?;
        let offset = self.layout.offset(mt, name);
        if offset.is_none() {
            self.miss(format!("field {type_name}.{name}"));
        }
        offset
    }

    fn miss(&mut self, what: String) {
        if !self.missing.contains(&what) {
            self.missing.push(what);
        }
    }

    // a value type's field as it sits inline
    fn inline(&mut self, type_name: &str, name: &str) -> Option<i64> {
        Some(self.field(type_name, name)? - OBJECT_HEADER)
    }
}

// the table the rest of the reader uses, filled from the game: the template (the bundled table)
// gives which fields to look for and the parts that aren't field offsets
pub fn live_table(
    source: &dyn Source,
    modules: &[(u64, String)],
    game_base: u64,
    template: &OffsetTable,
    target: &super::table::TargetInfo,
) -> Result<LiveTable, String> {
    let mut layout = Layout::new(source, modules);
    let game = read_u64(source, game_base).filter(|mt| plausible_ptr(*mt)).ok_or("game object unreadable")?;
    if !layout.learn_typedef_map(game) {
        return Err("no type list in the game's module".into());
    }
    layout.fields(game).ok_or("the game object's fields can't be read")?;

    // the modules to look types up in: those of the game's own types, the collections', the rulesets'
    let mut search: Vec<u64> = Vec::new();
    let mut mt = game;
    for _ in 0..MAX_PARENTS {
        search.extend(layout.module_of(mt));
        match layout.parent(mt) {
            Some(parent) => mt = parent,
            None => break,
        }
    }
    let stack = layout.follow(game_base, "<ScreenStack>k__BackingField").and_then(|s| layout.follow(s, "stack"));
    search.extend(stack.and_then(|s| read_u64(source, s)).and_then(|mt| layout.module_of(mt)));
    search.extend(ruleset_modules(&mut layout, game_base));
    let mut seen = Vec::new();
    search.retain(|m| if seen.contains(m) { false } else { seen.push(*m); true });
    let mut l = Lookup { layout: &mut layout, search, missing: Vec::new() };

    let mut types: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
    for (key, fields) in &template.types {
        let Some(mt) = l.find(key) else { continue };
        let mut found = BTreeMap::new();
        for name in fields.keys() {
            match l.layout.offset(mt, name) {
                Some(offset) => {
                    found.insert(name.clone(), offset);
                }
                None => l.miss(format!("field {key}.{name}")),
            }
        }
        types.insert(key.clone(), found);
    }

    const PLAYER: &str = "osu.Game.Screens.Play.Player";
    const PROCESSOR: &str = "osu.Game.Rulesets.Scoring.ScoreProcessor";
    const HIT_EVENT: &str = "osu.Game.Rulesets.Scoring.HitEvent";
    const SCORE: &str = "osu.Game.Scoring.Score";
    const SCORE_INFO: &str = "osu.Game.Scoring.ScoreInfo";
    const LIST: &str = "System.Collections.Generic.List`1";
    const RESULTS: &str = "osu.Game.Screens.Ranking.ResultsScreen";
    let score = (|| {
        let hit_event = l.find(HIT_EVENT)?;
        Some(ScoreSection {
            player_score_processor: l.field(PLAYER, "<ScoreProcessor>k__BackingField")?,
            hit_events: l.field(PROCESSOR, "hitEvents")?,
            hit_object: l.inline(HIT_EVENT, "HitObject")?,
            time_offset: l.inline(HIT_EVENT, "TimeOffset")?,
            result: l.inline(HIT_EVENT, "Result")?,
            // a HitObjectProperty<int>: its bindable, then the value
            column: l.field("osu.Game.Rulesets.Mania.Objects.ManiaHitObject", "column")?
                + l.inline("osu.Game.Rulesets.Objects.HitObjectProperty`1", "backingValue")?,
            stride: l.layout.value_size(hit_event)?,
            player_score: l.field(PLAYER, "<Score>k__BackingField")?,
            score_replay: l.field(SCORE, "Replay")?,
            replay_frames: l.field("osu.Game.Replays.Replay", "Frames")?,
            replay_time: l.field("osu.Game.Rulesets.Replays.ReplayFrame", "Time")?,
            replay_actions: l.field("osu.Game.Rulesets.Mania.Replays.ManiaReplayFrame", "Actions")?,
            actions_items: l.field(LIST, "_items")?,
            actions_size: l.field(LIST, "_size")?,
            score_result_counts: l.field(PROCESSOR, "ScoreResultCounts"),
            score_info: l.field(SCORE, "ScoreInfo"),
            score_hash: l.field(SCORE_INFO, "<Hash>k__BackingField"),
            pause_overlay: l.field(PLAYER, "<PauseOverlay>k__BackingField"),
            drawable_alpha: l.field("osu.Framework.Graphics.Drawable", "alpha"),
            fail_overlay: l.field(PLAYER, "<FailOverlay>k__BackingField"),
            health_processor: l.field(PLAYER, "<HealthProcessor>k__BackingField"),
            health: l.field("osu.Game.Rulesets.Scoring.HealthProcessor", "Health"),
            bindable_double_value: l.field("osu.Framework.Bindables.BindableDouble", "value"),
            has_failed: l.field("osu.Game.Rulesets.Scoring.HealthProcessor", "<HasFailed>k__BackingField"),
            witness: "read from the running game".into(),
        })
    })();

    let results = (|| {
        Some(ResultsSection {
            score_offset: l.field(RESULTS, "Score")?,
            statistics_offset: l.field(SCORE_INFO, "<StatisticsJson>k__BackingField"),
            player_offset: l.field(RESULTS, "<player>k__BackingField"),
            statistics_dict_offset: l.field(SCORE_INFO, "statistics"),
            dict_entries_offset: l.field("System.Collections.Generic.Dictionary`2", "_entries"),
            score_hash: l.field(SCORE_INFO, "<Hash>k__BackingField"),
            // HitResult's values aren't field offsets, and its members haven't moved in years
            hit_result_values: template.results.as_ref().map(|r| r.hit_result_values.clone()).unwrap_or_default(),
            witness: "read from the running game".into(),
        })
    })();

    let speed = &[("speed_change", "<SpeedChange>k__BackingField")][..];
    let speed_pitch = &[("speed_change", "<SpeedChange>k__BackingField"), ("adjust_pitch", "<AdjustPitch>k__BackingField")][..];
    let adjust = &[("drain_rate", "<DrainRate>k__BackingField"), ("extended_limits", "<ExtendedLimits>k__BackingField"), ("overall_difficulty", "<OverallDifficulty>k__BackingField")][..];
    let mods = (|| {
        let mut settings = BTreeMap::new();
        for (acronym, type_name, pairs) in [
            ("DT", "osu.Game.Rulesets.Mods.ModDoubleTime", speed_pitch),
            ("NC", "osu.Game.Rulesets.Mods.ModNightcore", speed),
            ("HT", "osu.Game.Rulesets.Mods.ModHalfTime", speed_pitch),
            ("DC", "osu.Game.Rulesets.Mods.ModDaycore", speed),
            ("DA", "osu.Game.Rulesets.Mania.Mods.ManiaModDifficultyAdjust", adjust),
        ] {
            let mut found = BTreeMap::new();
            for (key, name) in pairs {
                found.insert(key.to_string(), l.field(type_name, name)?);
            }
            settings.insert(acronym.to_string(), found);
        }
        // the settings are BindableDouble, BindableFloat and BindableBool, read with one offset
        let value = l.field("osu.Framework.Bindables.BindableDouble", "value")?;
        for other in ["osu.Framework.Bindables.BindableFloat", "osu.Framework.Bindables.BindableBool"] {
            if l.field(other, "value")? != value {
                l.miss(format!("{other}.value differs"));
                return None;
            }
        }
        Some(ModsSection {
            bindable_value: value,
            difficulty_current: l.field("osu.Game.Rulesets.Mods.DifficultyBindable", "CurrentNumber")?,
            settings,
        })
    })();

    let mut missing = l.missing;
    for (section, present) in [("score", score.is_some()), ("results", results.is_some()), ("mods", mods.is_some())] {
        if !present {
            missing.push(format!("section {section}"));
        }
    }
    // the bundled table's type names by row belong to its own lazer version
    let runtime = template.runtime.clone().map(|mut runtime| {
        runtime.typedefs.clear();
        runtime.observed.clear();
        runtime
    });
    let (lazer_version, runtime_version, arch) = (&target.lazer_version, &target.runtime_version, &target.arch);
    Ok(LiveTable {
        table: OffsetTable {
            lazer_version: lazer_version.to_string(),
            runtime_version: runtime_version.to_string(),
            arch: arch.to_string(),
            game_base_vtable: Some(game),
            anchors: template.anchors.clone(),
            screen_states: template.screen_states.clone(),
            types,
            runtime,
            results,
            mods,
            score,
            verified_build: format!("osu!lazer {lazer_version} / .NET {runtime_version} / {arch} / read from the running game"),
            evidence: "field offsets from the game's own type information".into(),
        },
        missing,
    })
}

// the rulesets' assemblies, through RulesetStore.LoadedAssemblies: a Dictionary<Assembly, Type>
// whose Types (RuntimeType) hold their MethodTable
fn ruleset_modules(layout: &mut Layout, game_base: u64) -> Vec<u64> {
    let mut out = Vec::new();
    let source = layout.source;
    let Some(store) = layout.follow(game_base, "<RulesetStore>k__BackingField") else { return out };
    let Some(dict) = layout.follow(store, "LoadedAssemblies") else { return out };
    let (Some(entries), Some(count_at)) = (layout.follow(dict, "_entries"), layout.object_offset(dict, "_count")) else { return out };
    let Some(count) = read_i32(source, dict + count_at as u64).filter(|c| (0..=MAX_DICT_ENTRIES).contains(c)) else { return out };
    // an array's MethodTable keeps its element size in the low half of its flags
    let Some(size) = read_u64(source, entries).and_then(|mt| read_u32(source, mt)).map(|f| u64::from(f & 0xFFFF)).filter(|s| *s >= 16 && s % 8 == 0) else {
        return out;
    };
    for i in 0..count as u64 {
        let Some(entry) = source.read(entries + 16 + i * size, size as usize) else { continue };
        for word in entry.as_chunks::<8>().0.iter() {
            let object = u64::from_le_bytes(*word);
            if !plausible_ptr(object) {
                continue;
            }
            let Some(mt) = read_u64(source, object) else { continue };
            if layout.type_name(mt).as_deref() != Some("System.RuntimeType") {
                continue;
            }
            let handle = layout.follow(object, "m_handle");
            if let Some(module) = handle.and_then(|h| layout.module_of(h)) {
                out.push(module);
            }
        }
    }
    out
}
