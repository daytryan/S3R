//! Custom Graphics: the designs a player saved from the Graphic Creator, beside the skater profile.
//! A saved Custom Graphic is the whole item (every side's design), kept per customizable item
//! model (`MAX_SAVES` each, since a design is laid out on that model's own rectangles) in
//! `custom-graphics.json`; its preview tile is a PNG in `custom-graphics/<item id>/<hash16>.png`.
//! Decoding is strict and an entry that fails it is skipped but kept: it stays in the file, in
//! place, and is never written over or counted.
use crate::custom_graphic::{self as graphic, Doc, ZONES};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Saves per item model.
pub(crate) const MAX_SAVES: usize = 30;
/// Bumped when the tile's look or framing changes, so old tiles are drawn again. 2: no capture while a pipeline
/// compiles (graphic_tile SETTLE) and the CAS waves' grip / coverage-map fixes; tiles drawn before could miss a piece.
/// Not retail (user 10-19) 3: a top's tile faces the side its design is on (`graphic_tile::shown_zone`).
pub(crate) const TILE_VERSION: u32 = 3;

/// One saved design for an item: each side's version 3 design text, by zone.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Saved {
    pub sides: BTreeMap<String, String>,
}
impl Saved {
    /// Strict: only known zones with designs that decode, and at least one.
    pub(crate) fn parse(v: &Value) -> Option<Self> {
        let sides = v["sides"].as_object().filter(|s| !s.is_empty() && v.as_object().is_some_and(|o| o.len() == 1))?;
        let sides = sides
            .iter()
            .map(|(zone, text)| {
                let text = text.as_str().filter(|t| Doc::decode(t).is_ok())?;
                ZONES.iter().any(|z| z.0 == zone).then(|| (zone.clone(), text.to_owned()))
            })
            .collect::<Option<_>>()?;
        Some(Self { sides })
    }
    pub(crate) fn json(&self) -> Value {
        json!({ "sides": self.sides })
    }
    /// The tile's file name stem: the sides and the tile version, 16 hex digits.
    pub fn hash(&self) -> String {
        let mut hash = blake3::Hasher::new();
        hash.update(&TILE_VERSION.to_le_bytes());
        for (zone, text) in &self.sides {
            hash.update(zone.as_bytes());
            hash.update(&[0]);
            hash.update(text.as_bytes());
            hash.update(&[0]);
        }
        hash.finalize().to_hex()[..16].to_owned()
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Valid(Saved),
    /// Failed to decode: kept exactly as found.
    Kept(Value),
}

/// A list is full: the caller asks which save to replace.
#[derive(Debug, PartialEq)]
pub(crate) struct Full;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Store {
    items: BTreeMap<String, Vec<Entry>>,
    /// Top-level values that are not a list of entries, kept as found.
    others: Map<String, Value>,
}

pub(crate) fn file(profile: &Path) -> PathBuf {
    profile.with_file_name("custom-graphics.json")
}
/// Where a save's preview tile is drawn.
pub(crate) fn tile_file(profile: &Path, item: &str, saved: &Saved) -> PathBuf {
    profile.with_file_name("custom-graphics").join(item).join(format!("{}.png", saved.hash()))
}

impl Store {
    pub fn load(profile: &Path) -> Self {
        // Not a JSON object: kept aside as .bad-<unix> and restored from its last good copy (saves::load); unreadable:
        // empty and not saved over.
        let read = crate::saves::load(&file(profile), |b| match serde_json::from_slice(b) {
            Ok(Value::Object(top)) => Some(top),
            _ => None,
        });
        let Some(top) = read.ok() else { return Self::default() };
        let mut store = Self::default();
        for (item, list) in top {
            match list {
                Value::Array(list) => {
                    let entries = list.into_iter().map(|v| Saved::parse(&v).map_or(Entry::Kept(v), Entry::Valid)).collect();
                    store.items.insert(item, entries);
                }
                other => {
                    store.others.insert(item, other);
                }
            }
        }
        store
    }
    pub fn save(&self, profile: &Path) -> std::io::Result<()> {
        let path = file(profile);
        let mut top = self.others.clone();
        for (item, entries) in &self.items {
            let list = entries.iter().map(|e| match e {
                Entry::Valid(saved) => saved.json(),
                Entry::Kept(v) => v.clone(),
            });
            top.insert(item.clone(), Value::Array(list.collect()));
        }
        crate::saves::write_atomic(&path, &serde_json::to_vec_pretty(&Value::Object(top))?)?;
        crate::autosave_tag::saving();
        Ok(())
    }
    /// The item's saves that decoded, in order.
    pub fn saves(&self, item: &str) -> Vec<&Saved> {
        self.items.get(item).into_iter().flatten().filter_map(|e| if let Entry::Valid(s) = e { Some(s) } else { None }).collect()
    }
    /// Every item's id and its saves that decoded (New's From Another Item).
    pub fn items(&self) -> impl Iterator<Item = (&str, Vec<&Saved>)> {
        self.items.keys().map(|item| (item.as_str(), self.saves(item)))
    }
    /// Entries kept because they did not decode.
    #[cfg(test)]
    pub fn kept(&self, item: &str) -> usize {
        self.items.get(item).map_or(0, |l| l.iter().filter(|e| matches!(e, Entry::Kept(_))).count())
    }
    /// Save as new. The same design twice is one save. Returns its number among `saves`.
    pub fn add(&mut self, item: &str, saved: Saved) -> Result<usize, Full> {
        if let Some(at) = self.saves(item).iter().position(|s| **s == saved) {
            return Ok(at);
        }
        let count = self.saves(item).len();
        if count >= MAX_SAVES {
            return Err(Full);
        }
        self.items.entry(item.to_owned()).or_default().push(Entry::Valid(saved));
        Ok(count)
    }
    /// Overwrite save number `n` (of `saves`) in place.
    pub fn replace(&mut self, item: &str, n: usize, saved: Saved) -> bool {
        self.slot(item, n).map(|e| *e = Entry::Valid(saved)).is_some()
    }
    /// Delete save number `n` (of `saves`), giving it back.
    pub fn remove(&mut self, item: &str, n: usize) -> Option<Saved> {
        let list = self.items.get_mut(item)?;
        let at = list.iter().enumerate().filter(|(_, e)| matches!(e, Entry::Valid(_))).nth(n)?.0;
        match list.remove(at) {
            Entry::Valid(saved) => Some(saved),
            Entry::Kept(_) => None,
        }
    }
    fn slot(&mut self, item: &str, n: usize) -> Option<&mut Entry> {
        self.items.get_mut(item)?.iter_mut().filter(|e| matches!(e, Entry::Valid(_))).nth(n)
    }
}

/// Fold My Graphics (8 single designs) into the lists of the tees `targets` (item, front rectangle,
/// female), each design as a front design drawn on that tee's front rectangle: the old file never said
/// which body made them, so every tee gets them and whichever gender is played finds them. Then set
/// the old file aside as `my-graphics.json.migrated` (the thumbnails folder stays). Returns how many
/// were added in all. When a list fills, the rest wait.
pub(crate) fn migrate_my_graphics(profile: &Path, targets: &[(&str, [f32; 4], bool)]) -> usize {
    let old = graphic::slots_file(profile);
    if !old.exists() {
        return 0;
    }
    let slots = graphic::load_slots(profile);
    let mut store = Store::load(profile);
    let (mut moved, mut left) = (0, 0);
    for text in slots.into_iter().flatten() {
        for &(item, rect, female) in targets {
            let design = match Doc::decode(&text) {
                Ok(_) => text.clone(),
                Err(_) => match Doc::decode_old(&text) {
                    Ok(doc) => graphic::upgrade(&doc, graphic::entry("front"), female, graphic::Adjust::default(), rect).encode(),
                    Err(_) => continue,
                },
            };
            match store.add(item, Saved { sides: BTreeMap::from([("front".to_owned(), design)]) }) {
                Ok(_) => moved += 1,
                Err(Full) => left += 1,
            }
        }
    }
    if moved > 0 && store.save(profile).is_err() {
        return 0;
    }
    if left == 0 {
        let _ = std::fs::rename(&old, old.with_extension("json.migrated"));
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_graphic::{Art, Layer};
    fn design(n: u8) -> String {
        Doc { layers: vec![Layer { x: n as i16 * 10, ..Layer::new(Art::Shape(n % 15), [200, 10, 10, 255]) }], ..Default::default() }.encode()
    }
    fn saved(n: u8) -> Saved {
        Saved { sides: BTreeMap::from([("front".to_owned(), design(n)), ("back".to_owned(), design(n + 1))]) }
    }
    fn profile(name: &str) -> PathBuf {
        // ponytail: one folder per test, wiped and reused (a pid suffix left one per run: ~2,000).
        // Two test runs at once would share it.
        let dir = std::env::temp_dir().join(format!("skate-graphic-store-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("profile.json")
    }
    const TEE: &str = "801095192c838106";

    #[test]
    fn saves_load_edit_overwrite_save_as_new_and_delete() {
        let path = profile("round");
        let mut store = Store::load(&path);
        assert!(store.saves(TEE).is_empty());
        assert_eq!(store.add(TEE, saved(1)), Ok(0));
        assert_eq!(store.add(TEE, saved(3)), Ok(1));
        assert_eq!(store.add(TEE, saved(1)), Ok(0), "the same design is one save");
        assert_eq!(store.add("c620cb8d10a788b4", saved(5)), Ok(0), "each item model has its own list");
        store.save(&path).unwrap();
        let mut store = Store::load(&path);
        assert_eq!(store.saves(TEE), [&saved(1), &saved(3)]);
        // Edit and save again overwrites that save in place; save as new appends.
        assert!(store.replace(TEE, 0, saved(7)));
        assert_eq!(store.add(TEE, saved(9)), Ok(2));
        assert_eq!(store.saves(TEE), [&saved(7), &saved(3), &saved(9)]);
        store.save(&path).unwrap();
        let mut store = Store::load(&path);
        assert_eq!(store.saves(TEE).len(), 3);
        assert_eq!(store.remove(TEE, 1), Some(saved(3)));
        assert_eq!(store.saves(TEE), [&saved(7), &saved(9)]);
        assert_eq!(store.remove(TEE, 5), None);
        assert!(!store.replace(TEE, 5, saved(2)));
        store.save(&path).unwrap();
        assert_eq!(Store::load(&path).saves(TEE), [&saved(7), &saved(9)]);
        assert_eq!(Store::load(&path).saves("c620cb8d10a788b4"), [&saved(5)]);
    }

    #[test]
    fn thirty_saves_per_item_then_the_caller_picks_one_to_replace() {
        let mut store = Store::default();
        for n in 0..MAX_SAVES as u8 {
            assert!(store.add(TEE, Saved { sides: BTreeMap::from([("front".to_owned(), design(n + 20))]) }).is_ok());
        }
        assert_eq!(store.saves(TEE).len(), MAX_SAVES);
        assert_eq!(store.add(TEE, saved(1)), Err(Full), "a full list asks which save to replace");
        assert!(store.add("c620cb8d10a788b4", saved(1)).is_ok(), "another item is not full");
        assert!(store.replace(TEE, 12, saved(1)));
        assert_eq!(store.saves(TEE).len(), MAX_SAVES);
        assert_eq!(store.saves(TEE)[12], &saved(1));
        assert_eq!(store.add(TEE, saved(1)), Ok(12), "and it is there once");
        store.remove(TEE, 0);
        assert!(store.add(TEE, saved(2)).is_ok(), "room again after a delete");
    }

    #[test]
    fn a_corrupt_entry_is_skipped_and_kept() {
        let path = profile("corrupt");
        let (good, other) = (saved(1).json(), saved(2).json());
        let bad: [Value; 6] = [
            json!({"sides": {"front": "not a design"}}),
            json!({"sides": {"collar": design(1)}}),
            json!({"sides": {}}),
            json!({"sides": {"front": design(1)}, "extra": 1}),
            json!("text"),
            json!({"future": [1, 2, 3]}),
        ];
        let list: Vec<Value> = [vec![bad[0].clone(), good.clone()], bad[1..].to_vec(), vec![other.clone()]].concat();
        let file_text = json!({ TEE: list, "note": {"keep": "me"} });
        std::fs::write(file(&path), serde_json::to_vec(&file_text).unwrap()).unwrap();
        let mut store = Store::load(&path);
        assert_eq!(store.saves(TEE), [&saved(1), &saved(2)], "only the valid ones show");
        assert_eq!(store.kept(TEE), 6);
        // Saving more (even deleting a valid one) writes the corrupt entries back, in their places.
        assert_eq!(store.add(TEE, saved(4)), Ok(2));
        store.remove(TEE, 0);
        store.save(&path).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(file(&path)).unwrap()).unwrap();
        let entries = written[TEE].as_array().unwrap();
        assert_eq!(entries.len(), 8);
        for kept in &bad {
            assert!(entries.contains(kept), "{kept} still on disk");
        }
        assert_eq!(&entries[..1], &[bad[0].clone()], "in place");
        assert_eq!(written["note"], json!({"keep": "me"}));
        assert_eq!(Store::load(&path).saves(TEE), [&saved(2), &saved(4)]);
        // A file that is not even JSON is kept aside before it is replaced.
        let broken = profile("broken");
        std::fs::write(file(&broken), b"{ not json").unwrap();
        let mut store = Store::load(&broken);
        assert!(store.saves(TEE).is_empty());
        store.add(TEE, saved(1)).unwrap();
        store.save(&broken).unwrap();
        let aside = broken.parent().unwrap().read_dir().unwrap().flatten().find(|e| e.file_name().to_string_lossy().starts_with("custom-graphics.json.bad-")).unwrap();
        assert_eq!(std::fs::read(aside.path()).unwrap(), b"{ not json");
        assert_eq!(Store::load(&broken).saves(TEE).len(), 1);
    }

    #[test]
    fn my_graphics_move_into_the_custom_tee_as_front_designs() {
        let path = profile("migrate");
        let rect = [0.98, 0.48, 0.13, 0.37];
        // Old (version 2) designs, with a gap in the slots, and one already version 3.
        let old = |n: u8| {
            let d = Doc { layers: vec![Layer::new(Art::Shape(n), [255, 0, 0, 255])], background: [0, 0, 255, 255] };
            let raw = base64_raw(&d.encode());
            let v2 = [&[b'G', 2][..], &raw[2..]].concat();
            base64_text(&v2)
        };
        let slots = vec![Some(old(2)), None, Some(old(4)), Some(design(6)), None, None, None, None];
        graphic::save_slots(&path, &slots).unwrap();
        assert_eq!(migrate_my_graphics(&path, &[(TEE, rect, false)]), 3);
        let store = Store::load(&path);
        let saves = store.saves(TEE);
        assert_eq!(saves.len(), 3);
        for s in &saves {
            assert_eq!(s.sides.keys().collect::<Vec<_>>(), ["front"]);
            assert!(Doc::decode(&s.sides["front"]).is_ok(), "converted to version 3");
        }
        assert_eq!(saves[2].sides["front"], design(6), "a version 3 design is kept as it is");
        assert!(!graphic::slots_file(&path).exists() && graphic::slots_file(&path).with_extension("json.migrated").exists());
        assert_eq!(migrate_my_graphics(&path, &[(TEE, rect, false)]), 0, "once");
        assert_eq!(Store::load(&path).saves(TEE).len(), 3);
    }
    #[test]
    fn my_graphics_land_on_the_tee_of_each_body_drawn_for_that_body() {
        let path = profile("migrate-both");
        let (male, female) = ([0.98, 0.48, 0.13, 0.37], [0.94, 0.48, 0.08, 0.42]);
        let d = Doc { layers: vec![Layer { x: 900, y: 300, ..Layer::new(Art::Shape(4), [255, 0, 0, 255]) }], background: [0; 4] };
        let v2 = base64_text(&[&[b'G', 2][..], &base64_raw(&d.encode())[2..]].concat());
        graphic::save_slots(&path, &[Some(v2), None, None, None, None, None, None, None]).unwrap();
        let targets = [("male-tee", male, false), ("female-tee", female, true)];
        assert_eq!(migrate_my_graphics(&path, &targets), 2, "one design, two tees");
        let store = Store::load(&path);
        let (m, f) = (store.saves("male-tee"), store.saves("female-tee"));
        assert_eq!((m.len(), f.len()), (1, 1));
        let (m, f) = (Doc::decode(&m[0].sides["front"]).unwrap(), Doc::decode(&f[0].sides["front"]).unwrap());
        assert_ne!((m.layers[0].x, m.layers[0].y, m.layers[0].w), (f.layers[0].x, f.layers[0].y, f.layers[0].w), "each is drawn for its own body and rectangle");
        assert!(graphic::slots_file(&path).with_extension("json.migrated").exists());
    }
    fn base64_raw(text: &str) -> Vec<u8> {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD_NO_PAD.decode(text).unwrap()
    }
    fn base64_text(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes)
    }

    #[test]
    fn tiles_are_named_by_what_they_show() {
        let path = PathBuf::from("profile/profile.json");
        let a = tile_file(&path, TEE, &saved(1));
        assert_eq!(a, PathBuf::from("profile/custom-graphics").join(TEE).join(format!("{}.png", saved(1).hash())));
        assert_eq!(saved(1).hash().len(), 16);
        assert_ne!(saved(1).hash(), saved(2).hash());
        assert_eq!(saved(1).hash(), saved(1).clone().hash());
    }
}
