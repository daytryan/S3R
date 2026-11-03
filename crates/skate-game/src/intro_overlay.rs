//! The online intros' OVERLAY track: the front-end screen each camera take keys, as retail's NIS sends it
//! (cMsgNISFEEvent: S3 0x82675b68 / 0x82683aa8, S2 0x825a38a0, S1 0x824b25b0) to the screen table (S3 0x8302f1a8,
//! S2 0x82d6de98, S1 0x8309e9b0):
//! Spot Found's corner card (challenge/spotfound), Skate 2's photo snap (hudComponents/phototreatment) and its San Van TV
//! logo (nis/sanvantv). The movies play their own sounds (FE_SOUND_SPOT_FOUND, FE_SOUND_PHOTO_FLYTHROUGH, Skate 1's
//! Audio.PlaySpotFound): Skate 2's cut sounds are these (the intros' posts are flat beds).
//! .superpowers/sdd/2026-10-09-bug-list/item10-flashes.md B, C, D1.
//!
//! Skate 1's table is traced (StateManagerBackEnd 0x82351eb0: a change plays the old corner's Outro, then opens the new
//! one; Clear only closes). ponytail: Skate 2's and 3's native id -> (screen, state) tables aren't; each pairing is the
//! names' (overlay SpotFound_<Corner> <-> spotfound's kScreenState_Intro<Corner>), the same as Skate 1's traced one. A
//! scene that ends with a screen up plays it out as a Clear would (I).
use crate::{
    apt_screen::{ONLINE, SKATE1_ONLINE, SKATE2_ONLINE, Screen, Screens},
    challenge::cutscene::Cutscene,
    config::Config,
    match_core::Match,
};
use bevy::prelude::*;
use serde_json::json;
use std::{collections::BTreeSet, path::Path};

/// Long enough for every outro to finish before its screen closes: spotfound's 12 (S2) / 19 (S3) frames, sanvantv's
/// hide 9, phototreatment's whole 26-frame snapshot (60 fps).
const OUTRO: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Game {
    Skate1,
    Skate2,
    Skate3,
    /// The Skate 1 tutorial's scenes (challenges-skate1/nis): the pro intros' bio card and sponsor logos.
    Skate1Tutorial,
}
impl Game {
    /// Whose online intros a scene folder holds (`private/skate1-online/intros`, `private/skate2-online/intros`,
    /// `private/skate3-online/spot_battle`).
    fn of(root: &Path) -> Option<Self> {
        root.components().find_map(|c| match c.as_os_str().to_str() {
            Some("skate1-online") => Some(Self::Skate1),
            Some("skate2-online") => Some(Self::Skate2),
            Some("skate3-online") => Some(Self::Skate3),
            Some("challenges-skate1") => Some(Self::Skate1Tutorial),
            _ => None,
        })
    }
    fn layer(self) -> u8 {
        match self {
            Self::Skate1 => SKATE1_ONLINE,
            Self::Skate2 => SKATE2_ONLINE,
            Self::Skate3 => ONLINE,
            Self::Skate1Tutorial => 1,
        }
    }
    /// The exported movies (old_online1.py's HUD_MOVIES; old_online_hud.py --intro-overlays; skate3_online.py's SCREENS).
    fn movies(self) -> &'static str {
        match self {
            Self::Skate1 => "private/hud-skate1/online/runtime",
            Self::Skate2 => "private/hud-skate2/online/runtime",
            Self::Skate3 => "private/skate3-online/hud/runtime",
            Self::Skate1Tutorial => crate::skate1_screens::TUTORIAL_MOVIES,
        }
    }
    /// How long a played-out screen stays: the bio card's outros are 34 frames of 16 ms.
    fn outro(self) -> f32 {
        if self == Self::Skate1Tutorial { 0.6 } else { OUTRO }
    }
}

/// An overlay id's screen: the movie, the SetState that opens it, and the one it plays out with when the track moves
/// on (phototreatment has none: its snapshot fades by itself).
#[derive(Debug, PartialEq)]
struct Overlay {
    movie: &'static str,
    open: i64,
    close: Option<i64>,
}

fn overlay(game: Game, id: u8) -> Option<Overlay> {
    // SpotFound_BottomLeft/BottomRight/TopLeft/TopRight -> spotfound's Intro/Outro<Corner> (the same enum in S1-S3;
    // Skate 1's rows 0x823521fc.., 0x82352870..).
    let corner = |(open, close)| Some(Overlay { movie: "spotfound", open, close: Some(close) });
    match (game, id) {
        (_, 2) => corner((4, 5)),
        (Game::Skate1 | Game::Skate2, 3) => corner((0, 1)),
        (Game::Skate1 | Game::Skate2, 4) => corner((6, 7)),
        (Game::Skate1 | Game::Skate2, 5) => corner((2, 3)),
        // Take_Photo: SetState(1) "snapshot".
        (Game::Skate2, 54) => Some(Overlay { movie: "phototreatment", open: 1, close: None }),
        // SanVanTV: kScreenState_Show 0 / Clear -1 (the logo's show / hide).
        (Game::Skate2, 84) => Some(Overlay { movie: "sanvantv", open: 0, close: Some(-1) }),
        // overnight-1015 C2, RETAIL SetSubState enter 0x82352018 / exit 0x82352688 (table index id - 2):
        // ProBioInfoOverlay_Left/Right -> pro_bioinfo_overlay IntroLeft 0 / OutroLeft 1, IntroRight 2 / OutroRight 3.
        (Game::Skate1Tutorial, 10) => Some(Overlay { movie: "pro_bioinfo_overlay", open: 0, close: Some(1) }),
        (Game::Skate1Tutorial, 11) => Some(Overlay { movie: "pro_bioinfo_overlay", open: 2, close: Some(3) }),
        // SponsorLogo_* -> sponsorlogo_setup SetState(brand) on enter, kOutroLeft 50 on exit.
        (Game::Skate1Tutorial, 21..=52 | 64..=68 | 71..=78) => Some(Overlay { movie: "sponsorlogo_setup", open: brand(id), close: Some(50) }),
        _ => None,
    }
}

/// A SponsorLogo id's state: sponsorlogo_setup's brand enum (0 adidas .. 44 supra; RETAIL 0x82352018: 21-52 -> 0-31,
/// 64-68 -> 32-36, 71-78 -> 37-44).
fn brand(id: u8) -> i64 {
    i64::from(id) - match id {
        64..=68 => 32,
        71..=78 => 34,
        _ => 21,
    }
}

/// sponsorlogo_setup's LoadImage file for brand `state` (its own if-chain); 7's file isn't on the disc.
fn logo(state: i64) -> Option<&'static str> {
    const FILES: [&str; 45] = [
        "adidas", "alien", "almost", "autobahn", "bakerboard", "circa", "dcshoes", "", "emerica", "enjoi", "es", "etnies",
        "fallen", "flipboard", "girl", "globe", "gold", "hubba", "independent", "ipath", "lakai", "momentum", "mysteryboard",
        "nike", "osiris", "planbBoard", "real", "royal", "silver", "spitfire", "thunder", "zeroBoard", "bakerwheels",
        "flipwheels", "mysterywheels", "planbWheels", "zeroWheels", "venture", "alienwheels", "icecream", "fillmore",
        "blacklabel", "chocolate", "krooked", "supra",
    ];
    usize::try_from(state).ok().and_then(|i| FILES.get(i)).copied().filter(|f| !f.is_empty())
}

#[derive(Debug, PartialEq)]
enum Act {
    Open(Game, String, &'static str, i64),
    State(Game, String, i64),
    Close(Game, String),
}

/// The overlay id the track asked for last, the screen it put up, and the ones playing out.
#[derive(Default)]
struct Driver {
    id: u8,
    shown: Option<(Game, String, Option<i64>)>,
    closing: Vec<(Game, String, f32)>,
    opened: u32,
}
impl Driver {
    /// The running scene's overlay `id` (0 none) at `now`: on a change the shown screen plays out (closed `OUTRO`
    /// later) and the new id's opens, each under its own name (a photo snap re-keyed after a Clear is a new screen).
    fn step(&mut self, game: Option<Game>, id: u8, now: f32) -> Vec<Act> {
        let mut acts = Vec::new();
        if id != self.id {
            self.id = id;
            if let Some((game, name, close)) = self.shown.take() {
                if let Some(state) = close {
                    acts.push(Act::State(game, name.clone(), state));
                }
                self.closing.push((game, name, now + game.outro()));
            }
            if let Some((game, o)) = game.and_then(|g| Some((g, overlay(g, id)?))) {
                self.opened += 1;
                let name = format!("intro-overlay-{}", self.opened);
                acts.push(Act::Open(game, name.clone(), o.movie, o.open));
                self.shown = Some((game, name, o.close));
            }
        }
        self.closing.retain(|(game, name, at)| {
            let due = now >= *at;
            if due {
                acts.push(Act::Close(*game, name.clone()));
            }
            !due
        });
        acts
    }
}

pub(crate) struct IntroOverlayPlugin;
impl Plugin for IntroOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, drive.before(crate::apt_screen::render));
    }
}

fn drive(
    cutscene: Option<Res<Cutscene>>,
    screens: Option<ResMut<Screens>>,
    config: Res<Config>,
    m: Option<Res<Match>>,
    tutorial: Option<Res<crate::skate1_challenge::Skate1Challenge>>,
    time: Res<Time<Real>>,
    mut driver: Local<Driver>,
    mut missing: Local<BTreeSet<String>>,
) {
    let (Some(cutscene), Some(mut screens)) = (cutscene, screens) else { return };
    let game = cutscene.root().and_then(Game::of);
    for act in driver.step(game, cutscene.overlay(), time.elapsed_secs()) {
        match act {
            Act::Open(game, name, movie, state) => {
                let file = config.asset_root.join(game.movies()).join(format!("{movie}.json"));
                if missing.contains(movie) {
                    continue;
                }
                // The online / party branch of the card's text (the movie ORs the two): "Spot Battle" over the title
                // (Skate 1's names the spot by Mission.GetLocation, which its Match.title is).
                let title = m.as_ref().map_or("", |m| m.title.as_str());
                let mission = tutorial.as_deref().and_then(|t| t.running());
                let answers = [
                    ("Mission.GetIsOnlineMission", json!(true)),
                    ("Mission.GetIsPartyPlayMission", json!(false)),
                    ("Mission.GetTitle", json!(title)),
                    ("Mission.GetLocation", json!(title)),
                    ("NIS.GetProInfo", json!(crate::skate1_pros::pro_info(mission))),
                    // ponytail: the driver opens the logo itself (`-logo` below), so the movie loads none. Retail
                    // preloads the scene's logos into LoadArea (hudmgr+1732; UNTRACED where that list is filled).
                    ("NIS.GetLogosToLoad", json!([])),
                ];
                let opened = Screen::load(&file, &answers).and_then(|mut s| s.call("SetState", &[json!(state)]).map(|_| s));
                match opened {
                    Ok(mut screen) => {
                        if game == Game::Skate1Tutorial {
                            // The layer's root is challenges-skate1/screens; the movies' textures are hud-skate1's.
                            screen.own_assets(&file);
                        }
                        if let Some(stem) = (movie == "sponsorlogo_setup").then(|| logo(state)).flatten() {
                            let file = config.asset_root.join(game.movies()).join(format!("sponsorlogo_{stem}.json"));
                            match Screen::load(&file, &[]) {
                                Ok(mut logo) => {
                                    logo.own_assets(&file);
                                    screens.layer(game.layer()).open.insert(format!("{name}-logo"), logo);
                                }
                                Err(e) => warn!("intro overlay logo {stem}: {e}"),
                            }
                        }
                        screens.layer(game.layer()).open.insert(name, screen);
                    }
                    Err(e) => {
                        warn!("intro overlay {movie}: {e}");
                        missing.insert(movie.to_owned());
                    }
                }
            }
            Act::State(game, name, state) => {
                if let Some(screen) = screens.layer(game.layer()).get(&name)
                    && let Err(e) = screen.call("SetState", &[json!(state)])
                {
                    warn!("intro overlay {name}: {e}");
                }
            }
            Act::Close(game, name) => {
                screens.layer(game.layer()).close(&name);
                screens.layer(game.layer()).close(&format!("{name}-logo"));
            }
        }
    }
    // loadMovie: each logo draws in its card's LoadArea (sponsorlogo_setup's createEmptyMovieClip there).
    let open = &mut screens.layer(1).open;
    // Only the open logos (review 10-15 minor 7: no per-screen format! each frame).
    let placed: Vec<(String, [f32; 6])> = open
        .keys()
        .filter_map(|logo| {
            let card = open.get(logo.strip_suffix("-logo")?)?;
            Some((logo.clone(), card.stage_matrix(card.clip("LoadArea")?)))
        })
        .collect();
    for (logo, m) in placed {
        if let Some(s) = open.get_mut(&logo) {
            s.place(m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_ids_open_their_screens_by_game() {
        assert_eq!(Game::of(Path::new("C:/a/assets/private/skate2-online/intros")), Some(Game::Skate2));
        assert_eq!(Game::of(Path::new("private/skate3-online/spot_battle")), Some(Game::Skate3));
        assert_eq!(Game::of(Path::new("private/skate1-online/intros")), Some(Game::Skate1));
        assert_eq!(Game::of(Path::new("private/skate2-tutorial")), None, "the tutorial keys its own (skate2_screens)");
        let open = |g, id| overlay(g, id).map(|o| (o.movie, o.open, o.close));
        assert_eq!(open(Game::Skate2, 2), Some(("spotfound", 4, Some(5))));
        assert_eq!(open(Game::Skate2, 3), Some(("spotfound", 0, Some(1))));
        assert_eq!(open(Game::Skate2, 4), Some(("spotfound", 6, Some(7))));
        assert_eq!(open(Game::Skate2, 5), Some(("spotfound", 2, Some(3))));
        assert_eq!(open(Game::Skate2, 54), Some(("phototreatment", 1, None)));
        assert_eq!(open(Game::Skate2, 84), Some(("sanvantv", 0, Some(-1))));
        assert_eq!(open(Game::Skate3, 2), Some(("spotfound", 4, Some(5))));
        // Skate 1's traced rows (item10-flashes.md D1): 3 BottomRight 0/1, 4 TopLeft 6/7, 5 TopRight 2/3.
        assert_eq!(open(Game::Skate1, 3), Some(("spotfound", 0, Some(1))));
        assert_eq!(open(Game::Skate1, 4), Some(("spotfound", 6, Some(7))));
        assert_eq!(open(Game::Skate1, 5), Some(("spotfound", 2, Some(3))));
        assert_eq!(open(Game::Skate1, 54), None, "Skate 1's photo overlay is 63, keyed by no intro");
        assert_eq!(open(Game::Skate3, 54), None, "Skate 3's 54 is another name");
        assert_eq!(open(Game::Skate2, 0), None);
    }

    /// ownspot_f_1a_01_ots_books_intro's track: three photo snaps with Clears between, then the corner card; the
    /// scene's end plays the card out.
    #[test]
    fn a_spot_battle_intro_snaps_three_photos_then_shows_the_card() {
        let g = Some(Game::Skate2);
        let mut d = Driver::default();
        let name = |n: u32| format!("intro-overlay-{n}");
        assert_eq!(d.step(g, 0, 0.), vec![]);
        assert_eq!(d.step(g, 54, 0.85), vec![Act::Open(Game::Skate2, name(1), "phototreatment", 1)]);
        assert_eq!(d.step(g, 0, 1.36), vec![]);
        assert_eq!(d.step(g, 54, 1.68), vec![Act::Open(Game::Skate2, name(2), "phototreatment", 1)]);
        assert_eq!(d.step(g, 54, 1.86), vec![Act::Close(Game::Skate2, name(1))], "the first snap closes, not the second");
        assert_eq!(d.step(g, 0, 2.15), vec![]);
        assert_eq!(d.step(g, 2, 5.17), vec![Act::Open(Game::Skate2, name(3), "spotfound", 4), Act::Close(Game::Skate2, name(2))]);
        // The scene ends (no game, overlay 0): the card plays its BottomLeft outro, then closes.
        assert_eq!(d.step(None, 0, 7.), vec![Act::State(Game::Skate2, name(3), 5)]);
        assert_eq!(d.step(None, 0, 7.49), vec![]);
        assert_eq!(d.step(None, 0, 7.5), vec![Act::Close(Game::Skate2, name(3))]);
        assert_eq!(d.step(None, 0, 9.), vec![]);
    }

    /// overnight-1015 C2: the pro intros' bio card and sponsor logos (RETAIL 0x82352018 / 0x82352688), e.g. P-Rod's
    /// track [0 11, 4.52 0, 8.10 46]: the card's IntroRight, its OutroRight, then Plan B's logo.
    #[test]
    fn a_skate1_pro_intro_shows_the_bio_card_then_the_sponsor_logos() {
        let g = Game::of(Path::new("C:/a/assets/private/challenges-skate1/nis"));
        assert_eq!(g, Some(Game::Skate1Tutorial));
        let open = |id| overlay(Game::Skate1Tutorial, id).map(|o| (o.movie, o.open, o.close));
        assert_eq!(open(10), Some(("pro_bioinfo_overlay", 0, Some(1))));
        assert_eq!(open(11), Some(("pro_bioinfo_overlay", 2, Some(3))));
        assert_eq!([21, 46, 52, 64, 68, 71, 78].map(|id| open(id).map(|o| o.1)), [0, 25, 31, 32, 36, 37, 44].map(Some));
        assert_eq!(open(46).and_then(|o| o.2), Some(50), "kOutroLeft");
        assert_eq!([55, 58, 69, 79, 53].map(open), [None, None, None, None, None], "B's tutorial movies, NIS_Overlay, others");
        assert_eq!([25, 23, 37, 7].map(logo), [Some("planbBoard"), Some("nike"), Some("venture"), None]);
        let mut d = Driver::default();
        let name = |n: u32| format!("intro-overlay-{n}");
        let t = Game::Skate1Tutorial;
        assert_eq!(d.step(g, 11, 0.), vec![Act::Open(t, name(1), "pro_bioinfo_overlay", 2)]);
        assert_eq!(d.step(g, 0, 4.52), vec![Act::State(t, name(1), 3)]);
        assert_eq!(d.step(g, 46, 8.1), vec![Act::Open(t, name(2), "sponsorlogo_setup", 25), Act::Close(t, name(1))]);
    }

    /// Playtest 10-15: P-Rod's card read "undefined" on every line. GetInfo is NIS.GetProInfo(0)[0..2].
    #[test]
    #[ignore = "requires $SKATE3_ASSET_ROOT/private/hud-skate1/tutorial"]
    fn the_pro_card_shows_the_running_pros_info() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT"));
        let file = root.join(Game::Skate1Tutorial.movies()).join("pro_bioinfo_overlay.json");
        let info = json!(crate::skate1_pros::pro_info(Some("fp_u3_tutorial02")));
        let mut s = Screen::load(&file, &[("NIS.GetProInfo", info)]).unwrap();
        s.call("SetState", &[json!(2)]).unwrap();
        let text = |path| s.clip(path).map(|c| s.vm.get(c, "text"));
        assert!(matches!(text("mcTextHeader.labelText"), Some(crate::apt_vm::Value::Text(t)) if t == "ID_CHEAT_PROSKATER17"));
    }
}
