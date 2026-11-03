//! Original APT movie hierarchy and timeline control, independent of Bevy.
use crate::{
    apt_display::{Control, DisplayList, Placement},
    apt_vm::{Instruction, ObjectKind, Value, Vm},
};
use serde::Deserialize;
use std::collections::{BTreeMap, VecDeque};

/// Not retail (user 10-25): a text field whose text is a gold literal (apt_text::gold, the creator tag); apt_scene
/// paints it in the tag's colour (creator::colour, a rainbow sweep since 10-26).
pub const GOLD_FIELD: &str = "_s3rGold";
/// On the movie's root once any of its fields showed the tag: apt_scene::draw_key then redraws the movie at each step of
/// the sweep. It stays for the movie's life (a Player List that showed the creator keeps redrawing 20 times a second).
pub const GOLD_SHOWN: &str = "_s3rGoldShown";
/// Not retail (user 10-27): a text field whose text is a fit literal (apt_text::fit); apt_scene shrinks its line into the
/// field, or into [`FIT_WIDTH`] when narrower.
pub const FIT_FIELD: &str = "_s3rFit";
/// On a movie's root: the most a fit field's line may span (field units), where the field runs past its panel (the
/// Player List's rows are 558 wide on a panel showing about 400).
pub const FIT_WIDTH: &str = "_s3rFitWidth";

#[derive(Clone, Deserialize)]
pub struct Frame {
    pub controls: Vec<Control>,
}
#[derive(Clone, Deserialize)]
pub struct Character {
    pub id: i32,
    pub type_name: String,
    #[serde(default)]
    pub frames: Vec<Frame>,
    pub text: Option<serde_json::Value>,
    pub bounds: Option<[f32; 4]>,
}
#[derive(Clone)]
pub struct Instance {
    pub character: i32,
    pub frame: usize,
    pub playing: bool,
    pub children: BTreeMap<i32, usize>,
    /// attachMovie clips. Timeline seeks never retire them; they draw above
    /// authored children, like Flash's dynamic depth range.
    pub dynamic: BTreeMap<i32, usize>,
    pub placement: Option<Placement>,
}
#[derive(Clone)]
pub struct Movie {
    pub characters: BTreeMap<i32, Character>,
    pub instances: BTreeMap<usize, Instance>,
    pub actions: BTreeMap<String, Vec<Instruction>>,
    pub pending: VecDeque<(usize, u32)>,
    /// Clips of registered classes awaiting their constructor, drained by the host
    /// before frame actions (Object.registerClass semantics).
    pub constructing: VecDeque<(usize, usize)>,
    /// Linkage name -> character, for registerClass/attachMovie. Empty for the HUD.
    pub exports: BTreeMap<String, i32>,
    /// Bundle index (character id / 10000) -> that library's own linkage names;
    /// several merged controls export the same name ("Notch").
    pub scoped_exports: BTreeMap<String, BTreeMap<String, i32>>,
    /// image_renderer slots: a render quad renamed to its slot index draws this
    /// texture instead of its authored fill (native ImageRenderer_Load).
    pub images: BTreeMap<String, crate::apt_scene::Texture>,
    /// swatch_renderer slots: the renamed quad draws in this colour (RGBA 0-1).
    pub colours: BTreeMap<String, [f32; 4]>,
    /// Character -> registered class function object.
    pub classes: BTreeMap<i32, usize>,
    /// Clips whose position a script assigned. As in Flash, the timeline stops
    /// moving them. Populated only by hosts that opt in (not the HUD).
    pub scripted: std::collections::BTreeSet<usize>,
    pub root: usize,
    pub text_assets: crate::apt_text::TextAssets,
    states: BTreeMap<i32, Vec<DisplayList>>,
}
impl Movie {
    pub fn load(json: &serde_json::Value) -> Result<Self, String> {
        let characters: Vec<Character> =
            serde::Deserialize::deserialize(&json["characters"]).map_err(|e| e.to_string())?;
        let mut states = BTreeMap::new();
        for c in &characters {
            let mut list = DisplayList::default();
            let mut frames = Vec::new();
            for frame in &c.frames {
                for control in &frame.controls {
                    list.apply(control)?;
                }
                frames.push(list.clone());
            }
            states.insert(c.id, frames);
        }
        Ok(Self {
            characters: characters.into_iter().map(|c| (c.id, c)).collect(),
            instances: BTreeMap::new(),
            actions: serde::Deserialize::deserialize(&json["actions"]).map_err(|e| e.to_string())?,
            pending: VecDeque::new(),
            constructing: VecDeque::new(),
            exports: match json.get("exports") {
                Some(value) => serde::Deserialize::deserialize(value).map_err(|e| e.to_string())?,
                None => BTreeMap::new(),
            },
            scoped_exports: match json.get("scoped_exports") {
                Some(value) => serde::Deserialize::deserialize(value).map_err(|e| e.to_string())?,
                None => BTreeMap::new(),
            },
            images: BTreeMap::new(),
            colours: BTreeMap::new(),
            classes: BTreeMap::new(),
            scripted: Default::default(),
            root: usize::MAX,
            text_assets: crate::apt_text::TextAssets::load(json)?,
            states,
        })
    }
    pub fn initialize(&mut self, vm: &mut Vm) -> Result<(), String> {
        self.root = self.create(vm, 0, None, 0)?;
        vm.set(self.root, "_root", Value::Object(self.root))?;
        // Every clip sees the same authored root, including unnamed children.
        for id in self.instances.keys() {
            vm.set(*id, "_root", Value::Object(self.root))?;
        }
        Ok(())
    }
    fn create(
        &mut self,
        vm: &mut Vm,
        character: i32,
        parent: Option<usize>,
        depth: usize,
    ) -> Result<usize, String> {
        if depth > 32 || self.instances.len() > 8192 {
            return Err(format!("APT movie hierarchy limit (depth {depth}, {} clips)", self.instances.len()));
        }
        let c = self
            .characters
            .get(&character)
            .ok_or("APT unknown character")?
            .clone();
        let id = vm.object(ObjectKind::Native(format!("movie:{character}")));
        if self.root != usize::MAX {
            vm.set(id, "_root", Value::Object(self.root))?;
        }
        if let Some(parent) = parent {
            vm.set(id, "_parent", Value::Object(parent))?;
        }
        vm.set(id, "_x", Value::Number(0.0))?;
        vm.set(id, "_y", Value::Number(0.0))?;
        vm.set(id, "_visible", Value::Bool(true))?;
        vm.set(id, "_alpha", Value::Number(100.0))?;
        if let Some(text) = &c.text {
            vm.set(
                id,
                "text",
                Value::Text(text["initial_text"].as_str().unwrap_or("").into()),
            )?;
        }
        self.instances.insert(
            id,
            Instance {
                character,
                frame: 0,
                playing: !c.frames.is_empty(),
                children: BTreeMap::new(),
                dynamic: BTreeMap::new(),
                placement: None,
            },
        );
        let class = self.classes.get(&character).copied();
        if let Some(Value::Object(proto)) = class.map(|class| vm.get(class, "prototype")) {
            vm.set_prototype(id, proto)?;
        }
        self.text_changed(vm, id)?;
        if !c.frames.is_empty() {
            self.seek(vm, id, 0, depth + 1)?;
        }
        if c.text.is_none() {
            // ponytail: measured once, on the first frame, in the clip's own
            // units; layout scripts read it right after placing/attaching.
            self.measure(vm, id)?;
        }
        // After the first frame's children exist, as Flash runs registered
        // constructors with authored child clips already addressable.
        if let Some(class) = class {
            self.constructing.push_back((id, class));
        }
        Ok(id)
    }
    /// MovieClip.attachMovie(linkage, name, depth). Replaces any clip at that depth. An unknown linkage attaches
    /// nothing (None), as in Flash: gesture_item's constructor asks for "g_" + its unset image.
    pub fn attach(
        &mut self,
        vm: &mut Vm,
        parent: usize,
        linkage: &str,
        name: &str,
        depth: i32,
    ) -> Result<Option<usize>, String> {
        let bundle = self.instances.get(&parent).map_or(0, |i| i.character / 10000).to_string();
        let scoped = self.scoped_exports.get(&bundle);
        // Linkages resolve case-insensitively too, exact matches first: menu_picker asks for "ITEMLABEL_DARK",
        // sk8popup exports "ITEMLABEL_Dark".
        let Some(&character) = scoped.and_then(|e| e.get(linkage)).or_else(|| self.exports.get(linkage)).or_else(|| {
            let mut all = scoped.into_iter().chain([&self.exports]).flatten();
            all.find(|(k, _)| k.eq_ignore_ascii_case(linkage)).map(|(_, v)| v)
        }) else {
            return Ok(None);
        };
        if let Some(old) = self
            .instances
            .get_mut(&parent)
            .ok_or("APT attachMovie on absent clip")?
            .dynamic
            .remove(&depth)
        {
            self.remove(vm, old);
        }
        let child = self.create(vm, character, Some(parent), 0)?;
        vm.set(child, "_root", Value::Object(self.root))?;
        vm.set(child, "_name", Value::Text(name.into()))?;
        self.instances.get_mut(&child).unwrap().placement = Some(Placement {
            character,
            matrix: [1., 0., 0., 1., 0., 0.],
            color: [255, 255, 255, 255, 0, 0, 0, 0],
            ratio: 0.,
            name: name.into(),
            clip_depth: -1,
            blend_mode: -1,
            init: Default::default(),
        });
        self.instances.get_mut(&parent).unwrap().dynamic.insert(depth, child);
        vm.touch();
        vm.set(parent, name, Value::Object(child))?;
        // A createEmptyMovieClip clip sizes to what is attached into it: dynamic_selectable_list centres its picker
        // on ROW0._height.
        if self.instances[&parent].character >= 1_000_000 {
            self.measure(vm, parent)?;
        }
        Ok(Some(child))
    }
    fn measure(&self, vm: &mut Vm, id: usize) -> Result<(), String> {
        if let Some([x0, y0, x1, y1]) = self.extent(vm, id, 0) {
            vm.set(id, "_width", Value::Number((x1 - x0) as f64))?;
            vm.set(id, "_height", Value::Number((y1 - y0) as f64))?;
        }
        Ok(())
    }
    /// MovieClip.createTextField(name, depth, x, y, width, height): a dynamic,
    /// initially empty text field; setTextFormat later chooses font and style.
    pub fn create_text_field(
        &mut self,
        vm: &mut Vm,
        parent: usize,
        name: &str,
        depth: i32,
        rect: [f32; 4],
    ) -> Result<usize, String> {
        let id = 1_000_000 + self.characters.len() as i32;
        let font = self.text_assets.font_named("").ok_or("createTextField: movie has no font")?;
        self.characters.insert(
            id,
            Character {
                id,
                type_name: "text".into(),
                frames: Vec::new(),
                text: Some(serde_json::json!({
                    "font_id": font, "alignment": 0, "color_argb": "#ff000000", "font_height": 12.0,
                    "read_only": true, "multiline": false, "word_wrap": false, "initial_text": "", "variable": ""
                })),
                bounds: Some([0., 0., rect[2], rect[3]]),
            },
        );
        let child = self.dynamic(vm, parent, id, name, depth)?;
        vm.set(child, "_x", Value::Number(rect[0] as f64))?;
        vm.set(child, "_y", Value::Number(rect[1] as f64))?;
        Ok(child)
    }
    /// MovieClip.createEmptyMovieClip(name, depth): dynamic_selectable_list attaches its row columns into one.
    pub fn create_empty_clip(&mut self, vm: &mut Vm, parent: usize, name: &str, depth: i32) -> Result<usize, String> {
        let id = 1_000_000 + self.characters.len() as i32;
        self.characters.insert(id, Character { id, type_name: "sprite".into(), frames: Vec::new(), text: None, bounds: None });
        let clip = self.dynamic(vm, parent, id, name, depth)?;
        vm.set(clip, "_width", Value::Number(0.))?;
        vm.set(clip, "_height", Value::Number(0.))?;
        Ok(clip)
    }
    fn dynamic(&mut self, vm: &mut Vm, parent: usize, character: i32, name: &str, depth: i32) -> Result<usize, String> {
        if let Some(old) = self.instances.get_mut(&parent).ok_or("dynamic clip on absent clip")?.dynamic.remove(&depth) {
            self.remove(vm, old);
        }
        let child = self.create(vm, character, Some(parent), 0)?;
        vm.set(child, "_root", Value::Object(self.root))?;
        vm.set(child, "_name", Value::Text(name.into()))?;
        self.instances.get_mut(&parent).unwrap().dynamic.insert(depth, child);
        vm.touch();
        vm.set(parent, name, Value::Object(child))?;
        Ok(child)
    }
    /// TextField.setTextFormat: font, size, colour and alignment of a field.
    pub fn set_text_format(&mut self, vm: &mut Vm, field: usize, format: usize) -> Result<(), String> {
        let Some(character) = self.instances.get(&field).map(|i| i.character) else { return Ok(()) };
        let font = match vm.get(format, "font") {
            Value::Text(name) => self.text_assets.font_named(&name),
            _ => None,
        };
        let Some(text) = self.characters.get_mut(&character).and_then(|c| c.text.as_mut()) else { return Ok(()) };
        // Authored fields keep their font and layout (shared characters); only
        // fields created at run time take the whole format.
        if character >= 1_000_000 {
            if let Some(font) = font {
                text["font_id"] = font.into();
            }
            if let Value::Number(size) = vm.get(format, "size") {
                text["font_height"] = size.into();
            }
            if let Value::Text(align) = vm.get(format, "align") {
                text["alignment"] = match align.as_str() {
                    "right" => 1,
                    "center" => 2,
                    _ => 0,
                }
                .into();
            }
        }
        if let Value::Number(color) = vm.get(format, "color") {
            vm.set(field, "textColor", Value::Number(color))?;
        }
        vm.touch(); // perf 10-18: the font, size or alignment may have changed
        self.text_changed(vm, field)
    }
    /// MovieClip.removeMovieClip(): only dynamic clips may be removed, as in Flash.
    fn detach(&mut self, vm: &mut Vm, id: usize) {
        let Value::Object(parent) = vm.get(id, "_parent") else {
            return;
        };
        let Some(instance) = self.instances.get_mut(&parent) else {
            return;
        };
        let Some(depth) = instance.dynamic.iter().find(|(_, c)| **c == id).map(|(d, _)| *d) else {
            return;
        };
        instance.dynamic.remove(&depth);
        if let Value::Text(name) = vm.get(id, "_name") {
            if vm.get(parent, &name) == Value::Object(id) {
                vm.remove(parent, &name);
            }
        }
        self.remove(vm, id);
    }
    /// Local bounds of a clip: authored shape/text bounds, or the union of its
    /// children placed by their matrices and script positions.
    pub(crate) fn extent(&self, vm: &Vm, id: usize, nesting: usize) -> Option<[f32; 4]> {
        let instance = self.instances.get(&id)?;
        let character = self.characters.get(&instance.character)?;
        if let Some(b) = character.bounds {
            // An autoSize'd field's box is its text (Flash): "left" keeps its left edge, "right" its right.
            let autosize = vm.get(id, "autoSize").text();
            if character.text.is_none() || !matches!(autosize.as_str(), "left" | "right" | "center") {
                return Some(b);
            }
            let (w, h) = (vm.get(id, "_width").number() as f32, vm.get(id, "_height").number() as f32);
            let x0 = match autosize.as_str() {
                "left" => b[0],
                "right" => b[2] - w,
                _ => (b[0] + b[2] - w) / 2.,
            };
            return Some([x0, b[1], x0 + w, b[1] + h]);
        }
        if nesting > 32 {
            return None;
        }
        let mut out: Option<[f32; 4]> = None;
        for child in instance.children.values().chain(instance.dynamic.values()) {
            let Some(b) = self.extent(vm, *child, nesting + 1) else { continue };
            let m = self.instances[child].placement.as_ref().map_or([1., 0., 0., 1., 0., 0.], |p| p.matrix);
            let (tx, ty) = (vm.get(*child, "_x").number() as f32, vm.get(*child, "_y").number() as f32);
            for (x, y) in [(b[0], b[1]), (b[2], b[1]), (b[0], b[3]), (b[2], b[3])] {
                let p = [m[0] * x + m[2] * y + tx, m[1] * x + m[3] * y + ty];
                let o = out.get_or_insert([p[0], p[1], p[0], p[1]]);
                *o = [o[0].min(p[0]), o[1].min(p[1]), o[2].max(p[0]), o[3].max(p[1])];
            }
        }
        out
    }
    pub fn text_changed(&self, vm: &mut Vm, id: usize) -> Result<(), String> {
        let Some(instance) = self.instances.get(&id) else {
            return Ok(());
        };
        let character = &self.characters[&instance.character];
        let Some(text) = &character.text else {
            return Ok(());
        };
        let localized = self.text_assets.localize(&vm.get(id, "text").text());
        // User 10-25: a gold literal (apt_text::gold, the creator tag) loses its marker here and flags the field.
        let (value, gold) = crate::apt_text::split_gold(&localized);
        // User 10-27: so does a fit literal (apt_text::fit), after the gold marker.
        let (value, fit) = crate::apt_text::split_fit(value);
        let value = value.to_owned();
        if fit || vm.get_ref(id, FIT_FIELD).is_some() {
            vm.set(id, FIT_FIELD, Value::Bool(fit))?;
        }
        vm.set(id, "_displayText", Value::Text(value.clone()))?;
        // Only fields that ever held one carry the flag (no new member on every text field).
        if gold || vm.get_ref(id, GOLD_FIELD).is_some() {
            vm.set(id, GOLD_FIELD, Value::Bool(gold))?;
        }
        if gold {
            // The sweep's redraws (GOLD_SHOWN); cosmetic, so a root not made yet never fails the text.
            let _ = vm.set(self.root, GOLD_SHOWN, Value::Bool(true));
        }
        let font = self
            .text_assets
            .fonts
            .get(&(text["font_id"].as_i64().ok_or("Invalid text font id")? as i32))
            .ok_or("Missing original text font")?;
        let height = text["font_height"].as_f64().ok_or("Invalid text size")? as f32;
        let bounds = character.bounds.ok_or("Missing text bounds")?;
        // A fit field reads as wide as it is drawn, no wider than its room: scripts size by it (the Player List's
        // highlight glow ran past the server browser's panel, user 10-27).
        let width = font.width(&value, height);
        let width = crate::apt_scene::fit_room(self, vm, id, bounds).map_or(width, |room| width.min(room));
        vm.set(id, "textWidth", Value::Number(width as f64))?;
        vm.set(id, "textHeight", Value::Number(height as f64))?;
        // TextField.maxscroll is not Flash's (lines - visible + 1): retail APT's getter (0x82E56AC0, property 9)
        // returns TextField +16, which the SkateAptString layout (0x82CA1A8C..0x82CA1DB8) sets to the wrapped
        // line count (starting at 1, +1 per wrap or '^' break). Without word wrap (test at 0x82CA1A84) the
        // no-wrap path copies +88 into +84 (0x82CA1EC0..C8); +88's only writer is the ctor's 1 (0x82CA1524), and
        // +84 reaches TextField +16 via output+112 (0x82CA1F1C, 0x82E59BB8), so no-wrap fields return 1. Skate 2
        // is the same: getter 0x823E0C98, relayout 0x82311E60, layout 0x8286B500 (no-wrap +68 = 1 copied at
        // 0x8286BA8C). Layout scripts multiply it by their line height (save_warning's Refresh: maxscroll * 24).
        let wrap = crate::apt_text::wrap_width(text, bounds, &vm.get(id, "wordWrap"));
        let lines = font.lines(&value, height, wrap).len();
        vm.set(id, "maxscroll", Value::Number(if wrap.is_some() { lines.max(1) } else { 1 } as f64))?;
        let autosize = vm.get(id, "autoSize").text();
        let fit = autosize == "left" || autosize == "right" || autosize == "center";
        vm.set(id, "_width", Value::Number(if fit { width } else { bounds[2] - bounds[0] } as f64))?;
        // TextField._height: the field's box, or its lines when autoSize grows it
        // (layout scripts stack panels by it).
        let fitted = lines.max(1) as f32 * height * crate::apt_text::LINE_PITCH + 4.;
        vm.set(id, "_height", Value::Number(if fit { fitted } else { bounds[3] - bounds[1] } as f64))?;
        // The clip holding it resizes with it: endchallenge_online's table centres its "Header Text" clips by
        // _width after setting the text and autoSize (user 10-14, "Rank" off the panel's edge).
        // ponytail: the immediate parent only, unscaled, as measure() at creation; deeper ancestors keep theirs.
        if let Value::Object(parent) = vm.get(id, "_parent") {
            let is_clip = self.instances.get(&parent).is_some_and(|p| self.characters[&p.character].text.is_none());
            if fit && is_clip {
                self.measure(vm, parent)?;
            }
        }
        Ok(())
    }
    fn remove(&mut self, vm: &mut Vm, id: usize) {
        if let Some(instance) = self.instances.remove(&id) {
            for child in instance.children.values().chain(instance.dynamic.values()) {
                self.remove(vm, *child);
            }
        }
        // VM handles are recycled after collection; a new clip starts timeline-driven.
        self.scripted.remove(&id);
        vm.touch();
        // Retired handles can remain referenced by ActionScript, but no longer
        // participate in timeline advancement or rendering.
        let _ = vm.set(id, "_visible", Value::Bool(false));
    }
    pub fn seek(
        &mut self,
        vm: &mut Vm,
        id: usize,
        frame: usize,
        nesting: usize,
    ) -> Result<(), String> {
        let instance = self
            .instances
            .get(&id)
            .ok_or("APT absent movie instance")?
            .clone();
        let character = self
            .characters
            .get(&instance.character)
            .ok_or("APT absent movie character")?;
        if frame >= character.frames.len() {
            return Err(format!(
                "APT frame {frame} outside character {}",
                character.id
            ));
        }
        let frame_count = character.frames.len();
        let actions: Vec<_> = character.frames[frame]
            .controls
            .iter()
            .filter(|c| c.type_name == "do_action" && c.actions_offset != 0)
            .map(|c| c.actions_offset)
            .collect();
        let list = self.states[&instance.character][frame].clone();
        let mut children = BTreeMap::new();
        for (depth, placement) in list.depths {
            let previous = instance.children.get(&depth).copied();
            if let Some(name) = previous
                .and_then(|old| self.instances.get(&old))
                .and_then(|i| i.placement.as_ref())
                .map(|p| p.name.clone())
            {
                if !name.is_empty() && name != placement.name {
                    vm.remove(id, &name);
                }
            }
            let child = if let Some(old) = previous.filter(|old| {
                self.instances
                    .get(old)
                    .is_some_and(|i| i.character == placement.character)
            }) {
                old
            } else {
                if let Some(old) = previous {
                    self.remove(vm, old);
                }
                let child = self.create(vm, placement.character, Some(id), nesting + 1)?;
                for (key, value) in &placement.init {
                    vm.set(child, key.as_str(), Value::Text(value.clone()))?;
                }
                child
            };
            let old = self.instances[&child].placement.as_ref();
            if old.is_none_or(|old| old.matrix != placement.matrix) && !self.scripted.contains(&child) {
                vm.set(child, "_x", Value::Number(placement.matrix[4] as f64))?;
                vm.set(child, "_y", Value::Number(placement.matrix[5] as f64))?;
            }
            if !placement.name.is_empty() {
                vm.set(id, &placement.name, Value::Object(child))?;
            }
            self.instances.get_mut(&child).unwrap().placement = Some(placement);
            children.insert(depth, child);
        }
        for (depth, child) in &instance.children {
            if !children.contains_key(depth) {
                if let Some(name) = self
                    .instances
                    .get(child)
                    .and_then(|i| i.placement.as_ref())
                    .map(|p| p.name.clone())
                {
                    if !name.is_empty() {
                        vm.remove(id, &name);
                    }
                }
                self.remove(vm, *child);
            }
        }
        let current = self.instances.get_mut(&id).unwrap();
        // perf 10-18: a new frame or display list is a new draw (a frame's placements are fixed per child).
        if current.frame != frame || current.children != children {
            vm.touch();
        }
        current.frame = frame;
        current.children = children;
        vm.set(id, "_currentframe", Value::Number((frame + 1) as f64))?;
        vm.set(id, "_totalframes", Value::Number(frame_count as f64))?;
        for offset in actions {
            self.pending.push_back((id, offset));
        }
        Ok(())
    }
    pub fn advance(&mut self, vm: &mut Vm) -> Result<(), String> {
        let playing: Vec<_> = self
            .instances
            .iter()
            .filter(|(_, i)| i.playing)
            .map(|(id, i)| (*id, i.character, i.frame))
            .collect();
        for (id, character, frame) in playing {
            if !self.instances.contains_key(&id) {
                continue;
            }
            let count = self.characters[&character].frames.len();
            if count > 1 {
                self.seek(vm, id, (frame + 1) % count, 0)?;
            }
        }
        Ok(())
    }
    pub fn method(
        &mut self,
        vm: &mut Vm,
        id: usize,
        method: &str,
        args: &[Value],
    ) -> Result<bool, String> {
        if !self.instances.contains_key(&id) {
            return Ok(false);
        }
        match method {
            "stop" => self.instances.get_mut(&id).unwrap().playing = false,
            "play" => self.instances.get_mut(&id).unwrap().playing = true,
            "gotoAndPlay" | "gotoAndStop" => {
                let c = &self.characters[&self.instances[&id].character];
                let frame = match args.first().ok_or("APT goto requires frame")? {
                    Value::Text(label) => {
                        // Frame labels match without case, as in Flash: hudcountdown's Go! frame is "English" and
                        // FELanguage.GetLanguage answers "english".
                        let found = c.frames.iter().position(|f| {
                            f.controls.iter().any(|control| {
                                control.type_name == "frame_label"
                                    && control.label.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(label))
                            })
                        });
                        // The player ignores a goto to a label the clip lacks; Skate 2's HUD sends
                        // "intro" to one-frame clips (trickdisplay2 mcSwitch).
                        let Some(frame) = found else { return Ok(true) };
                        frame
                    }
                    value => {
                        let frame = value.number();
                        if !frame.is_finite() || frame < 0.0 {
                            return Err(format!(
                                "Invalid APT frame {frame} in {} on character {}",
                                method, c.id
                            ));
                        }
                        // The shipped line timer deliberately requests zero at
                        // full capacity. Frame zero addresses the first frame.
                        (frame as usize).saturating_sub(1)
                    }
                };
                self.seek(vm, id, frame, 0)?;
                self.instances.get_mut(&id).unwrap().playing = method == "gotoAndPlay";
            }
            "removeMovieClip" => self.detach(vm, id),
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// Each `FE_SOUND_<KEY>` name in `json`'s keys and strings, once per occurrence, in the order `json.to_string()`
/// spells them (a map's key before its value): what a scan of that text finds, without building it (the larger
/// screens are 1-5 MB of JSON). The game defines each as `_global.FE_SOUND_<KEY>` = `<key>` for the movies.
pub fn fe_sound_names(json: &serde_json::Value, found: &mut impl FnMut(&str) -> Result<(), String>) -> Result<(), String> {
    // A byte search: str::match_indices sets up a substring searcher per call, which dominated on the
    // hundreds of thousands of short strings a screen holds. Same matches (non-overlapping, left to right).
    fn scan(text: &str, found: &mut impl FnMut(&str) -> Result<(), String>) -> Result<(), String> {
        let bytes = text.as_bytes();
        let mut from = 0;
        while let Some(i) = bytes[from..].windows(9).position(|w| w[0] == b'F' && w == b"FE_SOUND_") {
            let at = from + i;
            let end = bytes[at..].iter().position(|b| !(b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_'));
            found(&text[at..end.map_or(bytes.len(), |n| at + n)])?;
            from = at + 9;
        }
        Ok(())
    }
    match json {
        serde_json::Value::String(text) => scan(text, found),
        serde_json::Value::Array(items) => items.iter().try_for_each(|item| fe_sound_names(item, found)),
        serde_json::Value::Object(map) => map.iter().try_for_each(|(key, value)| {
            scan(key, found)?;
            fe_sound_names(value, found)
        }),
        _ => Ok(()),
    }
}
