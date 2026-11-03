//! Learn > Trick Guide (retail FETrickTutorial, TU3 0x82619368 and the natives at 0x825ae7e0): the menu tree the
//! `trickguide` movie lists, built from the stock vault as retail builds it, and what the movie's
//! `FETrickTutorial.*` / `Game.GetMenuItems` calls answer. retail_ui.rs drives the screen.
//!
//! - Level 0: the eight categories of the root table 0x830274B0 (class 0x8BD17173D2F48577: name 0x7296.., description
//!   0x273D.., children 0x5703.. RefSpecs into class 0x187447EFFFDF8563).
//! - Level 1: a category's children. A child whose bool 0xA990.. is set (the submenu rows, which inherit `default`'s
//!   01) opens level 2; the others (Hall of Meat, Other Tricks, ...) are tricks themselves. A category with one child
//!   (Manuals) goes straight to that child's level 2 (0x82619530: fewer than 2 children bumps the level).
//! - Level 2: every scorable (class 0x6918469984A8C596) with an animation (0x3B71..) whose trick guide parent
//!   (0x0651.. RefSpec) is the chosen submenu, in the class's iteration order, then EASTL-sorted by layout +128
//!   (0xF9E5.., the guide order).
use skate_data::{attrib_hash::numeric_id, collections::Collections};

const CATEGORY: &str = "Hash_8BD17173D2F48577";
const ROW: &str = "Hash_187447EFFFDF8563";
const SCORABLE: &str = "Hash_6918469984A8C596";
const NAME: &str = "Hash_7296FFD6010D73EF";
const DESCRIPTION: &str = "Hash_273DF7D4EE7A4C1C";
const CHILDREN: &str = "Hash_57035D975F71D506";
const HAS_SUBMENU: &str = "Hash_A990472B974734C3";
const GUIDE: &str = "Hash_03ACC9F200807B87";
const ANIMATION: &str = "Hash_3B71EA27D53D7C4F";
const PARENT: &str = "Hash_06513CC28A532524";
const ORDER: &str = "Hash_F9E5D2F69066E925";
const TRICK_NAME: &str = "Hash_843613E915014627";
const TRICK_DESCRIPTION: &str = "Hash_4663ADFED2CD5EF7";
/// 0x830274B0: Flip Tricks, Grab Tricks, Grinds and Lip Tricks, Manuals, Hall of Meat, Other Tricks, Off-board
/// Actions, Terminology.
const ROOTS: [u64; 8] = [
    0x9673_45FF_A616_9017,
    0x9E2B_56D3_EAC1_11E3,
    0x0AE5_C05D_3211_7E6C,
    0x046E_D0CC_BDA1_091F,
    0x7680_DF06_42C4_A9BE,
    0x5300_D382_AC62_289A,
    0xF5C5_F450_B0C8_703E,
    0xD6CA_2A68_FFE6_A743,
];
/// The scorable class's hash-table size (skaterschema, class 0x6918469984A8C596): Attrib iterates a class in slot
/// order (home slot = low 32 bits of the key mod this, linear probing, filled in vault order).
const SCORABLE_RESERVE: u64 = 439;
/// 0x83027350: tTrickGuideInputSlot button ids -> the names button_item2 takes.
const BUTTONS: [&str; 28] = [
    "", "Back", "Start", "A", "B", "Y", "X", "R_Trigger", "L_Trigger", "L_Bumper", "R_Bumper", "L3_Click", "R3_Click",
    "Stick_Right_Up", "Stick_Right_Down", "Stick_Right_L", "Stick_Right_R", "Stick_Right", "DPad_Up", "DPad_Down",
    "DPad_Left", "DPad_Right", "Stick_Left_Up", "Stick_Left_Down", "Stick_Left_L", "Stick_Left_R", "Stick_Left", "DPad",
];
/// 0x825AEAEC..: connector ids 1, 2, 3.
const CONNECTORS: [&str; 4] = ["", "Plus", "Arrow", "Or"];

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Entry {
    pub name: String,
    pub description: String,
    /// Retail 0x826192A0: A opens a deeper list.
    pub submenu: bool,
    /// tTrickGuideInputSlot (gesture, button, connector) x 4.
    pub guide: [[u32; 3]; 4],
    pub children: Vec<Entry>,
    /// A category with a single child shows that child's list in its place (the level counts both).
    pub skips: bool,
}

#[derive(Debug, Default)]
pub(crate) struct TrickGuide {
    roots: Vec<Entry>,
    /// The highlighted row of each open level, outermost first.
    path: Vec<usize>,
}

fn text(data: &Collections, class: &str, key: &str, field: &str) -> String {
    data.field(class, key, field).map(|f| f.data.clone()).unwrap_or_default()
}
fn hex_key(item: &str) -> Option<u64> {
    u64::from_str_radix(item.get(16..32)?, 16).ok()
}
fn guide(data: &Collections, class: &str, key: &str) -> [[u32; 3]; 4] {
    let mut out = [[0; 3]; 4];
    if let Ok(items) = data.field(class, key, GUIDE).map(|f| f.array.as_ref().map(|a| a.items.clone()).unwrap_or_default()) {
        for (slot, item) in out.iter_mut().zip(items) {
            for (i, word) in slot.iter_mut().enumerate() {
                *word = item.get(i * 8..i * 8 + 8).and_then(|w| u32::from_str_radix(w, 16).ok()).unwrap_or(0);
            }
        }
    }
    out
}

/// EASTL quick_sort (kQuickSortLimit 28; 0x82631FC8 / 0x82632120 / 0x82632230), ordered by `key`: stable only up to
/// 28 rows, as retail's lists are.
pub(crate) fn eastl_sort<T>(items: &mut [T], key: impl Fn(&T) -> i32 + Copy) {
    const LIMIT: usize = 28;
    fn partition<T>(v: &mut [T], pivot: i32, key: impl Fn(&T) -> i32) -> usize {
        let (mut first, mut last) = (0, v.len());
        loop {
            while key(&v[first]) < pivot {
                first += 1;
            }
            last -= 1;
            while pivot < key(&v[last]) {
                last -= 1;
            }
            if first >= last {
                return first;
            }
            v.swap(first, last);
            first += 1;
        }
    }
    fn median(a: i32, b: i32, c: i32) -> i32 {
        if a < b {
            if b < c { b } else if a < c { c } else { a }
        } else if a < c {
            a
        } else if b < c {
            c
        } else {
            b
        }
    }
    fn quick<T>(v: &mut [T], depth: u32, key: impl Fn(&T) -> i32 + Copy) {
        let (mut end, mut depth) = (v.len(), depth);
        while end > LIMIT && depth > 0 {
            let pivot = median(key(&v[0]), key(&v[end / 2]), key(&v[end - 1]));
            let at = partition(&mut v[..end], pivot, key);
            depth -= 1;
            quick(&mut v[at..end], depth, key);
            end = at;
        }
        if depth == 0 {
            // partial_sort(first, last, last): a heap sort; never reached for the guide's list sizes.
            v[..end].sort_by_key(key);
        }
    }
    fn insertion<T>(v: &mut [T], key: impl Fn(&T) -> i32) {
        for i in 1..v.len() {
            let mut j = i;
            while j > 0 && key(&v[j]) < key(&v[j - 1]) {
                v.swap(j, j - 1);
                j -= 1;
            }
        }
    }
    let n = items.len();
    if n < 2 {
        return;
    }
    quick(items, 2 * n.ilog2(), key);
    insertion(items, key);
}

/// Class iteration order: the key's slot in an open-addressed table of `reserve` slots, filled in vault order.
fn slot_order(keys: &[u64], reserve: u64) -> Vec<usize> {
    let mut slots: Vec<Option<usize>> = vec![None; reserve.max(keys.len() as u64) as usize];
    let len = slots.len() as u64;
    for (i, key) in keys.iter().enumerate() {
        let mut at = (key & 0xFFFF_FFFF) % len;
        while slots[at as usize].is_some() {
            at = (at + 1) % len;
        }
        slots[at as usize] = Some(i);
    }
    slots.into_iter().flatten().collect()
}

impl TrickGuide {
    pub(crate) fn build(data: &Collections) -> Self {
        let scorables: Vec<&skate_data::collections::Collection> =
            data.entries().iter().filter(|c| numeric_id(&c.class_name) == numeric_id(SCORABLE)).collect();
        let keys: Vec<u64> = scorables.iter().map(|c| numeric_id(&c.key)).collect();
        let order = slot_order(&keys, SCORABLE_RESERVE);
        let tricks = |submenu: u64| -> Vec<Entry> {
            let mut rows: Vec<(i32, Entry)> = order
                .iter()
                .map(|&i| &scorables[i].key)
                .filter(|key| !text(data, SCORABLE, key, ANIMATION).is_empty())
                .filter(|key| data.field(SCORABLE, key, PARENT).ok().and_then(|f| hex_key(&f.data)) == Some(submenu))
                .map(|key| {
                    let order = data.integer(SCORABLE, key, ORDER).unwrap_or(0) as i32;
                    let entry = Entry {
                        name: text(data, SCORABLE, key, TRICK_NAME),
                        description: text(data, SCORABLE, key, TRICK_DESCRIPTION),
                        guide: guide(data, SCORABLE, key),
                        ..Default::default()
                    };
                    (order, entry)
                })
                .collect();
            eastl_sort(&mut rows, |r| r.0);
            rows.into_iter().map(|r| r.1).collect()
        };
        let roots = ROOTS
            .iter()
            .map(|&root| {
                let key = format!("Hash_{root:016X}");
                let refs = data.field(CATEGORY, &key, CHILDREN).ok().and_then(|f| f.array.as_ref()).map(|a| a.items.clone());
                let mut children: Vec<Entry> = (refs.unwrap_or_default().iter().filter_map(|item| hex_key(item)))
                    .map(|child| {
                        let row = format!("Hash_{child:016X}");
                        let submenu = data.boolean(ROW, &row, HAS_SUBMENU).unwrap_or(false);
                        Entry {
                            name: text(data, ROW, &row, NAME),
                            description: text(data, ROW, &row, DESCRIPTION),
                            submenu,
                            guide: guide(data, ROW, &row),
                            children: if submenu { tricks(child) } else { Vec::new() },
                            skips: false,
                        }
                    })
                    .collect();
                let skips = children.len() == 1;
                if skips {
                    children = std::mem::take(&mut children[0].children);
                }
                Entry {
                    name: text(data, CATEGORY, &key, NAME),
                    description: text(data, CATEGORY, &key, DESCRIPTION),
                    submenu: true,
                    children,
                    skips,
                    ..Default::default()
                }
            })
            .collect();
        Self { roots, path: vec![0] }
    }

    #[cfg(test)]
    pub(crate) fn from_roots(roots: Vec<Entry>) -> Self {
        Self { roots, path: vec![0] }
    }

    /// Back to the top, first row (each opening).
    pub(crate) fn reset(&mut self) {
        self.path = vec![0];
    }

    /// The rows of the open level.
    pub(crate) fn rows(&self) -> &[Entry] {
        let mut rows = &self.roots[..];
        for &i in &self.path[..self.path.len() - 1] {
            rows = &rows[i].children;
        }
        rows
    }
    fn current(&self) -> Option<&Entry> {
        self.rows().get(*self.path.last()?)
    }
    /// Retail +112: the level number (a skipped single child counts).
    pub(crate) fn level(&self) -> usize {
        let mut level = self.path.len() - 1;
        if self.path.len() > 1 && self.roots.get(self.path[0]).is_some_and(|r| r.skips) {
            level += 1;
        }
        level
    }
    pub(crate) fn highlight(&mut self, index: usize) {
        let last = self.rows().len().saturating_sub(1);
        if let Some(at) = self.path.last_mut() {
            *at = index.min(last);
        }
    }
    /// A on the highlighted row: true when a deeper list opened.
    pub(crate) fn select(&mut self) -> bool {
        if self.current().is_some_and(|e| e.submenu && !e.children.is_empty()) {
            self.path.push(0);
            true
        } else {
            false
        }
    }
    /// B: true when a level closed (false at the top: the screen closes).
    pub(crate) fn back(&mut self) -> bool {
        if self.path.len() > 1 {
            self.path.pop();
            true
        } else {
            false
        }
    }
    /// The highlighted row is a trick (its gestures show).
    pub(crate) fn on_trick(&self) -> bool {
        self.current().is_some_and(|e| !e.submenu)
    }

    /// What the movie's natives answer for the open level.
    pub(crate) fn answers(&self) -> Vec<(String, serde_json::Value)> {
        use serde_json::json;
        let current = self.current().cloned().unwrap_or_default();
        let index = *self.path.last().unwrap_or(&0);
        // 0x825AE968: [count of used slots, then each of the 4 slots' gesture art, button, connector].
        let mut gestures = vec![json!(current.guide.iter().filter(|s| s[0] != 0 || s[1] != 0).count())];
        for [gesture, button, connector] in current.guide {
            let gesture = if matches!(gesture, 58 | 59) { 0 } else { gesture };
            gestures.push(json!(crate::hud_meters::GESTURES.get(gesture as usize).copied().unwrap_or("")));
            gestures.push(json!(BUTTONS.get(button as usize).copied().unwrap_or("")));
            gestures.push(json!(CONNECTORS.get(connector as usize).copied().unwrap_or("")));
        }
        vec![
            ("Game.GetMenuItems".into(), json!(self.rows().iter().map(|e| e.name.clone()).collect::<Vec<_>>())),
            // 0x825AE8A8: [level, has submenu, description].
            ("FETrickTutorial.GetMenuItemData".into(), json!([self.level(), current.submenu, current.description])),
            ("FETrickTutorial.GetCurrentMenuItem".into(), json!(index)),
            ("FETrickTutorial.GetMenuItemGesturesButtons".into(), json!(gestures)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bug 10-17 #30/#39: every button the guide names has art on both pads (a direction variant, `Stick_Left_L`,
    /// takes its pad's, as apt_scene::glyph resolves it), the stick clicks included.
    #[test]
    fn every_guide_button_has_art_on_both_pads() {
        use crate::button_icons::{PS_PREFIX, replace};
        for name in BUTTONS.into_iter().filter(|n| !n.is_empty()) {
            let art = |path: &dyn Fn(&str) -> String| {
                [name, name.rsplit_once('_').map_or(name, |(pad, _)| pad)].iter().any(|n| replace(&path(n), [40, 30]).is_some())
            };
            assert!(art(&|n| format!("runtime/buttons/{n}.rgba")), "{name}: Xbox");
            assert!(art(&|n| format!("{PS_PREFIX}{n}.rgba")), "{name}: PlayStation");
        }
    }

    fn leaf(name: &str) -> Entry {
        Entry { name: name.into(), description: format!("{name}_DESC"), guide: [[37, 3, 0], [0; 3], [0; 3], [0; 3]], ..Default::default() }
    }

    #[test]
    fn eastl_sort_keeps_short_lists_stable_and_orders_long_ones() {
        let mut short: Vec<(i32, usize)> = [3, 1, 2, 1, 3, 1].iter().copied().zip(0..).collect();
        eastl_sort(&mut short, |r| r.0);
        assert_eq!(short, [(1, 1), (1, 3), (1, 5), (2, 2), (3, 0), (3, 4)]);
        let mut long: Vec<(i32, usize)> = (0..35).map(|i| ((i * 7 % 18) as i32, i)).collect();
        eastl_sort(&mut long, |r| r.0);
        assert!(long.windows(2).all(|w| w[0].0 <= w[1].0));
        assert_eq!(long.len(), 35);
    }

    #[test]
    fn slots_fill_home_first_then_probe() {
        // 5 and 0 both home at slot 0 of 5: the later one probes to 1; 7 homes at 2.
        assert_eq!(slot_order(&[5, 0, 7], 5), [0, 1, 2]);
        assert_eq!(slot_order(&[7, 5], 5), [1, 0]);
    }

    #[test]
    fn levels_descend_skip_single_children_and_climb_back() {
        let flips = Entry { name: "Flips".into(), submenu: true, children: vec![leaf("Ollie"), leaf("Kickflip")], ..Default::default() };
        let category = Entry { name: "Flip Tricks".into(), submenu: true, children: vec![flips], ..Default::default() };
        let manuals = Entry { name: "Manuals".into(), submenu: true, skips: true, children: vec![leaf("Manual")], ..Default::default() };
        let mut guide = TrickGuide::from_roots(vec![category, manuals]);
        assert_eq!(guide.level(), 0);
        assert!(!guide.on_trick());
        assert!(guide.select());
        assert_eq!(guide.rows()[0].name, "Flips");
        assert!(guide.select());
        guide.highlight(1);
        assert_eq!(guide.level(), 2);
        assert!(guide.on_trick());
        let answers: std::collections::HashMap<_, _> = guide.answers().into_iter().collect();
        assert_eq!(answers["Game.GetMenuItems"], serde_json::json!(["Ollie", "Kickflip"]));
        assert_eq!(answers["FETrickTutorial.GetMenuItemData"], serde_json::json!([2, false, "Kickflip_DESC"]));
        let gestures = answers["FETrickTutorial.GetMenuItemGesturesButtons"].as_array().unwrap();
        assert_eq!(gestures[0..4], [serde_json::json!(1), "ollie".into(), "A".into(), "".into()]);
        assert_eq!(gestures.len(), 13);
        assert!(!guide.select(), "a trick opens nothing");
        assert!(guide.back() && guide.back());
        assert!(!guide.back(), "B at the top closes the screen");
        // Manuals lists its one child's tricks at level 2.
        guide.highlight(1);
        assert!(guide.select());
        assert_eq!((guide.level(), guide.rows()[0].name.as_str()), (2, "Manual"));
        assert!(guide.back());
        assert_eq!(guide.rows().len(), 2);
    }

    /// The stock vault's tree (SKATE3_ASSET_ROOT): eight categories, Flip Tricks > Ollie Flips lists its 21 tricks
    /// by guide order.
    #[test]
    fn stock_tree_matches_the_vault() {
        let Some(root) = std::env::var_os("SKATE3_ASSET_ROOT").map(std::path::PathBuf::from) else { return };
        let Ok(data) = Collections::load(&root) else { return };
        let mut guide = TrickGuide::build(&data);
        let names: Vec<&str> = guide.rows().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names[0], "ID_TRICK_TYPE_FLIP_TRICKS");
        assert_eq!(names.len(), 8);
        assert!(guide.select());
        assert_eq!(guide.rows()[0].name, "ID_TRICK_TYPE_OLLIE_FLIPS");
        assert!(guide.select());
        assert_eq!(guide.rows().len(), 21);
        assert!(guide.rows().iter().all(|e| e.name.starts_with("ID_TRICK_FLIP_")));
    }
}
