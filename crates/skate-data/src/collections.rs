//! Typed access to converted stock skater XML values, without engine objects.
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::OnceLock,
};

#[derive(Debug, Deserialize)]
pub struct Field {
    #[serde(rename = "type")]
    pub type_name: String,
    pub data: String,
    /// Array attributes (`vlt.py` `array_items`): `data` holds only the header lanes, the
    /// elements are here, one hex string per item.
    #[serde(default)]
    pub array: Option<FieldArray>,
}

#[derive(Debug, Deserialize)]
pub struct FieldArray {
    pub capacity: u32,
    pub element_size: u32,
    pub alignment: u32,
    pub items: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Collection {
    #[serde(rename = "class")]
    pub class_name: String,
    pub key: String,
    pub parent: String,
    pub fields: BTreeMap<String, Field>,
    pub source: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
pub struct Collections {
    version: u32,
    collections: Vec<Collection>,
    /// (class, key) numeric identity -> first collection with it, built on first
    /// lookup. A linear scan hashing every entry made each load take seconds.
    #[serde(skip)]
    index: OnceLock<HashMap<(u64, u64), usize>>,
}

impl Collections {
    pub fn entries(&self) -> &[Collection] {
        &self.collections
    }

    pub fn load(asset_root: &Path) -> Result<Self, String> {
        let path = asset_root.join("private/stock/skater-collections.json");
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let data: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if data.version != 1 {
            return Err(format!(
                "Unsupported skater collections version {}",
                data.version
            ));
        }
        let mut identities = std::collections::BTreeSet::new();
        for item in &data.collections {
            if !identities.insert((
                crate::attrib_hash::numeric_name(&item.class_name),
                crate::attrib_hash::numeric_name(&item.key),
            )) {
                return Err(format!(
                    "Duplicate collection {}/{}",
                    item.class_name, item.key
                ));
            }
        }
        Ok(data)
    }

    /// perf 10-18: [`Self::load`] once per process: a map load parsed the 9.8 MB vault 4+ times (physics, skater,
    /// gestures, camera, HUDs). Reparsed when the file's size or mtime changes; SKATE_SHARED_VAULT=0 loads every call.
    /// The lock is held across the parse, so concurrent callers wait for one parse rather than each doing their own.
    pub fn shared(asset_root: &Path) -> Result<std::sync::Arc<Self>, String> {
        type Slot = Option<(std::path::PathBuf, Option<std::time::SystemTime>, u64, std::sync::Arc<Collections>)>;
        static SHARED: std::sync::Mutex<Slot> = std::sync::Mutex::new(None);
        static ON: OnceLock<bool> = OnceLock::new();
        if !*ON.get_or_init(|| std::env::var_os("SKATE_SHARED_VAULT").is_none_or(|v| v != "0")) {
            return Self::load(asset_root).map(std::sync::Arc::new);
        }
        let path = asset_root.join("private/stock/skater-collections.json");
        let meta = std::fs::metadata(&path).ok();
        let (modified, len) = (meta.as_ref().and_then(|m| m.modified().ok()), meta.map_or(0, |m| m.len()));
        let mut slot = SHARED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, _, _, data)) = slot.as_ref().filter(|(p, m, l, _)| *p == path && *m == modified && *l == len) {
            return Ok(data.clone());
        }
        let data = std::sync::Arc::new(Self::load(asset_root)?);
        *slot = Some((path, modified, len, data.clone()));
        Ok(data)
    }

    /// Replace every collection of the classes `path` defines with its collections
    /// (e.g. another game's camera classes over the stock ones).
    pub fn overlay(&mut self, path: &Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let other: Self =
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let classes: std::collections::BTreeSet<_> = other
            .collections
            .iter()
            .map(|c| c.class_name.clone())
            .collect();
        self.collections
            .retain(|c| !classes.contains(&c.class_name));
        self.collections.extend(other.collections);
        self.index = OnceLock::new();
        Ok(())
    }

    pub fn field(&self, class: &str, key: &str, name: &str) -> Result<&Field, String> {
        // Converted vaults can retain numeric identities when debug names were
        // unavailable. Both spellings identify the same native AttribSys entry.
        use crate::attrib_hash::numeric_id;
        let index = self.index.get_or_init(|| {
            let mut index = HashMap::with_capacity(self.collections.len());
            for (i, c) in self.collections.iter().enumerate() {
                index
                    .entry((numeric_id(&c.class_name), numeric_id(&c.key)))
                    .or_insert(i);
            }
            index
        });
        let class_id = numeric_id(class);
        let field_id = numeric_id(name);
        let mut current = key;
        for _ in 0..=self.collections.len() {
            let item = index
                .get(&(class_id, numeric_id(current)))
                .map(|&i| &self.collections[i])
                .ok_or_else(|| format!("Missing stock collection {class}/{current}"))?;
            if let Some(field) = item.fields.get(name).or_else(|| {
                item.fields
                    .iter()
                    .find(|(key, _)| numeric_id(key) == field_id)
                    .map(|(_, field)| field)
            }) {
                return Ok(field);
            }
            if item.parent.is_empty() {
                return Err(format!("Missing stock field {class}/{key}/{name}"));
            }
            current = &item.parent;
        }
        Err(format!("Cyclic stock collection inheritance {class}/{key}"))
    }

    pub fn float(&self, class: &str, key: &str, name: &str) -> Result<f32, String> {
        let field = self.field(class, key, name)?;
        if field.type_name != "EA::Reflection::Float" {
            return Err(format!("Expected float at {class}/{key}/{name}"));
        }
        let words = decode_words::<1>(&field.data)?;
        let value = f32::from_bits(words[0]);
        if !value.is_finite() {
            return Err(format!("Non-finite stock float {class}/{key}/{name}"));
        }
        Ok(value)
    }

    pub fn integer(&self, class: &str, key: &str, name: &str) -> Result<u32, String> {
        let field = self.field(class, key, name)?;
        if !matches!(
            field.type_name.as_str(),
            "EA::Reflection::Int32" | "EA::Reflection::UInt32"
        ) {
            return Err(format!("Expected integer at {class}/{key}/{name}"));
        }
        Ok(decode_words::<1>(&field.data)?[0])
    }

    pub fn boolean(&self, class: &str, key: &str, name: &str) -> Result<bool, String> {
        let field = self.field(class, key, name)?;
        if field.type_name != "EA::Reflection::Bool" {
            return Err(format!("Expected boolean at {class}/{key}/{name}"));
        }
        // Attributes store the byte followed by padding; layout bools may
        // contain only the byte. Reading a big-endian u32 gives the wrong bit.
        match field.data.get(..2) {
            Some("00") => Ok(false),
            Some("01") => Ok(true),
            _ => Err(format!("Invalid stock boolean {class}/{key}/{name}")),
        }
    }

    /// Every element of a float array attribute, in order (the index AttribSys
    /// `sub_82B72420` takes as its third argument).
    pub fn float_items(&self, class: &str, key: &str, name: &str) -> Result<Vec<f32>, String> {
        let field = self.field(class, key, name)?;
        if field.type_name != "EA::Reflection::Float" {
            return Err(format!("Expected float array at {class}/{key}/{name}"));
        }
        let array = field
            .array
            .as_ref()
            .ok_or_else(|| format!("Expected array payload at {class}/{key}/{name}"))?;
        array
            .items
            .iter()
            .map(|item| {
                let value = f32::from_bits(decode_words::<1>(item)?[0]);
                if value.is_finite() {
                    Ok(value)
                } else {
                    Err(format!("Non-finite stock float {class}/{key}/{name}"))
                }
            })
            .collect()
    }

    pub fn words<const N: usize>(
        &self,
        class: &str,
        key: &str,
        name: &str,
    ) -> Result<[u32; N], String> {
        decode_words(&self.field(class, key, name)?.data)
    }
}

fn decode_words<const N: usize>(text: &str) -> Result<[u32; N], String> {
    let hex: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if hex.len() != N * 8 || !hex.is_ascii() {
        return Err(format!(
            "Expected {N} big-endian words, found {} bytes of hex",
            hex.len()
        ));
    }
    let mut words = [0; N];
    for (i, word) in words.iter_mut().enumerate() {
        *word = u32::from_str_radix(&hex[i * 8..i * 8 + 8], 16)
            .map_err(|e| format!("Invalid collection payload: {e}"))?;
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_vault_parses_once_until_its_file_changes() {
        let root = std::env::temp_dir().join(format!("s3r-shared-vault-{}", std::process::id()));
        let file = root.join("private/stock/skater-collections.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"version":1,"collections":[]}"#).unwrap();
        let (a, b) = (Collections::shared(&root).unwrap(), Collections::shared(&root).unwrap());
        assert!(std::sync::Arc::ptr_eq(&a, &b));
        std::fs::write(&file, r#"{"version": 1, "collections": []}"#).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&a, &Collections::shared(&root).unwrap()), "a changed file is reparsed");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn numeric_export_names_resolve_readable_manual_settings() {
        // Identities and payload from the prepared vault; no asset IO or gameplay.
        let data: Collections = serde_json::from_value(serde_json::json!({
            "version": 1, "collections": [{
                "class": "anim_motion", "key": crate::attrib_hash::numeric_name("manual"),
                "parent": "", "source": "fixture", "sha256": "",
                "fields": { "Hash_A353CE1670D3AA40": { "type": "EA::Reflection::Float", "data": "3D23D70A" } }
            }]
        })).unwrap();
        assert_eq!(
            data.float("anim_motion", "manual", "manual_clamp_vel")
                .unwrap()
                .to_bits(),
            0x3d23d70a
        );
        assert!(data.field("anim_motion", "manual", "missing").is_err());
    }

    #[test]
    fn indexed_lookup_matches_either_spelling_and_keeps_the_first_entry() {
        let data: Collections = serde_json::from_value(serde_json::json!({
            "version": 1, "collections": [
                {"class":crate::attrib_hash::numeric_name("example"), "key":"a", "parent":"", "source":"fixture", "sha256":"",
                 "fields":{"value":{"type":"EA::Reflection::Float", "data":"3F800000"}}},
                {"class":"example", "key":crate::attrib_hash::numeric_name("a"), "parent":"", "source":"fixture", "sha256":"",
                 "fields":{"value":{"type":"EA::Reflection::Float", "data":"40000000"}}}
            ]
        })).unwrap();
        // Both entries share one identity; the scan it replaces returned the first.
        assert_eq!(data.float("example", "a", "value").unwrap(), 1.0);
        assert_eq!(
            data.float(&crate::attrib_hash::numeric_name("example"), "a", "value")
                .unwrap(),
            1.0
        );
        assert!(data.field("example", "b", "value").is_err());
    }

    #[test]
    fn readable_names_resolve_numeric_queries_through_inheritance() {
        let data: Collections = serde_json::from_value(serde_json::json!({
            "version": 1, "collections": [
                {"class":"example", "key":"child", "parent":crate::attrib_hash::numeric_name("base"),
                 "source":"fixture", "sha256":"", "fields":{}},
                {"class":"example", "key":"base", "parent":"", "source":"fixture", "sha256":"",
                 "fields":{"value":{"type":"EA::Reflection::Float", "data":"3F800000"}}}
            ]
        })).unwrap();
        assert_eq!(
            data.float(
                &crate::attrib_hash::numeric_name("example"),
                &crate::attrib_hash::numeric_name("child"),
                &crate::attrib_hash::numeric_name("value")
            )
            .unwrap(),
            1.0
        );
    }

    #[test]
    fn float_arrays_read_their_items_not_the_header_lanes() {
        let data: Collections = serde_json::from_value(serde_json::json!({
            "version": 1, "collections": [{
                "class": "example", "key": "default", "parent": "", "source": "fixture",
                "sha256": "", "fields": { "thresholds": {
                    "type": "EA::Reflection::Float", "data": "00030003",
                    "array": {"capacity": 3, "element_size": 4, "alignment": 4,
                              "items": ["3FC00000", "40266666", "405CCCCD"]}
                }}
            }]
        }))
        .unwrap();
        let items = data
            .float_items("example", "default", "thresholds")
            .unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], 1.5);
        assert_eq!(items[2].to_bits(), 0x405C_CCCD);
    }
}
