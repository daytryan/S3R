//! The trick stats Skate 1 and Skate 2 put on their trick displays, measured from the engine's own scoring:
//! Skate 2's big-trick callouts (trickdisplay2's ContextTrickDone: "Air Time 8s", "Manual Distance 12.3 m") and
//! Skate 1's personal bests (its trickdisplay's mcPersonalbest: "LONGEST AIR TIME 2.53 s"). Skate 3 shows neither.
//! The retail evidence is in skate2port/TRICK_HUD_STATS_REPORT.md; the disc data comes in each HUD manifest's
//! `trick_stats` (skate2port/old_hud.py, old_hud1.py).
use crate::{apt_vm::Value, scoring_runtime::Runtime as Scoring};
use std::path::{Path, PathBuf};

/// A measurement as an element ends: metres, seconds, counts or points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stat {
    Manual,
    Grind,
    Powerslide,
    Grab,
    AirTime,
    AirDistance,
    AirHeight,
    StepUp,
    Drop,
    GrassGap,
    StairGap,
    HighGap,
    VeryHighGap,
    SequenceScore,
    SequenceTime,
    SequenceTricks,
    LineScore,
    LineTime,
    LineTricks,
}
use Stat::*;

/// One tick of the scoring runtime, as the sampler reads it.
#[derive(Clone, Default)]
pub(crate) struct Tap {
    riding: bool,
    in_air: bool,
    grinding: bool,
    /// Each carrier slot's (scorable id, class, seconds held, metres covered).
    carriers: [Option<(usize, u32, f32, f32)>; 4],
    /// Take-off position, peak height, this frame's position, the four gap runs (grass, stairs, 3 m, 6 m drops).
    air: ([f32; 3], f32, [f32; 3], [f32; 4]),
    sequence_open: bool,
    sequence_landed: bool,
    bailed: bool,
    announced: u32,
    sequence_score: f32,
    line: f32,
}
impl Tap {
    pub(crate) fn of(s: &Scoring) -> Self {
        Self {
            riding: s.riding(),
            in_air: s.in_air(),
            grinding: s.grinding(),
            carriers: s.carrier_metrics(),
            air: s.air_measure(),
            sequence_open: s.sequence_open(),
            sequence_landed: s.sequence_landed,
            bailed: s.close_tricks,
            announced: s.announced.len() as u32,
            sequence_score: s.sequence_score,
            line: s.session.holder.snapshot.line,
        }
    }
}

/// Reports each element's measurement the tick it ends. The scoring runtime zeroes a carrier's held time and
/// distance the tick it finishes, so the last sample stands for it (one tick short).
#[derive(Default)]
pub(crate) struct Sampler {
    live: Vec<(Stat, f32)>,
    air_ticks: u32,
    /// The open sequence's ticks and tricks.
    sequence: (u32, u32),
    /// The open line's score, ticks and tricks.
    line: (f32, u32, u32),
}
impl Sampler {
    pub(crate) fn sample(&mut self, t: &Tap, dt: f32) -> Vec<(Stat, f32)> {
        let mut out = Vec::new();
        // The ground collector's powerslide (slot 0, scorables 0/1) and manual (slot 1, 2/3), a grind, a grab (class 2).
        let mut live = Vec::new();
        match (t.grinding, t.in_air, t.carriers[0], t.carriers[1]) {
            (true, _, Some((_, _, _, metres)), _) => live.push((Grind, metres)),
            (_, true, Some((_, 2, held, _)), _) => live.push((Grab, held)),
            (false, false, slide, manual) => {
                if let Some((0 | 1, _, _, metres)) = slide {
                    live.push((Powerslide, metres));
                }
                if let Some((2 | 3, _, _, metres)) = manual {
                    live.push((Manual, metres));
                }
            }
            _ => {}
        }
        for &(stat, value) in &self.live {
            // Gone, or started over (a new manual counts from zero): the last sample was the element's.
            // ponytail: one bailed out of reports nothing; retail submits Skate 1's bests every frame, so it keeps those.
            if t.riding && value > 0. && !live.iter().any(|&(s, v)| s == stat && v >= value) {
                out.push((stat, value));
            }
        }
        self.live = live;
        let air_ticks = if t.in_air { 0 } else { std::mem::take(&mut self.air_ticks) };
        if t.in_air {
            self.air_ticks += 1;
        } else if air_ticks > 0 && t.riding {
            let (start, peak, at, gaps) = t.air;
            out.extend([
                (AirTime, air_ticks as f32 * dt),
                (AirDistance, (at[0] - start[0]).hypot(at[2] - start[2])),
                (AirHeight, peak - start[1]),
            ]);
            let step = at[1] - start[1];
            out.push(if step >= 0. { (StepUp, step) } else { (Drop, -step) });
            out.extend([GrassGap, StairGap, HighGap, VeryHighGap].into_iter().zip(gaps).filter(|(_, m)| *m > 0.));
        }
        self.sequence.1 += t.announced;
        if t.sequence_open {
            self.sequence.0 += 1;
        } else {
            let (ticks, tricks) = std::mem::take(&mut self.sequence);
            if t.sequence_landed {
                out.extend([(SequenceScore, t.sequence_score), (SequenceTime, ticks as f32 * dt), (SequenceTricks, tricks as f32)]);
                self.line.2 += tricks;
            }
        }
        // The line runs from its first trick until its score is gone; a bail loses it.
        if t.line <= 0. && self.line.0 > 0. {
            let (score, ticks, tricks) = std::mem::take(&mut self.line);
            if !t.bailed {
                out.extend([(LineScore, score), (LineTime, ticks as f32 * dt), (LineTricks, tricks as f32)]);
            }
        }
        if t.line > 0. {
            self.line.0 = t.line;
        }
        if t.line > 0. || t.sequence_open {
            self.line.1 += 1;
        } else if self.line.0 <= 0. {
            self.line = Default::default();
        }
        out
    }
}

/// Skate 2's context-trick slots (the manager at [0x82d98bb0]: 7 slots of 40 bytes at +260), here only its big-trick
/// callouts. ponytail: the entity ones ("On/Off/Over" a car or a passer-by) have no engine counterpart.
#[derive(Default)]
pub(crate) struct BigTricks {
    slots: [Option<BigTrick>; 7],
    dirty: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct BigTrick {
    kind: u32,
    achievement: u32,
    value: f32,
    timer: i32,
}
/// A callout stays this long after its last report (0x82472338's `li r6,2000`; ms by the 2 s it reads as, INFERRED).
const SHOWN_MS: i32 = 2000;
impl BigTricks {
    /// AddBigTrick, 0x82472338: at most three; the same type again restarts its timer and raises its achievement or
    /// measurement where higher (only then redrawn); otherwise the first free slot.
    pub(crate) fn add(&mut self, kind: u32, achievement: u32, value: f32) {
        if self.slots.iter().flatten().count() >= 3 {
            return;
        }
        if let Some(t) = self.slots.iter_mut().flatten().find(|t| t.kind == kind) {
            t.timer = SHOWN_MS;
            if achievement > t.achievement {
                t.achievement = achievement;
                self.dirty = true;
            }
            if value > t.value {
                t.value = value;
                self.dirty = true;
            }
            return;
        }
        if let Some(free) = self.slots.iter_mut().find(|s| s.is_none()) {
            *free = Some(BigTrick { kind, achievement, value, timer: SHOWN_MS });
            self.dirty = true;
        }
    }
    /// The manager's update, 0x823bd0a8: count the callouts down. True when the screen's ContextTrickDone is due (HUD
    /// call 4, after anything changed).
    pub(crate) fn tick(&mut self, ms: i32) -> bool {
        for slot in &mut self.slots {
            if let Some(t) = slot {
                t.timer -= ms;
                if t.timer <= 0 {
                    *slot = None;
                    self.dirty = true;
                }
            }
        }
        std::mem::take(&mut self.dirty)
    }
    /// Tricks.TrickDisplay_GetAllTrickData (0x82508c70): the slot count, then nine values a slot (active, kind 0 = big
    /// trick, its type, achievement and measurement, then the entity fields).
    pub(crate) fn data(&self) -> Vec<Value> {
        let mut out = vec![Value::Number(self.slots.len() as f64)];
        for slot in &self.slots {
            let field = |f: fn(&BigTrick) -> f64| Value::Number(slot.as_ref().map_or(-1., f));
            out.extend([
                Value::Bool(slot.is_some()),
                Value::Number(0.),
                field(|t| t.kind as f64),
                field(|t| t.achievement as f64),
                field(|t| t.value as f64),
                Value::Number(0.),
                Value::Bool(false),
                Value::Number(0.),
                Value::Number(0.),
            ]);
        }
        out
    }
}

/// Skate 2's callout for a measurement, (type, achievement), by the manifest's `thresholds` ([big, huge] per type).
pub(crate) fn big_trick(stats: &serde_json::Value, stat: Stat, value: f32) -> Option<(u32, u32)> {
    let kind = threshold_type(stat)?;
    let limit = |i: usize| stats["thresholds"][kind as usize][i].as_f64().map(|v| v as f32);
    Some((kind, achievement(value, [limit(0)?, limit(1)?])?))
}

/// eScoreThresholdType (0x8250dd78's registrations) of an engine measurement. HandplantHeight (3) has none.
fn threshold_type(stat: Stat) -> Option<u32> {
    Some(match stat {
        Manual => 0,
        Grind => 1,
        Powerslide => 2,
        Grab => 4,
        AirTime => 5,
        AirDistance => 6,
        AirHeight => 7,
        StepUp => 8,
        Drop => 9,
        SequenceTricks => 10,
        GrassGap => 11,
        StairGap => 12,
        HighGap => 13,
        VeryHighGap => 14,
        _ => return None,
    })
}

/// eScoreThreshold_Big (0) or _Huge (1) for a measurement against its type's [big, huge] (the skater vault's
/// ScoreThresholdDataSet). The threshold scan 0x8233e3a8 checks Huge first. ponytail: `>=` (INFERRED; the flag
/// writer wasn't found).
fn achievement(value: f32, [big, huge]: [f32; 2]) -> Option<u32> {
    (value >= huge).then_some(1).or((value >= big).then_some(0))
}

/// The Skate 1 personal best a measurement counts toward (its table at 0x830a37a8, 0-based). The chase bests (3, 7,
/// 8) and Most Broken Bones (15) have no engine counterpart.
pub(crate) fn best_index(stat: Stat) -> Option<usize> {
    Some(match stat {
        SequenceScore => 0,
        SequenceTime => 1,
        SequenceTricks => 2,
        LineScore => 4,
        LineTime => 5,
        LineTricks => 6,
        AirTime => 9,
        Powerslide => 10,
        Manual => 11,
        Grind => 12,
        AirHeight => 13,
        Drop => 14,
        _ => return None,
    })
}

#[derive(serde::Deserialize, Clone, Debug)]
struct Best {
    label: String,
    minimum: f32,
    /// 0x82330718's jump table: "number", "time" or "distance".
    format: String,
}
/// Skate 1's text for a value (0x82330718), with its strings from the manifest.
#[derive(serde::Deserialize, Clone, Debug, Default)]
struct Formats {
    bests: Vec<Best>,
    /// ID_COMMON_METRIC_TIME, ID_COMMON_HYPHEN and the distance format (string hash 0x7f263aa8).
    time: String,
    hyphen: String,
    distance: String,
    /// The imperial distance format (0xbe745eef, "{0:f.1} ft"); empty in a manifest from before Units.
    #[serde(default)]
    distance_feet: String,
}

/// Skate 1's personal bests: the 16-row table (best, minimum, title), kept for the profile beside its skater
/// (settings/skate1-personal-bests.json, by title label).
#[derive(Default)]
pub(crate) struct Bests {
    formats: Formats,
    values: Vec<f32>,
    pending: Vec<usize>,
    path: Option<PathBuf>,
}
impl Bests {
    pub(crate) fn load(stats: &serde_json::Value, path: &Path) -> Self {
        let formats: Formats = serde_json::from_value(stats.clone()).unwrap_or_default();
        let saved: serde_json::Map<String, serde_json::Value> = crate::saves::load_json(path).ok().unwrap_or_default();
        let values = formats.bests.iter().map(|b| saved.get(&b.label).and_then(|v| v.as_f64()).unwrap_or(0.) as f32).collect();
        Self { formats, values, pending: Vec::new(), path: Some(path.into()) }
    }
    /// Submit (0x82330988) and pend (0x82331b90) in one: above both the best and the minimum, it is the new best.
    pub(crate) fn submit(&mut self, index: usize, value: f32) {
        let Some(best) = self.formats.bests.get(index) else { return };
        if value > self.values[index] && value > best.minimum {
            self.values[index] = value;
            if !self.pending.contains(&index) {
                self.pending.push(index);
            }
        }
    }
    /// Commit (0x82331aa0): the new bests, (title label, value text) in table order, for HUD call 16
    /// (UpdatePersonalBests); saved for the profile. `imperial`: Settings > Units.
    pub(crate) fn commit(&mut self, imperial: bool) -> Vec<(String, String)> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        self.pending.sort_unstable();
        let pending = std::mem::take(&mut self.pending);
        let out = pending.into_iter().map(|i| (self.formats.bests[i].label.clone(), self.text(i, imperial))).collect();
        if let Err(error) = self.save() {
            bevy::log::warn!("Skate 1 personal bests not saved ({error})");
        }
        out
    }
    fn save(&self) -> std::io::Result<()> {
        // a launcher hero clip run is never the player's session
        let Some(path) = self.path.as_ref().filter(|_| !crate::hero_clip::running()) else { return Ok(()) };
        let map: serde_json::Map<_, _> = self.formats.bests.iter().zip(&self.values).map(|(b, v)| (b.label.clone(), (*v).into())).collect();
        crate::saves::write_atomic(path, &serde_json::to_vec_pretty(&map)?)
    }
    /// 0x82330718: a hyphen until the best clears its minimum; a whole number with thousands commas (0x828850e0);
    /// a time cut to hundredths, "S.FF", "M:SS.FF" or "H:MM:SS" (0x82332378, 0x82883d40) and the time unit; metres
    /// to one place, or with Units on imperial (settings +18904 == 1) x3.0 feet (0x82331c80) in the feet format.
    fn text(&self, index: usize, imperial: bool) -> String {
        let (best, value, f) = (&self.formats.bests[index], self.values[index], &self.formats);
        if value < best.minimum {
            return f.hyphen.clone();
        }
        match best.format.as_str() {
            "time" => {
                let (h, m) = ((value / 3600.) as u32, (value / 60.) as u32 % 60);
                let (s, hundredths) = (value as u32 % 60, (value.fract() * 100.) as u32);
                let clock = match (h, m) {
                    (0, 0) => format!("{s}.{hundredths:02}"),
                    (0, _) => format!("{m}:{s:02}.{hundredths:02}"),
                    _ => format!("{h}:{m:02}:{s:02}"),
                };
                format!("{clock} {}", f.time)
            }
            "distance" if imperial && !f.distance_feet.is_empty() => f.distance_feet.replace("{0:f.1}", &format!("{:.1}", value * 3.)),
            "distance" => f.distance.replace("{0:f.1}", &format!("{value:.1}")),
            _ => crate::hud_runtime::score(value as i32),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_callouts_at_most_and_a_repeat_only_raises() {
        let mut b = BigTricks::default();
        b.add(5, 0, 7.5);
        assert!(b.tick(16));
        b.add(5, 0, 7.2); // lower: timer restarts, nothing to redraw
        assert!(!b.tick(16));
        b.add(5, 1, 10.4);
        assert!(b.tick(16));
        assert_eq!(b.data()[3..6], [Value::Number(5.), Value::Number(1.), Value::Number(10.4f32 as f64)]);
        b.add(0, 0, 12.);
        b.add(1, 0, 11.);
        b.add(6, 0, 15.); // a fourth is dropped
        b.tick(16);
        assert_eq!(b.slots.iter().flatten().count(), 3);
        // Two seconds after the last report each one goes (and the screen plays its outro).
        let mut gone = 0;
        while b.slots.iter().any(Option::is_some) {
            gone += 1;
            b.tick(16);
        }
        assert!((120..=126).contains(&gone), "{gone} ticks");
    }

    #[test]
    fn achievement_takes_huge_first() {
        assert_eq!(achievement(6.9, [7., 10.]), None);
        assert_eq!(achievement(7., [7., 10.]), Some(0));
        assert_eq!(achievement(12., [7., 10.]), Some(1));
    }

    fn bests() -> Bests {
        let stats = serde_json::json!({
            "bests": [
                {"label": "ID_RECORD_BESTS_01", "minimum": 500., "format": "number"},
                {"label": "ID_RECORD_BESTS_10", "minimum": 2., "format": "time"},
                {"label": "ID_RECORD_BESTS_12", "minimum": 15., "format": "distance"},
            ],
            "time": "s", "hyphen": "-", "distance": "{0:f.1} m", "distance_feet": "{0:f.1} ft"});
        Bests { formats: serde_json::from_value(stats).unwrap(), values: vec![0.; 3], pending: Vec::new(), path: None }
    }

    #[test]
    fn a_best_must_beat_the_old_one_and_the_minimum() {
        let mut b = bests();
        b.submit(1, 1.9);
        assert!(b.commit(false).is_empty());
        b.submit(1, 2.534);
        b.submit(0, 12_345.9);
        b.submit(2, 15.04);
        assert_eq!(b.commit(false), [
            ("ID_RECORD_BESTS_01".into(), "12,345".into()),
            ("ID_RECORD_BESTS_10".into(), "2.53 s".into()),
            ("ID_RECORD_BESTS_12".into(), "15.0 m".into()),
        ]);
        // Units on imperial: x3.0 in the feet format (a manifest without one keeps metres).
        assert_eq!(b.text(2, true), "45.1 ft");
        b.formats.distance_feet.clear();
        assert_eq!(b.text(2, true), "15.0 m");
        b.submit(1, 2.5);
        assert!(b.commit(false).is_empty());
        b.values[1] = 75.25;
        assert_eq!(b.text(1, false), "1:15.25 s");
        b.values[1] = 3725.;
        assert_eq!(b.text(1, false), "1:02:05 s");
        b.values[1] = 1.;
        assert_eq!(b.text(1, false), "-");
    }

    fn air(at: [f32; 3]) -> Tap {
        Tap { riding: true, in_air: true, air: ([0., 10., 0.], 13., at, [0., 0., 4.5, 0.]), ..Default::default() }
    }

    #[test]
    fn an_air_reports_at_its_landing() {
        let mut s = Sampler::default();
        for _ in 0..5 {
            assert!(s.sample(&air([3., 11., 4.]), 0.5).is_empty());
        }
        let landed = s.sample(&Tap { in_air: false, ..air([6., 7., 8.]) }, 0.5);
        assert_eq!(landed, [(AirTime, 2.5), (AirDistance, 10.), (AirHeight, 3.), (Drop, 3.), (HighGap, 4.5)]);
        // A bail reports nothing.
        s.sample(&air([0.; 3]), 1. / 60.);
        assert!(s.sample(&Tap { riding: false, in_air: false, ..air([0.; 3]) }, 1. / 60.).is_empty());
    }

    #[test]
    fn a_manual_reports_its_last_sample_as_it_ends_or_restarts() {
        let manual = |id, metres| Tap { riding: true, carriers: [None, Some((id, 0, 0., metres)), None, None], ..Default::default() };
        let mut s = Sampler::default();
        assert!(s.sample(&manual(2, 5.), 0.016).is_empty());
        assert!(s.sample(&manual(2, 12.3), 0.016).is_empty());
        assert_eq!(s.sample(&manual(3, 0.), 0.016), [(Manual, 12.3)]);
        assert!(s.sample(&manual(3, 4.), 0.016).is_empty());
        assert_eq!(s.sample(&Tap { riding: true, ..Default::default() }, 0.016), [(Manual, 4.)]);
    }

    #[test]
    fn a_sequence_and_its_line_report_when_they_end() {
        let tick = |open, landed, announced, line| Tap {
            riding: true, sequence_open: open, sequence_landed: landed, announced, sequence_score: 900., line, ..Default::default()
        };
        let mut s = Sampler::default();
        s.sample(&tick(true, false, 1, 0.), 0.5);
        s.sample(&tick(true, false, 2, 0.), 0.5);
        assert_eq!(s.sample(&tick(false, true, 0, 900.), 0.5), [(SequenceScore, 900.), (SequenceTime, 1.), (SequenceTricks, 3.)]);
        s.sample(&tick(false, false, 0, 900.), 0.5);
        assert_eq!(s.sample(&tick(false, false, 0, 0.), 0.5), [(LineScore, 900.), (LineTime, 2.), (LineTricks, 3.)]);
    }
}
