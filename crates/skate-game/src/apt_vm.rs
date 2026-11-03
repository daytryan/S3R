//! APT's compact ActionScript instruction stream. All execution is bounded;
//! native callbacks are supplied by the HUD owner, never by extracted scripts.
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Value {
    #[default]
    Undefined,
    Number(f64),
    Bool(bool),
    Text(String),
    Object(usize),
}
impl Value {
    pub fn number(&self) -> f64 {
        match self {
            Self::Number(n) => *n,
            Self::Bool(v) => {
                if *v {
                    1.0
                } else {
                    0.0
                }
            }
            Self::Text(s) => s.parse().unwrap_or(f64::NAN),
            _ => f64::NAN,
        }
    }
    pub fn truth(&self) -> bool {
        match self {
            Self::Undefined => false,
            Self::Number(n) => *n != 0.0 && !n.is_nan(),
            Self::Bool(v) => *v,
            Self::Text(s) => !s.is_empty(),
            Self::Object(_) => true,
        }
    }
    pub fn text(&self) -> String {
        match self {
            Self::Undefined => "undefined".into(),
            Self::Number(n) => n.to_string(),
            Self::Bool(v) => v.to_string(),
            Self::Text(s) => s.clone(),
            Self::Object(_) => "[object Object]".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Constant {
    pub kind: u32,
    pub value: serde_json::Value,
}
impl Constant {
    fn value(&self) -> Result<Value, String> {
        Ok(match self.kind {
            1 => Value::Text(
                self.value
                    .as_str()
                    .ok_or("Invalid APT string constant")?
                    .into(),
            ),
            5 => Value::Bool(self.value.as_u64().ok_or("Invalid APT bool constant")? != 0),
            6 | 7 => Value::Number(self.value.as_f64().ok_or("Invalid APT number constant")?),
            3 => Value::Undefined,
            _ => return Err(format!("Unresolved APT constant type {}", self.kind)),
        })
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct Parameter {
    pub register: usize,
    pub name: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Instruction {
    pub offset: u32,
    pub opcode: u8,
    #[serde(default)]
    pub next: u32,
    #[serde(default)]
    pub operand: serde_json::Value,
    pub target: Option<u32>,
    #[serde(default)]
    pub values: Vec<Constant>,
    #[serde(default)]
    pub body: Vec<Instruction>,
    #[serde(default)]
    pub flags: u32,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Debug)]
pub enum ObjectKind {
    Plain,
    Function(usize),
    Native(String),
}
#[derive(Clone, Debug)]
pub struct Object {
    pub kind: ObjectKind,
    pub fields: BTreeMap<String, Value>,
    pub prototype: Option<usize>,
}
#[derive(Clone, Debug)]
struct Function {
    code: Instruction,
    constants: Vec<Value>,
}

struct Scope {
    this: usize,
    local_definitions: bool,
    locals: BTreeMap<String, Value>,
}

pub trait Host {
    fn property_changed(&mut self, _vm: &mut Vm, _object: usize, _key: &str) -> Result<(), String> {
        Ok(())
    }
    fn call(
        &mut self,
        vm: &mut Vm,
        object: usize,
        method: &str,
        arguments: Vec<Value>,
    ) -> Result<Value, String>;
}
#[derive(Clone, Default)]
pub struct Vm {
    pub objects: Vec<Object>,
    free_objects: Vec<usize>,
    functions: Vec<Function>,
    pub global: usize,
    remaining: usize,
    /// Offset of the instruction last executed; names the site of a script error.
    last_offset: u64,
    random: u32,
    depth: usize,
    /// perf 10-18: changes whenever an object's fields, prototype or existence do (and the movie's clips, through
    /// apt_movie), to a value unique across every Vm: an unchanged revision is an unchanged draw (apt_scene::draw_key).
    pub revision: u64,
}
/// The last [`Vm::revision`] handed out.
static REVISIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// The first value of `fields` whose key equals `key` ignoring ASCII case, in key order. perf 10-18: only keys
/// starting with `key`'s first letter (either case) can match, so only those ranges are scanned; the same result.
fn find_ignoring_case<'a>(fields: &'a BTreeMap<String, Value>, key: &str) -> Option<&'a Value> {
    use std::ops::Bound::{Excluded, Included};
    static RANGES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let first = key.as_bytes().first().copied().unwrap_or(0x7f);
    // SKATE_VM_LOOKUP=0: the whole map, as before.
    if first >= 0x7f || !*RANGES.get_or_init(|| std::env::var_os("SKATE_VM_LOOKUP").is_none_or(|v| v != "0")) {
        return fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v);
    }
    let (upper, lower) = (first.to_ascii_uppercase(), first.to_ascii_lowercase());
    // Upper-case keys sort before lower-case ones, so the first match is the full scan's.
    for &b in &[upper, lower][..if upper == lower { 1 } else { 2 }] {
        let (from, to) = ([b], [b + 1]);
        let (from, to) = (std::str::from_utf8(&from).unwrap(), std::str::from_utf8(&to).unwrap());
        if let Some((_, v)) = fields.range::<str, _>((Included(from), Excluded(to))).find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            return Some(v);
        }
    }
    None
}
impl Vm {
    /// A new [`Self::revision`]: something drawn may have changed.
    pub fn touch(&mut self) {
        self.revision = REVISIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    }
    fn variable(&self, scope: &Scope, key: &str) -> Value {
        if let Some(value) = scope.locals.get(key) {
            return value.clone();
        }
        let value = self.get(scope.this, key);
        if value != Value::Undefined {
            return value;
        }
        self.get(self.global, key)
    }
    pub fn new() -> Self {
        let mut vm = Self::default();
        vm.global = vm.object(ObjectKind::Plain);
        vm
    }
    pub fn object(&mut self, kind: ObjectKind) -> usize {
        self.touch();
        if let Some(id) = self.free_objects.pop() {
            self.objects[id] = Object {
                kind,
                fields: BTreeMap::new(),
                prototype: None,
            };
            return id;
        }
        let id = self.objects.len();
        self.objects.push(Object {
            kind,
            fields: BTreeMap::new(),
            prototype: None,
        });
        id
    }
    /// Collect only between updates. Host-owned movie handles are explicit
    /// roots; script references and prototypes preserve retired clips when
    /// they remain reachable. Stable handles never move during collection.
    pub fn collect(&mut self, host_roots: impl IntoIterator<Item = usize>) -> Result<(), String> {
        if self.depth != 0 {
            return Err("Cannot collect during an APT call".into());
        }
        let mut marked = vec![false; self.objects.len()];
        let mut pending: Vec<_> = host_roots.into_iter().collect();
        pending.push(self.global);
        while let Some(id) = pending.pop() {
            let seen = marked.get_mut(id).ok_or("Invalid APT collection root")?;
            if *seen {
                continue;
            }
            *seen = true;
            let object = &self.objects[id];
            if let Some(id) = object.prototype {
                pending.push(id);
            }
            for value in object.fields.values() {
                if let Value::Object(id) = value {
                    pending.push(*id);
                }
            }
        }
        let freed = self.free_objects.len();
        self.free_objects.clear();
        for (id, live) in marked.into_iter().enumerate() {
            if !live {
                self.objects[id] = Object {
                    kind: ObjectKind::Plain,
                    fields: BTreeMap::new(),
                    prototype: None,
                };
                self.free_objects.push(id);
            }
        }
        if self.free_objects.len() != freed {
            self.touch();
        }
        Ok(())
    }
    pub fn set(
        &mut self,
        object: usize,
        key: impl Into<String>,
        value: Value,
    ) -> Result<(), String> {
        let key = key.into();
        let fields = &mut self.objects.get_mut(object).ok_or("Invalid APT object handle")?.fields;
        // Array semantics: storing past the end grows `length` (menu_picker's
        // SetItemText writes mItemTextArray[index] directly).
        let mut grew = false;
        if let (Ok(index), Some(Value::Number(length))) = (key.parse::<usize>(), fields.get("length")) {
            if index as f64 >= *length {
                fields.insert("length".into(), Value::Number(index as f64 + 1.));
                grew = true;
            }
        }
        // perf 10-18: an equal store keeps the revision.
        match fields.get_mut(&key) {
            Some(old) if *old == value && !grew => return Ok(()),
            Some(old) if *old == value => {}
            Some(old) => *old = value,
            None => {
                fields.insert(key, value);
            }
        }
        self.touch();
        Ok(())
    }
    /// A script's store (SetMember / SetVariable), told to the host. Flash ignores an undefined or non-finite store to
    /// a clip's transform property and the clip keeps its value (Ruffle's property_coerce_to_number); without it a
    /// NaN _x stops the whole screen's draw (apt_scene's nonfinite check). Retail APT: UNTRACED, Flash's rule taken.
    pub(crate) fn script_set<H: Host + ?Sized>(&mut self, host: &mut H, object: usize, key: &str, value: Value) -> Result<(), String> {
        let transform = matches!(key, "_x" | "_y" | "_xscale" | "_yscale" | "_width" | "_height" | "_rotation" | "_alpha");
        let clip = matches!(self.objects.get(object).map(|o| &o.kind), Some(ObjectKind::Native(n)) if n.starts_with("movie:"));
        // ponytail: an Object is dropped, where Flash would coerce it through valueOf; add that if a screen needs it.
        if transform && clip && !value.number().is_finite() {
            return Ok(());
        }
        self.set(object, key, value)?;
        host.property_changed(self, object, key)
    }
    pub fn get(&self, object: usize, key: &str) -> Value {
        self.get_ref(object, key).cloned().unwrap_or_default()
    }
    /// [`Self::get`] without the clone (None: undefined).
    pub fn get_ref(&self, object: usize, key: &str) -> Option<&Value> {
        let mut current = Some(object);
        for _ in 0..64 {
            let Some(o) = current.and_then(|i| self.objects.get(i)) else {
                break;
            };
            if let Some(v) = o.fields.get(key) {
                return Some(v);
            }
            current = o.prototype;
        }
        // APT ActionScript resolves names case-insensitively, as Flash 6 did: the
        // shipped scripts address "mMenuPicker" as "mMenupicker", "LABELTEXT" as
        // "labeltext". Exact matches win; this only runs on a miss.
        let mut current = Some(object);
        for _ in 0..64 {
            let Some(o) = current.and_then(|i| self.objects.get(i)) else {
                break;
            };
            if let Some(v) = find_ignoring_case(&o.fields, key) {
                return Some(v);
            }
            current = o.prototype;
        }
        None
    }
    /// Sets `object`'s prototype (a new revision).
    pub fn set_prototype(&mut self, object: usize, prototype: usize) -> Result<(), String> {
        self.objects.get_mut(object).ok_or("Invalid APT object handle")?.prototype = Some(prototype);
        self.touch();
        Ok(())
    }
    /// Removes `object`'s own field `key`; whether it had one (then a new revision).
    pub fn remove(&mut self, object: usize, key: &str) -> bool {
        let removed = self.objects.get_mut(object).is_some_and(|o| o.fields.remove(key).is_some());
        if removed {
            self.touch();
        }
        removed
    }
    pub fn begin_update(&mut self) {
        self.remaining = 100_000;
    }
    pub fn construct(
        &mut self,
        class: usize,
        args: Vec<Value>,
        host: &mut impl Host,
    ) -> Result<usize, String> {
        let id = self.object(ObjectKind::Plain);
        if let Value::Object(proto) = self.get(class, "prototype") {
            self.set_prototype(id, proto)?;
        }
        if let ObjectKind::Function(f) = self.objects[class].kind.clone() {
            self.invoke(f, id, args, host)?;
        }
        Ok(id)
    }
    /// Registered-class construction of an existing clip (Object.registerClass):
    /// the clip adopts the class prototype, then the constructor runs on it.
    pub fn construct_on(
        &mut self,
        class: usize,
        instance: usize,
        host: &mut impl Host,
    ) -> Result<(), String> {
        if let Value::Object(proto) = self.get(class, "prototype") {
            self.set_prototype(instance, proto)?;
        }
        if let Some(ObjectKind::Function(f)) = self.objects.get(class).map(|o| o.kind.clone()) {
            self.invoke(f, instance, vec![], host)
                .map_err(|e| format!("{e} (last instruction {:x})", self.last_offset))?;
        }
        Ok(())
    }
    pub fn run(&mut self, code: &[Instruction], host: &mut impl Host) -> Result<Value, String> {
        self.begin_update();
        self.run_on(self.global, code, host)
    }
    pub fn run_on(
        &mut self,
        object: usize,
        code: &[Instruction],
        host: &mut impl Host,
    ) -> Result<Value, String> {
        self.execute(
            code,
            &mut vec![Value::Undefined; 256],
            &mut Vec::new(),
            &mut Scope {
                this: object,
                local_definitions: false,
                locals: BTreeMap::new(),
            },
            host,
        )
        .map_err(|e| format!("{e} (last instruction {:x})", self.last_offset))
    }
    pub fn call_method(
        &mut self,
        object: usize,
        method: &str,
        args: Vec<Value>,
        host: &mut impl Host,
    ) -> Result<Value, String> {
        if let Value::Object(f) = self.get(object, method) {
            if let Some(Object {
                kind: ObjectKind::Function(index),
                ..
            }) = self.objects.get(f)
            {
                return self.invoke(*index, object, args, host);
            }
        }
        host.call(self, object, method, args)
    }
    fn invoke(
        &mut self,
        index: usize,
        this: usize,
        args: Vec<Value>,
        host: &mut impl Host,
    ) -> Result<Value, String> {
        if self.depth >= 32 {
            return Err("APT call-depth limit".into());
        }
        let f = self
            .functions
            .get(index)
            .ok_or("Invalid APT function")?
            .clone();
        let mut regs = vec![Value::Undefined; 256];
        let mut reg = 1;
        let mut scope = Scope {
            this,
            local_definitions: true,
            locals: BTreeMap::new(),
        };
        if f.code.flags & 2 == 0 {
            scope.locals.insert("this".into(), Value::Object(this));
        }
        let root = match self.get(this, "_root") {
            Value::Undefined => Value::Object(self.global),
            value => value,
        };
        let parent = self.get(this, "_parent");
        let arguments = if f.code.flags & 4 != 0 || f.code.flags & 8 == 0 {
            let object = self.object(ObjectKind::Plain);
            for (index, value) in args.iter().enumerate() {
                self.set(object, index.to_string(), value.clone())?;
            }
            self.set(object, "length", Value::Number(args.len() as f64))?;
            Value::Object(object)
        } else {
            Value::Undefined
        };
        if f.code.flags & 8 == 0 {
            scope.locals.insert("arguments".into(), arguments.clone());
        }
        // DefineFunction2 preload flags (82E47688). Native integer registers
        // are scoped to a call, unlike persistent display-object properties.
        for (flag, value) in [
            (1, Value::Object(this)),
            (4, arguments),
            (16, Value::Undefined),
            (64, root),
            (128, parent),
            (256, Value::Object(self.global)),
        ] {
            if f.code.flags & flag != 0 {
                regs[reg] = value;
                reg += 1;
            }
        }
        for (i, p) in f.code.parameters.iter().enumerate() {
            if p.register >= regs.len() {
                return Err("APT parameter register outside bank".into());
            }
            if p.register != 0 {
                regs[p.register] = args.get(i).cloned().unwrap_or_default();
            } else {
                scope
                    .locals
                    .insert(p.name.clone(), args.get(i).cloned().unwrap_or_default());
            }
        }
        self.depth += 1;
        let result = self.execute(
            &f.code.body,
            &mut regs,
            &mut f.constants.clone(),
            &mut scope,
            host,
        );
        self.depth -= 1;
        result
    }
    fn execute(
        &mut self,
        code: &[Instruction],
        regs: &mut Vec<Value>,
        constants: &mut Vec<Value>,
        scope: &mut Scope,
        host: &mut impl Host,
    ) -> Result<Value, String> {
        let mut stack = Vec::<Value>::new();
        let mut pc = 0;
        fn pop(s: &mut Vec<Value>) -> Result<Value, String> {
            s.pop().ok_or("APT stack underflow".into())
        }
        while let Some(i) = code.get(pc) {
            if self.remaining == 0 || self.objects.len() > 32768 {
                return Err("APT execution/object budget exceeded".into());
            }
            self.remaining -= 1;
            self.last_offset = i.offset as u64;
            pc += 1;
            let op = i.opcode;
            let operand = i.operand.as_u64().unwrap_or(0) as usize;
            let constant = |k: usize| {
                constants
                    .get(k)
                    .cloned()
                    .ok_or_else(|| format!("APT constant index {k} at {:x}", i.offset))
            };
            match op {
                0 => break,
                0x70 => stack.push(Value::Object(scope.this)),
                0xa1 => stack.push(Value::Text(
                    i.operand
                        .as_str()
                        .ok_or("APT string operand missing")?
                        .into(),
                )),
                0xa4 => stack.push(self.variable(
                    scope,
                    i.operand.as_str().ok_or("APT variable operand missing")?,
                )),
                // EA_GETSTRINGMEMBER: member lookup with an inline string name.
                0xa5 => {
                    let name = i.operand.as_str().ok_or("APT member operand missing")?;
                    let obj = pop(&mut stack)?;
                    stack.push(if let Value::Object(id) = obj {
                        self.get(id, name)
                    } else {
                        Value::Undefined
                    });
                }
                // trace(): authored debug output only.
                0x26 => {
                    let value = pop(&mut stack)?;
                    bevy::log::debug!("APT trace: {}", value.text());
                }
                0x06 | 0x07 => {
                    host.call(
                        self,
                        scope.this,
                        if op == 6 { "play" } else { "stop" },
                        vec![],
                    )?;
                }
                // ActionGotoFrame (0-based frame; the clip stops there, a following Play resumes it). Skate 1's
                // trick display compiles gotoAndPlay(n) on its own timeline to this plus 0x06.
                0x81 => {
                    host.call(self, scope.this, "gotoAndStop", vec![Value::Number(operand as f64 + 1.)])?;
                }
                // ActionGotoLabel, the same by frame label (Skate 1: "pulse").
                0x8c => {
                    let label = i.operand.as_str().ok_or("APT goto label operand missing")?;
                    host.call(self, scope.this, "gotoAndStop", vec![Value::Text(label.into())])?;
                }
                // Pop tolerates an empty stack, as in Flash: shipped class guards
                // (`if (!_global.X) {...}`) branch straight to a trailing pop.
                0x17 => {
                    stack.pop();
                }
                0x12 => {
                    let a = pop(&mut stack)?;
                    stack.push(Value::Bool(!a.truth()));
                }
                // ActionToInteger (`int(x)`): truncates; NaN is 0 (Skate 2's homscoring rounds its score with it).
                0x18 => {
                    let a = pop(&mut stack)?.number();
                    stack.push(Value::Number(if a.is_nan() { 0. } else { a.trunc() }));
                }
                0x4c => stack.push(
                    stack
                        .last()
                        .cloned()
                        .ok_or("APT duplicate on empty stack")?,
                ),
                0x59 => stack.push(Value::Number(0.0)),
                0x5a => stack.push(Value::Number(1.0)),
                0x71 => stack.push(Value::Object(self.global)),
                0x73 => stack.push(Value::Bool(true)),
                0x74 => stack.push(Value::Bool(false)),
                0x75 | 0x76 => stack.push(Value::Undefined),
                0xb5 => stack.push(Value::Number((operand as u8 as i8) as f64)),
                0xb6 => stack.push(Value::Number((operand as u16 as i16) as f64)),
                0xb7 => stack.push(Value::Number((operand as u32 as i32) as f64)),
                0xb4 => stack.push(Value::Number(f32::from_bits(operand as u32) as f64)),
                0xb9 => stack.push(
                    regs.get(operand)
                        .cloned()
                        .ok_or("APT register outside bank")?,
                ),
                0x87 => {
                    *regs.get_mut(operand).ok_or("APT register outside bank")? = stack
                        .last()
                        .cloned()
                        .ok_or("APT register store on empty stack")?;
                }
                0x88 => {
                    *constants = i
                        .values
                        .iter()
                        .map(Constant::value)
                        .collect::<Result<_, _>>()?;
                }
                0x96 => {
                    for c in &i.values {
                        // Kind 4 is a register read (SWF ActionPush), which Skate 1's compiler uses.
                        stack.push(if c.kind == 4 {
                            let register = c.value.as_u64().ok_or("Invalid APT register constant")? as usize;
                            regs.get(register).cloned().ok_or("APT register outside bank")?
                        } else {
                            c.value()?
                        });
                    }
                }
                0xa2 | 0xa3 => stack.push(constant(operand)?),
                0xae => stack.push(self.variable(scope, &constant(operand)?.text())),
                0x4e | 0xaf => {
                    let name = if op == 0xaf {
                        constant(operand)?
                    } else {
                        pop(&mut stack)?
                    }
                    .text();
                    let obj = pop(&mut stack)?;
                    stack.push(match obj {
                        Value::Object(id) => self.get(id, &name),
                        Value::Text(s) if name == "length" => Value::Number(s.chars().count() as f64),
                        _ => Value::Undefined,
                    });
                }
                0x4f => {
                    let v = pop(&mut stack)?;
                    let k = pop(&mut stack)?.text();
                    let o = pop(&mut stack)?;
                    if let Value::Object(id) = o {
                        self.script_set(host, id, &k, v)?;
                    }
                }
                0x1c => {
                    let k = pop(&mut stack)?.text();
                    let mut value = self.variable(scope, &k);
                    // A dotted target path (nucleus_tos finds its line fields as "mcTOSText.txtBox" + i).
                    if value == Value::Undefined && k.contains('.') {
                        let mut parts = k.split('.');
                        value = parts.next().map_or(Value::Undefined, |first| self.variable(scope, first));
                        for part in parts {
                            value = match value {
                                Value::Object(o) => self.get(o, part),
                                _ => Value::Undefined,
                            };
                        }
                    }
                    stack.push(value);
                }
                // 0x72: EA's zero-a-variable, `pushc 'index'; 0x72` = `index = 0` (Skate 3's music player loops).
                0x1d | 0x3c | 0x72 => {
                    let v = if op == 0x72 { Value::Number(0.) } else { pop(&mut stack)? };
                    let k = pop(&mut stack)?.text();
                    if (op == 0x3c && scope.local_definitions) || scope.locals.contains_key(&k) {
                        scope.locals.insert(k, v);
                    } else {
                        self.script_set(host, scope.this, &k, v)?;
                    }
                }
                0x3a => {
                    let k = pop(&mut stack)?.text();
                    let o = pop(&mut stack)?;
                    let existed = if let Value::Object(id) = o {
                        self.remove(id, &k)
                    } else {
                        false
                    };
                    stack.push(Value::Bool(existed));
                }
                // Delete2: the name off the scope chain (roundresults' `delete row`).
                0x3b => {
                    let k = pop(&mut stack)?.text();
                    let existed = scope.locals.remove(&k).is_some()
                        || [scope.this, self.global].into_iter().any(|o| self.remove(o, &k));
                    stack.push(Value::Bool(existed));
                }
                0x47 | 0x0b | 0x0c | 0x0d | 0x48 | 0x49 | 0x66 | 0x67 => {
                    let b = pop(&mut stack)?;
                    let a = pop(&mut stack)?;
                    stack.push(match op {
                        0x47 if matches!(a, Value::Text(_)) || matches!(b, Value::Text(_)) => {
                            Value::Text(a.text() + &b.text())
                        }
                        0x47 => Value::Number(a.number() + b.number()),
                        0x0b => Value::Number(a.number() - b.number()),
                        0x0c => Value::Number(a.number() * b.number()),
                        0x0d => Value::Number(a.number() / b.number()),
                        0x48 => Value::Bool(a.number() < b.number()),
                        0x67 => Value::Bool(a.number() > b.number()),
                        0x66 => Value::Bool(a == b),
                        _ => Value::Bool(a == b || a.number() == b.number()),
                    });
                }
                // InitArray / InitObject (SWF semantics: count, then elements or value/name pairs).
                0x42 | 0x43 => {
                    let n = pop(&mut stack)?.number() as usize;
                    if n > 1024 {
                        return Err("APT literal limit".into());
                    }
                    let id = self.object(ObjectKind::Plain);
                    for j in 0..n {
                        let value = pop(&mut stack)?;
                        let key = if op == 0x42 { j.to_string() } else { pop(&mut stack)?.text() };
                        self.set(id, key, value)?;
                    }
                    if op == 0x42 {
                        self.set(id, "length", Value::Number(n as f64))?;
                    }
                    stack.push(Value::Object(id));
                }
                0x3f => {
                    let b = pop(&mut stack)?.number();
                    let a = pop(&mut stack)?.number();
                    stack.push(Value::Number(a % b));
                }
                0x4a => {
                    let a = pop(&mut stack)?.number();
                    stack.push(Value::Number(a));
                }
                // ToString (Skate 2's hudobjective2).
                0x4b => {
                    let a = pop(&mut stack)?.text();
                    stack.push(Value::Text(a));
                }
                // TypeOf, as Flash names the types (Skate 3's endchallenge_online).
                0x44 => {
                    let name = match pop(&mut stack)? {
                        Value::Undefined => "undefined",
                        Value::Number(_) => "number",
                        Value::Bool(_) => "boolean",
                        Value::Text(_) => "string",
                        Value::Object(id) => match &self.objects[id].kind {
                            ObjectKind::Function(_) => "function",
                            ObjectKind::Native(n) if n.starts_with("movie:") => "movieclip",
                            _ => "object",
                        },
                    };
                    stack.push(Value::Text(name.into()));
                }
                // BitRShift (Skate 2's challenge_signup).
                0x64 => {
                    let b = pop(&mut stack)?.number() as i64 as u32 & 31;
                    let a = pop(&mut stack)?.number() as i64 as i32;
                    stack.push(Value::Number((a >> b) as f64));
                }
                // ImplementsOp: interfaces only type-check in Flash; nothing to record (Skate 2's nis_overlay).
                0x2c => {
                    pop(&mut stack)?;
                    let n = pop(&mut stack)?.number().max(0.) as usize;
                    for _ in 0..n.min(256) {
                        pop(&mut stack)?;
                    }
                }
                // random(n): presentation only; a fixed sequence keeps runs reproducible.
                0x30 => {
                    let n = pop(&mut stack)?.number().max(0.);
                    self.random = self.random.wrapping_mul(1664525).wrapping_add(1013904223);
                    stack.push(Value::Number(((self.random >> 8) as f64 / 16777216. * n).floor()));
                }
                0x50 | 0x51 => {
                    let a = pop(&mut stack)?.number();
                    stack.push(Value::Number(a + if op == 0x50 { 1.0 } else { -1.0 }));
                }
                0x99 | 0x9d | 0xb8 => {
                    let jump = op == 0x99 || {
                        let v = pop(&mut stack)?.truth();
                        if op == 0xb8 { !v } else { v }
                    };
                    if jump {
                        let target = i.target.ok_or("APT branch lacks target")?;
                        if target == code.last().map_or(0, |x| x.next) {
                            break;
                        }
                        pc = code
                            .iter()
                            .position(|x| x.offset == target)
                            .ok_or_else(|| format!("APT invalid jump target {target:x}"))?;
                    }
                }
                0x8e | 0x9b => {
                    let index = self.functions.len();
                    self.functions.push(Function {
                        code: i.clone(),
                        constants: constants.clone(),
                    });
                    let object = self.object(ObjectKind::Function(index));
                    let proto = self.object(ObjectKind::Plain);
                    self.set(object, "prototype", Value::Object(proto))?;
                    if i.name.is_empty() {
                        stack.push(Value::Object(object));
                    } else {
                        self.set(self.global, &i.name, Value::Object(object))?;
                    }
                }
                0x69 => {
                    let super_class = pop(&mut stack)?;
                    let sub_class = pop(&mut stack)?;
                    if let (Value::Object(a), Value::Object(b)) = (sub_class, super_class) {
                        if let (Value::Object(ap), Value::Object(bp)) =
                            (self.get(a, "prototype"), self.get(b, "prototype"))
                        {
                            self.set_prototype(ap, bp)?;
                        }
                    }
                }
                0x40 => {
                    let name = pop(&mut stack)?.text();
                    let n = pop(&mut stack)?.number() as usize;
                    if n > 256 {
                        return Err("APT argument limit".into());
                    }
                    let args = (0..n)
                        .map(|_| pop(&mut stack))
                        .collect::<Result<Vec<_>, _>>()?;
                    let id = self.object(ObjectKind::Plain);
                    if name == "TextFormat" {
                        // Plain property bag; setTextFormat reads it (host).
                    } else if name == "Array" {
                        for (j, v) in args.into_iter().enumerate() {
                            self.set(id, j.to_string(), v)?;
                        }
                        self.set(id, "length", Value::Number(n as f64))?;
                    } else if let Value::Object(class) = self.get(self.global, &name) {
                        if let Value::Object(proto) = self.get(class, "prototype") {
                            self.set_prototype(id, proto)?;
                        }
                        if let ObjectKind::Function(f) = self.objects[class].kind.clone() {
                            self.invoke(f, id, args, host)?;
                        }
                    } else {
                        return Err(format!("APT constructor absent: {name}"));
                    }
                    stack.push(Value::Object(id));
                }
                0x52 | 0xb0 | 0xb1 | 0xb2 | 0xb3 | 0x3d | 0x5d => {
                    let method = if matches!(op, 0xb0 | 0xb1 | 0xb2 | 0xb3) {
                        constant(operand)?
                    } else {
                        pop(&mut stack)?
                    };
                    let object = if matches!(op, 0x3d | 0xb0 | 0xb1) {
                        Value::Object(self.global)
                    } else {
                        pop(&mut stack)?
                    };
                    let count = pop(&mut stack)?.number() as usize;
                    if count > 256 {
                        return Err("APT argument limit".into());
                    }
                    let args = (0..count)
                        .map(|_| pop(&mut stack))
                        .collect::<Result<Vec<_>, _>>()?;
                    let value = match (object, method.text().as_str()) {
                        (Value::Object(id), method) => self.call_method(id, method, args, host)?,
                        // String.substr(start, length): dpo turns "mini_x" into "challenge_x".
                        (Value::Text(s), "substr") => {
                            let chars: Vec<char> = s.chars().collect();
                            let n = chars.len() as f64;
                            let start = args.first().map_or(0., Value::number);
                            let start = (if start < 0. { (n + start).max(0.) } else { start.min(n) }) as usize;
                            Value::Text(chars[start..].iter().take(args.get(1).map_or(n, Value::number).max(0.) as usize).collect())
                        }
                        // String.substring(start, end): clamped, swapped when reversed (Skate 3's endchallenge_online
                        // turns a "#3" cell into the letters frame "3").
                        (Value::Text(s), "substring") => {
                            let chars: Vec<char> = s.chars().collect();
                            let n = chars.len();
                            let at = |k: Option<&Value>, or: usize| k.map_or(or, |v| (v.number().max(0.) as usize).min(n));
                            let (a, b) = (at(args.first(), 0), at(args.get(1), n));
                            Value::Text(chars[a.min(b)..a.max(b)].iter().collect())
                        }
                        _ => Value::Undefined,
                    };
                    if matches!(op, 0xb1 | 0xb3) {
                        return Ok(value);
                    }
                    if !matches!(op, 0xb0 | 0xb2 | 0x5d) {
                        stack.push(value);
                    }
                }
                0x3e => return Ok(stack.pop().unwrap_or_default()),
                // GETURL: two inline strings, [url, target] (native relocation 82E67DF4 fixes up both
                // words); no stack effect. main.apt's first frame loads its level movies with it
                // (`getURL('source/controls/autosave.swf', '_level2')`); the host composes those itself.
                0x83 => {
                    if i.operand.get(1).and_then(serde_json::Value::as_str).is_none() {
                        return Err(format!("APT getURL operand missing at {:x}", i.offset));
                    }
                }
                _ => return Err(format!("Unsupported APT opcode {op:02x} at {:x}", i.offset)),
            }
            if stack.len() > 1024 {
                return Err("APT stack limit".into());
            }
        }
        Ok(Value::Undefined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_free_lookup_matches_a_full_scan() {
        let keys = ["", "_x", "_X", "A", "a", "ab", "AB", "aB", "b", "{", "z~", "Z~", "1", "é", "É", "mRenderComponent", "mrendercomponent"];
        let fields: BTreeMap<String, Value> = keys.iter().enumerate().map(|(i, k)| (k.to_string(), Value::Number(i as f64))).collect();
        for key in keys.iter().copied().chain(["Ab", "_Y", "", "MRENDERCOMPONENT", "c", "~", "Z~"]) {
            let full = fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v);
            assert_eq!(find_ignoring_case(&fields, key), full, "{key:?}");
        }
    }

    struct NoHost;
    impl Host for NoHost {
        fn call(&mut self, _: &mut Vm, _: usize, _: &str, _: Vec<Value>) -> Result<Value, String> {
            Ok(Value::Undefined)
        }
    }

    /// Preview 4 (10-15): menupicker's PrepareTransition divides 0 by 0 for an item at the stage centre and writes the
    /// NaN to its _x; Flash keeps the clip's value (a non-finite or undefined transform store is ignored).
    #[test]
    fn a_clip_ignores_a_non_finite_transform() {
        let mut vm = Vm::default();
        let clip = vm.object(ObjectKind::Native("movie:5".into()));
        let plain = vm.object(ObjectKind::Plain);
        vm.set(clip, "_x", Value::Number(3.)).unwrap();
        for v in [Value::Number(f64::NAN), Value::Number(f64::INFINITY), Value::Undefined] {
            vm.script_set(&mut NoHost, clip, "_x", v).unwrap();
        }
        assert!(matches!(vm.get(clip, "_x"), Value::Number(n) if n == 3.));
        vm.script_set(&mut NoHost, clip, "_x", Value::Number(7.)).unwrap();
        assert!(matches!(vm.get(clip, "_x"), Value::Number(n) if n == 7.));
        vm.script_set(&mut NoHost, plain, "_x", Value::Number(f64::NAN)).unwrap();
        assert!(matches!(vm.get(plain, "_x"), Value::Number(n) if n.is_nan()), "a plain object keeps what it is given");
        vm.script_set(&mut NoHost, clip, "vectorx", Value::Number(f64::NAN)).unwrap();
        assert!(matches!(vm.get(clip, "vectorx"), Value::Number(n) if n.is_nan()), "only the transform properties");
        // Review 10-15 minor 10: text that isn't a number is ignored too; numeric text and booleans still land.
        vm.script_set(&mut NoHost, clip, "_x", Value::Text("abc".into())).unwrap();
        assert!(matches!(vm.get(clip, "_x"), Value::Number(n) if n == 7.));
        vm.script_set(&mut NoHost, clip, "_y", Value::Text("12".into())).unwrap();
        assert_eq!(vm.get(clip, "_y").number(), 12.);
        vm.script_set(&mut NoHost, clip, "_alpha", Value::Bool(true)).unwrap();
        assert_eq!(vm.get(clip, "_alpha").number(), 1.);
    }

    /// Skate 1's HUD scripts push a register with a kind-4 Push value (SWF ActionPush's register type).
    #[test]
    fn push_kind_4_reads_a_register() {
        let code: Vec<Instruction> = serde_json::from_value(serde_json::json!([
            {"offset": 0, "opcode": 150, "values": [{"kind": 7, "value": 7}]},
            {"offset": 1, "opcode": 135, "operand": 3},
            {"offset": 2, "opcode": 150, "values": [{"kind": 7, "value": 9}]},
            {"offset": 3, "opcode": 150, "values": [{"kind": 4, "value": 3}]},
            {"offset": 4, "opcode": 62}
        ]))
        .unwrap();
        let value = Vm::default().run(&code, &mut NoHost).unwrap();
        assert!(matches!(value, Value::Number(n) if n == 7.0), "{value:?}");
    }

    /// main.apt frame 0 (decoded by tools/vendor/skate3_ui/actions.py): GETURL takes [url, target] and
    /// leaves the stack alone, so the value pushed before it is still returned.
    #[test]
    fn get_url_runs_the_level_load() {
        let code: Vec<Instruction> = serde_json::from_value(serde_json::json!([
            {"offset": 0, "opcode": 150, "values": [{"kind": 7, "value": 5}]},
            {"offset": 1, "opcode": 0x83, "operand": ["source/controls/autosave.swf", "_level2"]},
            {"offset": 9, "opcode": 0x83, "operand": ["source/screens/main/fmv_popup.swf", "_level3"]},
            {"offset": 17, "opcode": 62}
        ]))
        .unwrap();
        let mut vm = Vm::new();
        let value = vm.run(&code, &mut NoHost).unwrap();
        assert_eq!(value, Value::Number(5.));
        let broken: Vec<Instruction> =
            serde_json::from_value(serde_json::json!([{"offset": 4, "opcode": 0x83, "operand": ["only-url"]}])).unwrap();
        assert!(Vm::new().run(&broken, &mut NoHost).is_err(), "a GETURL without its target is a decode error");
    }
}
