//! The title's gameplay vault (AttribSys: `skaterschema` + `skatercollections`, `.vlt` + `.bin`),
//! read the way `tools/asset_pipeline/vlt.py` reads it, and laid out in guest memory for code run
//! through [`crate::ppc`].
//!
//! The data the title's code reads (collection layouts, by-hash field data, strings, arrays) is the
//! vault's own bytes: both files are copied into guest memory and their pointer table (`PtrN`) is
//! applied with guest addresses. What the loader would build around them (the database, class and
//! collection objects and their hash tables) is answered by the hooks in [`crate::game`], which
//! hand out small objects carrying the fields the original code reads.

use std::collections::HashMap;

use crate::{Error, Guest, Result};

const PTRN: u32 = 0x5074_724E;
const EXPN: u32 = 0x4578_704E;
const ENDC: u32 = 0x456E_6443;
const CLASS_EXPORT: u64 = 0x2A78_95AC_4A87_6152;
const COLLECTION_EXPORT: u64 = 0xAD30_3B8F_42B3_307E;

/// A schema field: `{type, offset, size, maximum, flags, alignment}` (flags bit 0 = array,
/// bit 1 = stored in the layout).
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub offset: u16,
    pub size: u16,
    pub flags: u8,
    pub alignment: u8,
}

pub struct Class {
    /// Hash-table slots: iteration visits keys in slot order (home = low 32 bits mod reserve,
    /// linear probing, vault order), as `sub_82B6DED0` / `sub_82B6DF30` do.
    pub reserve: u32,
    pub layout_bytes: u32,
    pub fields: HashMap<u64, Field>,
    /// Collection keys in iteration order.
    pub order: Vec<u64>,
}

/// Where a collection's own (non-layout) field data sits.
#[derive(Clone, Copy, Debug)]
pub enum Data {
    /// The 4-byte value slot of the entry in the `.vlt`, at this offset.
    Inline(u32),
    /// At this offset in the `.bin`.
    Bin(u32),
}

pub struct Collection {
    pub class: u64,
    pub parent: u64,
    /// Offset of the layout block in the `.bin`, if it has one.
    pub layout: Option<u32>,
    pub entries: HashMap<u64, Data>,
    /// Each entry's 16-byte `.vlt` record (`{u64 field, u32 data, u16 type, u8, u8 flags}`), by
    /// field: the node `sub_82B722C0` finds and every accessor reads.
    pub nodes: HashMap<u64, u32>,
}

pub struct Vault {
    pub classes: HashMap<u64, Class>,
    pub collections: HashMap<(u64, u64), Collection>,
    /// Collection key → class, for parent lookups (a parent is named by key alone).
    pub class_of: HashMap<u64, u64>,
    /// The collections `.vlt` and `.bin` with the pointer table applied as offsets (the `.bin`
    /// offsets `vlt.py` works with).
    vlt: Vec<u8>,
    bin: Vec<u8>,
    /// Where each pointer slot is, for relocating copies: `(in the .vlt?, offset, .bin offset)`.
    pointers: Vec<(bool, u32, Option<u32>)>,
}

fn be16(d: &[u8], at: usize) -> Result<u16> {
    d.get(at..at + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
        .ok_or_else(|| Error::new(at as u32, "vault truncated"))
}
fn be32(d: &[u8], at: usize) -> Result<u32> {
    d.get(at..at + 4)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
        .ok_or_else(|| Error::new(at as u32, "vault truncated"))
}
fn be64(d: &[u8], at: usize) -> Result<u64> {
    d.get(at..at + 8)
        .map(|b| u64::from_be_bytes(b.try_into().unwrap()))
        .ok_or_else(|| Error::new(at as u32, "vault truncated"))
}

/// A vault pair: `(key, kind, at)` of its exports, with its pointer table applied as offsets.
struct Pair {
    vlt: Vec<u8>,
    bin: Vec<u8>,
    exports: Vec<(u64, u64, u32)>,
    pointers: Vec<(bool, u32, Option<u32>)>,
}

fn pair(vlt: &[u8], bin: &[u8]) -> Result<Pair> {
    let (mut v, mut b) = (vlt.to_vec(), bin.to_vec());
    let mut exports = Vec::new();
    let mut pointers = Vec::new();
    let mut at = 0usize;
    while at < v.len() {
        let (tag, size) = (be32(&v, at)?, be32(&v, at + 4)? as usize);
        if size < 8 {
            return Err(Error::new(at as u32, "vault chunk length"));
        }
        let body = at + 8;
        match tag {
            PTRN => {
                let mut in_vlt = false;
                for p in (body..at + size).step_by(16) {
                    let (offset, kind, index) = (be32(&v, p)?, be16(&v, p + 4)?, be16(&v, p + 6)?);
                    let dest = be64(&v, p + 8)? as u32;
                    match kind {
                        0 => break,
                        2 => in_vlt = index == 0,
                        1 | 3 => {
                            let value = if kind == 1 { 0 } else { dest };
                            let target = if in_vlt { &mut v } else { &mut b };
                            target
                                .get_mut(offset as usize..offset as usize + 4)
                                .ok_or_else(|| Error::new(offset, "vault pointer out of range"))?
                                .copy_from_slice(&value.to_be_bytes());
                            pointers.push((in_vlt, offset, (kind == 3).then_some(dest)));
                        }
                        4 => {}
                        _ => return Err(Error::new(p as u32, "unknown vault pointer kind")),
                    }
                }
            }
            EXPN => {
                let count = be64(&v, body)? as usize;
                for i in 0..count {
                    let e = body + 8 + 24 * i;
                    exports.push((be64(&v, e)?, be64(&v, e + 8)?, be32(&v, e + 20)?));
                }
            }
            _ => {}
        }
        at += size;
        if tag == ENDC {
            break;
        }
    }
    Ok(Pair { vlt: v, bin: b, exports, pointers })
}

impl Vault {
    pub fn parse(
        schema_vlt: &[u8],
        schema_bin: &[u8],
        collections_vlt: &[u8],
        collections_bin: &[u8],
    ) -> Result<Self> {
        let schema = pair(schema_vlt, schema_bin)?;
        let mut classes = HashMap::new();
        for &(_, kind, at) in &schema.exports {
            if kind != CLASS_EXPORT {
                continue;
            }
            let at = at as usize;
            let (sv, sb) = (&schema.vlt, &schema.bin);
            let key = be64(sv, at)?;
            let reserve = be32(sv, at + 8)?;
            let count = be32(sv, at + 12)? as usize;
            let defs = be32(sv, at + 16)? as usize;
            let layout_bytes = be32(sv, at + 28)?;
            let mut fields = HashMap::new();
            for i in 0..count {
                let d = defs + 24 * i;
                fields.insert(
                    be64(sb, d)?,
                    Field {
                        offset: be16(sb, d + 16)?,
                        size: be16(sb, d + 18)?,
                        flags: sb[d + 22],
                        alignment: sb[d + 23],
                    },
                );
            }
            classes.insert(key, Class { reserve, layout_bytes, fields, order: Vec::new() });
        }
        let c = pair(collections_vlt, collections_bin)?;
        let mut collections = HashMap::new();
        let mut class_of = HashMap::new();
        let mut vault_order: HashMap<u64, Vec<u64>> = HashMap::new();
        for &(_, kind, at) in &c.exports {
            if kind != COLLECTION_EXPORT {
                continue;
            }
            let at = at as usize;
            let cv = &c.vlt;
            let key = be64(cv, at)?;
            let class = be64(cv, at + 8)?;
            let parent = be64(cv, at + 16)?;
            let count = be32(cv, at + 32)? as usize;
            let types_len = be16(cv, at + 38)? as usize;
            let layout = be32(cv, at + 40)?;
            let schema = classes
                .get(&class)
                .ok_or_else(|| Error::new(at as u32, "collection of an unknown class"))?;
            let mut entries = HashMap::new();
            let mut nodes = HashMap::new();
            let first = at + 48 + 8 * types_len;
            for i in 0..count {
                let e = first + 16 * i;
                let field_key = be64(cv, e)?;
                let field = schema
                    .fields
                    .get(&field_key)
                    .ok_or_else(|| Error::new(e as u32, "entry of an unknown field"))?;
                let data = if field.size <= 4 && field.flags & 1 == 0 {
                    Data::Inline(e as u32 + 8)
                } else {
                    Data::Bin(be32(cv, e + 8)?)
                };
                entries.insert(field_key, data);
                nodes.insert(field_key, e as u32);
            }
            collections.insert(
                (class, key),
                Collection {
                    class,
                    parent,
                    layout: (layout != 0).then_some(layout),
                    entries,
                    nodes,
                },
            );
            class_of.insert(key, class);
            vault_order.entry(class).or_default().push(key);
        }
        for (class, keys) in vault_order {
            let Some(schema) = classes.get_mut(&class) else { continue };
            let mut table = vec![None; (schema.reserve as usize).max(keys.len())];
            for key in keys {
                let mut slot = (key as u32 as usize) % table.len();
                while table[slot].is_some() {
                    slot = (slot + 1) % table.len();
                }
                table[slot] = Some(key);
            }
            schema.order = table.into_iter().flatten().collect();
        }
        Ok(Self {
            classes,
            collections,
            class_of,
            vlt: c.vlt,
            bin: c.bin,
            pointers: c.pointers,
        })
    }

    /// Copy the collections `.vlt` and `.bin` to `vlt_at` / `bin_at` with every pointer slot
    /// rewritten to a guest address (`bin_at + offset`).
    pub fn install(&self, g: &mut Guest, vlt_at: u32, bin_at: u32) -> Result<()> {
        let (mut v, mut b) = (self.vlt.clone(), self.bin.clone());
        for &(in_vlt, offset, dest) in &self.pointers {
            let value = dest.map_or(0, |d| bin_at + d);
            let target = if in_vlt { &mut v } else { &mut b };
            target[offset as usize..offset as usize + 4].copy_from_slice(&value.to_be_bytes());
        }
        g.put(vlt_at, v);
        g.put(bin_at, b);
        Ok(())
    }

    /// Sizes of the two spans [`Self::install`] maps.
    pub fn spans(&self) -> (u32, u32) {
        (self.vlt.len() as u32, self.bin.len() as u32)
    }
}
