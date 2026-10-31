// Realm's file format 24 (Realm Core 14). lazer may be writing while this reads, so every read is
// bounds checked and a bad one gives None.

use std::collections::HashMap;

pub(crate) struct Realm {
    b: Vec<u8>,
    top: usize,
}

struct Node {
    inner: bool,
    refs: bool,
    big: bool,
    // 0: bits per element, 1: bytes per element, 2: size is in bytes
    wtype: u8,
    width: usize,
    size: usize,
    data: usize,
}

pub(crate) struct Table<'a> {
    realm: &'a Realm,
    top: Vec<i64>,
    columns: Vec<(String, usize)>,
}

impl Realm {
    pub fn open(b: Vec<u8>) -> Option<Self> {
        let header = b.get(16..24)?;
        if &header[..4] != b"T-DB" {
            return None;
        }
        let slot = usize::from(header[7] & 1);
        let mut top = u64::from_le_bytes(b.get(slot * 8..slot * 8 + 8)?.try_into().ok()?);
        // a file written in one go keeps its top ref in a footer
        if top == u64::MAX {
            let at = b.len().checked_sub(16)?;
            top = u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?);
        }
        Some(Self { top: usize::try_from(top).ok()?, b })
    }

    fn node(&self, at: usize) -> Option<Node> {
        let h = self.b.get(at..at.checked_add(8)?)?;
        let f = h[4];
        let n = Node {
            inner: f & 0x80 != 0,
            refs: f & 0x40 != 0,
            big: f & 0x20 != 0,
            wtype: (f & 0x18) >> 3,
            width: (1usize << (f & 7)) >> 1,
            size: (usize::from(h[5]) << 16) | (usize::from(h[6]) << 8) | usize::from(h[7]),
            data: at + 8,
        };
        let bytes = match n.wtype {
            0 => (n.size * n.width).div_ceil(8),
            1 => n.size * n.width,
            _ => n.size,
        };
        self.b.get(n.data..n.data + bytes)?;
        Some(n)
    }

    fn ints_of(&self, n: &Node, signed: bool) -> Option<Vec<i64>> {
        // node() only checked the bytes its width type says, not what reading it as ints needs
        let d = self.b.get(n.data..n.data + (n.size * n.width).div_ceil(8))?;
        let ints = (0..n.size)
            .map(|i| match n.width {
                0 => 0,
                1 | 2 | 4 => {
                    let bit = i * n.width;
                    i64::from((d[bit / 8] >> (bit % 8)) & ((1u8 << n.width) - 1))
                }
                w => {
                    let bytes = &d[i * w / 8..(i + 1) * w / 8];
                    let mut v = [0u8; 8];
                    v[..bytes.len()].copy_from_slice(bytes);
                    if signed && bytes[bytes.len() - 1] & 0x80 != 0 {
                        v[bytes.len()..].fill(0xff);
                    }
                    i64::from_le_bytes(v)
                }
            })
            .collect();
        Some(ints)
    }

    fn ints(&self, at: i64) -> Option<Vec<i64>> {
        self.ints_of(&self.node(usize::try_from(at).ok()?)?, true)
    }

    fn unsigned(&self, at: i64) -> Option<Vec<i64>> {
        self.ints_of(&self.node(usize::try_from(at).ok()?)?, false)
    }

    fn bytes(&self, at: i64) -> Option<&[u8]> {
        let n = self.node(usize::try_from(at).ok()?)?;
        (n.wtype == 2).then(|| &self.b[n.data..n.data + n.size])
    }

    fn strings(&self, at: i64) -> Option<Vec<Option<String>>> {
        let n = self.node(usize::try_from(at).ok()?)?;
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        if !n.refs {
            // short strings: fixed width, zero padded, the last byte says how much padding
            if n.wtype != 1 {
                return None;
            }
            let w = n.width;
            return Some(
                (0..n.size)
                    .map(|i| {
                        let e = self.b.get(n.data + i * w..n.data + (i + 1) * w)?;
                        // width 0: every value is null or empty
                        let len = w.checked_sub(1)?.checked_sub(usize::from(*e.last()?))?;
                        Some(text(e.get(..len)?))
                    })
                    .collect(),
            );
        }
        let refs = self.ints_of(&n, true)?;
        if !n.big {
            let (ends, blob) = (self.ints(*refs.first()?)?, self.bytes(*refs.get(1)?)?);
            let nulls = match refs.get(2) {
                Some(&r) if r != 0 => self.ints(r)?,
                _ => vec![],
            };
            return ends
                .iter()
                .enumerate()
                .map(|(i, &end)| {
                    if nulls.get(i).is_some_and(|&v| v != 0) {
                        return Some(None);
                    }
                    let start = if i == 0 { 0 } else { ends[i - 1] };
                    let s = blob.get(usize::try_from(start).ok()?..usize::try_from(end.checked_sub(1)?).ok()?)?;
                    Some(Some(text(s)))
                })
                .collect();
        }
        refs.iter()
            .map(|&r| match r {
                0 => Some(None),
                r => self.bytes(r).map(|b| Some(text(&b[..b.len().saturating_sub(1)]))),
            })
            .collect()
    }

    // 16 byte values in blocks of eight, each block led by a byte of null flags
    fn uuids(&self, at: i64) -> Option<Vec<Option<uuid::Uuid>>> {
        let n = self.node(usize::try_from(at).ok()?)?;
        let d = self.b.get(n.data..n.data + n.size)?;
        let count = (n.size - n.size.div_ceil(129)) / 16;
        (0..count)
            .map(|i| {
                let block = (i / 8) * 129;
                if d.get(block)? & (1 << (i % 8)) != 0 {
                    return Some(None);
                }
                let at = block + 1 + (i % 8) * 16;
                Some(Some(uuid::Uuid::from_bytes(d.get(at..at + 16)?.try_into().ok()?)))
            })
            .collect()
    }

    pub fn table(&self, name: &str) -> Option<Table<'_>> {
        let top = self.ints(i64::try_from(self.top).ok()?)?;
        let names = self.strings(*top.first()?)?;
        let refs = self.ints(*top.get(1)?)?;
        let at = names.iter().position(|n| n.as_deref() == Some(name))?;
        let top = self.ints(*refs.get(at)?)?;
        let spec = self.ints(*top.first()?)?;
        let names = self.strings(*spec.get(1)?)?;
        let keys = self.ints(*spec.get(5)?)?;
        let columns = names.into_iter().zip(keys).map(|(n, k)| (n.unwrap_or_default(), (k & 0xffff) as usize)).collect();
        Some(Table { realm: self, top, columns })
    }
}

#[derive(Clone)]
pub(crate) enum Cell {
    Str(Option<String>),
    Uuid(Option<uuid::Uuid>),
    Link(Option<i64>),
    Links(Vec<Option<i64>>),
}

pub(crate) enum Kind {
    Str,
    Uuid,
    Link,
    Links,
}

impl Table<'_> {
    pub fn rows(&self, columns: &[(&str, Kind)]) -> Option<Vec<(i64, Vec<Cell>)>> {
        let at: Vec<usize> = columns.iter().map(|(name, _)| self.columns.iter().find(|(n, _)| n == name).map(|c| c.1)).collect::<Option<_>>()?;
        let r = self.realm;
        let mut out = vec![];
        self.walk(*self.top.get(2)?, 0, &mut |keys, leaf| {
            let mut cols = vec![];
            for ((_, kind), &i) in columns.iter().zip(&at) {
                let at = *leaf.get(1 + i)?;
                cols.push(match kind {
                    Kind::Str => r.strings(at)?.into_iter().map(Cell::Str).collect::<Vec<_>>(),
                    Kind::Uuid => r.uuids(at)?.into_iter().map(Cell::Uuid).collect(),
                    Kind::Link => r.ints(at)?.into_iter().map(|v| Cell::Link(link(v))).collect(),
                    Kind::Links => r.ints(at)?.into_iter().map(|l| Some(Cell::Links(if l == 0 { vec![] } else { r.key_list(l)? }))).collect::<Option<_>>()?,
                });
            }
            for (row, key) in keys.into_iter().enumerate() {
                out.push((key, cols.iter().map(|c| c.get(row).cloned()).collect::<Option<_>>()?));
            }
            Some(())
        })?;
        Some(out)
    }

    fn walk(&self, at: i64, offset: i64, each: &mut dyn FnMut(Vec<i64>, Vec<i64>) -> Option<()>) -> Option<()> {
        let n = self.realm.node(usize::try_from(at).ok()?)?;
        let a = self.realm.ints_of(&n, true)?;
        let keys = *a.first()?;
        if n.inner {
            let shift = u32::try_from((a.get(1)? >> 1) * 8).ok()?;
            let offsets = if keys & 1 == 0 && keys != 0 { Some(self.realm.unsigned(keys)?) } else { None };
            for (i, &child) in a.get(3..)?.iter().enumerate() {
                let o = match &offsets {
                    Some(o) => *o.get(i)?,
                    None => (i as i64).checked_shl(shift)?,
                };
                self.walk(child, offset.checked_add(o)?, each)?;
            }
            return Some(());
        }
        // a node holds at most 2^24 values
        let keys = if keys & 1 == 1 { (0..(keys >> 1).min(1 << 24)).collect() } else { self.realm.unsigned(keys)? };
        each(keys.into_iter().map(|k| k.checked_add(offset)).collect::<Option<_>>()?, a)
    }
}

// links are stored one up, so 0 can mean none; negative ones point at an object that's gone
fn link(v: i64) -> Option<i64> {
    (v > 0).then(|| v - 1)
}

// a list's links are the keys themselves, not one up like a single link; negative ones point at
// an object that's gone
fn listed(v: i64) -> Option<i64> {
    (v >= 0).then_some(v)
}

impl Realm {
    fn key_list(&self, at: i64) -> Option<Vec<Option<i64>>> {
        let n = self.node(usize::try_from(at).ok()?)?;
        let a = self.ints_of(&n, true)?;
        if !n.inner {
            return Some(a.into_iter().map(listed).collect());
        }
        // an inner node: offsets, its children, then the tree's size
        let mut out = vec![];
        for &child in a.get(1..a.len().checked_sub(1)?)? {
            out.extend(self.key_list(child)?);
        }
        Some(out)
    }
}

pub(crate) struct LazerSkin {
    pub name: String,
    // (path in the skin, hash of the file in lazer's files folder)
    pub files: Vec<(String, String)>,
}

pub(crate) fn lazer_skin(realm: &Realm, id: uuid::Uuid) -> Option<LazerSkin> {
    let skins = realm.table("class_Skin")?.rows(&[("ID", Kind::Uuid), ("Name", Kind::Str), ("Files", Kind::Links)])?;
    let (name, uses) = skins.into_iter().find_map(|(_, row)| match &row[..] {
        [Cell::Uuid(Some(i)), Cell::Str(name), Cell::Links(uses)] if *i == id => Some((name.clone().unwrap_or_default(), uses.clone())),
        _ => None,
    })?;
    let usages: HashMap<i64, Vec<Cell>> = realm.table("class_RealmNamedFileUsage")?.rows(&[("File", Kind::Link), ("Filename", Kind::Str)])?.into_iter().collect();
    let hashes: HashMap<i64, Vec<Cell>> = realm.table("class_File")?.rows(&[("Hash", Kind::Str)])?.into_iter().collect();
    let files = uses
        .into_iter()
        .flatten()
        .filter_map(|k| {
            let [Cell::Link(Some(file)), Cell::Str(Some(name))] = &usages.get(&k)?[..] else { return None };
            let [Cell::Str(Some(hash))] = &hashes.get(file)?[..] else { return None };
            Some((name.clone(), hash.clone()))
        })
        .collect();
    Some(LazerSkin { name, files })
}

pub(crate) fn skin_names(realm: &Realm) -> Option<Vec<String>> {
    let rows = realm.table("class_Skin")?.rows(&[("Name", Kind::Str)])?;
    Some(rows.into_iter().filter_map(|(_, row)| match row.into_iter().next()? {
        Cell::Str(name) => name,
        _ => None,
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::Realm;

    fn node(flags: u8, size: u32, data: &[u8]) -> Realm {
        let mut b = b"AAAA".to_vec();
        b.push(flags);
        b.extend_from_slice(&size.to_be_bytes()[1..]);
        b.extend_from_slice(data);
        Realm { b, top: 0 }
    }

    #[test]
    fn bad_reads_give_none() {
        let mut short = vec![0u8; 20];
        short[16..20].copy_from_slice(b"T-DB");
        assert!(Realm::open(short).is_none());
        // short strings of width 0 are all null
        assert_eq!(node(0x08, 3, &[]).strings(0), Some(vec![None, None, None]));
        // sized in bytes but read as 64 bit ints
        assert_eq!(node(0x17, 4, &[1, 2, 3, 4]).ints(0), None);
        assert_eq!(node(0x00, 200, &[]).uuids(0), None);
    }
}
