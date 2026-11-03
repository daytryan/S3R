//! Skate 3's power-on flow as a Bevy-independent state machine: StateBootFlow (vtable 0x8230D168, sub-states
//! 0x826FDC50..0x826FE4F8) and the career flow's new-career branch (0x82704038, 0x82704BA8..0x827052C0).
//! Research and every citation: .superpowers/sdd/2026-10-04-startup/research.md (Stage 0 results win).
//!
//! The flow owns order, timers and input rules; it emits [`Command`]s for the presentation (boot.rs: APT
//! screens, movies, popups, sounds) and is told back what the movies and pickers did ([`Flow::movie_finished`],
//! [`Flow::menu_select`], [`Flow::popup_select`], ...). Language Select is skipped (decision 3: the PC build
//! behaves as if launch data set StateBootFlow+56 to 0).

/// Every active FE state's update gets a constant 16 FE-ms (0x82D0A158), once per 60 Hz sim tick (0x826D78B0
/// -> FE tick 0x825D8B50(fe, 1666)): N ms take ceil(N / 16) ticks.
pub(crate) const TICK_MS: u32 = 16;
/// Slot 5 (ESRB) update 0x82608E38: done once its counter reaches 4000.
pub(crate) const ESRB_HOLD_MS: u32 = 4000;
/// Press Start update 0x82609050: input is ignored until the counter at 0x8308355C passes 1500.
pub(crate) const TITLE_LOCK_MS: u32 = 1500;
/// Press Start pushes the attract video after 30000 FE-ms without a key (0x8260910C).
pub(crate) const ATTRACT_IDLE_MS: u32 = 30000;
/// Keyboard maxLen for the team name (0x82704DD8 -> XamShowKeyboardUI cchResultText 16 + 1).
pub(crate) const TEAM_NAME_MAX: usize = 16;
/// The FE cover's alpha step per update (`[0x830CFE14]+304`): open 0x825E6B30 sets +16 to 0.04 (0x82216FEC), the
/// update 0x825E69A0 adds it to the alpha +12 and runs the open callbacks once the alpha reaches 1.0, which the f32
/// sum does on the 25th update.
pub(crate) const COVER_STEP: f32 = 0.04;
/// `Sk8::Audio::eAudioFEStates` of the FE slots the boot shows, from each slot's audio-state collection (table
/// 0x83026088, 24-byte entries, key at +16; class 0x1FCCED72C8A5FE81): "bootflow" (slots 5 ESRB and 22 Press Start)
/// sets 10, "cac" (slot 15, the CAS) 4, "tiploader" (slot 52) 7. "coachfrank" (slot 11), "importskater" (68),
/// "firstteammember" (67), "teammanagement" (63), "welcomeflow" (9, 10), slot 0 "idle" and slot 24 (movies) set no
/// audio state: the default row's 0 (the row names are the keys' attrib hashes). Popups go on the FE's popup list
/// (0x82D0A8D0), not the state stack, so the slot under a popup keeps the state.
pub(crate) const AUDIO_BOOTFLOW: u32 = 10;
pub(crate) const AUDIO_CAC: u32 = 4;
pub(crate) const AUDIO_TIPS_LOADER: u32 = 7;

/// The retail APT keys the boot flow reacts to (key table 0x83027BF0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    /// 0 AptStart (a press).
    Start,
    /// 44 AptStartReleased.
    StartReleased,
    /// 2 AptNext.
    A,
    /// 3 AptPrevious.
    B,
    /// 12..15.
    Up,
    Down,
    Left,
    Right,
    /// The on-screen keyboard's Backspace and Space (Xbox Guide keyboard buttons; no FE key).
    X,
    Y,
}
impl Key {
    /// The engine's APT key for a screen's UpdateInput, when the screen gets one.
    pub(crate) fn apt(self) -> Option<crate::core_menu_runtime::Key> {
        use crate::core_menu_runtime::Key as Apt;
        Some(match self {
            Key::Up => Apt::Up,
            Key::Down => Apt::Down,
            Key::Left => Apt::Left,
            Key::Right => Apt::Right,
            Key::A => Apt::Accept,
            Key::B => Apt::Back,
            _ => return None,
        })
    }
}

/// Who pressed: a controller slot, or the PC keyboard (decision 5: the keyboard always works).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Pad(u8),
    Keyboard,
}

/// The fe_movie records the boot flow plays (re/vault_fe_movie.txt).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Movie {
    EaIntro,
    Attract,
    Intro,
    CoachFrankIntro,
}
impl Movie {
    /// The record's file stem (`data\movies\%s_english_ntsc.vp6`): the prepared movies/<stem>.vp6 and .wav.
    pub(crate) fn stem(self) -> &'static str {
        match self {
            Movie::EaIntro => "EA_Blackbox",
            Movie::Attract => "Attract",
            Movie::Intro => "intro_movie",
            Movie::CoachFrankIntro => "Coach_frank_intro",
        }
    }
    /// The record's volume (ea_intro / attract_video / intro_movie 0x3F4CCCCD, coach_frank_intro 0x3F19999A).
    pub(crate) fn volume(self) -> f32 {
        match self {
            Movie::CoachFrankIntro => f32::from_bits(0x3F19_999A),
            _ => f32::from_bits(0x3F4C_CCCD),
        }
    }
    /// FE-ms that must pass before A/Start shows the skip prompt (fe_movie 0x0AC11EDA5B6E4E50, tested by
    /// 0x825E0680 with the IsSkippable byte 0x82D90FC57883ACCB). None: never (ea_intro is not skippable).
    pub(crate) fn skip_after(self) -> Option<u32> {
        match self {
            Movie::EaIntro => None,
            Movie::Attract => Some(5000),
            Movie::Intro | Movie::CoachFrankIntro => Some(10000),
        }
    }
}

/// The boot flow's APT screens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Screen {
    /// Slot 5, bootflow/esrb.
    Esrb,
    /// Slot 22, bootflow/pressstart.
    PressStart,
    /// Slot 9, bootflow/welcomescreen (TeamManagement.GetNewTeamState() = 0).
    Welcome,
    /// Slot 10, the same movie in state 10 ("Time to pick a name and a logo!").
    TeamPrompt,
    /// Slot 63, TeamSkater_Selection/team_management_boot ("Start your team!").
    TeamManagement,
    /// Slot 11, bootflow/coachfrank.
    CoachFrank,
}

/// Popup table entries (re/signin-re/popups.txt).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Popup {
    /// 23: type 1 (popup/save_warning), MC_AUTOSAVE_WARNING2_PS3, OK.
    Autosave,
    /// 263: type 0 (popup/sk8popup), ID_VIRTUALKEYBOARD_ALPHA_NUMERIC_STRING_ERROR, OK.
    InvalidName,
    /// 302: type 0, ID_TEAM_MANAGEMENT_EMPTY_TEAM_NAME, OK.
    EmptyName,
    /// 262: type 0, ID_VIRTUALKEYBOARD_REDO_PROFILE_NAME, OK (record 0x8302b9a8): slot 67's boss name.
    RedoProfileName,
    /// The new career's Unlock Everything? (spec 11.1), Unlock Everything / Earn It; B is Earn It.
    /// Not retail (user 10-20).
    UnlockEverything,
}
impl Popup {
    pub(crate) fn description(self) -> &'static str {
        match self {
            Popup::Autosave => "MC_AUTOSAVE_WARNING2_PS3",
            Popup::InvalidName => "ID_VIRTUALKEYBOARD_ALPHA_NUMERIC_STRING_ERROR",
            Popup::EmptyName => "ID_TEAM_MANAGEMENT_EMPTY_TEAM_NAME",
            Popup::RedoProfileName => "ID_VIRTUALKEYBOARD_REDO_PROFILE_NAME",
            Popup::UnlockEverything => crate::skate3_career::unlocks::POPUP_BODY,
        }
    }
    pub(crate) fn options(self) -> &'static [&'static str] {
        if self == Popup::UnlockEverything { &[crate::skate3_career::unlocks::CHOICE_YES, crate::skate3_career::unlocks::CHOICE_NO] } else { &["ID_COMMON_OK"] }
    }
    pub(crate) fn cancelable(self) -> bool {
        self == Popup::UnlockEverything
    }
    /// Type 1 popups render with popup/save_warning, type 0 with popup/sk8popup.
    pub(crate) fn save_warning(self) -> bool {
        self == Popup::Autosave
    }
}

/// What the presentation does, in order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    /// Load the screen (its intro plays).
    Open(Screen),
    /// Play the screen's outro and drop it.
    Close(Screen),
    /// The screen controller's SetState(n).
    SetState(Screen, f64),
    /// The key to the open screen's UpdateInput.
    ForwardScreen(Key),
    /// The key to the popup's UpdateInput.
    ForwardPopup(Key),
    PlayMovie(Movie),
    StopMovie,
    /// `_global.ShowMovieSkip` / `HideMovieSkip` (screens/main/fmv_popup).
    MovieSkip(bool),
    /// `_global.Load_EnableTag` / `Load_HideTag` (controls/autosave).
    LoadingTag(bool),
    OpenPopup(Popup),
    ClosePopup,
    /// GlobalFEPlaySound(hash(fe key), flag) (0x825DFAF0).
    Sound(&'static str, u32),
    /// The welcome screen's camera previews (cameraLow / cameraHigh) start or stop.
    CameraPreviews(bool),
    /// Career difficulty (+180): 0 Easy, 1 Normal, 2 Hardcore.
    Difficulty(usize),
    /// Career camera (+188): false low, true high.
    CameraHigh(bool),
    /// The team-name keyboard opens (or reopens after popup 263 / 302) on the default name.
    OpenKeyboard,
    CloseKeyboard,
    OpenCreateSkater,
    /// Slot 63 in new-career mode (state 30, the logo picker; team_select.rs): its close is [`Flow::team_closed`].
    OpenTeamLogos,
    /// Slot 68 Import Skater for the first member (mode 2: popup 293): its close is [`Flow::team_closed`].
    OpenImport,
    /// The key to slot 63 / 68 (team_select's manager).
    ForwardTeam(Key),
    /// The Unlock Everything? answer (true: Unlock Everything), written to unlocks.json at once.
    UnlockEverything(bool),
    /// The boot flow is over: the world (through the loading screen) takes over.
    Finish(Outcome),
}

/// What the boot decided, for the career save.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Outcome {
    pub first_boot: bool,
    pub team_name: Option<String>,
    /// Coach Frank: Yes (true) stays for the basics.
    pub tutorial: Option<bool>,
    /// Slot 67's "Enter your name:" (the boss, career+40).
    pub boss_name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WelcomeState {
    /// 0: difficulty picker.
    Difficulty,
    /// 1 / 2: camera Low / High highlighted.
    Low,
    High,
    /// 3: picked; the screen pops on its next update.
    Done,
}

#[derive(Clone, Debug, PartialEq)]
enum Step {
    Esrb { ms: u32 },
    EaIntro,
    /// Sub-state 5: the loading tag shows until the game is up.
    Loading,
    PressStart,
    /// Accepted; sub-state 7 pops the title on its next update.
    Accepted,
    /// Popup 23 is up; its OK callback (0x826FE638) arms the career message.
    AutosaveWarning,
    /// FlowMessage 21 goes out on this update (0x826FE4F8).
    Career,
    Welcome(WelcomeState),
    /// [`Popup::UnlockEverything`] is up (a new career with the Skate 3 career on).
    UnlockEverything,
    IntroMovie,
    TeamPrompt,
    Keyboard,
    /// Slot 63 in new-career mode: the team logo.
    TeamLogos,
    TeamManagement,
    /// Slot 68 for the first member (Preset Skaters).
    ImportSkater,
    CreateSkater,
    CoachFrankMovie,
    CoachFrank,
    /// StateLoading enter (0x82706A38): the loading tag on (0x825DF470(fe, 1, 1)) and the cover opened for
    /// requester 7 with callback 0x82706AE8, fading to black; the load and its TipsLoader (slot 52) follow.
    GameLoad { cover: f32 },
    Done,
}

#[derive(Clone, Debug, PartialEq)]
struct MovieRun {
    movie: Movie,
    /// Player +0: FE-ms while playing.
    elapsed: u32,
    /// Movie state +67: the skip prompt is up.
    prompt: bool,
}

/// Press Start's fields (0x82609050 / 0x82609238).
#[derive(Clone, Debug, Default, PartialEq)]
struct Title {
    /// +72: past the 1500 FE-ms lock.
    ready: bool,
    /// +80: Start was pressed (a release accepts).
    armed: bool,
    /// +60: accepted (fading out).
    accepting: bool,
    /// +76: FE-ms without a key.
    idle: u32,
}

pub(crate) struct Flow {
    step: Step,
    first_boot: bool,
    movie: Option<MovieRun>,
    popup: Option<Popup>,
    title: Title,
    /// 0x8308355C: the title's lock counter (a global; reset only on accept).
    title_ms: u32,
    /// input+28: -1 until Start is accepted on the title, then only this pad drives the front end.
    active_pad: Option<u8>,
    /// StateBootFlow +49: popup 23 shows once per boot.
    autosave_shown: bool,
    pub keyboard: TextEntry,
    outcome: Outcome,
    /// The localized default team name (ID_WELCOME_NEW_TEAM_NAME).
    default_name: String,
    /// The language's ALL_VALID_ALPHANUMERIC_CHARACTERS (keyboard type-0 validation 0x82639590).
    valid: String,
    /// The boss-name keyboard is up for slot 67's choice (else the keyboard is the team name's).
    boss_choice: Option<usize>,
    /// Career +108 bit 2: the boss has been named (slot 67 asks once).
    named: bool,
    /// ID_DEFAULT_PLAYER_NAME ("The Legend").
    default_boss: String,
    /// Ask Unlock Everything? after the welcome screen (the Skate 3 career is on), and its saved answer (the cursor).
    unlock_popup: Option<bool>,
    /// Coach Frank asks about Skate School: only with story mode in the build (its lessons are career rows).
    skate_school: bool,
    /// Settings > Gameplay > Skip Intro ([`Flow::skip_intro`]): the load goes straight on, past Press Start.
    skip_intro: bool,
    out: Vec<Command>,
}

impl Flow {
    /// Starts at the ESRB screen. `first_boot`: no career with the intro done (career flow +108 bit 1).
    pub(crate) fn new(first_boot: bool, default_name: &str, valid: &str) -> Self {
        let mut flow = Self {
            step: Step::Esrb { ms: 0 },
            first_boot,
            movie: None,
            popup: None,
            title: Title::default(),
            title_ms: 0,
            active_pad: None,
            autosave_shown: false,
            keyboard: TextEntry::new(default_name, TEAM_NAME_MAX),
            outcome: Outcome { first_boot, ..Outcome::default() },
            default_name: default_name.to_owned(),
            valid: valid.to_owned(),
            boss_choice: None,
            named: false,
            default_boss: "The Legend".into(),
            unlock_popup: None,
            skate_school: crate::skate3_career::STORY_MODE,
            skip_intro: false,
            out: Vec::new(),
        };
        flow.out.push(Command::Open(Screen::Esrb));
        flow
    }
    /// Not retail (user 11-02, Settings > Gameplay > Skip Intro): no ESRB, EA logo, Press Start or autosave notice;
    /// the loading tag shows until the game is up, then a returning career goes straight in (a new one still gets its
    /// welcome screens). Call before the first [`Flow::take`].
    pub(crate) fn skip_intro(&mut self) {
        self.out.retain(|c| *c != Command::Open(Screen::Esrb));
        self.skip_intro = true;
        self.autosave_shown = true;
        self.step = Step::Loading;
        self.emit(Command::LoadingTag(true));
    }
    pub(crate) fn take(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.out)
    }
    pub(crate) fn finished(&self) -> bool {
        self.step == Step::Done
    }
    /// The pad that accepted Press Start (None: any pad).
    pub(crate) fn active_pad(&self) -> Option<u8> {
        self.active_pad
    }
    /// The host's shared active pad (`input::ActivePad`), the one copy the filter and Press Start use.
    pub(crate) fn set_active_pad(&mut self, pad: Option<u8>) {
        self.active_pad = pad;
    }
    /// The CAS owns the screen and its own input.
    pub(crate) fn in_create_skater(&self) -> bool {
        self.step == Step::CreateSkater
    }
    pub(crate) fn keyboard_open(&self) -> bool {
        self.step == Step::Keyboard && self.popup.is_none()
    }
    /// The keyboard's header string id: slot 67's boss name, else the team name.
    pub(crate) fn keyboard_header(&self) -> &'static str {
        if self.boss_choice.is_some() { "ID_KEYBOARD_PROFILE_NAME_HEADER" } else { "ID_WELCOME_NEW_TEAM_HEADER" }
    }
    /// The Skate 3 career is on: a new career asks Unlock Everything?, its cursor on `saved` (Earn It when false).
    pub(crate) fn ask_unlock_everything(&mut self, saved: bool) {
        self.unlock_popup = Some(saved);
    }
    /// Unlock Everything? opens on Unlock Everything (else Earn It).
    pub(crate) fn unlock_cursor_on_yes(&self) -> bool {
        self.unlock_popup == Some(true)
    }
    /// The localized ID_DEFAULT_PLAYER_NAME.
    pub(crate) fn set_default_boss(&mut self, name: &str) {
        self.default_boss = name.to_owned();
    }
    /// What the new career has decided so far (the team name for the logo picker).
    pub(crate) fn outcome(&self) -> &Outcome {
        &self.outcome
    }
    pub(crate) fn movie_playing(&self) -> Option<Movie> {
        self.movie.as_ref().map(|m| m.movie)
    }

    fn emit(&mut self, command: Command) {
        self.out.push(command);
    }
    fn play(&mut self, movie: Movie) {
        self.movie = Some(MovieRun { movie, elapsed: 0, prompt: false });
        self.emit(Command::PlayMovie(movie));
    }
    fn open_popup(&mut self, popup: Popup) {
        self.popup = Some(popup);
        self.emit(Command::OpenPopup(popup));
        // Popup OnEnter 0x825EF528..0x825EF544 (a placeholder tone in sk8_menu.bnk).
        self.emit(Command::Sound("core_popup", 0));
    }
    fn open_title(&mut self) {
        self.step = Step::PressStart;
        self.title = Title::default();
        self.emit(Command::Open(Screen::PressStart));
        // OnEnter 0x82608FB0: titlescreen_startfx with flag 1, then SetState(1) (text, gotoAndPlay "fadein").
        self.emit(Command::Sound("titlescreen_startfx", 1));
        self.emit(Command::SetState(Screen::PressStart, 1.));
    }
    /// Into the game: StateLoading's cover fade first ([`Step::GameLoad`]), then [`Command::Finish`].
    fn finish(&mut self) {
        self.step = Step::GameLoad { cover: 0. };
        self.emit(Command::LoadingTag(true));
    }
    /// The black cover over the boot's screens (0 clear, 1 black).
    pub(crate) fn cover(&self) -> f32 {
        match self.step {
            Step::GameLoad { cover } => cover.min(1.),
            Step::Done => 1.,
            _ => 0.,
        }
    }
    /// The top FE slot's audio state ([`AUDIO_BOOTFLOW`]).
    pub(crate) fn audio_state(&self) -> u32 {
        match (&self.step, &self.movie) {
            (_, Some(_)) => 0,
            (Step::Esrb { .. } | Step::PressStart | Step::Accepted, None) => AUDIO_BOOTFLOW,
            (Step::CreateSkater, None) => AUDIO_CAC,
            _ => 0,
        }
    }

    /// Real time a slow frame's capped ticks left out: a playing movie's FE-ms are wall-clock time (retail's FE
    /// timer), so a first-launch frame rate doesn't hold the skip prompt back (bug 10-09 #1).
    pub(crate) fn movie_clock(&mut self, ms: u32) {
        if let Some(run) = self.movie.as_mut() {
            run.elapsed += ms;
        }
    }

    /// One 60 Hz FE tick. `game_ready`: the world is loaded (sub-state 5's loader counter).
    pub(crate) fn tick(&mut self, game_ready: bool) {
        if let Some(run) = self.movie.as_mut() {
            run.elapsed += TICK_MS;
        }
        match self.step.clone() {
            Step::Esrb { ms } => {
                let ms = ms + TICK_MS;
                if ms >= ESRB_HOLD_MS {
                    // Sub-state 0x826FDE48 replaces the ESRB state (FE op 3) with the EA intro.
                    self.emit(Command::Close(Screen::Esrb));
                    self.step = Step::EaIntro;
                    self.play(Movie::EaIntro);
                } else {
                    self.step = Step::Esrb { ms };
                }
            }
            Step::Loading if game_ready && self.skip_intro => {
                self.emit(Command::LoadingTag(false));
                self.step = Step::Career;
            }
            Step::Loading if game_ready => {
                self.emit(Command::LoadingTag(false));
                self.open_title();
            }
            // The title is inactive while the attract video plays over it.
            Step::PressStart if self.movie.is_none() => self.title_update(),
            Step::Accepted => {
                // 0x826FE1D8: pop the title, keep the active pad, popup 23 once per boot.
                self.emit(Command::Close(Screen::PressStart));
                if self.autosave_shown {
                    self.step = Step::Career;
                } else {
                    self.autosave_shown = true;
                    self.step = Step::AutosaveWarning;
                    self.open_popup(Popup::Autosave);
                }
            }
            Step::Career => {
                if self.first_boot {
                    self.step = Step::Welcome(WelcomeState::Difficulty);
                    self.emit(Command::Open(Screen::Welcome));
                } else {
                    self.finish();
                }
            }
            // Black: the open callback 0x82706AE8 starts the load, which the loading screen covers.
            Step::GameLoad { cover } => {
                let cover = cover + COVER_STEP;
                if cover >= 1. {
                    self.step = Step::Done;
                    let outcome = self.outcome.clone();
                    self.emit(Command::Finish(outcome));
                } else {
                    self.step = Step::GameLoad { cover };
                }
            }
            // 0x826099C0: once picked and the camera movies are released, the welcome state pops.
            Step::Welcome(WelcomeState::Done) => {
                self.emit(Command::Close(Screen::Welcome));
                if self.unlock_popup.is_some() {
                    self.step = Step::UnlockEverything;
                    self.open_popup(Popup::UnlockEverything);
                } else {
                    self.step = Step::IntroMovie;
                    self.play(Movie::Intro);
                }
            }
            _ => {}
        }
    }
    /// 0x82609050 with r4 = 16.
    fn title_update(&mut self) {
        let t = &mut self.title;
        if self.title_ms > TITLE_LOCK_MS && !t.ready {
            t.idle = 0;
            t.ready = true;
        } else {
            self.title_ms += TICK_MS;
        }
        if t.ready && !t.accepting {
            // The idle timer runs only with no popup up and the keyboard (slot 46) closed.
            if self.popup.is_none() {
                t.idle += TICK_MS;
            } else {
                t.idle = 0;
            }
            if t.idle >= ATTRACT_IDLE_MS {
                t.ready = false;
                self.play(Movie::Attract);
            }
        }
    }

    /// A key press (or Start release) from `source`.
    pub(crate) fn input(&mut self, source: Source, key: Key) {
        // FE input pump 0x825DFDC8: once a pad is active, the other pads are skipped.
        if let (Source::Pad(pad), Some(active)) = (source, self.active_pad) {
            if pad != active {
                return;
            }
        }
        if self.popup.is_some() {
            if key.apt().is_some() {
                self.emit(Command::ForwardPopup(key));
            }
            return;
        }
        if self.movie.is_some() {
            return self.movie_input(key);
        }
        match self.step {
            Step::PressStart => self.title_input(source, key),
            Step::Welcome(state) => self.welcome_input(state, key),
            Step::TeamPrompt if key == Key::A => {
                // 0x8260A238: core_a_button, then the prompt pops and the keyboard opens (0x82704DD8).
                self.emit(Command::Sound("core_a_button", 0));
                self.emit(Command::Close(Screen::TeamPrompt));
                self.open_keyboard();
            }
            Step::Keyboard => match self.keyboard.input(key) {
                Some(Typed::Done) => {
                    let text = self.keyboard.text.clone();
                    if self.boss_choice.is_some() { self.submit_boss(text) } else { self.submit_name(text) }
                }
                Some(Typed::Cancel) if self.boss_choice.is_some() => {
                    // Cancel leaves slot 67 as it was.
                    self.boss_choice = None;
                    self.emit(Command::CloseKeyboard);
                    self.step = Step::TeamManagement;
                }
                Some(Typed::Cancel) => self.cancel_name(),
                None => {}
            },
            Step::TeamLogos | Step::ImportSkater => self.emit(Command::ForwardTeam(key)),
            Step::TeamManagement | Step::CoachFrank => {
                if key.apt().is_some() {
                    self.emit(Command::ForwardScreen(key));
                }
            }
            _ => {}
        }
    }
    /// 0x82609238 (only while +68 == 0, the normal boot).
    fn title_input(&mut self, source: Source, key: Key) {
        let t = &mut self.title;
        t.idle = 0;
        match key {
            Key::Start => t.armed = true,
            Key::StartReleased => {
                if t.ready && t.armed && !t.accepting {
                    self.title_ms = 0;
                    self.emit(Command::Sound("titlescreen_start", 0));
                    // input+28 = the pad that pressed Start (it stays -1 for the PC keyboard: every pad works).
                    if self.active_pad.is_none() {
                        if let Source::Pad(pad) = source {
                            self.active_pad = Some(pad);
                        }
                    }
                    // Accept 0x82609188: +60 = 1, SetState(2) (fadeout).
                    self.title.accepting = true;
                    self.emit(Command::SetState(Screen::PressStart, 2.));
                    self.step = Step::Accepted;
                }
                self.title.armed = false;
            }
            _ => {}
        }
    }
    /// Slot 9 input vt[10] (0x82609BA0): states 1 and 2 consume every key; state 0 goes to the APT picker.
    fn welcome_input(&mut self, state: WelcomeState, key: Key) {
        match (state, key) {
            (WelcomeState::Difficulty, _) => {
                if key.apt().is_some() {
                    self.emit(Command::ForwardScreen(key));
                }
            }
            (WelcomeState::Low | WelcomeState::High, Key::A) => {
                self.emit(Command::Sound("core_a_button", 0));
                self.emit(Command::CameraHigh(state == WelcomeState::High));
                self.emit(Command::CameraPreviews(false));
                self.step = Step::Welcome(WelcomeState::Done);
            }
            (WelcomeState::Low | WelcomeState::High, Key::Left | Key::Right) => {
                // Both directions play core_nav_left (0x82609C34, 0x82609CF0).
                self.emit(Command::Sound("core_nav_left", 0));
                let (next, n) = if state == WelcomeState::Low { (WelcomeState::High, 2.) } else { (WelcomeState::Low, 1.) };
                self.step = Step::Welcome(next);
                self.emit(Command::SetState(Screen::Welcome, n));
            }
            _ => {}
        }
    }
    /// Movie state input 0x8260B240 (only while playing).
    fn movie_input(&mut self, key: Key) {
        let Some(run) = self.movie.as_mut() else { return };
        match key {
            Key::Start | Key::A if !run.prompt => {
                if run.movie.skip_after().is_some_and(|min| run.elapsed > min) {
                    run.prompt = true;
                    self.emit(Command::MovieSkip(true));
                }
            }
            Key::A => {
                // Second A: core_a_button, then the movie state pops.
                self.emit(Command::Sound("core_a_button", 0));
                self.emit(Command::StopMovie);
                self.movie_finished();
            }
            Key::B if run.prompt => {
                run.prompt = false;
                self.emit(Command::MovieSkip(false));
            }
            _ => {}
        }
    }

    /// The movie ran out or was skipped: the movie state exits (0x8260B150) and its caller continues.
    pub(crate) fn movie_finished(&mut self) {
        let Some(run) = self.movie.take() else { return };
        if run.prompt {
            self.emit(Command::MovieSkip(false));
        }
        match (run.movie, &self.step) {
            (Movie::EaIntro, Step::EaIntro) => {
                // Sub-state 5 enter (0x826FE058): the loading tag.
                self.step = Step::Loading;
                self.emit(Command::LoadingTag(true));
            }
            (Movie::Intro, Step::IntroMovie) => {
                self.step = Step::TeamPrompt;
                self.emit(Command::Open(Screen::TeamPrompt));
            }
            (Movie::CoachFrankIntro, Step::CoachFrankMovie) => {
                self.step = Step::CoachFrank;
                self.emit(Command::Open(Screen::CoachFrank));
            }
            // The attract video pops back to the title.
            _ => {}
        }
    }
    /// `Game.OnMenuSelect(menu, index)` from the open screen's picker.
    pub(crate) fn menu_select(&mut self, index: usize) {
        match self.step {
            Step::Welcome(WelcomeState::Difficulty) => {
                // 0x82609D38: career +180, the difficulty message, core_a_button, state 1, both camera movies.
                self.emit(Command::Difficulty(index.min(2)));
                self.emit(Command::Sound("core_a_button", 0));
                self.step = Step::Welcome(WelcomeState::Low);
                self.emit(Command::CameraPreviews(true));
                self.emit(Command::SetState(Screen::Welcome, 1.));
            }
            // Slot 67 OnMenuSelect: the first pick asks the boss's name first (career+108 bit 2), then goes on.
            Step::TeamManagement if !self.named => {
                self.boss_choice = Some(index);
                self.open_keyboard();
            }
            Step::TeamManagement => match index {
                0 => {
                    self.emit(Command::Close(Screen::TeamManagement));
                    self.step = Step::CreateSkater;
                    self.emit(Command::OpenCreateSkater);
                }
                2 => {
                    self.emit(Command::Close(Screen::TeamManagement));
                    self.coach_frank_movie();
                }
                // Preset Skaters: slot 68 in mode 2 for member 0 (popup 293), back here after.
                1 => {
                    self.emit(Command::Close(Screen::TeamManagement));
                    self.step = Step::ImportSkater;
                    self.emit(Command::OpenImport);
                }
                _ => {}
            },
            Step::CoachFrank => {
                // Button callback 0x8260A288: Yes (0) stays for the basics.
                self.outcome.tutorial = Some(index == 0);
                self.emit(Command::Close(Screen::CoachFrank));
                self.finish();
            }
            _ => {}
        }
    }
    /// The popup's OK.
    pub(crate) fn popup_select(&mut self) {
        self.popup_chose(0);
    }
    /// The popup's answer (`popup_runtime::CANCEL` for B).
    pub(crate) fn popup_chose(&mut self, index: usize) {
        let Some(popup) = self.popup.take() else { return };
        self.emit(Command::ClosePopup);
        match popup {
            // 0x826FE638 sets the flag; the next update sends FlowMessage 21.
            Popup::Autosave => self.step = Step::Career,
            // 0x82639CB0: back to the keyboard.
            Popup::InvalidName | Popup::EmptyName | Popup::RedoProfileName => self.open_keyboard(),
            Popup::UnlockEverything => {
                self.emit(Command::UnlockEverything(index == 0));
                self.step = Step::IntroMovie;
                self.play(Movie::Intro);
            }
        }
    }
    pub(crate) fn create_skater_closed(&mut self) {
        if self.step == Step::CreateSkater {
            self.coach_frank_movie();
        }
    }
    fn coach_frank_movie(&mut self) {
        // Without story mode there is no Skate School to offer: the boot goes straight to the game, no answer recorded.
        if !self.skate_school {
            self.finish();
            return;
        }
        self.step = Step::CoachFrankMovie;
        self.play(Movie::CoachFrankIntro);
    }
    fn open_keyboard(&mut self) {
        self.step = Step::Keyboard;
        self.keyboard = TextEntry::new(if self.boss_choice.is_some() { &self.default_boss } else { &self.default_name }, TEAM_NAME_MAX);
        self.emit(Command::OpenKeyboard);
    }
    /// OK on slot 67's "Enter your name:" (0x82710760): popup 262 when the name is empty, all spaces, or has a
    /// character that is neither a space nor alphanumeric (isalnum, 0x82f4ef98); else it is the boss's name and the
    /// choice goes on.
    fn submit_boss(&mut self, text: String) {
        self.emit(Command::CloseKeyboard);
        if !valid_profile_name(&text) {
            return self.open_popup(Popup::RedoProfileName);
        }
        self.outcome.boss_name = Some(text);
        self.named = true;
        self.step = Step::TeamManagement;
        if let Some(choice) = self.boss_choice.take() {
            self.menu_select(choice);
        }
    }
}
/// 0x82710760's test: at least one character, only spaces and ASCII letters/digits, not all spaces.
pub(crate) fn valid_profile_name(text: &str) -> bool {
    text.chars().all(|c| c == ' ' || c.is_ascii_alphanumeric()) && !text.trim().is_empty()
}
impl Flow {
    /// Slot 63 (the logo, committed) or slot 68 (an import, or B) closed: on to slot 67.
    pub(crate) fn team_closed(&mut self) {
        if matches!(self.step, Step::TeamLogos | Step::ImportSkater) {
            self.team_management();
        }
    }
    /// Slot 63 in new-career mode (OnLoaded 0x82651a48 with career+104 set: SetState 30).
    fn team_logos(&mut self) {
        self.step = Step::TeamLogos;
        self.emit(Command::OpenTeamLogos);
    }
    /// Typed text (the PC keyboard), applied while the keyboard is open.
    pub(crate) fn type_text(&mut self, text: &str) {
        if self.keyboard_open() {
            self.keyboard.type_text(text);
        }
    }
    /// OK on the keyboard (0x82639590 validation, then 0x82705470).
    fn submit_name(&mut self, text: String) {
        self.emit(Command::CloseKeyboard);
        if text.is_empty() {
            return self.open_popup(Popup::EmptyName);
        }
        if !text.chars().all(|c| self.valid.contains(c)) {
            return self.open_popup(Popup::InvalidName);
        }
        self.outcome.team_name = Some(text);
        self.team_logos();
    }
    /// B on the keyboard (0x82705528): the default name.
    fn cancel_name(&mut self) {
        self.emit(Command::CloseKeyboard);
        self.outcome.team_name = Some(self.default_name.clone());
        self.team_logos();
    }
    fn team_management(&mut self) {
        self.step = Step::TeamManagement;
        self.emit(Command::Open(Screen::TeamManagement));
    }
    /// A screen failed to load or run: continue as its default choice would, so the player is never stranded.
    pub(crate) fn screen_failed(&mut self) {
        match self.step {
            Step::Esrb { .. } => self.step = Step::Esrb { ms: ESRB_HOLD_MS - TICK_MS },
            Step::PressStart => {
                self.title.accepting = true;
                self.step = Step::Accepted;
            }
            // The camera choice is the same screen: skip it too (Low, its default) rather than wait on black.
            Step::Welcome(WelcomeState::Difficulty) => {
                self.menu_select(1);
                self.welcome_input(WelcomeState::Low, Key::A);
            }
            Step::Welcome(WelcomeState::Low | WelcomeState::High) => self.welcome_input(WelcomeState::Low, Key::A),
            Step::TeamPrompt => self.input(Source::Keyboard, Key::A),
            Step::TeamManagement => {
                self.named = true;
                self.menu_select(2)
            }
            Step::TeamLogos | Step::ImportSkater => self.team_closed(),
            Step::CoachFrank => self.menu_select(1),
            _ => {}
        }
    }
    pub(crate) fn popup_failed(&mut self) {
        // Unlock Everything? unshown is Earn It.
        self.popup_chose(usize::from(self.popup == Some(Popup::UnlockEverything)));
    }
}

/// The team-name text entry (decision 9) lives in `text_entry`; its paths stay here for the boot screens.
pub(crate) use crate::text_entry::{COMMANDS, Command2, GRID, TextEntry, Typed};

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const VALID: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789',.?!@#$*()\"/_ -,.' ";

    fn flow(first: bool) -> Flow {
        Flow::new(first, "New Team", VALID)
    }
    fn ticks(f: &mut Flow, n: u32, ready: bool) -> Vec<Command> {
        let mut out = Vec::new();
        for _ in 0..n {
            f.tick(ready);
            out.extend(f.take());
        }
        out
    }
    /// ESRB through to the title, ready for Start.
    pub(crate) fn to_title(f: &mut Flow) {
        ticks(f, 250, false);
        f.movie_finished();
        ticks(f, 1, true);
        ticks(f, 95, true);
        f.take();
    }
    #[test]
    fn real_time_the_frame_cap_drops_still_counts_toward_a_movie_skip() {
        // Bug 10-09 #1: under a slow first-launch frame the FE ticks are capped, but the movie's FE-ms are real time.
        let mut f = flow(false);
        to_title(&mut f);
        assert_eq!(ticks(&mut f, 1874, true), [Command::PlayMovie(Movie::Attract)]);
        // Twelve 0.5 s frames: 4 capped ticks each (64 ms) plus the 436 ms the cap dropped.
        for _ in 0..12 {
            ticks(&mut f, 4, true);
            f.movie_clock(436);
        }
        f.input(Source::Pad(0), Key::A);
        assert_eq!(f.take(), [Command::MovieSkip(true)], "6 s of real time is past the attract video's 5 s");
    }
    /// User 11-02: Skip Intro goes from the loading tag straight into the game; no ESRB, EA logo, title or autosave notice.
    #[test]
    fn skip_intro_loads_straight_into_the_game() {
        let mut f = flow(false);
        f.skip_intro();
        assert_eq!(f.take(), [Command::LoadingTag(true)]);
        assert!(ticks(&mut f, 300, false).is_empty(), "it waits for the load");
        let out = ticks(&mut f, 40, true);
        assert_eq!(out.first(), Some(&Command::LoadingTag(false)));
        assert!(!out.iter().any(|c| matches!(c, Command::Open(_) | Command::PlayMovie(_) | Command::OpenPopup(_))), "{out:?}");
        assert!(matches!(out.last(), Some(Command::Finish(o)) if !o.first_boot), "{out:?}");
        assert!(f.finished());
        // A new career still gets its welcome screens.
        let mut f = flow(true);
        f.skip_intro();
        f.take();
        assert!(ticks(&mut f, 2, true).contains(&Command::Open(Screen::Welcome)));
    }
    fn press_start(f: &mut Flow, source: Source) -> Vec<Command> {
        f.input(source, Key::Start);
        f.input(source, Key::StartReleased);
        f.take()
    }

    /// A new career through the welcome screen (skipped), up to what follows it.
    fn past_welcome(f: &mut Flow) -> Vec<Command> {
        to_title(f);
        press_start(f, Source::Pad(0));
        ticks(f, 1, true);
        f.popup_select();
        ticks(f, 1, true);
        f.screen_failed();
        f.take();
        ticks(f, 1, true)
    }

    #[test]
    fn unlock_everything_follows_the_welcome_screen_then_the_intro() {
        let mut f = flow(true);
        f.ask_unlock_everything(false);
        let out = past_welcome(&mut f);
        assert!(out.contains(&Command::OpenPopup(Popup::UnlockEverything)) && !out.contains(&Command::PlayMovie(Movie::Intro)), "{out:?}");
        assert!(!f.unlock_cursor_on_yes(), "a new answer starts on Earn It");
        f.popup_chose(0);
        let out = f.take();
        assert_eq!(out, [Command::ClosePopup, Command::UnlockEverything(true), Command::PlayMovie(Movie::Intro)]);
    }

    #[test]
    fn b_or_a_failed_popup_means_earn_it() {
        for fail in [false, true] {
            let mut f = flow(true);
            f.ask_unlock_everything(true);
            assert!(f.unlock_cursor_on_yes(), "the saved answer is the cursor");
            past_welcome(&mut f);
            if fail { f.popup_failed() } else { f.popup_chose(crate::popup_runtime::CANCEL) }
            assert!(f.take().contains(&Command::UnlockEverything(false)));
        }
    }

    #[test]
    fn without_the_career_or_on_a_returning_career_nothing_asks() {
        let mut f = flow(true);
        assert!(past_welcome(&mut f).contains(&Command::PlayMovie(Movie::Intro)));
        let mut f = flow(false);
        f.ask_unlock_everything(false);
        to_title(&mut f);
        press_start(&mut f, Source::Pad(0));
        ticks(&mut f, 1, true);
        f.popup_select();
        let out = ticks(&mut f, 200, true);
        assert!(!out.contains(&Command::OpenPopup(Popup::UnlockEverything)), "{out:?}");
    }

    #[test]
    fn a_welcome_screen_that_fails_skips_its_camera_step_too() {
        let mut f = flow(true);
        to_title(&mut f);
        press_start(&mut f, Source::Pad(0));
        ticks(&mut f, 1, true);
        f.popup_select();
        assert!(ticks(&mut f, 1, true).contains(&Command::Open(Screen::Welcome)));
        f.screen_failed();
        let out = f.take();
        assert!(out.contains(&Command::Difficulty(1)) && out.contains(&Command::CameraHigh(false)), "{out:?}");
        assert_eq!(out.last(), Some(&Command::CameraPreviews(false)), "no camera step waiting on black: {out:?}");
        assert!(ticks(&mut f, 1, true).contains(&Command::PlayMovie(Movie::Intro)));
    }

    #[test]
    fn esrb_holds_250_ticks_then_the_unskippable_ea_intro() {
        let mut f = flow(false);
        assert_eq!(f.take(), [Command::Open(Screen::Esrb)]);
        // 4000 / 16 = 250 updates: the 249th is still the ESRB screen.
        assert!(ticks(&mut f, 249, true).is_empty());
        f.input(Source::Pad(0), Key::A);
        f.input(Source::Pad(0), Key::Start);
        assert!(f.take().is_empty(), "the ESRB screen takes no input");
        assert_eq!(ticks(&mut f, 1, true), [Command::Close(Screen::Esrb), Command::PlayMovie(Movie::EaIntro)]);
        // Never skippable, however long it has played.
        ticks(&mut f, 600, true);
        for key in [Key::A, Key::Start, Key::A, Key::B] {
            f.input(Source::Pad(0), key);
        }
        assert!(f.take().is_empty());
        f.movie_finished();
        assert_eq!(f.take(), [Command::LoadingTag(true)]);
    }

    #[test]
    fn loading_tag_waits_for_the_game_then_the_title_opens() {
        let mut f = flow(false);
        ticks(&mut f, 250, false);
        f.movie_finished();
        f.take();
        assert!(ticks(&mut f, 30, false).is_empty(), "the tag stays while the game loads");
        assert_eq!(
            ticks(&mut f, 1, true),
            [
                Command::LoadingTag(false),
                Command::Open(Screen::PressStart),
                Command::Sound("titlescreen_startfx", 1),
                Command::SetState(Screen::PressStart, 1.),
            ]
        );
    }

    #[test]
    fn title_ignores_start_for_the_first_1500_ms_and_accepts_on_release() {
        let mut f = flow(false);
        ticks(&mut f, 250, false);
        f.movie_finished();
        ticks(&mut f, 1, true);
        // 94 updates bring the counter to 1504; the 95th sets ready.
        ticks(&mut f, 94, true);
        assert!(press_start(&mut f, Source::Pad(1)).is_empty(), "still locked");
        ticks(&mut f, 1, true);
        // A press alone does nothing; a release without a press does nothing; A never starts.
        f.input(Source::Pad(1), Key::A);
        f.input(Source::Pad(1), Key::StartReleased);
        assert!(f.take().is_empty());
        f.input(Source::Pad(1), Key::Start);
        assert!(f.take().is_empty(), "accepted on the release, not the press");
        f.input(Source::Pad(1), Key::StartReleased);
        assert_eq!(f.take(), [Command::Sound("titlescreen_start", 0), Command::SetState(Screen::PressStart, 2.)]);
        assert_eq!(f.active_pad(), Some(1));
        // Next update: the title pops and popup 23 opens with its tone.
        assert_eq!(
            ticks(&mut f, 1, true),
            [Command::Close(Screen::PressStart), Command::OpenPopup(Popup::Autosave), Command::Sound("core_popup", 0)]
        );
    }

    #[test]
    fn release_while_locked_disarms() {
        let mut f = flow(false);
        ticks(&mut f, 250, false);
        f.movie_finished();
        ticks(&mut f, 1, true);
        // Pressed and released while locked: the release disarms, so a later release alone does nothing.
        f.input(Source::Pad(0), Key::Start);
        f.input(Source::Pad(0), Key::StartReleased);
        ticks(&mut f, 95, true);
        f.input(Source::Pad(0), Key::StartReleased);
        assert!(f.take().is_empty());
        assert!(!press_start(&mut f, Source::Pad(0)).is_empty(), "a fresh press and release accepts");
        // A press held through the end of the lock arms it: its release after the lock accepts (0x82609238).
        let mut g = flow(false);
        ticks(&mut g, 250, false);
        g.movie_finished();
        ticks(&mut g, 1, true);
        g.input(Source::Pad(0), Key::Start);
        ticks(&mut g, 95, true);
        g.take();
        g.input(Source::Pad(0), Key::StartReleased);
        assert!(!g.take().is_empty());
    }

    #[test]
    fn thirty_seconds_idle_plays_the_attract_video_and_returns() {
        let mut f = flow(false);
        to_title(&mut f);
        // 30000 / 16 = 1875 idle updates; the update that set ready already counted one.
        assert!(ticks(&mut f, 1873, true).is_empty());
        assert_eq!(ticks(&mut f, 1, true), [Command::PlayMovie(Movie::Attract)]);
        // Skippable once more than 5000 FE-ms have played: 312 * 16 = 4992 is too early.
        ticks(&mut f, 312, true);
        f.input(Source::Pad(0), Key::A);
        assert!(f.take().is_empty());
        ticks(&mut f, 1, true);
        f.input(Source::Pad(0), Key::A);
        assert_eq!(f.take(), [Command::MovieSkip(true)]);
        f.input(Source::Pad(0), Key::B);
        assert_eq!(f.take(), [Command::MovieSkip(false)], "B hides the prompt and the movie continues");
        f.input(Source::Pad(0), Key::Start);
        assert_eq!(f.take(), [Command::MovieSkip(true)]);
        f.input(Source::Pad(0), Key::Start);
        assert!(f.take().is_empty(), "a second Start does nothing");
        f.input(Source::Pad(0), Key::A);
        assert_eq!(
            f.take(),
            [Command::Sound("core_a_button", 0), Command::StopMovie, Command::MovieSkip(false)]
        );
        // Back on the title: ready again on its next update, then Start works.
        ticks(&mut f, 1, true);
        assert!(!press_start(&mut f, Source::Pad(0)).is_empty());
    }

    #[test]
    fn any_key_resets_the_idle_timer() {
        let mut f = flow(false);
        to_title(&mut f);
        ticks(&mut f, 1800, true);
        f.input(Source::Pad(2), Key::Down);
        assert!(ticks(&mut f, 1874, true).is_empty());
        assert_eq!(ticks(&mut f, 1, true), [Command::PlayMovie(Movie::Attract)]);
    }

    #[test]
    fn only_the_active_pad_and_the_keyboard_drive_the_front_end() {
        let mut f = flow(false);
        to_title(&mut f);
        press_start(&mut f, Source::Pad(2));
        ticks(&mut f, 1, true);
        f.input(Source::Pad(0), Key::A);
        assert!(f.take().is_empty(), "pad 0 is filtered after pad 2 started");
        f.input(Source::Pad(2), Key::A);
        assert_eq!(f.take(), [Command::ForwardPopup(Key::A)]);
        f.input(Source::Keyboard, Key::A);
        assert_eq!(f.take(), [Command::ForwardPopup(Key::A)], "decision 5: the keyboard always works");
    }

    #[test]
    fn keyboard_start_leaves_every_pad_active() {
        let mut f = flow(false);
        to_title(&mut f);
        press_start(&mut f, Source::Keyboard);
        assert_eq!(f.active_pad(), None);
    }

    #[test]
    fn returning_boot_goes_from_the_autosave_warning_to_the_world() {
        let mut f = flow(false);
        to_title(&mut f);
        press_start(&mut f, Source::Pad(0));
        ticks(&mut f, 1, true);
        f.popup_select();
        assert_eq!(f.take(), [Command::ClosePopup]);
        assert_eq!(f.audio_state(), 0, "the title has popped: slot 0's idle state under the popup");
        // FlowMessage 21 -> StateLoading: the loading tag, then the cover fades to black over 25 updates.
        assert_eq!(ticks(&mut f, 1, true), [Command::LoadingTag(true)]);
        assert_eq!(f.cover(), 0.);
        assert!(ticks(&mut f, 24, true).is_empty(), "24 updates leave the cover short of black");
        assert!(f.cover() > 0.9 && f.cover() < 1.);
        assert!(!f.finished());
        assert_eq!(
            ticks(&mut f, 1, true),
            [Command::Finish(Outcome { first_boot: false, team_name: None, tutorial: None, boss_name: None })]
        );
        assert!(f.finished());
        assert_eq!(f.cover(), 1.);
    }

    /// The FE audio state the top slot's collection sets (bed 23 plays while it is 10).
    #[test]
    fn audio_state_follows_the_top_fe_slot() {
        let mut f = flow(false);
        assert_eq!(f.audio_state(), AUDIO_BOOTFLOW, "ESRB, slot 5: bootflow");
        ticks(&mut f, 250, false);
        assert_eq!(f.audio_state(), 0, "the EA movie, slot 24");
        f.movie_finished();
        assert_eq!(f.audio_state(), 0, "the loading wait: slot 0");
        ticks(&mut f, 1, true);
        assert_eq!(f.audio_state(), AUDIO_BOOTFLOW, "Press Start, slot 22: bootflow");
        ticks(&mut f, 95 + 1874, true);
        assert_eq!(f.movie_playing(), Some(Movie::Attract));
        assert_eq!(f.audio_state(), 0, "the attract video over the title");
        let mut g = flow(true);
        g.step = Step::CoachFrank;
        assert_eq!(g.audio_state(), 0, "coachfrank sets none");
        g.step = Step::CreateSkater;
        assert_eq!(g.audio_state(), AUDIO_CAC, "cac, slot 15");
    }

    /// The cover's f32 steps reach 1.0 on the 25th update, as 0x825E69A0's fadds do.
    #[test]
    fn cover_is_black_on_the_25th_update() {
        let (mut alpha, mut n) = (0f32, 0);
        while alpha < 1. {
            alpha += COVER_STEP;
            n += 1;
        }
        assert_eq!(n, 25);
        assert_eq!(COVER_STEP.to_bits(), 0x3D23_D70A, "0x82216FEC");
    }

    #[test]
    fn first_boot_runs_the_corrected_new_career_order() {
        let mut f = flow(true);
        to_title(&mut f);
        press_start(&mut f, Source::Pad(0));
        ticks(&mut f, 1, true);
        f.popup_select();
        f.take();
        assert_eq!(ticks(&mut f, 1, true), [Command::Open(Screen::Welcome)]);
        // Difficulty: the picker is the movie's; its OnMenuSelect comes back here.
        f.input(Source::Pad(0), Key::Down);
        assert_eq!(f.take(), [Command::ForwardScreen(Key::Down)]);
        f.menu_select(2);
        assert_eq!(
            f.take(),
            [
                Command::Difficulty(2),
                Command::Sound("core_a_button", 0),
                Command::CameraPreviews(true),
                Command::SetState(Screen::Welcome, 1.),
            ]
        );
        // Camera: Left/Right both play core_nav_left and toggle; other keys are consumed.
        f.input(Source::Pad(0), Key::Right);
        assert_eq!(f.take(), [Command::Sound("core_nav_left", 0), Command::SetState(Screen::Welcome, 2.)]);
        f.input(Source::Pad(0), Key::Up);
        assert!(f.take().is_empty());
        f.input(Source::Pad(0), Key::A);
        assert_eq!(
            f.take(),
            [Command::Sound("core_a_button", 0), Command::CameraHigh(true), Command::CameraPreviews(false)]
        );
        // The welcome state pops on its next update; the intro movie follows.
        assert_eq!(ticks(&mut f, 1, true), [Command::Close(Screen::Welcome), Command::PlayMovie(Movie::Intro)]);
        // Skippable after 10000 FE-ms (625 updates, strictly more).
        ticks(&mut f, 625, true);
        f.input(Source::Pad(0), Key::A);
        assert!(f.take().is_empty());
        ticks(&mut f, 1, true);
        f.input(Source::Pad(0), Key::A);
        f.input(Source::Pad(0), Key::A);
        assert_eq!(
            f.take(),
            [
                Command::MovieSkip(true),
                Command::Sound("core_a_button", 0),
                Command::StopMovie,
                Command::MovieSkip(false),
                Command::Open(Screen::TeamPrompt),
            ]
        );
        f.input(Source::Pad(0), Key::A);
        assert_eq!(
            f.take(),
            [Command::Sound("core_a_button", 0), Command::Close(Screen::TeamPrompt), Command::OpenKeyboard]
        );
        assert_eq!(f.keyboard.text, "New Team");
        // Empty -> popup 302 -> the keyboard again; invalid -> popup 263 -> again.
        f.keyboard.text.clear();
        f.input(Source::Pad(0), Key::Start);
        assert_eq!(
            f.take(),
            [Command::CloseKeyboard, Command::OpenPopup(Popup::EmptyName), Command::Sound("core_popup", 0)]
        );
        f.input(Source::Pad(0), Key::A);
        assert_eq!(f.take(), [Command::ForwardPopup(Key::A)]);
        f.popup_select();
        assert_eq!(f.take(), [Command::ClosePopup, Command::OpenKeyboard]);
        f.keyboard.text = "Bad~Name".into();
        f.input(Source::Keyboard, Key::Start);
        assert_eq!(
            f.take(),
            [Command::CloseKeyboard, Command::OpenPopup(Popup::InvalidName), Command::Sound("core_popup", 0)]
        );
        f.popup_select();
        f.take();
        f.keyboard.text.clear();
        f.type_text("Daytryan's Crew 12345");
        assert_eq!(f.keyboard.text, "Daytryan's Crew ", "16 characters at most");
        f.input(Source::Pad(0), Key::Start);
        assert_eq!(f.take(), [Command::CloseKeyboard, Command::OpenTeamLogos]);
        // The logo picker (slot 63, state 30) owns every key; its commit closes it and slot 67 opens.
        f.input(Source::Pad(0), Key::Right);
        assert_eq!(f.take(), [Command::ForwardTeam(Key::Right)]);
        f.team_closed();
        assert_eq!(f.take(), [Command::Open(Screen::TeamManagement)]);
        // Start your team: the first pick asks the boss's name ("The Legend"), then Customize Skater -> CAS ->
        // the Coach Frank movie -> Coach Frank.
        f.menu_select(0);
        assert_eq!(f.take(), [Command::OpenKeyboard]);
        assert_eq!((f.keyboard.text.as_str(), f.keyboard_header()), ("The Legend", "ID_KEYBOARD_PROFILE_NAME_HEADER"));
        f.keyboard.text = "Ryan".into();
        f.input(Source::Pad(0), Key::Start);
        assert_eq!(f.take(), [Command::CloseKeyboard, Command::Close(Screen::TeamManagement), Command::OpenCreateSkater]);
        assert!(f.in_create_skater());
        f.create_skater_closed();
        // A build without story mode has no Skate School: the CAS's close loads the game, no answer recorded.
        let tutorial = f.skate_school.then_some(true);
        if f.skate_school {
            assert_eq!(f.take(), [Command::PlayMovie(Movie::CoachFrankIntro)]);
            f.movie_finished();
            assert_eq!(f.take(), [Command::Open(Screen::CoachFrank)]);
            f.input(Source::Pad(0), Key::Down);
            assert_eq!(f.take(), [Command::ForwardScreen(Key::Down)]);
            f.menu_select(0);
            assert_eq!(f.take(), [Command::Close(Screen::CoachFrank), Command::LoadingTag(true)]);
        } else {
            assert_eq!(f.take(), [Command::LoadingTag(true)]);
        }
        assert_eq!(
            ticks(&mut f, 25, true),
            [Command::Finish(Outcome {
                first_boot: true,
                team_name: Some("Daytryan's Crew ".into()),
                tutorial,
                boss_name: Some("Ryan".into()),
            })]
        );
    }

    #[test]
    fn keyboard_cancel_keeps_the_default_name() {
        let mut f = flow(true);
        f.step = Step::TeamPrompt;
        f.input(Source::Pad(0), Key::A);
        f.take();
        f.type_text("zz");
        f.input(Source::Pad(0), Key::B);
        assert_eq!(f.take(), [Command::CloseKeyboard, Command::OpenTeamLogos]);
        assert_eq!(f.outcome.team_name.as_deref(), Some("New Team"));
    }

    /// Slot 67's Preset Skaters: the name first (cancel stays put), then Import Skater; its close reopens slot 67.
    #[test]
    #[cfg_attr(not(feature = "story-mode"), ignore = "needs the story-mode feature")]
    fn preset_skaters_names_the_boss_then_imports() {
        let mut f = flow(true);
        f.step = Step::TeamManagement;
        f.take();
        f.menu_select(1);
        assert_eq!(f.take(), [Command::OpenKeyboard]);
        f.input(Source::Pad(0), Key::B);
        assert_eq!(f.take(), [Command::CloseKeyboard]);
        assert_eq!(f.outcome.boss_name, None);
        f.menu_select(1);
        f.take();
        f.input(Source::Pad(0), Key::Start);
        assert_eq!(f.take(), [Command::CloseKeyboard, Command::Close(Screen::TeamManagement), Command::OpenImport]);
        assert_eq!(f.outcome.boss_name.as_deref(), Some("The Legend"));
        f.input(Source::Pad(0), Key::Down);
        assert_eq!(f.take(), [Command::ForwardTeam(Key::Down)]);
        f.team_closed();
        assert_eq!(f.take(), [Command::Open(Screen::TeamManagement)]);
        // Named once: the next pick goes straight on.
        f.menu_select(2);
        assert_eq!(f.take(), [Command::Close(Screen::TeamManagement), Command::PlayMovie(Movie::CoachFrankIntro)]);
    }

    /// A release without story mode (user 10-22): Continue and a closed Create a Skater skip Coach Frank and load the
    /// game, with no Skate School answer recorded.
    #[test]
    fn without_story_mode_coach_frank_is_skipped() {
        let mut f = flow(true);
        f.skate_school = false;
        f.take();
        f.step = Step::CreateSkater;
        f.create_skater_closed();
        assert_eq!(f.take(), [Command::LoadingTag(true)]);
        assert!(matches!(f.step, Step::GameLoad { .. }));
        assert_eq!(f.outcome.tutorial, None);
    }

    /// 0x82710760: popup 262 for an empty, all-space or non-alphanumeric boss name; its OK reopens the keyboard.
    #[test]
    fn the_boss_name_must_be_alphanumeric() {
        for bad in ["", "   ", "Bob's", "Zoë"] {
            assert!(!valid_profile_name(bad), "{bad:?}");
        }
        for good in ["Ryan", "The Legend", " a1 "] {
            assert!(valid_profile_name(good), "{good:?}");
        }
        let mut f = flow(true);
        f.step = Step::TeamManagement;
        f.take();
        f.menu_select(1);
        f.take();
        f.keyboard.text = "   ".into();
        f.input(Source::Pad(0), Key::Start);
        assert!(f.take().contains(&Command::OpenPopup(Popup::RedoProfileName)));
        f.popup_select();
        assert!(f.take().contains(&Command::OpenKeyboard));
        assert_eq!(f.outcome.boss_name, None);
    }

    #[test]
    fn on_screen_keyboard_types_with_a_controller() {
        let mut k = TextEntry::new("", TEAM_NAME_MAX);
        // 1, then Right x2 = 3, Down = e (QWERTY).
        k.input(Key::A);
        k.input(Key::Right);
        k.input(Key::Right);
        k.input(Key::A);
        k.input(Key::Down);
        k.input(Key::A);
        assert_eq!(k.text, "13e");
        k.input(Key::X);
        assert_eq!(k.text, "13");
        k.input(Key::Y);
        assert_eq!(k.text, "13 ");
        // The command row: Shift spans columns 1-2, Done 8-9.
        k.row = GRID.len();
        k.column = 1;
        k.input(Key::A);
        assert!(k.shift);
        k.row = 1;
        k.column = 0;
        k.input(Key::A);
        assert_eq!(k.text, "13 Q");
        k.row = GRID.len();
        k.column = 9;
        assert_eq!(k.input(Key::A), Some(Typed::Done));
        // Every grid character is one the validation accepts.
        assert!(GRID.iter().flat_map(|r| r.chars()).chain(['/']).all(|c| VALID.contains(c) && VALID.contains(c.to_ascii_uppercase())));
        // ...and every one of them has a key (the command row has '/' and space).
        let keys: std::collections::BTreeSet<char> = GRID.iter().flat_map(|r| r.chars()).collect();
        assert_eq!(keys.len(), 50, "no key twice");
        assert!(VALID.chars().filter(|c| !c.is_ascii_uppercase() && !" /".contains(*c)).all(|c| keys.contains(&c)));
    }

    #[test]
    fn movie_records_match_the_vault() {
        assert_eq!(Movie::EaIntro.volume(), 0.8);
        assert_eq!(Movie::CoachFrankIntro.volume(), 0.6);
        assert_eq!(Movie::Intro.skip_after(), Some(f32::from_bits(0x461C_4000) as u32));
        assert_eq!(Movie::Attract.skip_after(), Some(f32::from_bits(0x459C_4000) as u32));
    }
}
