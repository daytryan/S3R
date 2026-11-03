//! In-challenge speech (spec 15.3, stage1_traces 1): which voice file retail plays for a presentation's
//! `Speech` / `SpeechNoWait` (character, line id). The id tables are the exe constants in `speech_tables.json`; the
//! files are the export's `speech/` folder and its `speech.json` (Task 7). Nothing here plays audio. Not tested in play.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::Deserialize;

/// eSk8Characters values the play function 0x824a4ee8 tests.
pub const REDA: u32 = 0x1E;
pub const SKATE_COACH: u32 = 0x1F;
pub const UNKNOWN: u32 = 0x27;
pub const CAMERA_MAN: u32 = 0x28;
/// No character (AudioCharacterOverride / HostCharacter unset).
pub const NONE: u32 = 0x68;

/// Ids from here up are scripted `nis.big` lines (0x824a4f30).
const SCRIPTED: u32 = 5000;
/// The coach's quit line, the one scripted line that stops the current one (0x824a5224-0x824a5284).
const QUIT: (u32, u32) = (SKATE_COACH, 5706);
/// Cameraman event 702 retry, which sets its second condition to 4 (0x824accb0).
const RETRY_EVENT: u32 = 0x8003;

/// The exe's tables: the 104 names sorted by hash (0x8257b43c..), main cast 0x8224f1b8, cameraman 0x8224f400.
struct Tables {
    names: Vec<(u32, u32)>,
    maincast: Vec<(u32, u32)>,
    cameraman: Vec<(u32, u32)>,
}

static TABLES: LazyLock<Tables> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct Pairs {
        pairs: Vec<(u32, u32)>,
    }
    #[derive(Deserialize)]
    struct Json {
        maincast: Pairs,
        cameraman: Pairs,
        characters: Vec<(String, u32)>,
    }
    let json: Json = serde_json::from_str(include_str!("speech_tables.json")).expect("speech_tables.json parses");
    let mut names: Vec<_> = json.characters.iter().map(|(name, character)| (elf_hash(name), *character)).collect();
    names.sort_unstable();
    Tables { names, maincast: json.maincast.pairs, cameraman: json.cameraman.pairs }
});

/// The name hash 0x8296ea10 (ELF-style, but the high nibble folds in at bit 23, not 24).
#[must_use]
pub fn elf_hash(s: &str) -> u32 {
    s.bytes().fold(0, |h: u32, c| {
        let h = (h << 4).wrapping_add(u32::from(c));
        let g = h & 0xF000_0000;
        h ^ g ^ (g >> 23)
    })
}

/// A presentation's character name as the speech handler resolves it: a lower bound with no equality test over the
/// 104 names (0x8257c418-0x8257c44c), so an unknown name takes the next larger hash's character (`Slappy` is
/// Vacant10, 62). `Unknown` becomes `override_` unless that is [`NONE`] (0x8257c458-0x8257c4b8).
#[must_use]
pub fn speaker(name: &str, override_: u32) -> u32 {
    let names = &TABLES.names;
    let hash = elf_hash(name);
    // INFERRED (retail reads the map's end node past the largest hash; no name lands there in stage 1).
    let found = names.get(names.partition_point(|&(h, _)| h < hash)).map_or(NONE, |&(_, character)| character);
    if found == UNKNOWN && override_ != NONE { override_ } else { found }
}

/// A line's takes in play order (0x82482130): more than two are a deck shuffled by `count / 2` random swaps at the
/// start of each cycle (0x824821c0), so each take plays once per cycle; two alternate in directory order, first take
/// first; one always plays. Nothing resets it, so a Retry keeps the deck (stage1_traces 1.3).
#[derive(Clone, Debug)]
pub struct Deck {
    order: Vec<usize>,
    cursor: usize,
}

impl Deck {
    /// `count` takes in directory order.
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self { order: (0..count).collect(), cursor: 0 }
    }

    /// The next take's index. `rng(n)` is retail's `rand() % n` (0x82a8af10).
    ///
    /// # Panics
    ///
    /// On a deck of no takes.
    pub fn next(&mut self, rng: &mut impl FnMut(usize) -> usize) -> usize {
        let n = self.order.len();
        if self.cursor == 0 && n > 2 {
            for _ in 0..n / 2 {
                // INFERRED (a swap of two random indexes; 0x824821c0's body is summarized, not dumped).
                let (a, b) = (rng(n) % n, rng(n) % n);
                self.order.swap(a, b);
            }
        }
        let take = self.order[self.cursor];
        self.cursor = (self.cursor + 1) % n;
        take
    }
}

/// One line's files and their deck.
struct Line {
    files: Vec<String>,
    deck: Deck,
}

impl Line {
    fn new(files: Vec<String>) -> Self {
        Self { deck: Deck::new(files.len()), files }
    }

    /// A line with no file plays nothing (`62_5033`, stage1_traces 1.4).
    fn play(&mut self, dir: &Path, rng: &mut impl FnMut(usize) -> usize) -> Option<PathBuf> {
        if self.files.is_empty() {
            return None;
        }
        Some(dir.join(&self.files[self.deck.next(rng)]))
    }
}

/// A bank event: its condition kinds (the `.evt` record's condition list) and its entries' masks per condition.
struct Event {
    conditions: Vec<u32>,
    entries: Vec<(Vec<u32>, Line)>,
}

/// INFERRED (stage1_traces 1.5: the matcher 0x824f2df0 was not traced): an entry plays when each nonzero mask
/// shares a bit with its condition's value; a 0 mask is any. Kinds count from 1 (`query[0]` is kind 1).
fn matches(masks: &[u32], conditions: &[u32], query: [u32; 2]) -> bool {
    masks.iter().enumerate().all(|(i, &mask)| {
        let kind = conditions.get(i).map_or(i + 1, |&kind| kind as usize);
        mask == 0 || mask & kind.checked_sub(1).and_then(|k| query.get(k)).copied().unwrap_or(0) != 0
    })
}

/// A main-cast speaker's two mask words. INFERRED (from the bank's file names, stage1_traces 1.5): word 1 is bit
/// `character - 1` for 1..=29 (Fairfax 1, Haslam 4, MApple 0x200, LizKing 0x1000000); word 2 is Abita 2, Burnette
/// 4, Reda 8, SkateCoach 0x10, 77 0x80.
fn cast_masks(character: u32) -> [u32; 2] {
    let word1 = if (1..=29).contains(&character) { 1 << (character - 1) } else { 0 };
    let word2 = match character {
        38 => 2,
        37 => 4,
        REDA => 8,
        SKATE_COACH => 0x10,
        77 => 0x80,
        _ => 0,
    };
    [word1, word2]
}

/// `speech/speech.json` (Task 7).
#[derive(Deserialize)]
struct Index {
    scripted: HashMap<String, Vec<String>>,
    banks: HashMap<String, HashMap<u32, Vec<Entry>>>,
    /// Each event's condition kinds, in its entries' mask order; absent, the masks are kinds 1, 2, ...
    #[serde(default)]
    conditions: HashMap<String, HashMap<u32, Vec<u32>>>,
}

#[derive(Deserialize)]
struct Entry {
    masks: Vec<u32>,
    files: Vec<String>,
}

/// The speech object: scripted lines by (character, id) (the `nisresident.big` table 0x824a5a08), the main-cast and
/// cameraman bank events, and the scripted line playing (state 2 at +0x1bd28).
pub struct Speech {
    dir: PathBuf,
    scripted: HashMap<(u32, u32), Line>,
    maincast: HashMap<u32, Event>,
    cameraman: HashMap<u32, Event>,
    playing: Option<(u32, u32)>,
    rng: Box<dyn FnMut(usize) -> usize + Send + Sync>,
}

impl Speech {
    /// Reads `speech.json` (`index`); the files are under `dir`. `rng(n)` is the game's random source, an index below
    /// `n`.
    ///
    /// # Errors
    ///
    /// The index does not parse, or a scripted key is not `<character>_<id>`.
    pub fn new(index: &str, dir: &Path, rng: impl FnMut(usize) -> usize + Send + Sync + 'static) -> Result<Self, serde_json::Error> {
        let Index { scripted, mut banks, mut conditions } = serde_json::from_str(index)?;
        let mut lines = HashMap::new();
        for (key, files) in scripted {
            // atoi of both halves, as 0x824a5c58-0x824a5c70 reads the `.snr` names.
            let parsed = key.split_once('_').and_then(|(c, id)| Some((c.parse::<u32>().ok()?, id.parse::<u32>().ok()?)));
            let Some(line) = parsed else {
                return Err(serde::de::Error::custom(format!("speech.json: scripted key {key:?} is not <character>_<id>")));
            };
            lines.insert(line, Line::new(files));
        }
        let mut bank = |name: &str| -> HashMap<u32, Event> {
            let kinds = conditions.remove(name).unwrap_or_default();
            let events = banks.remove(name).unwrap_or_default();
            events
                .into_iter()
                .map(|(event, entries)| {
                    let conditions = kinds.get(&event).cloned().unwrap_or_default();
                    let entries = entries.into_iter().map(|e| (e.masks, Line::new(e.files))).collect();
                    (event, Event { conditions, entries })
                })
                .collect()
        };
        let (maincast, cameraman) = (bank("maincast"), bank("cameraman"));
        Ok(Self { dir: dir.to_owned(), scripted: lines, maincast, cameraman, playing: None, rng: Box::new(rng) })
    }

    /// The file to play for `character`'s line `id` (the override already applied, as [`speaker`] does) under a row
    /// whose HostCharacter is `host`, or `None` for nothing.
    ///
    /// Scripted (`id >= 5000`): dropped while a scripted line plays, except the coach's 5706, whose file comes back
    /// for the caller to play in place of the current scripted line. Bank lines play beside it (0x824a4ee8 tests the
    /// state on the scripted path only). Not retail (user 10-20): retail's "starting" state, in which a request fails,
    /// is the frame the stream opens; here a line plays the frame it is asked for, so 5706 always stops the current
    /// line.
    #[must_use]
    pub fn request(&mut self, character: u32, id: u32, host: u32) -> Option<PathBuf> {
        if id >= SCRIPTED {
            if self.playing.is_some() && (character, id) != QUIT {
                return None;
            }
            let path = self.scripted.get_mut(&(character, id))?.play(&self.dir, &mut self.rng)?;
            self.playing = Some((character, id));
            return Some(path);
        }
        let (table, events, mut query) = match character {
            CAMERA_MAN => {
                // The speaker mask by host (0x824acc5c-0x824acc94): 2 the `_30_` files, 4 `_31_`, 1 `_40_`.
                let speaker = match host {
                    REDA => 2,
                    SKATE_COACH => 4,
                    _ => 1,
                };
                // INFERRED (the second condition is 705's caller argument, not passed here; 1 picks win_sc1).
                (&TABLES.cameraman, &mut self.cameraman, [speaker, 1])
            }
            // INFERRED (retail asks 0x824f8248 / 0x824f7708 for a nearby world speaker; no stage-1 line reaches one:
            // every Unknown request that is not overridden is id 0 or 801, in no table).
            UNKNOWN => return None,
            // INFERRED (Cuz / Cyrus go to 0x824aa148, not traced, and the living-world records to 0x824ab590; neither
            // speaks in stage 1): every other character uses the main-cast bank (0x824ac050).
            _ => (&TABLES.maincast, &mut self.maincast, cast_masks(character)),
        };
        // An id in no table plays nothing (0x824ac09c, 0x824acc30).
        let event = table.iter().find(|&&(line, _)| line == id).map(|&(_, event)| event)?;
        if character == CAMERA_MAN && event == RETRY_EVENT {
            query[1] = 4;
        }
        let Event { conditions, entries } = events.get_mut(&event)?;
        // INFERRED (the entries' weights 0x39 / 0x2D are not applied; the first match plays).
        let (_, line) = entries.iter_mut().find(|(masks, _)| matches(masks, conditions, query))?;
        // INFERRED (which take of a `.dat` plays was not traced; the scripted deck stands in).
        line.play(&self.dir, &mut self.rng)
    }

    /// `character`'s line `id` ended (its speech-done message): a scripted line no longer blocks others. A stopped
    /// line's late end does not unblock the 5706 that replaced it.
    pub fn finished(&mut self, character: u32, id: u32) {
        if self.playing == Some((character, id)) {
            self.playing = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn seeded(mut s: u32) -> impl FnMut(usize) -> usize + Send + Sync + 'static {
        move |n| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as usize % n
        }
    }

    fn never(_: usize) -> usize {
        panic!("no shuffle for two takes or fewer")
    }

    const INDEX: &str = r#"{
        "scripted": {"30_8572": ["8572_1.wav", "8572_2.wav"], "31_5706": ["5706_1.wav"], "62_5033": []},
        "banks": {
            "cameraman": {"32783": [
                {"masks": [1], "files": ["709_40_oob_0.wav"]},
                {"masks": [2], "files": ["709_30_oob_0.wav"]},
                {"masks": [4], "files": ["709_31_oob_0.wav"]}
            ]},
            "maincast": {"195": [
                {"masks": [4, 0], "files": ["1010_37_burn_wrap_0.wav"]},
                {"masks": [8, 0], "files": ["1010_30_Reda_wrap_0.wav"]}
            ]}
        },
        "conditions": {"cameraman": {"32783": [1]}, "maincast": {"195": [2, 1]}}
    }"#;

    fn speech() -> Speech {
        Speech::new(INDEX, Path::new("speech"), seeded(7)).unwrap()
    }

    fn name(path: Option<PathBuf>) -> Option<String> {
        path.map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
    }

    #[test]
    fn a_three_take_line_plays_each_take_once_per_cycle() {
        let (mut deck, mut rng) = (Deck::new(3), seeded(1));
        let cycles: Vec<Vec<usize>> = (0..12).map(|_| (0..3).map(|_| deck.next(&mut rng)).collect()).collect();
        for cycle in &cycles {
            assert_eq!(cycle.iter().copied().collect::<BTreeSet<_>>(), BTreeSet::from([0, 1, 2]), "{cycles:?}");
        }
        assert!(cycles.iter().any(|c| *c != cycles[0]), "the deck reshuffles each cycle: {cycles:?}");
    }

    #[test]
    fn two_takes_alternate() {
        let mut deck = Deck::new(2);
        assert_eq!((0..5).map(|_| deck.next(&mut never)).collect::<Vec<_>>(), [0, 1, 0, 1, 0]);
    }

    #[test]
    fn one_take_repeats() {
        let mut deck = Deck::new(1);
        assert_eq!((0..3).map(|_| deck.next(&mut never)).collect::<Vec<_>>(), [0, 0, 0]);
    }

    #[test]
    fn a_request_during_a_scripted_line_is_dropped_except_5706() {
        let mut s = speech();
        assert_eq!(name(s.request(REDA, 8572, NONE)).as_deref(), Some("8572_1.wav"));
        assert_eq!(s.request(REDA, 8572, NONE), None);
        assert_eq!(name(s.request(SKATE_COACH, 5706, NONE)).as_deref(), Some("5706_1.wav"));
        s.finished(REDA, 8572);
        assert_eq!(s.request(REDA, 8572, NONE), None, "the stopped line's end does not unblock 5706");
        s.finished(SKATE_COACH, 5706);
        assert_eq!(name(s.request(REDA, 8572, NONE)).as_deref(), Some("8572_2.wav"), "dropped requests keep the deck");
    }

    #[test]
    fn slappy_is_vacant10_and_5033_plays_nothing() {
        assert_eq!(elf_hash("Slappy"), 0x05A2_8779);
        assert_eq!(elf_hash("Vacant10"), 0x0798_5CC0);
        assert_eq!(speaker("Slappy", CAMERA_MAN), 62);
        assert_eq!(speaker("Reda", CAMERA_MAN), REDA);
        assert_eq!(speaker("Unknown", CAMERA_MAN), CAMERA_MAN);
        assert_eq!(speaker("Unknown", NONE), UNKNOWN);
        let mut s = speech();
        assert_eq!(s.request(62, 5033, NONE), None);
        assert!(s.request(REDA, 8572, NONE).is_some(), "a line with no file blocks nothing");
    }

    #[test]
    fn an_id_in_no_table_plays_nothing() {
        let mut s = speech();
        assert_eq!(s.request(CAMERA_MAN, 0, REDA), None);
        assert_eq!(s.request(REDA, 0, NONE), None);
        assert_eq!(s.request(UNKNOWN, 801, REDA), None);
        assert_eq!(name(s.request(REDA, 1010, NONE)).as_deref(), Some("1010_30_Reda_wrap_0.wav"));
    }

    #[test]
    fn the_cameraman_bank_follows_the_host() {
        let mut s = speech();
        for (host, file) in [
            (REDA, "709_30_oob_0.wav"),
            (SKATE_COACH, "709_31_oob_0.wav"),
            (CAMERA_MAN, "709_40_oob_0.wav"),
            (NONE, "709_40_oob_0.wav"),
        ] {
            assert_eq!(name(s.request(CAMERA_MAN, 709, host)).as_deref(), Some(file), "host {host:#x}");
        }
    }

    #[test]
    #[ignore = "requires the skate3-career export (SKATE3_CAREER_DIR)"]
    fn the_university_table_resolves_row_by_row() {
        let dir = PathBuf::from(std::env::var_os("SKATE3_CAREER_DIR").expect("set SKATE3_CAREER_DIR")).join("speech");
        let index = std::fs::read_to_string(dir.join("speech.json")).unwrap();
        let mut s = Speech::new(&index, &dir, seeded(3)).unwrap();
        // stage1_traces 1.6, the override applied: ftl hosts None (Reda override), photo_univ_08 / film_univ_05 Reda.
        let rows: &[(u32, u32, u32, Option<(&str, usize)>)] = &[
            (REDA, 8572, NONE, Some(("30_8572_PRO-CHRI_", 2))),
            (REDA, 8575, NONE, Some(("30_8575_", 3))),
            (REDA, 8569, NONE, Some(("30_8569_", 3))),
            (REDA, 8566, NONE, Some(("30_8566_", 2))),
            (REDA, 8564, NONE, Some(("30_8564_", 3))),
            (REDA, 1010, NONE, Some(("1010_30_Reda_wrap_", 7))),
            (REDA, 0, NONE, None),
            (UNKNOWN, 0, NONE, None),
            (REDA, 309, NONE, Some(("309_30_follow_behind_", 13))),
            (REDA, 308, NONE, Some(("308_30_follow_chatter_", 21))),
            (REDA, 310, NONE, Some(("310_30_follow_pass_", 8))),
            (REDA, 1703, NONE, Some(("1703_30_Reda_decline_", 11))),
            (CAMERA_MAN, 1015, REDA, Some(("1015_30_Reda_pickone_", 10))),
            (CAMERA_MAN, 1307, REDA, Some(("1307_30_Reda_start_", 8))),
            (CAMERA_MAN, 709, REDA, Some(("709_30_oob_", 8))),
            (REDA, 1016, REDA, Some(("1016_30_wrap_idea_", 10))),
            (CAMERA_MAN, 1011, REDA, Some(("1011_30_Reda_PC_Win_", 11))),
            (CAMERA_MAN, 0, REDA, None),
            (CAMERA_MAN, 1703, REDA, Some(("1703_30_decline_", 6))),
            (CAMERA_MAN, 712, REDA, Some(("712_30_replay_", 10))),
            (CAMERA_MAN, 701, REDA, Some(("701_30_start_", 10))),
            (CAMERA_MAN, 702, REDA, Some(("702_30_retry_rt3_", 7))),
            (CAMERA_MAN, 711, REDA, Some(("711_30_almost_", 8))),
            (CAMERA_MAN, 705, REDA, Some(("705_30_win_sc1_", 5))),
            (CAMERA_MAN, 801, REDA, Some(("801_30_FilmChalQuit_", 10))),
            (UNKNOWN, 801, REDA, None),
            (CAMERA_MAN, 713, REDA, Some(("713_30_timewarn_", 13))),
            (CAMERA_MAN, 707, REDA, None),
            (CAMERA_MAN, 706, REDA, None),
            (CAMERA_MAN, 701, CAMERA_MAN, Some(("701_40_start_", 31))),
            (CAMERA_MAN, 709, NONE, Some(("709_40_oob_", 15))),
            (speaker("Slappy", NONE), 5033, NONE, None),
            (UNKNOWN, 1307, CAMERA_MAN, None),
        ];
        for &(character, id, host, expect) in rows {
            let Some((prefix, takes)) = expect else {
                assert_eq!(s.request(character, id, host), None, "{character}:{id}");
                continue;
            };
            let mut seen = BTreeSet::new();
            for _ in 0..takes * 2 {
                let file = name(s.request(character, id, host)).unwrap_or_else(|| panic!("{character}:{id} plays"));
                assert!(file.starts_with(prefix), "{character}:{id} played {file}");
                seen.insert(file);
                s.finished(character, id);
            }
            assert_eq!(seen.len(), takes, "{character}:{id}");
        }
    }
}
