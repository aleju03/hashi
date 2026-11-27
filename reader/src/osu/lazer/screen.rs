use crate::osu::model::OBSERVED_STATE_NAMES;
use crate::osu::offsets::{LookupError, OffsetTable};
use super::fields::{screen_state_for, GAME_TYPE_ANY, ONLINE_PLAY_SCREEN, ONLINE_PLAY_SCREENS, SCREEN_STACK_MAX};
use super::resolve::plausible_ptr;
use super::source::{field_addr, read_i32, read_ptr_field, read_u32, read_u64, Source};
use super::types::{ChainAddrs, Resolved};

/// 一次 EEType→类型名 解析的全部中间读数。
#[derive(Clone, Debug, Default)]
pub struct EetypeRead {
    pub object: u64,
    pub method_table: u64,
    pub rid: u32,
    pub module: u64,
    pub image_base: u64,
    pub module_name: Option<String>,
    pub type_name: Option<String>,
}

impl EetypeRead {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "object": format!("0x{:016X}", self.object),
            "method_table": format!("0x{:016X}", self.method_table),
            "rid": self.rid,
            "module": format!("0x{:016X}", self.module),
            "image_base": format!("0x{:016X}", self.image_base),
            "module_name": self.module_name,
            "type_name": self.type_name,
        })
    }
}

pub fn read_eetype(
    source: &dyn Source,
    table: &OffsetTable,
    object: u64,
    modules: &[(u64, String)],
) -> Result<EetypeRead, String> {
    let token = table
        .runtime_entry("eetype", "token")
        .ok_or_else(|| "offsets-missing-runtime:eetype.token".to_string())?;
    if token.shift > 24 {
        return Err("token-domain".to_string());
    }
    let loader_module = table
        .runtime_entry("eetype", "loader_module")
        .ok_or_else(|| "offsets-missing-runtime:eetype.loader_module".to_string())?;
    let image_base_entry = table
        .runtime_entry("module", "image_base")
        .ok_or_else(|| "offsets-missing-runtime:module.image_base".to_string())?;

    let mut read = EetypeRead {
        object,
        ..EetypeRead::default()
    };
    read.method_table = read_u64(source, object).ok_or_else(|| "read-methodtable".to_string())?;
    if !plausible_ptr(read.method_table) {
        return Err("methodtable-implausible".to_string());
    }
    let token_addr = field_addr(read.method_table, token.offset).ok_or_else(|| "token-domain".to_string())?;
    let packed = read_u32(source, token_addr).ok_or_else(|| "read-token".to_string())?;
    read.rid = packed >> token.shift;
    if read.rid == 0 {
        return Err("token-domain".to_string());
    }
    let module_addr =
        field_addr(read.method_table, loader_module.offset).ok_or_else(|| "module-implausible".to_string())?;
    read.module = read_u64(source, module_addr).ok_or_else(|| "read-loader-module".to_string())?;
    if !plausible_ptr(read.module) {
        return Err("module-implausible".to_string());
    }
    let image_addr =
        field_addr(read.module, image_base_entry.offset).ok_or_else(|| "image-base-implausible".to_string())?;
    read.image_base = read_u64(source, image_addr).ok_or_else(|| "read-image-base".to_string())?;
    let known = modules.iter().find(|(base, _)| *base == read.image_base);
    let (base, module_name) = match known {
        Some((base, name)) => (*base, name.clone()),
        None => found_image_base(source, read.module, modules).ok_or_else(|| format!("module-unresolved:0x{:016X}", read.image_base))?,
    };
    read.image_base = base;
    let type_name = match table.runtime_type_name(&module_name, read.rid) {
        Some(name) => name.to_string(),
        None => read_pe_typedef_name(source, read.image_base, read.rid)
            .ok_or_else(|| format!("type-unresolved:{module_name}#{:X}", read.rid))?,
    };
    read.module_name = Some(module_name);
    read.type_name = Some(type_name);
    Ok(read)
}

// where a Module keeps its image's base: (offset in the Module, offset in what that points to, if
// it's one pointer further). the tables' offset is windows coreclr's, linux's lays the Module out
// differently, so there it's looked for once: the field holding a loaded module's base
static IMAGE_BASE_AT: std::sync::Mutex<Option<(u64, Option<u64>)>> = std::sync::Mutex::new(None);
pub(crate) const MODULE_SPAN: usize = 0x400;
const INNER_SPAN: usize = 0x100;

pub(crate) fn forget_image_base() {
    *IMAGE_BASE_AT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(crate) fn found_image_base(source: &dyn Source, module: u64, modules: &[(u64, String)]) -> Option<(u64, String)> {
    let known = |value: u64| modules.iter().find(|(base, _)| *base == value).cloned();
    let at = |addr: u64, offset: u64| read_u64(source, addr.checked_add(offset)?);
    let cached = *IMAGE_BASE_AT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((outer, inner)) = cached {
        let value = at(module, outer)?;
        let value = match inner {
            Some(inner) => at(value, inner)?,
            None => value,
        };
        return known(value);
    }
    let words = |addr: u64, span: usize| -> Vec<u64> {
        source.read(addr, span).map(|b| b.as_chunks::<8>().0.iter().map(|c| u64::from_le_bytes(*c)).collect()).unwrap_or_default()
    };
    let outer = words(module, MODULE_SPAN);
    let remember = |found: (u64, Option<u64>)| *IMAGE_BASE_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(found);
    for (i, value) in outer.iter().enumerate() {
        if let Some(hit) = known(*value) {
            remember(((i * 8) as u64, None));
            return Some(hit);
        }
    }
    for (i, pointer) in outer.iter().enumerate().filter(|(_, p)| plausible_ptr(**p)) {
        for (j, value) in words(*pointer, INNER_SPAN).iter().enumerate() {
            if let Some(hit) = known(*value) {
                remember(((i * 8) as u64, Some((j * 8) as u64)));
                return Some(hit);
            }
        }
    }
    None
}

// a type's name from its module's metadata, for types the table doesn't name
pub fn read_pe_typedef_name(source: &dyn Source, image_base: u64, rid: u32) -> Option<String> {
    super::layout::metadata_at(source, image_base)?.type_name(rid)
}

/// state.{name,number} 的读数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenState {
    pub number: i32,
    pub name: String,
    pub type_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenOutcome {
    Mapped(ScreenState),
    Unmapped(String),
    Unresolved(String),
}

pub fn read_screen_state(
    source: &dyn Source,
    table: &OffsetTable,
    game_base: u64,
    resolved: &Resolved,
    chain: &mut ChainAddrs,
    modules: &[(u64, String)],
) -> ScreenOutcome {
    let unresolved = |reason: String| ScreenOutcome::Unresolved(reason);
    let lookup = |offset: &Result<i64, LookupError>| -> Result<i64, String> {
        offset.clone().map_err(|error| format!("offsets-{error}"))
    };
    let stack_offset = match lookup(&resolved.screen_stack) {
        Ok(value) => value,
        Err(reason) => return unresolved(reason),
    };
    let list_offset = match lookup(&resolved.screen_stack_list) {
        Ok(value) => value,
        Err(reason) => return unresolved(reason),
    };
    let array_offset = match lookup(&resolved.screen_stack_array) {
        Ok(value) => value,
        Err(reason) => return unresolved(reason),
    };
    let size_offset = match lookup(&resolved.screen_stack_size) {
        Ok(value) => value,
        Err(reason) => return unresolved(reason),
    };

    chain.screen_stack = read_ptr_field(source, game_base, stack_offset);
    let Some(screen_stack) = chain.screen_stack else {
        return unresolved("read-<ScreenStack>".to_string());
    };
    chain.screen_stack_list = read_ptr_field(source, screen_stack, list_offset);
    let Some(stack) = chain.screen_stack_list else {
        return unresolved("read-OsuScreenStack.stack".to_string());
    };
    chain.screen_stack_array = read_ptr_field(source, stack, array_offset);
    let Some(array) = chain.screen_stack_array else {
        return unresolved("read-Stack._array".to_string());
    };
    let array_entry = match table.runtime_entry("screen_array", "elements") {
        Some(entry) => entry,
        None => return unresolved("offsets-missing-runtime:screen_array.elements".to_string()),
    };
    let Some(size_addr) = field_addr(stack, size_offset) else {
        return unresolved("read-Stack._size".to_string());
    };
    let Some(size) = read_i32(source, size_addr) else {
        return unresolved("read-Stack._size".to_string());
    };
    if size <= 0 || size > SCREEN_STACK_MAX {
        return unresolved(format!("screen-stack-empty:{size}"));
    }
    let index = (size - 1) as i64;
    let stride = array_entry.stride.max(8);
    let Some(element_addr) = field_addr(array, array_entry.offset + index.saturating_mul(stride)) else {
        return unresolved("element-address".to_string());
    };
    let Some(element) = read_u64(source, element_addr) else {
        return unresolved("read-screen-element".to_string());
    };
    if !plausible_ptr(element) {
        return unresolved("screen-element-implausible".to_string());
    }
    chain.screen_top = Some(element);
    let read = match read_eetype(source, table, element, modules) {
        Ok(read) => read,
        Err(reason) => {
            chain.screen_top_vtable = read_u64(source, element);
            return unresolved(reason);
        }
    };
    chain.screen_top_vtable = Some(read.method_table);
    chain.screen_top_module = Some(read.module);
    chain.screen_top_image_base = Some(read.image_base);
    let mut type_name = read.type_name.clone().unwrap_or_default();
    if ONLINE_PLAY_SCREENS.contains(&type_name.as_str()) {
        let inner = (|| {
            let offset = table.offset(ONLINE_PLAY_SCREEN, "screenStack").filter(|o| *o > 0)?;
            let sub = read_ptr_field(source, element, offset)?;
            let list = read_ptr_field(source, sub, list_offset)?;
            let array = read_ptr_field(source, list, array_offset)?;
            let size = read_i32(source, field_addr(list, size_offset)?)?;
            if size <= 0 || size > SCREEN_STACK_MAX {
                return None;
            }
            let top = read_u64(source, field_addr(array, array_entry.offset + (size as i64 - 1).saturating_mul(stride))?)?;
            plausible_ptr(top).then_some(top)
        })();
        if let Some(top) = inner {
            if let Ok(read) = read_eetype(source, table, top, modules) {
                chain.screen_top = Some(top);
                type_name = read.type_name.unwrap_or_default();
            }
        }
    }
    let mapped_state = table
        .screen_state_for(&type_name)
        .or_else(|| screen_state_for(&type_name));
    match mapped_state {
        Some(state) => match OBSERVED_STATE_NAMES
            .iter()
            .find(|(_, name)| *name == state)
            .map(|(number, _)| *number)
        {
            Some(number) => ScreenOutcome::Mapped(ScreenState {
                number,
                name: state.to_string(),
                type_name,
            }),
            None => unresolved(format!("state-not-in-observed-set:{state}")),
        },
        None => ScreenOutcome::Unmapped(type_name),
    }
}

pub fn runtime_probe(
    source: &dyn Source,
    table: &OffsetTable,
    game_base: u64,
    modules: &[(u64, String)],
) -> Result<String, String> {
    let read = read_eetype(source, table, game_base, modules)?;
    let name = read.type_name.clone().unwrap_or_default();
    if !GAME_TYPE_ANY.iter().any(|needle| name.contains(needle)) {
        return Err(format!(
            "`[gameBase]` (EEType 0x{:016X}) resolves to `{name}` in {}# {:X} — expected one of {:?}",
            read.method_table, read.module_name.as_deref().unwrap_or("<unknown>"), read.rid, GAME_TYPE_ANY
        ));
    }
    Ok(format!(
        "[gameBase] EEType 0x{:016X} -> {}# {:X} -> `{name}` (image base 0x{:016X})",
        read.method_table,
        read.module_name.as_deref().unwrap_or("<unknown>"),
        read.rid,
        read.image_base
    ))
}
