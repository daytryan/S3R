//! Skate 3's challenge objectives (TU3 `cChallengeObjectivesManager`, 0x82c5f000..0x82c77000) as data: each
//! objective's conditions are the disc's Lua snippets, turned into an expression tree by tools/skate3_online.py and
//! interpreted here ([`Snippet`]); no Lua runs in the game. A [`Group`] is one skater's objective group
//! (Objectives.MakeObjectivesFromGroup): its statuses, the events that evaluate them, the points objective and the
//! messages the challenge graph hears (objectivecomplete, allobjectivecomplete, didnotgetenoughpoints, ...).
//!
//! The runtime, as traced:
//! - eObjectiveStatus (0x82c62e88): Disabled 1, Enabled 2, Active 4, Complete 8, Failed 16 (objective+1272).
//! - Each game message a group hears is a trigger byte (0x82c702c0); an objective is evaluated by an event when its
//!   ObjectiveTriggers byte for it is 1 (matcher 0x82c6fbb8; bytes 0..10, 12, 13). Byte 4 is the second pass a handler
//!   runs after a pass that completed something.
//! - Process 0x82c6e4b0: every Active objective whose triggers match is evaluated (0x82c752a8: failures first when
//!   its fail-first flag is set, else the objective first); a completion marks the objective (a master objective marks
//!   them all) and posts ObjectiveComplete; a failure posts ObjectiveFailed; then AllObjectiveComplete when every
//!   objective is complete, or DidNotGetEnoughPoints when only the points objective is left after a second pass.
//! - The snippets run as `function h( vars ) <src> end` (0x820b88f4), `vars` one table per objective kept until the
//!   objectives reset (0x82c75c48); a boolean result is the answer, anything else (or a Lua error) the handler's
//!   default (execute 0x82c75ec8): Obj true, PointsValid false, Failure false.
//! - The objectives' Lua state uses single-precision numbers (GetGroupAccumulatedPoints 0x82c65188 pushes `frsp` /
//!   `stfs`): [`Value::Num`] is an f32.
use crate::skate3_present::{Event, Group as Presentation};
use serde::Deserialize;
use serde_json::Value as Json;
use std::{collections::HashMap, sync::{Arc, Mutex}};

pub(crate) const DISABLED: u8 = 1;
pub(crate) const ENABLED: u8 = 2;
pub(crate) const ACTIVE: u8 = 4;
pub(crate) const COMPLETE: u8 = 8;
pub(crate) const FAILED: u8 = 16;

/// Trigger bytes (registration 0x82c702c0).
pub(crate) const ON_SEQUENCE: usize = 1;
pub(crate) const ON_TRIGGER: usize = 2;
pub(crate) const SECOND_PASS: usize = 4;
pub(crate) const ON_MARKER: usize = 5;
pub(crate) const ON_TICK: usize = 12;

/// The entered-volume list's length (0x82c74290: the oldest goes past 32), and the trick list's (0x82c74a20).
const ENTERED_CAP: usize = 32;
const TRICKS_CAP: usize = 32;
/// The group's sequence trick list (+408, 0x82c72160: no push at 12) and unique trick list (+128, 0x82c6db50: none
/// past 32).
const SEQUENCE_TRICKS_CAP: usize = 12;
const UNIQUE_CAP: usize = 32;

// ---------------------------------------------------------------- data (tools/skate3_online.py)
#[derive(Deserialize, Clone, Debug)]
pub(crate) struct SnippetData {
    /// The snippet's main.bin offset (scripts/objectives.json keys its text by it).
    pub src: u32,
    pub tree: Json,
    #[serde(default)]
    pub natives: Vec<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub(crate) struct ObjectiveDef {
    pub key: String,
    /// eTriggerActivationMode (getter 0x82c5fa58).
    pub activation: u32,
    pub triggers: Vec<u8>,
    pub fail_first: bool,
    /// MasterObjective (ctor 0x82c73908 -> objective+260): completing it completes every objective.
    pub master: bool,
    pub requires_highlight: bool,
    pub obj: Option<SnippetData>,
    pub points_valid: Option<SnippetData>,
    #[serde(default)]
    pub failures: Vec<Option<SnippetData>>,
    /// The unnamed Int16 2B245EC6D20A64AA: over 1, GetTotalNumCompletedObjectives counts the objective only on its
    /// argument 2 (0x82c6f5a8).
    #[serde(default)]
    pub count_kind: i16,
    /// DisplayInHUD (014F34B886D669A4, absent: true): GetNumCompletedObjectivesForScoring counts only these.
    #[serde(default = "yes")]
    pub display_in_hud: bool,
    /// The Hash_ECAEB8E619A68AF3 failure records, which Evaluate runs as hints (0x82c755b8, stage1_traces 4.2-4.3).
    #[serde(default)]
    pub hints: Vec<Option<SnippetData>>,
    /// The presentation groups the objective ctor 0x82c726e8 builds (plan Task 6's `groups`).
    #[serde(default)]
    pub groups: ObjectiveGroups,
}

fn yes() -> bool {
    true
}

/// ObjIntroEvents (+0xf78), ObjCompleteEvents (+0xf7c) and a group per hint (+0xee8). INFERRED: the export writes an
/// absent group and an empty one alike, `[]`.
#[derive(Deserialize, Clone, Debug, Default)]
pub(crate) struct ObjectiveGroups {
    #[serde(default)]
    pub intro: Vec<Event>,
    #[serde(default)]
    pub complete: Vec<Event>,
    #[serde(default)]
    pub hints: Vec<Vec<Event>>,
}

#[derive(Deserialize, Clone, Debug)]
pub(crate) struct GroupDef {
    #[serde(rename = "PointRequirement")]
    pub point_requirement: Option<i32>,
    pub objectives: Vec<Option<ObjectiveDef>>,
}

// ---------------------------------------------------------------- Lua values
#[derive(Clone, Debug)]
pub(crate) enum Value {
    Nil,
    Bool(bool),
    Num(f32),
    Str(Arc<str>),
    Table(Arc<Mutex<Table>>),
}
impl PartialEq for Value {
    /// Lua's raw equality: same type and value; tables by identity.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Num(a), Value::Num(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Table(a), Value::Table(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}
impl Value {
    pub fn truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }
    fn num(&self) -> Option<f32> {
        match self {
            Value::Num(n) => Some(*n),
            // lvm.c luaV_tonumber: a numeric string converts.
            Value::Str(s) => s.trim().parse::<f32>().ok(),
            _ => None,
        }
    }
    pub fn table() -> Self {
        Value::Table(Arc::new(Mutex::new(Table::default())))
    }
    /// LUAI_NUMFMT "%.14g", as tostring prints a number (the oracle compares by it).
    pub fn show(&self) -> String {
        match self {
            Value::Nil => "nil".into(),
            Value::Bool(b) => b.to_string(),
            Value::Num(n) => fmt_g14(f64::from(*n)),
            Value::Str(s) => format!("{s:?}"),
            Value::Table(t) => {
                let mut items: Vec<String> = t.lock().expect("one thread").0.iter().map(|(k, v)| format!("{}={}", k.show(), v.show())).collect();
                items.sort();
                format!("{{{}}}", items.join(","))
            }
        }
    }
}

fn fmt_g14(x: f64) -> String {
    if x.fract() == 0. && x.abs() < 1e14 {
        return format!("{}", x as i64);
    }
    let s = format!("{:.13e}", x);
    let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if (-4..14).contains(&exp) {
        let digits = (13 - exp).max(0) as usize;
        let t = format!("{x:.digits$}");
        if t.contains('.') { t.trim_end_matches('0').trim_end_matches('.').to_owned() } else { t }
    } else {
        let m = mantissa.trim_end_matches('0').trim_end_matches('.');
        format!("{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

/// A Lua table: few keys, kept in insertion order.
#[derive(Default, Debug)]
pub(crate) struct Table(Vec<(Value, Value)>);
impl Table {
    pub fn get(&self, key: &Value) -> Value {
        self.0.iter().find(|(k, _)| k == key).map_or(Value::Nil, |(_, v)| v.clone())
    }
    pub fn set(&mut self, key: Value, value: Value) {
        match self.0.iter().position(|(k, _)| *k == key) {
            Some(i) if matches!(value, Value::Nil) => drop(self.0.remove(i)),
            Some(i) => self.0[i].1 = value,
            None if matches!(value, Value::Nil) => {}
            None => self.0.push((key, value)),
        }
    }
    /// `#t`: the border from 1 (ltable.c luaH_getn for an array part).
    fn border(&self) -> usize {
        let mut n = 0;
        while self.get(&Value::Num((n + 1) as f32)).truthy() || !matches!(self.get(&Value::Num((n + 1) as f32)), Value::Nil) {
            n += 1;
        }
        n
    }
}

/// A runtime error in a snippet: execute 0x82c75ec8 takes the handler's default.
#[derive(Debug)]
pub(crate) struct LuaError(pub String);

// ---------------------------------------------------------------- the tree
#[derive(Clone, Debug)]
enum Expr {
    Nil,
    Bool(bool),
    Num(f32),
    Str(Arc<str>),
    Slot(usize),
    Vars,
    Global(Arc<str>),
    Index(Box<Expr>, Box<Expr>),
    Native(Arc<str>, Vec<Expr>),
    Const(Arc<str>),
    Bin(Arc<str>, Box<Expr>, Box<Expr>),
    Un(Arc<str>, Box<Expr>),
    Table(Vec<(Option<Expr>, Expr)>),
}

#[derive(Clone, Debug)]
enum Stmt {
    Local(Vec<usize>, Vec<Expr>),
    Set(Vec<Expr>, Vec<Expr>),
    Call(Expr),
    Do(Vec<Stmt>),
    If(Vec<(Expr, Vec<Stmt>)>, Option<Vec<Stmt>>),
    Return(Vec<Expr>),
}

fn tree_err(what: &str, j: &Json) -> String {
    format!("objective tree: bad {what}: {j}")
}

fn expr(j: &Json) -> Result<Expr, String> {
    let a = j.as_array().ok_or_else(|| tree_err("expression", j))?;
    let tag = a.first().and_then(Json::as_str).ok_or_else(|| tree_err("expression", j))?;
    let s = |i: usize| a.get(i).and_then(Json::as_str).map(Arc::from).ok_or_else(|| tree_err(tag, j));
    let e = |i: usize| a.get(i).ok_or_else(|| tree_err(tag, j)).and_then(expr).map(Box::new);
    let list = |i: usize| a.get(i).and_then(Json::as_array).ok_or_else(|| tree_err(tag, j))?.iter().map(expr).collect::<Result<Vec<_>, _>>();
    Ok(match tag {
        "nil" => Expr::Nil,
        "bool" => Expr::Bool(a.get(1).and_then(Json::as_bool).ok_or_else(|| tree_err(tag, j))?),
        // The tool writes the literal as parsed; the f32 Lua compiles it to a float.
        "num" => Expr::Num(a.get(1).and_then(Json::as_f64).ok_or_else(|| tree_err(tag, j))? as f32),
        "str" => Expr::Str(s(1)?),
        "slot" => Expr::Slot(a.get(1).and_then(Json::as_u64).ok_or_else(|| tree_err(tag, j))? as usize),
        "vars" => Expr::Vars,
        "global" => Expr::Global(s(1)?),
        "index" => Expr::Index(e(1)?, e(2)?),
        "native" => Expr::Native(s(1)?, list(2)?),
        "const" => Expr::Const(s(1)?),
        "bin" => Expr::Bin(s(1)?, e(2)?, e(3)?),
        "un" => Expr::Un(s(1)?, e(2)?),
        "table" => {
            let items = a.get(1).and_then(Json::as_array).ok_or_else(|| tree_err(tag, j))?;
            Expr::Table(
                items
                    .iter()
                    .map(|it| {
                        let k = it.get(0).filter(|k| !k.is_null()).map(expr).transpose()?;
                        Ok((k, expr(it.get(1).ok_or_else(|| tree_err("table item", it))?)?))
                    })
                    .collect::<Result<_, String>>()?,
            )
        }
        _ => return Err(tree_err("expression", j)),
    })
}

fn block(j: &Json) -> Result<Vec<Stmt>, String> {
    j.as_array().ok_or_else(|| tree_err("block", j))?.iter().map(stmt).collect()
}

fn stmt(j: &Json) -> Result<Stmt, String> {
    let a = j.as_array().ok_or_else(|| tree_err("statement", j))?;
    let tag = a.first().and_then(Json::as_str).ok_or_else(|| tree_err("statement", j))?;
    let exprs = |i: usize| a.get(i).and_then(Json::as_array).ok_or_else(|| tree_err(tag, j))?.iter().map(expr).collect::<Result<Vec<_>, _>>();
    Ok(match tag {
        "local" => {
            let slots = a.get(1).and_then(Json::as_array).ok_or_else(|| tree_err(tag, j))?;
            Stmt::Local(slots.iter().filter_map(Json::as_u64).map(|s| s as usize).collect(), exprs(2)?)
        }
        "set" => Stmt::Set(exprs(1)?, exprs(2)?),
        "call" => Stmt::Call(expr(a.get(1).ok_or_else(|| tree_err(tag, j))?)?),
        "do" => Stmt::Do(block(a.get(1).ok_or_else(|| tree_err(tag, j))?)?),
        "if" => {
            let arms = a.get(1).and_then(Json::as_array).ok_or_else(|| tree_err(tag, j))?;
            let arms = arms
                .iter()
                .map(|arm| Ok((expr(arm.get(0).ok_or_else(|| tree_err("if arm", arm))?)?, block(arm.get(1).ok_or_else(|| tree_err("if arm", arm))?)?)))
                .collect::<Result<Vec<_>, String>>()?;
            let other = a.get(2).filter(|o| !o.is_null()).map(block).transpose()?;
            Stmt::If(arms, other)
        }
        "return" => Stmt::Return(exprs(1)?),
        _ => return Err(tree_err("statement", j)),
    })
}

/// A compiled snippet.
#[derive(Clone, Debug)]
pub(crate) struct Snippet {
    pub src: u32,
    body: Vec<Stmt>,
    slots: usize,
}
impl Snippet {
    pub fn compile(data: &SnippetData) -> Result<Self, String> {
        let body = block(&data.tree)?;
        fn max_slot(b: &[Stmt], m: &mut usize) {
            for s in b {
                match s {
                    Stmt::Local(slots, _) => slots.iter().for_each(|&x| *m = (*m).max(x + 1)),
                    Stmt::Do(b) => max_slot(b, m),
                    Stmt::If(arms, other) => {
                        arms.iter().for_each(|(_, b)| max_slot(b, m));
                        if let Some(b) = other {
                            max_slot(b, m);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut slots = 0;
        max_slot(&body, &mut slots);
        Ok(Snippet { src: data.src, body, slots })
    }

    /// Runs the snippet: its first return value (Nil when it returns nothing).
    pub fn run(&self, vars: &Value, globals: &mut HashMap<Arc<str>, Value>, natives: &mut dyn Natives) -> Result<Value, LuaError> {
        let mut env = Env { slots: vec![Value::Nil; self.slots], vars: vars.clone(), globals, natives };
        Ok(env.block(&self.body)?.and_then(|v| v.into_iter().next()).unwrap_or(Value::Nil))
    }

    /// execute 0x82c75ec8: a boolean result is the answer; anything else, or an error, the handler's default.
    pub fn decide(&self, default: bool, vars: &Value, globals: &mut HashMap<Arc<str>, Value>, natives: &mut dyn Natives) -> bool {
        match self.run(vars, globals, natives) {
            Ok(Value::Bool(b)) => b,
            _ => default,
        }
    }
}

/// What a snippet asks of the game: a native by its `NS.Fn` name, or a registered constant (`Flow.eObjectiveStatus`).
pub(crate) trait Natives {
    fn call(&mut self, name: &str, args: &[Value]) -> Result<Value, LuaError>;
    fn constant(&mut self, name: &str) -> Result<Value, LuaError> {
        constant(name).ok_or_else(|| LuaError(format!("no constant {name}")))
    }
}

/// The registered enum tables the snippets read.
fn constant(name: &str) -> Option<Value> {
    // eObjectiveStatus (0x82c62e88, names 0x820b7238).
    let status = [("Disabled", 1.), ("Enabled", 2.), ("Active", 4.), ("Complete", 8.), ("Failed", 16.)];
    match name {
        "Flow.eObjectiveStatus" => {
            let t = Value::table();
            if let Value::Table(tb) = &t {
                for (k, v) in status {
                    tb.lock().expect("one thread").set(Value::Str(k.into()), Value::Num(v));
                }
            }
            Some(t)
        }
        _ => {
            let field = name.strip_prefix("Flow.eObjectiveStatus.")?;
            status.iter().find(|s| s.0 == field).map(|s| Value::Num(s.1))
        }
    }
}

type Flow = Option<Vec<Value>>;

struct Env<'a> {
    slots: Vec<Value>,
    vars: Value,
    globals: &'a mut HashMap<Arc<str>, Value>,
    natives: &'a mut dyn Natives,
}

impl Env<'_> {
    fn block(&mut self, body: &[Stmt]) -> Result<Flow, LuaError> {
        for s in body {
            if let Some(r) = self.stmt(s)? {
                return Ok(Some(r));
            }
        }
        Ok(None)
    }

    /// The values of an expression list, the last call's results kept whole (lparser.c adjust_assign; the natives
    /// return one value).
    fn values(&mut self, exprs: &[Expr]) -> Result<Vec<Value>, LuaError> {
        exprs.iter().map(|e| self.expr(e)).collect()
    }

    fn stmt(&mut self, s: &Stmt) -> Result<Option<Vec<Value>>, LuaError> {
        match s {
            Stmt::Local(slots, exprs) => {
                let values = self.values(exprs)?;
                for (i, &slot) in slots.iter().enumerate() {
                    self.slots[slot] = values.get(i).cloned().unwrap_or(Value::Nil);
                }
            }
            Stmt::Set(targets, exprs) => {
                // lparser.c restassign: the values first, then the targets right to left.
                let keys: Vec<(Option<Value>, Option<Value>)> = targets
                    .iter()
                    .map(|t| match t {
                        Expr::Index(obj, key) => Ok((Some(self.expr(obj)?), Some(self.expr(key)?))),
                        _ => Ok((None, None)),
                    })
                    .collect::<Result<_, LuaError>>()?;
                let values = self.values(exprs)?;
                for (i, t) in targets.iter().enumerate().rev() {
                    let v = values.get(i).cloned().unwrap_or(Value::Nil);
                    match (t, &keys[i]) {
                        (Expr::Slot(n), _) => self.slots[*n] = v,
                        (Expr::Global(name), _) => {
                            self.globals.insert(name.clone(), v);
                        }
                        (Expr::Index(..), (Some(obj), Some(key))) => set_index(obj, key.clone(), v)?,
                        _ => return Err(LuaError("bad assignment".into())),
                    }
                }
            }
            Stmt::Call(e) => {
                self.expr(e)?;
            }
            Stmt::Do(body) => return self.block(body),
            Stmt::If(arms, other) => {
                for (cond, body) in arms {
                    if self.expr(cond)?.truthy() {
                        return self.block(body);
                    }
                }
                if let Some(body) = other {
                    return self.block(body);
                }
            }
            Stmt::Return(exprs) => return Ok(Some(self.values(exprs)?)),
        }
        Ok(None)
    }

    fn expr(&mut self, e: &Expr) -> Result<Value, LuaError> {
        Ok(match e {
            Expr::Nil => Value::Nil,
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Num(n) => Value::Num(*n),
            Expr::Str(s) => Value::Str(s.clone()),
            Expr::Slot(n) => self.slots[*n].clone(),
            Expr::Vars => self.vars.clone(),
            Expr::Global(name) => self.globals.get(name).cloned().unwrap_or(Value::Nil),
            Expr::Index(obj, key) => {
                let (obj, key) = (self.expr(obj)?, self.expr(key)?);
                match obj {
                    Value::Table(t) => t.lock().expect("one thread").get(&key),
                    other => return Err(LuaError(format!("attempt to index a {} value", type_name(&other)))),
                }
            }
            Expr::Native(name, args) => {
                let args = self.values(args)?;
                if &**name == "InitVars" {
                    // The prelude (0x820b87d0): if( table[ name ] == nil ) then table[ name ] = value; end
                    let (t, k, v) = (args.first().cloned().unwrap_or(Value::Nil), args.get(1).cloned().unwrap_or(Value::Nil), args.get(2).cloned().unwrap_or(Value::Nil));
                    let Value::Table(tb) = &t else { return Err(LuaError("attempt to index a non-table value".into())) };
                    let unset = matches!(tb.lock().expect("one thread").get(&k), Value::Nil);
                    if unset {
                        set_index(&t, k, v)?;
                    }
                    Value::Nil
                } else {
                    self.natives.call(name, &args)?
                }
            }
            Expr::Const(name) => self.natives.constant(name)?,
            Expr::Bin(op, a, b) => match &**op {
                "and" => {
                    let a = self.expr(a)?;
                    if a.truthy() { self.expr(b)? } else { a }
                }
                "or" => {
                    let a = self.expr(a)?;
                    if a.truthy() { a } else { self.expr(b)? }
                }
                _ => {
                    let (a, b) = (self.expr(a)?, self.expr(b)?);
                    binary(op, &a, &b)?
                }
            },
            Expr::Un(op, a) => {
                let a = self.expr(a)?;
                match &**op {
                    "not" => Value::Bool(!a.truthy()),
                    "-" => Value::Num(-a.num().ok_or_else(|| arith(&a))?),
                    "#" => match &a {
                        Value::Str(s) => Value::Num(s.len() as f32),
                        Value::Table(t) => Value::Num(t.lock().expect("one thread").border() as f32),
                        other => return Err(LuaError(format!("attempt to get length of a {} value", type_name(other)))),
                    },
                    _ => return Err(LuaError(format!("unary {op}"))),
                }
            }
            Expr::Table(items) => {
                let t = Value::table();
                let mut n = 0;
                for (k, v) in items {
                    let key = match k {
                        Some(k) => self.expr(k)?,
                        None => {
                            n += 1;
                            Value::Num(n as f32)
                        }
                    };
                    let v = self.expr(v)?;
                    set_index(&t, key, v)?;
                }
                t
            }
        })
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Nil => "nil",
        Value::Bool(_) => "boolean",
        Value::Num(_) => "number",
        Value::Str(_) => "string",
        Value::Table(_) => "table",
    }
}

fn arith(v: &Value) -> LuaError {
    LuaError(format!("attempt to perform arithmetic on a {} value", type_name(v)))
}

fn set_index(t: &Value, key: Value, v: Value) -> Result<(), LuaError> {
    match t {
        Value::Table(tb) if !matches!(key, Value::Nil) => {
            tb.lock().expect("one thread").set(key, v);
            Ok(())
        }
        Value::Table(_) => Err(LuaError("table index is nil".into())),
        other => Err(LuaError(format!("attempt to index a {} value", type_name(other)))),
    }
}

/// lvm.c's binary operators on the f32 numbers.
fn binary(op: &str, a: &Value, b: &Value) -> Result<Value, LuaError> {
    let cmp = |a: &Value, b: &Value| -> Result<std::cmp::Ordering, LuaError> {
        match (a, b) {
            (Value::Num(x), Value::Num(y)) => x.partial_cmp(y).ok_or_else(|| LuaError("nan".into())),
            (Value::Str(x), Value::Str(y)) => Ok(x.cmp(y)),
            _ => Err(LuaError(format!("attempt to compare {} with {}", type_name(a), type_name(b)))),
        }
    };
    let nums = || Ok::<_, LuaError>((a.num().ok_or_else(|| arith(a))?, b.num().ok_or_else(|| arith(b))?));
    Ok(match op {
        "==" => Value::Bool(a == b),
        "~=" => Value::Bool(a != b),
        // NaN compares false (luai_numlt); none of the snippets' numbers are NaN.
        "<" => Value::Bool(cmp(a, b)?.is_lt()),
        "<=" => Value::Bool(cmp(a, b)?.is_le()),
        ">" => Value::Bool(cmp(b, a)?.is_lt()),
        ">=" => Value::Bool(cmp(b, a)?.is_le()),
        "+" => nums().map(|(x, y)| Value::Num(x + y))?,
        "-" => nums().map(|(x, y)| Value::Num(x - y))?,
        "*" => nums().map(|(x, y)| Value::Num(x * y))?,
        "/" => nums().map(|(x, y)| Value::Num(x / y))?,
        "%" => nums().map(|(x, y)| Value::Num(x - (x / y).floor() * y))?,
        "^" => nums().map(|(x, y)| Value::Num(x.powf(y)))?,
        ".." => {
            let s = |v: &Value| match v {
                Value::Str(s) => Ok(s.to_string()),
                Value::Num(n) => Ok(fmt_g14(f64::from(*n))),
                other => Err(LuaError(format!("attempt to concatenate a {} value", type_name(other)))),
            };
            Value::Str(format!("{}{}", s(a)?, s(b)?).into())
        }
        _ => return Err(LuaError(format!("binary {op}"))),
    })
}

// ---------------------------------------------------------------- the objectives
/// What a group posts to the challenge graph (the Lua handlers of the same names).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Out {
    /// cMsgObjectiveComplete c075f1a5 {ObjectiveGroup, ObjectiveIndex, RequiresHighlight, IsPointObjective}.
    Complete { index: usize, requires_highlight: bool, points: bool },
    /// cMsgObjectiveFailed 2db67c46 {ObjectiveGroup, ObjectiveIndex, FailedIndex}.
    Failed { index: usize, failed: usize },
    /// cMsgObjectivePointsAdded 85dbb90d {group, index, points}.
    PointsAdded { index: usize, points: i32 },
    /// cMsgAllObjectiveComplete 524ceaac.
    AllComplete,
    /// cMsgDidNotGetEnoughPoints e3df06ae.
    NotEnoughPoints,
}

struct Objective {
    def: ObjectiveDef,
    obj: Option<Snippet>,
    points_valid: Option<Snippet>,
    failures: Vec<Option<Snippet>>,
    hints: Vec<Option<Snippet>>,
    status: u8,
    /// The per-objective `vars` (registry key from the objective id, 0x82c75ec8 / 0x82e93068).
    vars: Value,
    /// +1696: volumes entered while active (cap 32); +2264: in the current sequence (deduplicated, cleared at
    /// SequenceBegin).
    entered: Vec<u64>,
    entered_sequence: Vec<u64>,
    /// +700: points (AddPoints), +696: the last sequence's score.
    points: i32,
    sequence_score: i32,
    /// +1164 / +1165: a session marker was used / set while Active (0x82c6eff8 / 0x82c6ee98), until the tick's end.
    marker_used: bool,
    marker_set: bool,
    /// +3104: tricks done while Active (0x82c74a20, cap 32), by scorable identifier.
    tricks: Vec<String>,
    /// +1288: ObjectiveGameInfo.SetFractionValue's count (0x82c69418); not reset with the objective (0x82c73eb8 r5 = 0).
    fraction: i32,
    /// +1548: completed by ForceObjectiveComplete (0x82c61838).
    forced: bool,
}

/// The evaluation parameters (0x82c752a8's second argument): +8 the event was a SequenceComplete (or AddPoints' flag),
/// +9 newly complete in this sequence (AddPoints), +10 group+537.
#[derive(Clone, Copy, Default)]
struct Params {
    sequence: bool,
    newly_complete: bool,
}

/// What the skater-side natives read (the glue's sense of the skater this frame).
pub(crate) trait Skater {
    /// A native the group does not answer itself, or None when it is not ported (the snippet then errors).
    fn native(&mut self, name: &str, args: &[Value]) -> Option<Value>;
    /// Whether a scorable is in a skatercollections trick group (Tricks.DidSpecificTrickGroup's test).
    fn in_group(&self, _group: &str, _trick: &str) -> bool {
        false
    }
}

/// No skater natives (tests, and rows whose snippets need none).
pub(crate) struct NoSkater;
impl Skater for NoSkater {
    fn native(&mut self, _: &str, _: &[Value]) -> Option<Value> {
        None
    }
}

/// The natives this port answers (group and objective state); others go to [`Skater`].
pub(crate) const GROUP_NATIVES: &[&str] = &[
    "InitVars",
    "ObjectivePoints.GetGroupAccumulatedPoints",
    "ObjectivePoints.GetGroupPointsRequirement",
    "ObjectivePoints.IsSequenceNewlyCompletePoints",
    "ObjectiveTrigger.EnteredVolume",
    "ObjectiveSessionMarker.JustUsedSessionMarker",
    "ObjectiveInfo.GetTotalNumCompletedObjectives",
    "ObjectiveInfo.NumUniqueTrickDone",
    "Tricks.DidSpecificTrick",
    "Tricks.DidSpecificTrickGroup",
    "Flow.ActivateObjective",
    "Flow.DeactivateObjective",
    "Reset.ResetAllObjectives",
    "ObjectiveGameInfo.SetFractionValue",
    "ObjectiveGameInfo.GetFractionValue",
    "const:Flow.eObjectiveStatus",
    "const:Flow.eObjectiveStatus.Active",
    "const:Flow.eObjectiveStatus.Complete",
    "Flow.GetObjectiveStatus",
    "ObjectiveSessionMarker.JustSetSessionMarker",
];

/// One skater's objective group (ctor 0x82c6cd28).
pub(crate) struct Group {
    pub player: usize,
    objs: Vec<Objective>,
    /// group+72: the points objective (made by 0x82c6d4d8 for a group with a PointRequirement, last in the list).
    points_index: Option<usize>,
    /// group+76: the requirement (ChangeObjectivePointsRequirement 0x82c61100).
    pub requirement: i32,
    /// group+68: SetEvaluationStatus (0x82c61500); the message thunks skip a group with it off.
    pub enabled: bool,
    /// The objectives' Lua state's globals (one state for the manager: "cChallengeObjectivesManager::LuaStateManager").
    // ponytail: each group keeps its own globals (Film / Photo run an Owned and a Killed It group on one console, retail
    // one state for both); share them if a snippet is found reading globals another group wrote.
    globals: HashMap<Arc<str>, Value>,
    /// group+120: the challenge's TrackUniqueTrickGroup (MakeObjectivesFromGroup 0x82c60fc4 via 0x82c5fe60), a
    /// skatercollections trick group; None is retail's -1 (no unique-trick tracking).
    pub unique_group: Option<String>,
    /// +408: the tricks of the open sequence (0x82c72160), cleared at its SequenceComplete (0x82c6de08).
    sequence_tricks: Vec<String>,
    /// +128: the unique tricks kept (AddLastUniqueTrickToList 0x82c61900); +528: the last sequence's new one.
    unique: Vec<String>,
    pending_unique: Option<String>,
    /// ObjectiveFlow {type, count} (group+0x60 / +0x64).
    flow: [u32; 2],
    /// The groups SetStatus and the hints started (StartPresentation 0x82dbaca0), until [`Group::presentations`].
    presented: Vec<Presentation>,
}

fn compile(s: &Option<SnippetData>) -> Result<Option<Snippet>, String> {
    s.as_ref().map(Snippet::compile).transpose()
}

impl Group {
    /// MakeObjectivesFromGroup (0x82c60cc0): the group's objectives, then (with a PointRequirement) the points
    /// objective, then the flow's first activation (0x82c6d168).
    pub fn new(player: usize, def: &GroupDef, points_objective: &ObjectiveDef) -> Result<Self, String> {
        Self::with_flow(player, def, points_objective, [0, 1])
    }

    /// [`Group::new`] under ObjectiveFlow {type, count} (group+0x60 / +0x64).
    pub fn with_flow(player: usize, def: &GroupDef, points_objective: &ObjectiveDef, flow: [u32; 2]) -> Result<Self, String> {
        let mut defs: Vec<ObjectiveDef> = def.objectives.iter().map(|o| o.clone().ok_or("an objective record is missing")).collect::<Result<_, _>>()?;
        let points_index = def.point_requirement.map(|_| defs.len());
        if points_index.is_some() {
            defs.push(points_objective.clone());
        }
        let objs = defs
            .into_iter()
            .map(|d| {
                Ok(Objective {
                    obj: compile(&d.obj)?,
                    points_valid: compile(&d.points_valid)?,
                    failures: d.failures.iter().map(compile).collect::<Result<_, String>>()?,
                    hints: d.hints.iter().map(compile).collect::<Result<_, String>>()?,
                    def: d,
                    status: DISABLED,
                    vars: Value::table(),
                    entered: Vec::new(),
                    entered_sequence: Vec::new(),
                    points: 0,
                    sequence_score: 0,
                    marker_used: false,
                    marker_set: false,
                    tricks: Vec::new(),
                    fraction: 0,
                    forced: false,
                })
            })
            .collect::<Result<_, String>>()?;
        let mut g = Group {
            player,
            objs,
            points_index,
            requirement: def.point_requirement.unwrap_or(0),
            enabled: true,
            globals: HashMap::new(),
            unique_group: None,
            sequence_tricks: Vec::new(),
            unique: Vec::new(),
            pending_unique: None,
            flow,
            presented: Vec::new(),
        };
        g.activate_flow();
        Ok(g)
    }

    /// Whether this port runs every snippet and activation mode of a group's objectives.
    pub fn supported(def: &GroupDef, points: &ObjectiveDef, skater: &[&str]) -> Result<(), String> {
        let all = def.objectives.iter().flatten().chain(std::iter::once(points));
        for o in all {
            // Activation modes 0 (at once), 1 (at ActivateChallengeStartObjectives, 0x82c61158) and 5 (each sequence,
            // 0x82c6da68) are ported; 2/3 (on entering the activation volume, 0x82c74290) and 4 (at a wipeout,
            // 0x82c71bd8) are not.
            if ![0, 1, 5].contains(&o.activation) {
                return Err(format!("{}: activation mode {}", o.key, o.activation));
            }
            for s in [&o.obj, &o.points_valid].into_iter().chain(o.failures.iter()).chain(o.hints.iter()).flatten() {
                for n in &s.natives {
                    if !GROUP_NATIVES.contains(&n.as_str()) && !skater.contains(&n.as_str()) {
                        return Err(format!("{}: native {n}", o.key));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.objs.len()
    }
    pub fn status(&self, index: usize) -> u8 {
        self.objs[index].status
    }
    pub fn is_points(&self, index: usize) -> bool {
        self.points_index == Some(index)
    }

    /// GetAccumulatedPoints (0x82c61448) / GetGroupAccumulatedPoints (0x82c65188): the objectives' points summed.
    pub fn accumulated(&self) -> i32 {
        self.objs.iter().map(|o| o.points).sum()
    }

    /// SetStatus 0x82c73fc8: Active starts the objective's ObjIntroEvents and Complete its ObjCompleteEvents on every
    /// call, changed or not (stage1_traces 4.8). The indicators it adds and removes are the glue's.
    fn set_status(&mut self, i: usize, status: u8) {
        let o = &mut self.objs[i];
        o.status = status;
        let (kind, events) = match status {
            ACTIVE => ("intro", &o.def.groups.intro),
            COMPLETE => ("complete", &o.def.groups.complete),
            _ => return,
        };
        present(&mut self.presented, format!("{}/{kind}", o.def.key), events);
    }

    /// 0x82c75850: an objective activates Active (ActivationMode 0) or Enabled.
    fn activate(&mut self, i: usize) {
        let s = if self.objs[i].def.activation == 0 { ACTIVE } else { ENABLED };
        self.set_status(i, s);
    }

    /// The flow's activation (0x82c6d168, stage1_school 2), through Activate 0x82c75850 (only a Disabled objective
    /// changes): type 0 every objective; type 2 the first `count` by position (at least one); any other type the first
    /// `count` that activate, every later Enabled / Active one Disabled; the points objective last, outside the count.
    /// The message it posts per activation (0x72dcd7a9) has no handler in the ported graphs; not posted.
    fn activate_flow(&mut self) {
        let [kind, count] = self.flow;
        let mut activated = 0;
        for i in (0..self.objs.len()).filter(|&i| Some(i) != self.points_index).collect::<Vec<_>>() {
            match kind {
                0 => {
                    self.activate_objective(i);
                }
                2 => {
                    if i < count.max(1) as usize {
                        self.activate_objective(i);
                    }
                }
                _ if activated < count => activated += u32::from(self.activate_objective(i)),
                _ => self.deactivate_objective(i),
            }
        }
        if let Some(p) = self.points_index {
            self.activate_objective(p);
        }
    }

    /// 0x82c73eb8 with its progress flag: the per-attempt lists and the points.
    fn clear_progress(&mut self, i: usize) {
        let o = &mut self.objs[i];
        o.entered.clear();
        o.entered_sequence.clear();
        o.points = 0;
        o.sequence_score = 0;
        (o.marker_used, o.marker_set) = (false, false);
        o.tricks.clear();
    }

    /// ResetObjectives' reset of one objective (0x82c6f430): its progress, status Disabled, `vars` dropped (0x82c75c48).
    fn wipe(&mut self, i: usize) {
        self.clear_progress(i);
        let o = &mut self.objs[i];
        o.status = DISABLED;
        o.vars = Value::table();
        // ponytail: whether the reset clears +1548 is not traced; a reset objective reads as not forced.
        o.forced = false;
    }

    /// The presentation groups started since the last call, in order (SetStatus's intro and complete groups, the
    /// hints'); the glue hands each to its [`crate::skate3_present::Presenter`].
    pub fn presentations(&mut self) -> Vec<Presentation> {
        std::mem::take(&mut self.presented)
    }

    /// Objectives.ResetObjective(g, i, true, false) (0x82c613a8 -> 0x82c6f4d0, stage1_traces 6): the objective's
    /// progress wiped, its status and `vars` kept, then the flow; no intro.
    pub fn reset_objective(&mut self, i: usize) {
        if i < self.objs.len() {
            self.clear_progress(i);
            self.activate_flow();
        }
    }

    /// Objectives.ForceObjectiveComplete(group, k) (0x82c61838 -> 0x82c6f8c0): an objective not yet Complete is
    /// (forced, +1548), posting objectivecomplete (RequiresHighlight false), then allobjectivecomplete when every
    /// objective is (0x82c6f7d8).
    pub fn force_complete(&mut self, i: usize) -> Vec<Out> {
        let mut out = Vec::new();
        if self.objs.get(i).is_none_or(|o| o.status == COMPLETE) {
            return out;
        }
        self.set_status(i, COMPLETE);
        self.objs[i].forced = true;
        out.push(Out::Complete { index: i, requires_highlight: false, points: self.points_index == Some(i) });
        self.completed_pass(&mut out);
        out
    }

    /// 0x82c6f7d8, after a pass that completed an objective: the flow again (stage1_school 2), then
    /// allobjectivecomplete when every objective is Complete.
    fn completed_pass(&mut self, out: &mut Vec<Out>) {
        self.activate_flow();
        if self.objs.iter().all(|o| o.status == COMPLETE) {
            out.push(Out::AllComplete);
        }
    }

    /// Objectives.GetNumCompletedObjectivesForScoring(group, all = true) (0x82c604b0 -> 0x82c6f670): over the objectives
    /// shown in the HUD (DisplayInHUD), a counting one (Int16 2B245EC6D20A64AA over 1) adds its fraction (+1288, at most
    /// that Int16), any other Complete one but the points objective 1 (with `all` false, only if not forced).
    pub fn completed_for_scoring(&self, all: bool) -> i32 {
        (self.objs.iter().enumerate())
            .filter(|(_, o)| o.def.display_in_hud)
            .map(|(i, o)| match o.def.count_kind {
                n if n > 1 => o.fraction.min(i32::from(n)),
                _ => i32::from(o.status == COMPLETE && self.points_index != Some(i) && (all || !o.forced)),
            })
            .sum()
    }

    /// Objectives.ResetObjectives / Reset.ResetAllObjectives (0x82c61348 / 0x82c63fc0 -> 0x82c6f430): every objective
    /// reset, the unique trick list emptied (with tracking), then the flow's activation. Both natives' flag argument
    /// defaults to true, the only form the data uses.
    pub fn reset(&mut self) {
        for i in 0..self.objs.len() {
            self.wipe(i);
        }
        if self.unique_group.is_some() {
            self.unique.clear();
        }
        self.activate_flow();
    }

    /// Flow.ActivateObjective(i) (0x82c630a0 -> 0x82c75850), `i` the 0-based index: a Disabled objective activates
    /// (Active for ActivationMode 0, else Enabled); Enabled / Active stay; true unless it is Complete or Failed.
    /// ponytail: the message it posts (72dcd7a9) has no handler in the ported graphs; not posted.
    fn activate_objective(&mut self, i: usize) -> bool {
        match self.objs.get(i).map(|o| o.status) {
            Some(DISABLED) => {
                self.activate(i);
                true
            }
            Some(ENABLED | ACTIVE) => true,
            _ => false,
        }
    }

    /// Flow.DeactivateObjective(i) (0x82c63168): an Enabled or Active objective is Disabled.
    fn deactivate_objective(&mut self, i: usize) {
        if self.objs.get(i).is_some_and(|o| matches!(o.status, ENABLED | ACTIVE)) {
            self.set_status(i, DISABLED);
        }
    }

    /// ObjectiveInfo.NumUniqueTrickDone (0x82c69de8): the unique trick list, plus the last sequence's new one.
    pub fn unique_done(&self) -> usize {
        self.unique.len() + usize::from(self.pending_unique.is_some())
    }

    /// Objectives.AddLastUniqueTrickToList (0x82c61900): the last sequence's new unique trick kept; the list's length.
    pub fn add_last_unique(&mut self) -> usize {
        if let Some(t) = self.pending_unique.take() {
            self.unique.push(t);
        }
        self.unique.len()
    }

    /// Objectives.ResetObjective(group, index) (Domination after each completion): that objective reset and
    /// activated again. ponytail: the native is not traced; read as ResetObjectives for one objective.
    pub fn reset_one(&mut self, i: usize) {
        if i < self.objs.len() {
            self.wipe(i);
            self.activate(i);
        }
    }

    /// Re-arm 0x82c75740: back to the activation's status unless Disabled.
    fn rearm(&mut self, i: usize) {
        if self.objs[i].status != DISABLED {
            self.activate(i);
        }
    }

    fn run(&mut self, i: usize, which: Which, params: Params, skater: &mut dyn Skater) -> bool {
        let (snippet, default) = match which {
            Which::Obj => (self.objs[i].obj.clone(), true),
            Which::PointsValid => (self.objs[i].points_valid.clone(), false),
            Which::Failure(k) => (self.objs[i].failures.get(k).cloned().flatten(), false),
            Which::Hint(k) => (self.objs[i].hints.get(k).cloned().flatten(), false),
        };
        let Some(snippet) = snippet else { return default };
        let vars = self.objs[i].vars.clone();
        let mut globals = std::mem::take(&mut self.globals);
        let result = {
            let mut natives = GroupNatives { group: self, index: i, params, skater };
            snippet.decide(default, &vars, &mut globals, &mut natives)
        };
        self.globals = globals;
        result
    }

    /// Evaluate 0x82c752a8: -2 complete, a failure's index, or None.
    fn evaluate(&mut self, i: usize, params: Params, skater: &mut dyn Skater) -> Option<Result<(), usize>> {
        let objective = |g: &mut Self, skater: &mut dyn Skater| {
            // 0x82c75400: a true objective is Complete.
            g.run(i, Which::Obj, params, skater).then(|| g.set_status(i, COMPLETE))
        };
        // 0x82c755b8: the first true hint starts its group; nothing else, and its result is thrown away.
        // shortcut: its gate (group+0x219, 1 from MakeObjectives, and the objective's player the local one, INFERRED)
        // is always open; only the single-player lessons have hints.
        let hint = |g: &mut Self, skater: &mut dyn Skater| {
            if let Some(k) = (0..g.objs[i].hints.len()).find(|&k| g.run(i, Which::Hint(k), params, skater)) {
                let def = &g.objs[i].def;
                present(&mut g.presented, format!("{}/hint{k}", def.key), def.groups.hints.get(k).map_or(&[][..], Vec::as_slice));
            }
        };
        let failure = |g: &mut Self, skater: &mut dyn Skater| (0..g.objs[i].failures.len()).find(|&k| g.run(i, Which::Failure(k), params, skater));
        if self.objs[i].def.fail_first {
            hint(self, skater);
            if let Some(k) = failure(self, skater) {
                return Some(Err(k));
            }
            objective(self, skater).map(Ok)
        } else {
            if objective(self, skater).is_some() {
                return Some(Ok(()));
            }
            hint(self, skater);
            failure(self, skater).map(Err)
        }
    }

    /// Process 0x82c6e4b0 for one trigger byte: the objectives it completed (in order), and its messages.
    fn process(&mut self, event: usize, params: Params, skater: &mut dyn Skater, out: &mut Vec<Out>) -> Vec<usize> {
        let mut done = Vec::new();
        for i in 0..self.objs.len() {
            let o = &self.objs[i];
            if o.status != ACTIVE || o.def.triggers.get(event).copied() != Some(1) {
                continue;
            }
            match self.evaluate(i, params, skater) {
                Some(Ok(())) => {
                    if self.objs[i].def.master {
                        for k in 0..self.objs.len() {
                            if k != i {
                                self.set_status(k, COMPLETE);
                                done.push(k);
                            }
                        }
                    }
                    done.push(i);
                    if Some(i) == self.points_index {
                        if self.requirement != 0 {
                            out.push(Out::Complete { index: i, requires_highlight: false, points: true });
                        }
                    } else {
                        out.push(Out::Complete { index: i, requires_highlight: self.objs[i].def.requires_highlight, points: false });
                    }
                }
                Some(Err(k)) => out.push(Out::Failed { index: i, failed: k }),
                None => {}
            }
        }
        if !done.is_empty() {
            self.completed_pass(out);
        } else if event == SECOND_PASS
            && let Some(p) = self.points_index
            && self.objs[p].status != COMPLETE
            && self.objs.iter().enumerate().all(|(k, o)| k == p || o.status == COMPLETE)
        {
            out.push(Out::NotEnoughPoints);
        }
        done
    }

    /// AddPoints 0x82c6f2b8: each Active objective, and each objective this pass completed, whose PointsValid says so
    /// takes the points.
    fn add_points(&mut self, points: i32, sequence: bool, done: &[usize], skater: &mut dyn Skater, out: &mut Vec<Out>) {
        for i in 0..self.objs.len() {
            let newly_complete = match self.objs[i].status {
                ACTIVE => false,
                COMPLETE if done.contains(&i) => true,
                _ => continue,
            };
            if self.run(i, Which::PointsValid, Params { sequence, newly_complete }, skater) && points > 0 {
                self.objs[i].points += points;
                out.push(Out::PointsAdded { index: i, points });
            }
        }
    }

    /// cMsgSequenceBegin (0x82c6da68): the current-sequence lists cleared; mode-5 objectives Enabled -> Active.
    pub fn sequence_begin(&mut self) {
        if !self.enabled {
            return;
        }
        for i in 0..self.objs.len() {
            let o = &mut self.objs[i];
            o.entered_sequence.clear();
            o.sequence_score = 0;
            if o.def.activation == 5 && o.status == ENABLED {
                self.set_status(i, ACTIVE);
            }
        }
    }

    /// cMsgSequenceComplete (0x82c6db50) with the sequence's score; `aborted`: its +40 / +41 flags (Wipedout).
    pub fn sequence_complete(&mut self, score: i32, aborted: bool, skater: &mut dyn Skater) -> Vec<Out> {
        let mut out = Vec::new();
        if !self.enabled {
            return out;
        }
        if !aborted {
            for o in self.objs.iter_mut().filter(|o| o.status == ACTIVE) {
                o.sequence_score = score;
            }
            // 0x82c6dbec: the sequence's first trick in the tracked group not yet kept (while fewer than 32 are).
            if let Some(group) = self.unique_group.clone() {
                self.pending_unique = self
                    .sequence_tricks
                    .iter()
                    .find(|t| skater.in_group(&group, t) && !self.unique.contains(t) && self.unique.len() < UNIQUE_CAP)
                    .cloned();
            }
            let params = Params { sequence: true, newly_complete: false };
            let done = self.process(ON_SEQUENCE, params, skater, &mut out);
            self.add_points(score, true, &done, skater, &mut out);
            if !done.is_empty() {
                self.process(SECOND_PASS, Params::default(), skater, &mut out);
            }
        }
        for i in 0..self.objs.len() {
            if self.objs[i].def.activation == 5 && self.objs[i].status == ACTIVE {
                // 0x82c6ddd0: the trick list goes with the sequence.
                self.objs[i].tricks.clear();
                self.rearm(i);
            }
        }
        self.sequence_tricks.clear();
        out
    }

    /// The per-frame update (0x82c6f998): trigger byte 12, the second pass, then every objective's marker flags
    /// cleared (0x82c73fb8). ponytail: byte 13 (Hall of Meat's groups, group+536) is not set.
    pub fn tick(&mut self, skater: &mut dyn Skater) -> Vec<Out> {
        let mut out = Vec::new();
        if !self.enabled {
            return out;
        }
        let done = self.process(ON_TICK, Params::default(), skater, &mut out);
        if !done.is_empty() {
            self.process(SECOND_PASS, Params::default(), skater, &mut out);
        }
        for o in &mut self.objs {
            (o.marker_used, o.marker_set) = (false, false);
        }
        out
    }

    /// cMsgUseMarker (`used`, 0x82c6eff8) / cMsgSetMarker (0x82c6ee98): Active objectives note it, then byte 5 and
    /// the second pass.
    pub fn marker(&mut self, used: bool, skater: &mut dyn Skater) -> Vec<Out> {
        let mut out = Vec::new();
        if !self.enabled {
            return out;
        }
        for o in self.objs.iter_mut().filter(|o| o.status == ACTIVE) {
            if used { o.marker_used = true } else { o.marker_set = true }
        }
        let done = self.process(ON_MARKER, Params::default(), skater, &mut out);
        if !done.is_empty() {
            self.process(SECOND_PASS, Params::default(), skater, &mut out);
        }
        out
    }

    /// A trick done (0x82c72160): each Active objective lists it (0x82c74a20, the oldest past 32 dropped); with
    /// unique-trick tracking the group's sequence list takes it too (none past 12).
    pub fn trick(&mut self, name: &str) {
        if !self.enabled {
            return;
        }
        for o in self.objs.iter_mut().filter(|o| o.status == ACTIVE) {
            o.tricks.push(name.to_owned());
            if o.tricks.len() > TRICKS_CAP {
                o.tricks.remove(0);
            }
        }
        if self.unique_group.is_some() && self.sequence_tricks.len() < SEQUENCE_TRICKS_CAP {
            self.sequence_tricks.push(name.to_owned());
        }
    }

    /// Objectives.FailAllObjectiveStatus (0x82c6f540): every objective not Disabled is Failed.
    pub fn fail_all(&mut self) {
        for i in 0..self.objs.len() {
            if self.objs[i].status != DISABLED {
                self.set_status(i, FAILED);
            }
        }
    }

    /// Objectives.ActivateChallengeStartObjectives (0x82c61158): ActivationMode 1 objectives Active.
    pub fn activate_start(&mut self) {
        for i in 0..self.objs.len() {
            if self.objs[i].def.activation == 1 {
                self.set_status(i, ACTIVE);
            }
        }
    }

    /// GetTotalNumCompletedObjectives(points, others) (0x82c69d50 -> 0x82c6f5a8; both arguments default to true): the
    /// Complete objectives, the points objective only with `points`, one whose Int16 2B245EC6D20A64AA is over 1 only
    /// with `others`.
    pub fn completed_with(&self, points: bool, others: bool) -> usize {
        (self.objs.iter().enumerate())
            .filter(|(i, o)| o.status == COMPLETE && if Some(*i) == self.points_index { points } else { o.def.count_kind <= 1 || others })
            .count()
    }
    pub fn completed(&self) -> usize {
        self.completed_with(true, true)
    }

    /// cMsgTriggerEnterCollision (0x82c6e8c0): each Active objective notes the volume (0x82c74290), then a pass.
    pub fn trigger_enter(&mut self, volume: u64, skater: &mut dyn Skater) -> Vec<Out> {
        let mut out = Vec::new();
        if !self.enabled {
            return out;
        }
        for o in self.objs.iter_mut().filter(|o| o.status == ACTIVE) {
            o.entered.push(volume);
            if o.entered.len() > ENTERED_CAP {
                o.entered.remove(0);
            }
            o.entered_sequence.retain(|&v| v != volume);
            o.entered_sequence.push(volume);
        }
        let done = self.process(ON_TRIGGER, Params::default(), skater, &mut out);
        if !done.is_empty() {
            self.process(SECOND_PASS, Params::default(), skater, &mut out);
        }
        out
    }

    /// cMsgWipeoutOver (0x82c6d890), for a group that is not Hall of Meat's (group+536): mode-4 objectives re-arm.
    pub fn wipeout_over(&mut self) {
        if !self.enabled {
            return;
        }
        for i in 0..self.objs.len() {
            if self.objs[i].def.activation == 4 && self.objs[i].status == ACTIVE {
                self.rearm(i);
            }
        }
    }
}

/// StartPresentation of an objective's group. INFERRED: an empty group plays nothing, so it is not started.
fn present(to: &mut Vec<Presentation>, name: String, events: &[Event]) {
    if !events.is_empty() {
        to.push(Presentation { name, events: events.to_vec() });
    }
}

#[derive(Clone, Copy)]
enum Which {
    Obj,
    Hint(usize),
    PointsValid,
    Failure(usize),
}

struct GroupNatives<'a> {
    group: &'a mut Group,
    index: usize,
    params: Params,
    skater: &'a mut dyn Skater,
}

/// The decimal volume id a trigger native takes as argument 1 (0x829254f0 tostring, 0x82ae8f78 parse).
fn volume_arg(args: &[Value]) -> Option<u64> {
    match args.first()? {
        Value::Str(s) => s.parse().ok(),
        Value::Num(n) => Some(*n as u64),
        _ => None,
    }
}

/// A string argument 1 (a trick or trick group name).
fn str_arg(args: &[Value]) -> String {
    match args.first() {
        Some(Value::Str(s)) => s.to_string(),
        _ => String::new(),
    }
}

/// Argument `i` as a boolean, else `default` (0x82925280).
fn bool_arg(args: &[Value], i: usize, default: bool) -> bool {
    match args.get(i) {
        Some(Value::Bool(b)) => *b,
        _ => default,
    }
}

impl Natives for GroupNatives<'_> {
    fn call(&mut self, name: &str, args: &[Value]) -> Result<Value, LuaError> {
        // The natives that change the group (context group, upvalue 1 +4).
        let index = || args.first().and_then(Value::num).map(|n| n as i64).filter(|&n| n >= 0).map(|n| n as usize);
        match name {
            "Flow.ActivateObjective" => return Ok(Value::Bool(index().is_some_and(|i| self.group.activate_objective(i)))),
            "Flow.DeactivateObjective" => {
                if let Some(i) = index() {
                    self.group.deactivate_objective(i);
                }
                return Ok(Value::Nil);
            }
            "Reset.ResetAllObjectives" => {
                self.group.reset();
                return Ok(Value::Nil);
            }
            // 0x82c69418: the context objective's +1288 = optinteger(arg 1, 0) (+1584 set).
            "ObjectiveGameInfo.SetFractionValue" => {
                self.group.objs[self.index].fraction = args.first().and_then(Value::num).map_or(0, |n| n as i32);
                return Ok(Value::Nil);
            }
            _ => {}
        }
        let o = &self.group.objs[self.index];
        Ok(match name {
            // Pushed as floats (0x82c65188 / 0x82c65350: fcfid, frsp).
            "ObjectivePoints.GetGroupAccumulatedPoints" => Value::Num(self.group.accumulated() as f32),
            "ObjectivePoints.GetGroupPointsRequirement" => Value::Num(self.group.requirement as f32),
            // 0x82c652a0: params +8 && +9.
            "ObjectivePoints.IsSequenceNewlyCompletePoints" => Value::Bool(self.params.sequence && self.params.newly_complete),
            // 0x82c65438: the id in objective+1696.
            "ObjectiveTrigger.EnteredVolume" => Value::Bool(volume_arg(args).is_some_and(|v| o.entered.contains(&v))),
            // 0x82c690e8: objective+1164.
            "ObjectiveSessionMarker.JustUsedSessionMarker" => Value::Bool(o.marker_used),
            "ObjectiveInfo.GetTotalNumCompletedObjectives" => Value::Num(self.group.completed_with(bool_arg(args, 0, true), bool_arg(args, 1, true)) as f32),
            "ObjectiveInfo.NumUniqueTrickDone" => Value::Num(self.group.unique_done() as f32),
            // 0x82c69488.
            "ObjectiveGameInfo.GetFractionValue" => Value::Num(o.fraction as f32),
            // The 0-based objective's eObjectiveStatus, as it is now (tut_sksc_02_13 reads objective 4, _17).
            "Flow.GetObjectiveStatus" => index().and_then(|i| self.group.objs.get(i)).map_or(Value::Nil, |o| Value::Num(f32::from(o.status))),
            // 0x82c6ee98: objective+1165.
            "ObjectiveSessionMarker.JustSetSessionMarker" => Value::Bool(o.marker_set),
            // 0x82c66810 / 0x82c668f0: the objective's trick list (+3104).
            "Tricks.DidSpecificTrick" => {
                let name = str_arg(args);
                Value::Bool(o.tricks.iter().any(|t| *t == name))
            }
            "Tricks.DidSpecificTrickGroup" => {
                let group = str_arg(args);
                Value::Bool(o.tricks.iter().any(|t| self.skater.in_group(&group, t)))
            }
            _ => self.skater.native(name, args).ok_or_else(|| LuaError(format!("native {name} is not ported")))?,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    fn snippet(tree: Json) -> SnippetData {
        SnippetData { src: 0, tree, natives: vec![] }
    }

    fn obj(key: &str, activation: u32, triggers: &[usize], obj: Json, points_valid: Option<Json>) -> ObjectiveDef {
        let mut t = vec![0u8; 20];
        for &b in triggers {
            t[b] = 1;
        }
        ObjectiveDef {
            key: key.into(),
            activation,
            triggers: t,
            fail_first: false,
            master: false,
            requires_highlight: true,
            obj: Some(snippet(obj)),
            points_valid: points_valid.map(snippet),
            failures: vec![],
            count_kind: 0,
            display_in_hud: true,
            hints: vec![],
            groups: ObjectiveGroups::default(),
        }
    }

    /// The global points objective as the disc has it (challengebanks `global_points_objective`, triggers bytes 1 and 4,
    /// PointsValid `do return false; end`).
    pub(crate) fn points_objective() -> ObjectiveDef {
        let mut o = obj(
            "global_points_objective",
            0,
            &[1, 4],
            json!([["do", [["return", [["bin", ">=", ["native", "ObjectivePoints.GetGroupAccumulatedPoints", []], ["native", "ObjectivePoints.GetGroupPointsRequirement", []]]]]]]]),
            Some(json!([["do", [["return", [["bool", false]]]]]])),
        );
        o.requires_highlight = false;
        o
    }

    /// An OTS spot objective (ots_dwtn_01_0): mode 5, SequenceComplete, EnteredVolume, IsSequenceNewlyCompletePoints.
    pub(crate) fn spot(volume: u64) -> ObjectiveDef {
        obj(
            "spot",
            5,
            &[1],
            json!([["do", [["return", [["native", "ObjectiveTrigger.EnteredVolume", [["str", volume.to_string()], ["str", "DIST|spot"]]]]]]]]),
            Some(json!([["do", [["return", [["native", "ObjectivePoints.IsSequenceNewlyCompletePoints", []]]]]]])),
        )
    }

    /// OTS by hand (0x82c6da68 / 0x82c74290 / 0x82c6db50 / 0x82c6f2b8): the spot counts only when entered during an
    /// active sequence; its score lands only when it completes; the points objective then decides Owned or not enough.
    #[test]
    fn a_spot_completes_on_a_sequence_through_it() {
        let def = GroupDef { point_requirement: Some(1000), objectives: vec![Some(spot(77))] };
        let mut g = Group::new(0, &def, &points_objective()).unwrap();
        assert_eq!((g.status(0), g.status(1)), (ENABLED, ACTIVE), "mode 5 waits for a sequence; the points objective is active");
        // Entered before the sequence: not noted (the objective is only Enabled).
        assert!(g.trigger_enter(77, &mut NoSkater).is_empty());
        g.sequence_begin();
        assert_eq!(g.status(0), ACTIVE);
        let out = g.sequence_complete(900, false, &mut NoSkater);
        assert!(out.is_empty(), "never entered during the sequence: {out:?}");
        assert_eq!(g.status(0), ENABLED, "re-armed");
        g.sequence_begin();
        g.trigger_enter(77, &mut NoSkater);
        let out = g.sequence_complete(900, false, &mut NoSkater);
        assert_eq!(
            out,
            vec![Out::Complete { index: 0, requires_highlight: true, points: false }, Out::PointsAdded { index: 0, points: 900 }, Out::NotEnoughPoints]
        );
        assert_eq!(g.accumulated(), 900);
        g.reset();
        assert_eq!((g.accumulated(), g.status(0)), (0, ENABLED));
        g.sequence_begin();
        g.trigger_enter(77, &mut NoSkater);
        let out = g.sequence_complete(1200, false, &mut NoSkater);
        assert_eq!(out.last(), Some(&Out::AllComplete), "{out:?}");
        assert!(out.contains(&Out::Complete { index: 1, requires_highlight: false, points: true }));
    }

    /// The per-frame update, markers, the trick list, FailAll and the challenge-start activation (0x82c6f998,
    /// 0x82c6eff8, 0x82c74a20, 0x82c6f540, 0x82c61158).
    #[test]
    fn ticks_markers_and_tricks() {
        let ret = |native: &str, args: Json| json!([["do", [["return", [["native", native, args]]]]]]);
        let marker = obj("m", 0, &[ON_TICK], ret("ObjectiveSessionMarker.JustUsedSessionMarker", json!([])), None);
        let trick = obj("t", 0, &[ON_TICK], ret("Tricks.DidSpecificTrick", json!([["str", "indy"]])), None);
        let def = GroupDef { point_requirement: None, objectives: vec![Some(marker), Some(trick)] };
        let mut g = Group::new(0, &def, &points_objective()).unwrap();
        assert!(g.tick(&mut NoSkater).is_empty());
        assert!(g.marker(true, &mut NoSkater).is_empty(), "no byte 5: noted only");
        assert_eq!(g.tick(&mut NoSkater), vec![Out::Complete { index: 0, requires_highlight: true, points: false }]);
        g.trick("ollie");
        assert!(g.tick(&mut NoSkater).is_empty(), "the marker flag went with the last tick; no indy");
        g.trick("indy");
        assert_eq!(g.tick(&mut NoSkater), vec![Out::Complete { index: 1, requires_highlight: true, points: false }, Out::AllComplete]);
        assert_eq!(g.completed(), 2);
        g.reset();
        g.enabled = false;
        g.trick("indy");
        g.enabled = true;
        assert!(g.tick(&mut NoSkater).is_empty(), "evaluation off: the trick is not listed");
        g.fail_all();
        assert_eq!((g.status(0), g.status(1)), (FAILED, FAILED));
        let start = GroupDef { point_requirement: None, objectives: vec![Some(obj("s", 1, &[ON_TICK], json!([]), None))] };
        let mut g = Group::new(0, &start, &points_objective()).unwrap();
        assert_eq!(g.status(0), ENABLED);
        g.activate_start();
        assert_eq!(g.status(0), ACTIVE);
    }

    /// Flow.Activate/DeactivateObjective (0x82c630a0 / 0x82c63168 -> 0x82c75850), Reset.ResetAllObjectives
    /// (0x82c63fc0 -> 0x82c6f430) from inside a pass, GetTotalNumCompletedObjectives' arguments (0x82c6f5a8).
    #[test]
    fn flow_natives_and_a_reset_inside_a_pass() {
        let call = |native: &str, args: Json| json!(["native", native, args]);
        let ret = |e: Json| json!([["do", [["return", [e]]]]]);
        // Objective 0 resets the group, then answers false; objective 1 (Active again after the reset) completes in
        // the same pass; objective 2 is a mode-5 objective (Enabled until a sequence).
        let reset = json!([["do", [["call", call("Reset.ResetAllObjectives", json!([]))], ["return", [["bool", false]]]]]]);
        let def = GroupDef {
            point_requirement: None,
            objectives: vec![Some(obj("r", 0, &[ON_TICK], reset, None)), Some(obj("t", 0, &[ON_TICK], ret(json!(["bool", true])), None)), Some(obj("s", 5, &[], ret(json!(["bool", true])), None))],
        };
        let mut g = Group::new(0, &def, &points_objective()).unwrap();
        g.deactivate_objective(1);
        g.deactivate_objective(2);
        assert_eq!((g.status(1), g.status(2)), (DISABLED, DISABLED));
        assert!(g.activate_objective(2));
        assert_eq!(g.status(2), ENABLED, "mode 5 activates Enabled");
        assert!(g.activate_objective(2), "Enabled stays, true");
        assert!(!g.activate_objective(9), "no such objective");
        g.deactivate_objective(1);
        assert_eq!(g.tick(&mut NoSkater), vec![Out::Complete { index: 1, requires_highlight: true, points: false }]);
        assert_eq!((g.status(0), g.status(1)), (ACTIVE, COMPLETE), "the reset re-activated objective 1 before its turn");
        g.fail_all();
        assert!(!g.activate_objective(1), "a Failed objective is not activated");
        // The counts: the points objective on argument 1, an Int16-over-1 objective on argument 2.
        let mut heavy = obj("h", 0, &[ON_TICK], ret(json!(["bool", true])), None);
        heavy.count_kind = 5;
        let def = GroupDef { point_requirement: Some(0), objectives: vec![Some(heavy), Some(obj("l", 0, &[ON_TICK], ret(json!(["bool", true])), None))] };
        let mut g = Group::new(0, &def, &points_objective()).unwrap();
        assert_eq!(g.tick(&mut NoSkater).last(), Some(&Out::AllComplete), "the points objective (requirement 0) on the second pass");
        assert_eq!([g.completed_with(true, true), g.completed_with(false, true), g.completed_with(false, false), g.completed_with(true, false)], [3, 2, 1, 2]);
        let n = |g: &mut Group, args: Json| {
            let s = Snippet::compile(&snippet(ret(call("ObjectiveInfo.GetTotalNumCompletedObjectives", args)))).unwrap();
            let vars = Value::table();
            let mut globals = HashMap::new();
            let mut natives = GroupNatives { group: g, index: 0, params: Params::default(), skater: &mut NoSkater };
            s.run(&vars, &mut globals, &mut natives).unwrap()
        };
        assert_eq!(n(&mut g, json!([])), Value::Num(3.));
        assert_eq!(n(&mut g, json!([["bool", false]])), Value::Num(2.), "argument 2 defaults to true");
        assert_eq!(n(&mut g, json!([["bool", false], ["bool", false]])), Value::Num(1.));
    }

    /// Unique-trick tracking (group+120): the sequence's tricks (0x82c72160, 12 kept), the first new one in the group
    /// at SequenceComplete (0x82c6dbec), NumUniqueTrickDone (0x82c69de8), AddLastUniqueTrickToList (0x82c61900), the
    /// reset (0x82c6f4a0).
    #[test]
    fn unique_tricks() {
        struct Flips;
        impl Skater for Flips {
            fn native(&mut self, _: &str, _: &[Value]) -> Option<Value> {
                None
            }
            fn in_group(&self, group: &str, trick: &str) -> bool {
                group == "flips" && trick.ends_with("flip")
            }
        }
        let def = GroupDef { point_requirement: None, objectives: vec![Some(obj("x", 0, &[ON_TICK], json!([]), None))] };
        let mut g = Group::new(0, &def, &points_objective()).unwrap();
        let sequence = |g: &mut Group, tricks: &[&str], aborted: bool| {
            g.sequence_begin();
            tricks.iter().for_each(|t| g.trick(t));
            g.sequence_complete(100, aborted, &mut Flips);
            g.unique_done()
        };
        assert_eq!(sequence(&mut g, &["kickflip"], false), 0, "no tracking without a group");
        g.unique_group = Some("flips".into());
        assert_eq!(sequence(&mut g, &["ollie", "kickflip", "heelflip"], false), 1);
        assert_eq!(sequence(&mut g, &["ollie"], false), 0, "each sequence decides anew");
        assert_eq!(sequence(&mut g, &["kickflip"], false), 1);
        assert_eq!(g.add_last_unique(), 1);
        assert_eq!(sequence(&mut g, &["kickflip"], false), 1, "already kept");
        assert_eq!(sequence(&mut g, &["heelflip"], true), 1, "an aborted sequence decides nothing");
        let thirteen: Vec<&str> = std::iter::repeat_n("ollie", 12).chain(["heelflip"]).collect();
        assert_eq!(sequence(&mut g, &thirteen, false), 1, "the 13th trick is not listed");
        assert_eq!(sequence(&mut g, &["heelflip"], false), 2);
        assert_eq!(g.add_last_unique(), 2);
        g.reset();
        assert_eq!(g.unique_done(), 0);
    }

    /// An OTL tag row's shape (otl_bbx_01_tag_01): a counting objective (Int16 3, second pass) sets its fraction to the
    /// tags done and completes at 3; the tags (DisplayInHUD false) score only through it. ForceObjectiveComplete
    /// completes once, posting objectivecomplete then allobjectivecomplete; a forced objective counts only with `all`.
    #[test]
    fn fractions_and_forced_objectives_score() {
        let call = |native: &str, args: Json| json!(["native", native, args]);
        let total = call("ObjectiveInfo.GetTotalNumCompletedObjectives", json!([["bool", false]]));
        let counter_tree = json!([["do", [["call", call("ObjectiveGameInfo.SetFractionValue", json!([total]))], ["return", [["bin", ">=", total, ["num", 3.0]]]]]]]);
        let mut counter = obj("count", 0, &[SECOND_PASS], counter_tree, None);
        counter.count_kind = 3;
        let tag = |v: u64| {
            let mut o = obj("tag", 0, &[ON_TRIGGER], json!([["do", [["return", [call("ObjectiveTrigger.EnteredVolume", json!([["str", v.to_string()]]))]]]]]), None);
            o.display_in_hud = false;
            o
        };
        let def = GroupDef { point_requirement: None, objectives: vec![Some(counter), Some(tag(1)), Some(tag(2)), Some(tag(3)), Some(tag(4))] };
        let mut g = Group::new(0, &def, &points_objective()).unwrap();
        g.trigger_enter(1, &mut NoSkater);
        g.trigger_enter(2, &mut NoSkater);
        assert_eq!(g.completed_for_scoring(true), 2, "the counter's fraction, not the hidden tags");
        assert_eq!(g.status(0), ACTIVE);
        let out = g.trigger_enter(3, &mut NoSkater);
        assert_eq!(out, vec![Out::Complete { index: 3, requires_highlight: true, points: false }, Out::Complete { index: 0, requires_highlight: true, points: false }]);
        assert_eq!(g.completed_for_scoring(true), 3);
        g.reset();
        assert_eq!(g.completed_for_scoring(true), 3, "a reset keeps the fraction (0x82c73eb8, r5 = 0)");
        // Forcing: the last objective completes everything.
        let def = GroupDef { point_requirement: None, objectives: vec![Some(spot(1)), Some(spot(2))] };
        let mut g = Group::new(0, &def, &points_objective()).unwrap();
        assert_eq!(g.force_complete(0), vec![Out::Complete { index: 0, requires_highlight: false, points: false }]);
        assert!(g.force_complete(0).is_empty(), "already Complete");
        assert_eq!(g.force_complete(1).last(), Some(&Out::AllComplete));
        assert_eq!((g.completed_for_scoring(true), g.completed_for_scoring(false)), (2, 0));
    }

    #[test]
    fn a_wiped_sequence_is_not_evaluated() {
        let def = GroupDef { point_requirement: Some(10), objectives: vec![Some(spot(5))] };
        let mut g = Group::new(0, &def, &points_objective()).unwrap();
        g.sequence_begin();
        g.trigger_enter(5, &mut NoSkater);
        assert!(g.sequence_complete(500, true, &mut NoSkater).is_empty());
        assert_eq!(g.status(0), ENABLED);
        g.enabled = false;
        g.sequence_begin();
        assert_eq!(g.status(0), ENABLED, "evaluation off: the thunks skip the group");
    }

    // ------------------------------------------------ plan Task 25: what the lessons need of the runtime
    fn ret(e: Json) -> Json {
        json!([["do", [["return", [e]]]]])
    }
    fn answer(b: bool) -> Json {
        ret(json!(["bool", b]))
    }
    /// A group of these objectives under ObjectiveFlow `flow`, no PointRequirement.
    fn flowed(flow: [u32; 2], objectives: Vec<ObjectiveDef>) -> Group {
        let def = GroupDef { point_requirement: None, objectives: objectives.into_iter().map(Some).collect() };
        Group::with_flow(0, &def, &points_objective(), flow).unwrap()
    }
    fn statuses(g: &Group) -> Vec<u8> {
        (0..g.len()).map(|i| g.status(i)).collect()
    }
    /// The names of the groups started since the last call.
    fn shown(g: &mut Group) -> Vec<String> {
        g.presentations().into_iter().map(|p| p.name).collect()
    }
    /// ShowHUD, an event that holds nothing.
    fn show() -> Event {
        Event::from_args(9, &[], 0)
    }
    /// The objective with an intro and a complete group.
    fn presented(mut o: ObjectiveDef) -> ObjectiveDef {
        (o.groups.intro, o.groups.complete) = (vec![show()], vec![show()]);
        o
    }
    /// The objective with these hints, each with a group.
    fn hinted(mut o: ObjectiveDef, hints: Vec<Json>) -> ObjectiveDef {
        o.groups.hints = hints.iter().map(|_| vec![show()]).collect();
        o.hints = hints.into_iter().map(|t| Some(snippet(t))).collect();
        o
    }
    /// True the first time only: `vars.played`.
    fn once() -> Json {
        let played = json!(["index", ["vars"], ["str", "played"]]);
        json!([["if", [[["un", "not", played], [["set", [played], [["bool", true]]], ["return", [["bool", true]]]]]], null], ["return", [["bool", false]]]])
    }

    /// ActivationMode 1 (0x82c75850 / 0x82c61158): Enabled by the flow, Active at ActivateChallengeStartObjectives, which
    /// sets every mode-1 objective Active without checking its status.
    #[test]
    fn mode_1_waits_for_activate_start() {
        let def = GroupDef { point_requirement: None, objectives: vec![Some(obj("s", 1, &[ON_TICK], answer(true), None))] };
        assert!(Group::supported(&def, &points_objective(), &[]).is_ok(), "the gate takes mode 1");
        let mut g = Group::new(0, &def, &points_objective()).unwrap();
        assert_eq!(g.status(0), ENABLED);
        assert!(g.tick(&mut NoSkater).is_empty(), "an Enabled objective is not evaluated");
        g.activate_start();
        assert_eq!(g.tick(&mut NoSkater), vec![Out::Complete { index: 0, requires_highlight: true, points: false }, Out::AllComplete]);
        g.activate_start();
        assert_eq!(g.status(0), ACTIVE, "a Complete one too");
        let volume = GroupDef { point_requirement: None, objectives: vec![Some(obj("v", 2, &[ON_TICK], answer(true), None))] };
        assert!(Group::supported(&volume, &points_objective(), &[]).is_err(), "mode 2 (the activation volume) is not ported");
    }

    /// ObjectiveFlow {type, count} (0x82c6d168) at MakeObjectives, the five lessons' flows (stage1_school 2): type 0
    /// every objective, type 2 the first `count` by position (at least one), any other type the first `count` that
    /// activate; the points objective last, outside the count.
    #[test]
    fn flow_type_0_2_and_1() {
        let lesson = |flow, modes: &[u32]| {
            let mut g = flowed(flow, modes.iter().map(|&m| obj("o", m, &[ON_TICK], answer(false), None)).collect());
            g.activate_start();
            statuses(&g)
        };
        assert_eq!(lesson([0, 1], &[1, 1, 1]), [ACTIVE; 3], "tut_sksc_01 / 03: every objective");
        assert_eq!(lesson([2, 2], &[1, 1, 0, 0]), [ACTIVE, ACTIVE, DISABLED, DISABLED], "tut_sksc_02: the first two");
        assert_eq!(lesson([2, 0], &[0, 0]), [ACTIVE, DISABLED], "type 2 activates at least one");
        assert_eq!(lesson([1, 1], &[1, 0, 0]), [ACTIVE, DISABLED, DISABLED], "tut_sksc_06: one at a time");
        assert_eq!(lesson([1, 2], &[1, 1, 0, 0]), [ACTIVE, ACTIVE, DISABLED, DISABLED], "tut_sksc_04: the first two");
        let def = GroupDef { point_requirement: Some(0), objectives: vec![Some(obj("a", 0, &[], answer(false), None)), Some(obj("b", 0, &[], answer(false), None))] };
        let g = Group::with_flow(0, &def, &points_objective(), [1, 1]).unwrap();
        assert_eq!(statuses(&g), [ACTIVE, DISABLED, ACTIVE], "the points objective is not counted");
    }

    /// 0x82c6f7d8: a pass that completed an objective runs the flow again; type 1 brings the next one in and puts the
    /// later Enabled / Active ones out (tut_sksc_06 {1,1}, tut_sksc_04 {1,2}).
    #[test]
    fn the_flow_reruns_after_a_completing_pass() {
        let mut g = flowed([1, 1], vec![obj("a", 1, &[ON_TICK], answer(true), None), obj("b", 0, &[ON_TICK], answer(false), None), obj("c", 0, &[ON_TICK], answer(false), None)]);
        g.activate_start();
        assert!(g.activate_objective(2), "Flow.ActivateObjective out of turn");
        assert_eq!(statuses(&g), [ACTIVE, DISABLED, ACTIVE]);
        assert_eq!(g.tick(&mut NoSkater), vec![Out::Complete { index: 0, requires_highlight: true, points: false }]);
        assert_eq!(statuses(&g), [COMPLETE, ACTIVE, DISABLED], "the next one in, the later one out");
        let four = ["a", "b", "c", "d"].iter().enumerate().map(|(i, k)| obj(k, u32::from(i < 2), &[ON_TICK], answer(i == 0), None)).collect();
        let mut g = flowed([1, 2], four);
        g.activate_start();
        g.tick(&mut NoSkater);
        assert_eq!(statuses(&g), [COMPLETE, ACTIVE, ACTIVE, DISABLED], "one after another");
    }

    /// SetStatus 0x82c73fc8 starts ObjIntroEvents on every call with Active and ObjCompleteEvents on every call with
    /// Complete, changed or not (stage1_traces 4.8); Enabled starts nothing.
    #[test]
    fn set_status_4_and_8_restart_their_groups_even_unchanged() {
        let mut master = presented(obj("m", 1, &[ON_MARKER], answer(true), None));
        master.master = true;
        let mut g = flowed([0, 1], vec![master, presented(obj("t", 1, &[ON_TICK], answer(true), None))]);
        assert!(shown(&mut g).is_empty(), "Enabled");
        g.activate_start();
        assert_eq!(shown(&mut g), ["m/intro", "t/intro"]);
        g.activate_start();
        assert_eq!(shown(&mut g), ["m/intro", "t/intro"], "Active again, unchanged");
        g.tick(&mut NoSkater);
        assert_eq!(shown(&mut g), ["t/complete"], "the flow's re-run leaves an Active objective alone");
        g.marker(false, &mut NoSkater);
        assert_eq!(shown(&mut g), ["m/complete", "t/complete"], "the master completes t again");
        assert!(shown(&mut g).is_empty(), "taken");
    }

    /// Evaluate 0x82c752a8 (FailFirst false): the objective, then the hints (0x82c755b8: the first true one in list
    /// order starts its group), then the failures; in the frame an objective completes no hint runs (stage1_traces 4.3).
    #[test]
    fn the_first_true_hint_in_list_order_wins() {
        let hints = || vec![answer(false), answer(true), answer(true)];
        let mut g = flowed([0, 1], vec![hinted(obj("a", 0, &[ON_TICK], answer(false), None), hints()), hinted(presented(obj("b", 0, &[ON_TICK], answer(true), None)), hints())]);
        assert_eq!(shown(&mut g), ["b/intro"]);
        g.tick(&mut NoSkater);
        assert_eq!(shown(&mut g), ["a/hint1", "b/complete"]);
    }

    /// StartPresentation 0x82dbaca0 ignores a group that is playing: a hint true every frame replays only once its group
    /// has ended (stage1_traces 4.6-4.7).
    #[test]
    fn a_true_hint_replays_only_after_its_group_ends() {
        use crate::skate3_present::{Presenter, tests::Rec};
        let mut o = obj("o", 0, &[ON_TICK], answer(false), None);
        o.hints = vec![Some(snippet(answer(true)))];
        o.groups.hints = vec![vec![Event::from_args(4, &["1".into()], 0)]];
        let mut g = flowed([0, 1], vec![o]);
        let (mut presenter, mut stage) = (Presenter::default(), Rec::default());
        let mut frame = || {
            g.tick(&mut NoSkater);
            let started: Vec<bool> = g.presentations().into_iter().map(|p| presenter.start(p)).collect();
            (started, presenter.tick(0.5, &mut stage))
        };
        assert_eq!(frame(), (vec![true], vec![]));
        assert_eq!(frame(), (vec![false], vec![]), "still playing: ignored");
        assert_eq!(frame(), (vec![false], vec!["o/hint0".to_owned()]));
        assert_eq!(frame().0, [true], "ended: it plays again");
    }

    /// Only the snippets' own `vars` flags stop a hint repeating; ResetObjectives drops `vars` (0x82c75c48), so it plays
    /// again after (stage1_traces 4.7, 6.4).
    #[test]
    fn vars_flags_stop_repeats_until_reset_objectives() {
        let mut g = flowed([0, 1], vec![hinted(obj("o", 0, &[ON_TICK], answer(false), None), vec![once()])]);
        g.tick(&mut NoSkater);
        assert_eq!(shown(&mut g), ["o/hint0"]);
        g.tick(&mut NoSkater);
        assert!(shown(&mut g).is_empty(), "vars.played");
        g.reset();
        g.tick(&mut NoSkater);
        assert_eq!(shown(&mut g), ["o/hint0"], "the reset dropped vars");
    }

    /// A hint starts its group and nothing else: no status change, no objectivefailed; the failures still run after it,
    /// and the hint shares the objective's `vars` (stage1_traces 4.3-4.5).
    #[test]
    fn a_hint_changes_no_status_and_posts_no_objectivefailed() {
        let n = json!(["index", ["vars"], ["str", "n"]]);
        let init = json!(["call", ["native", "InitVars", [["vars"], ["str", "n"], ["num", 0.0]]]]);
        let count = json!([init, ["set", [n], [["bin", "+", n, ["num", 1.0]]]], ["return", [["bool", true]]]]);
        let mut o = hinted(obj("o", 0, &[ON_TICK], json!([init, ["return", [["bin", ">=", n, ["num", 2.0]]]]]), None), vec![count]);
        let mut g = flowed([0, 1], vec![o.clone()]);
        assert!(g.tick(&mut NoSkater).is_empty(), "the hint is true");
        assert_eq!((g.status(0), shown(&mut g)), (ACTIVE, vec!["o/hint0".to_owned()]));
        assert!(g.tick(&mut NoSkater).is_empty());
        assert_eq!(g.tick(&mut NoSkater), vec![Out::Complete { index: 0, requires_highlight: true, points: false }, Out::AllComplete], "the objective read the hint's count");
        o.failures = vec![Some(snippet(answer(true)))];
        let mut g = flowed([0, 1], vec![o]);
        assert_eq!(g.tick(&mut NoSkater), vec![Out::Failed { index: 0, failed: 0 }], "a failure after the hint");
        assert_eq!(shown(&mut g), ["o/hint0"]);
    }

    /// Objectives.ResetObjective(g, i, true, false) (0x82c6f4d0 -> 0x82c73eb8): the progress goes (the volumes entered,
    /// the tricks, the points), the status and `vars` stay, and no intro plays (stage1_traces 6).
    #[test]
    fn reset_objective_keeps_status_and_vars() {
        let entered = ret(json!(["native", "ObjectiveTrigger.EnteredVolume", [["str", "77"], ["str", "DIST|spot"]]]));
        let mut g = flowed([0, 1], vec![hinted(presented(obj("o", 0, &[ON_TICK], entered, None)), vec![once()])]);
        assert_eq!(shown(&mut g), ["o/intro"]);
        g.tick(&mut NoSkater);
        assert_eq!(shown(&mut g), ["o/hint0"]);
        g.trigger_enter(77, &mut NoSkater);
        g.reset_objective(0);
        assert_eq!(g.status(0), ACTIVE);
        assert!(shown(&mut g).is_empty(), "no intro");
        assert!(g.tick(&mut NoSkater).is_empty(), "the volume went with the progress");
        assert!(shown(&mut g).is_empty(), "vars kept: the hint's flag stands");
        g.trigger_enter(77, &mut NoSkater);
        assert_eq!(g.tick(&mut NoSkater).first(), Some(&Out::Complete { index: 0, requires_highlight: true, points: false }));
        g.reset_objective(0);
        assert_eq!(g.status(0), COMPLETE, "the status stays");
    }

    /// The natives the lessons call that were missing (stage1_school 7): the group answers Flow.GetObjectiveStatus from
    /// the statuses as they are, eObjectiveStatus.Active and JustSetSessionMarker (+1165, 0x82c6ee98); the four that read
    /// the skater go to the [`Skater`].
    #[test]
    fn each_of_the_seven_natives() {
        struct Rider;
        impl Skater for Rider {
            fn native(&mut self, name: &str, _: &[Value]) -> Option<Value> {
                Some(match name {
                    "ObjectiveLocomotion.IsOnBoard" | "ObjectiveLocomotion.IsInAir" => Value::Bool(true),
                    "ObjectiveLocomotion.GetSpeed" => Value::Num(9.),
                    "Input.GetRawRightStickYValue" => Value::Num(-0.5),
                    _ => return None,
                })
            }
        }
        const SKATER: [&str; 4] = ["ObjectiveLocomotion.IsOnBoard", "ObjectiveLocomotion.GetSpeed", "ObjectiveLocomotion.IsInAir", "Input.GetRawRightStickYValue"];
        let call = |name: &str, args: Json| json!(["native", name, args]);
        let status = |i: f32, is: &str| json!(["bin", "==", call("Flow.GetObjectiveStatus", json!([["num", i]])), ["const", format!("Flow.eObjectiveStatus.{is}")]]);
        let rider = json!(["bin", "and", ["bin", "and", call(SKATER[0], json!([])), call(SKATER[2], json!([]))], ["bin", "and", ["bin", ">", call(SKATER[1], json!([])), ["num", 8.0]], ["bin", "<", call(SKATER[3], json!([])), ["num", 0.0]]]]);
        let mut seven = obj("seven", 0, &[ON_TICK], ret(rider.clone()), None);
        seven.obj.as_mut().unwrap().natives = SKATER.iter().map(|s| s.to_string()).chain(["Flow.GetObjectiveStatus", "const:Flow.eObjectiveStatus.Active", "ObjectiveSessionMarker.JustSetSessionMarker"].map(String::from)).collect();
        let def = GroupDef { point_requirement: None, objectives: vec![Some(seven)] };
        assert!(Group::supported(&def, &points_objective(), &SKATER).is_ok(), "the group answers the other three");
        assert!(Group::supported(&def, &points_objective(), &[]).is_err(), "the skater's four are the skater's");
        // Read as they are: b sees a complete in the same pass; c sees b Active until then.
        let mut g = flowed([0, 1], vec![obj("a", 0, &[ON_TICK], answer(true), None), obj("b", 0, &[ON_TICK], ret(status(0., "Complete")), None)]);
        assert_eq!(g.tick(&mut NoSkater).len(), 3, "a, b, all complete");
        let watch = || flowed([0, 1], vec![obj("c", 0, &[ON_TICK], ret(status(1., "Active")), None), obj("d", 1, &[], answer(false), None)]);
        assert!(watch().tick(&mut NoSkater).is_empty(), "d is Enabled, not Active");
        let mut g = watch();
        g.activate_start();
        assert_eq!(g.tick(&mut NoSkater).len(), 1, "d is Active");
        // JustSetSessionMarker: set, not used, until the tick's end.
        let mut g = flowed([0, 1], vec![obj("m", 0, &[ON_MARKER], ret(call("ObjectiveSessionMarker.JustSetSessionMarker", json!([]))), None)]);
        assert!(g.marker(true, &mut NoSkater).is_empty(), "a use is not a set");
        assert_eq!(g.marker(false, &mut NoSkater).len(), 2, "m, all complete");
        // The skater's: answered (an error would take the objective's default, true).
        let mut g = flowed([0, 1], vec![obj("r", 0, &[ON_TICK], ret(rider.clone()), None), obj("n", 0, &[ON_TICK], ret(json!(["un", "not", rider])), None)]);
        assert_eq!(g.tick(&mut Rider), vec![Out::Complete { index: 0, requires_highlight: true, points: false }]);
    }

    #[test]
    fn lua_values_follow_lua() {
        let s = Snippet::compile(&snippet(json!([["do", [
            ["call", ["native", "InitVars", [["vars"], ["str", "n"], ["num", 5.0]]]],
            ["set", [["index", ["vars"], ["str", "n"]]], [["bin", "+", ["index", ["vars"], ["str", "n"]], ["num", 1.0]]]],
            ["local", [0], [["bin", "or", ["global", "unset"], ["num", 7.0]]]],
            ["if", [[["bin", ">=", ["index", ["vars"], ["str", "n"]], ["slot", 0]], [["return", [["str", "x"]]]]]], [["return", [["bool", false]]]]]
        ]]])))
        .unwrap();
        let vars = Value::table();
        let mut globals = HashMap::new();
        let mut stub = Stub::default();
        assert_eq!(s.run(&vars, &mut globals, &mut stub).unwrap(), Value::Bool(false), "6 >= 7 is false");
        assert_eq!(s.run(&vars, &mut globals, &mut stub).unwrap(), Value::Str("x".into()), "vars persist: 7 >= 7");
        assert!(!s.decide(false, &vars, &mut globals, &mut stub), "a string result takes the default");
        let bad = Snippet::compile(&snippet(json!([["return", [["bin", ">=", ["global", "nothing"], ["num", 1.0]]]]]))).unwrap();
        assert!(bad.decide(true, &vars, &mut globals, &mut stub), "comparing nil errors: the default");
        assert_eq!(fmt_g14(0.1f32 as f64), "0.10000000149012");
        assert_eq!((fmt_g14(1500.0), fmt_g14(1e14), fmt_g14(0.0001), fmt_g14(0.00001)), ("1500".into(), "1e+14".into(), "0.0001".into(), "1e-05".into()));
    }

    /// The oracle's natives: a value fixed by the native's name and call count (the Lua side's `pick` below).
    #[derive(Default)]
    struct Stub(HashMap<String, u32>);
    impl Natives for Stub {
        fn call(&mut self, name: &str, _: &[Value]) -> Result<Value, LuaError> {
            let n = self.0.entry(name.into()).or_default();
            *n += 1;
            let h = *n + name.bytes().map(u32::from).sum::<u32>();
            Ok(match h % 9 {
                0 => Value::Bool(true),
                1 => Value::Bool(false),
                2 => Value::Num(0.),
                3 => Value::Num(1.),
                4 => Value::Num(2.5),
                5 => Value::Num(1000.),
                6 => Value::Num(3.),
                7 => Value::Str("x".into()),
                _ => Value::Nil,
            })
        }
        fn constant(&mut self, name: &str) -> Result<Value, LuaError> {
            Ok(constant(name).unwrap_or(Value::Num(7.)))
        }
    }

    /// The disc's Lua side: the snippet wrapped as the manager wraps it (0x820b88f4) after the prelude's InitVars
    /// (0x820b87d0), natives stubbed as [`Stub`], numbers shown after rounding to the disc's f32.
    const HARNESS: &str = r#"
local pool = {true, false, 0, 1, 2.5, 1000, 3, "x"}
local calls = {}
local function pick(name)
  local n = (calls[name] or 0) + 1; calls[name] = n
  local h = n
  for i = 1, #name do h = h + name:byte(i) end
  local i = h % 9
  if i == 8 then return nil end
  return pool[i + 1]
end
local function show(v)
  local t = type(v)
  if t == "number" then return string.format("%.14g", (string.unpack("f", string.pack("f", v))))
  elseif t == "string" then return string.format("%q", v)
  elseif t == "table" then
    local items = {}
    for k, x in pairs(v) do items[#items + 1] = show(k) .. "=" .. show(x) end
    table.sort(items)
    return "{" .. table.concat(items, ",") .. "}"
  end
  return tostring(v)
end
function InitVars( table, name, value ) if( table[ name ] == nil ) then table[ name ] = value; end end
function install(name)
  local const = name:match("^const:(.*)$")
  local path = const or name
  local parts = {}
  for p in path:gmatch("[^.]+") do parts[#parts + 1] = p end
  local t = _G
  for i = 1, #parts - 1 do t[parts[i]] = t[parts[i]] or {}; t = t[parts[i]] end
  local last = parts[#parts]
  if const then
    if path == "Flow.eObjectiveStatus" then t[last] = {Disabled = 1, Enabled = 2, Active = 4, Complete = 8, Failed = 16}
    elseif t[last] == nil then t[last] = 7 end
  elseif name ~= "InitVars" then
    t[last] = function(...) return pick(name) end
  end
end
function run()
  local vars = {}
  local function once() local ok, v = pcall(h, vars); if ok then return show(v) end return "error" end
  local a = once()
  local b = once()
  return a .. ";" .. b .. ";" .. show(vars)
end
"#;

    fn lua_side(text: &str, natives: &[String]) -> Option<String> {
        let lua = mlua::Lua::new();
        lua.load(HARNESS).exec().unwrap();
        let install: mlua::Function = lua.globals().get("install").unwrap();
        for n in natives {
            install.call::<()>(n.as_str()).unwrap();
        }
        // Lua 5.4 compiles the disc's 5.1 text; one it does not take is reported, not compared.
        lua.load(format!("function h( vars ) {text} end")).exec().ok()?;
        lua.load("return run()").eval::<String>().ok()
    }

    fn rust_side(data: &SnippetData) -> String {
        let s = Snippet::compile(data).unwrap();
        let (vars, mut globals, mut stub) = (Value::table(), HashMap::new(), Stub::default());
        let mut once = || s.run(&vars, &mut globals, &mut stub).map_or("error".to_owned(), |v| v.show());
        let (a, b) = (once(), once());
        format!("{a};{b};{}", vars.show())
    }

    /// Every objective snippet on the disc (challengebanks/main.bin) through this interpreter and through Lua, twice
    /// with one `vars` (InitVars state), natives stubbed alike.
    #[test]
    #[ignore = "needs S3_OBJ_CORPUS: the corpus of parsed snippets (objres/oracle_corpus.py)"]
    fn snippets_match_the_disc_lua() {
        let path = std::env::var("S3_OBJ_CORPUS").expect("S3_OBJ_CORPUS");
        let corpus: Vec<Json> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (mut same, mut skipped, mut diffs) = (0, 0, Vec::new());
        for e in &corpus {
            let data = SnippetData { src: e["src"].as_u64().unwrap() as u32, tree: e["tree"].clone(), natives: serde_json::from_value(e["natives"].clone()).unwrap() };
            let Some(lua) = lua_side(e["text"].as_str().unwrap(), &data.natives) else {
                skipped += 1;
                continue;
            };
            let rust = rust_side(&data);
            if lua == rust {
                same += 1;
            } else {
                diffs.push(format!("{}: lua {lua} / rust {rust}", data.src));
            }
        }
        println!("objective snippets: {same} identical, {} differ, {skipped} not compiled by Lua 5.4", diffs.len());
        assert!(same > 0 && diffs.is_empty(), "{}", diffs.join("\n"));
    }
}
