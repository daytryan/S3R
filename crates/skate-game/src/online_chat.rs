//! Not retail (user 10-11): online text chat. Skate 3 had none; the user asked for one that looks like the game's own
//! online notices. In a session, Enter opens the typing bar ("Say: ..."), Enter sends the line to every member
//! (skate_net::chat, the sender named from the session, never from the text), Escape cancels. The feed, the last
//! [`FEED`] lines newest at the bottom, is drawn by online_hud in the event log's text style.
use crate::{multiplayer::Multiplayer, online_roster::OnlineRoster, text_entry::TextEntry};
use bevy::{
    ecs::system::SystemParam,
    input::{ButtonState, keyboard::KeyboardInput},
    prelude::*,
};
use skate_net::chat::MAX_CHARS;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Lines the feed shows while the bar is closed (newest at the bottom).
pub(crate) const FEED: usize = 6;
/// Lines a session keeps, for scrolling back with the bar open; never saved, gone when the session ends.
pub(crate) const HISTORY: usize = 200;
/// Seconds the "Press Enter to chat" hint shows after joining a session.
pub(crate) const HINT_FOR: f32 = 30.;
/// Rows one line takes at most, the last ending "..." (audit 10-24: six 120-character lines ran off the screen's top;
/// user 10-29: 4, so a whole 150-character line shows, the box leaving out the oldest lines instead: online_hud).
pub(crate) const LINE_ROWS: usize = 4;
/// Seconds a line shows before it fades, and the fade. Knobs: the event log's own line shows 5 s (online_hud SHOW_FOR).
pub(crate) const SHOW_FOR: f32 = 8.;
pub(crate) const FADE_FOR: f32 = 0.6;
/// A sender the roster does not name yet (its `@player` not in).
const UNNAMED: &str = "Skater";
/// Q9: distinct characters past Latin one sender may bring in a session; the rest read '?', so one player can't use
/// up text_fallback's glyph cache (2048) for everyone. Knob. ponytail: counts atlas characters too; ask text_fallback
/// if CJK chat outgrows it.
const RARE_EACH: usize = 256;
fn budget(seen: &mut BTreeSet<char>, text: &str) -> String {
    text.chars().map(|c| if c <= '\u{24f}' || seen.contains(&c) || seen.len() < RARE_EACH && seen.insert(c) { c } else { '?' }).collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Line {
    pub name: String,
    /// Ours: the name in the roster's highlight colour.
    pub local: bool,
    /// C4: the sender is the host / a Steam friend (online_hud marks the name; the text can't).
    pub host: bool,
    pub friend: bool,
    /// User 10-25: the sender is the verified creator (creator.rs; online_hud draws the name gold).
    pub creator: bool,
    pub text: String,
    pub born: f32,
}

#[derive(Resource, Default)]
pub(crate) struct Chat {
    /// The typing bar, while it is open.
    pub bar: Option<TextEntry>,
    /// When the bar opened (its caret blinks from then).
    pub opened: f32,
    /// The feed: oldest first, at most [`FEED`].
    pub lines: VecDeque<Line>,
    /// The session's lines, oldest first, at most [`HISTORY`]: what the open bar scrolls back through.
    pub history: VecDeque<Line>,
    /// With the bar open: how many of the newest lines are scrolled past (Up / Page Up older, Down / Page Down newer).
    pub scroll: usize,
    /// The hint shows until then (set on joining a session, not on moves within it).
    pub hint_until: f32,
    /// In a session ([`exchange`] keeps it): the bar opens only then.
    pub live: bool,
    /// Lines typed and not yet sent.
    outbox: Vec<String>,
    /// A line the bar was closed on (a popup, a menu, a session move): the bar opens with it again (audit 10-24).
    draft: String,
    /// Q9: each sender's uncommon characters so far ([`RARE_EACH`]).
    rare: BTreeMap<u64, BTreeSet<char>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Press<'a> {
    Enter,
    Escape,
    Backspace,
    Older,
    Newer,
    Text(&'a str),
}

impl Chat {
    /// One key. `free`: no menu, popup or other text field owns the keyboard.
    pub(crate) fn press(&mut self, press: Press, free: bool) {
        match (self.bar.as_mut(), press) {
            (None, Press::Enter) if free && self.live => {
                self.bar = Some(TextEntry::new(&std::mem::take(&mut self.draft), MAX_CHARS));
                self.scroll = 0;
            }
            (None, _) => {}
            (Some(_), Press::Older) => self.scroll = (self.scroll + 1).min(self.history.len().saturating_sub(1)),
            (Some(_), Press::Newer) => self.scroll = self.scroll.saturating_sub(1),
            (Some(_), Press::Enter) => {
                // An empty line just closes the bar.
                if let Some(text) = self.bar.take().and_then(|bar| skate_net::chat::clean(&bar.text)) {
                    self.outbox.push(text);
                }
            }
            (Some(_), Press::Escape) => self.bar = None,
            (Some(bar), Press::Backspace) => bar.backspace(),
            (Some(bar), Press::Text(text)) => bar.type_text(text),
        }
    }
    /// The session ended or Text Chat went off: all goes but a half-typed line.
    fn reset(&mut self) {
        let draft = self.bar.take().map_or(std::mem::take(&mut self.draft), |bar| bar.text);
        *self = Chat { draft, ..default() };
    }
    pub(crate) fn push(&mut self, line: Line) {
        self.history.push_back(line.clone());
        if self.history.len() > HISTORY {
            self.history.pop_front();
        }
        self.lines.push_back(line);
        // Audit 10-24: the oldest line of whoever holds the most goes, so one fast typist can't push everyone else out.
        while self.lines.len() > FEED {
            let held = |name: &str| self.lines.iter().filter(|l| l.name == name).count();
            let most = self.lines.iter().map(|l| held(&l.name)).max().unwrap_or(0);
            let oldest = self.lines.iter().position(|l| held(&l.name) == most).unwrap_or(0);
            self.lines.remove(oldest);
        }
    }
    /// The lines the feed shows at `now` with their opacity: each for [`SHOW_FOR`] s, then fading over [`FADE_FOR`];
    /// while the bar is open, every line held, unfaded.
    pub(crate) fn visible(&self, now: f32) -> Vec<(&Line, f32)> {
        if self.bar.is_some() {
            let end = self.history.len().saturating_sub(self.scroll);
            return self.history.iter().take(end).map(|line| (line, 1.)).collect();
        }
        (self.lines.iter())
            .filter_map(|line| {
                let alpha = if self.bar.is_some() { 1. } else { ((line.born + SHOW_FOR + FADE_FOR - now) / FADE_FOR).clamp(0., 1.) };
                (alpha > 0.).then_some((line, alpha))
            })
            .collect()
    }
}

/// `text` in at most [`LINE_ROWS`] rows no wider than `max` (measured by `width`), the first after a prefix `first`
/// wide: broken at spaces, a word longer than a row at characters.
pub(crate) fn wrap(width: &dyn Fn(&str) -> f32, first: f32, text: &str, max: f32) -> Vec<String> {
    let room = |row: usize| if row == 0 { max - first } else { max };
    let mut rows = vec![String::new()];
    for word in text.split_whitespace() {
        let n = rows.len() - 1;
        let joined = if rows[n].is_empty() { word.to_owned() } else { format!("{} {word}", rows[n]) };
        if width(&joined) <= room(n) {
            rows[n] = joined;
            continue;
        }
        if !rows[n].is_empty() {
            rows.push(String::new());
        }
        for c in word.chars() {
            let n = rows.len() - 1;
            rows[n].push(c);
            if width(&rows[n]) > room(n) && rows[n].chars().count() > 1 {
                rows[n].pop();
                rows.push(c.to_string());
            }
        }
    }
    if rows.len() > LINE_ROWS {
        rows.truncate(LINE_ROWS);
        let last = &mut rows[LINE_ROWS - 1];
        while !last.is_empty() && width(&format!("{}...", last.trim_end())) > room(LINE_ROWS - 1) {
            last.pop();
        }
        *last = format!("{}...", last.trim_end());
    }
    rows
}

/// What owns the keyboard's Enter in gameplay besides the chat: every reader of it (the Start menu, the classic
/// panel, the teleport menu, the Edit Skater / Graphic Creator / custom-model screens, the mod menu and a focused mod
/// panel, the Challenge Map, the online / music / retail / team / Party Play popups, the replay editor and its reel).
/// The Skate 1 / 2 tutorials' prompts are single player only.
#[derive(SystemParam)]
pub(crate) struct Overlays<'w> {
    menu: Option<Res<'w, crate::graphics_menu::Menu>>,
    crossbar: Option<Res<'w, crate::core_menu::Crossbar>>,
    transition: Option<Res<'w, crate::map_transition::MapTransition>>,
    travel: Option<Res<'w, crate::teleport_menu::Travel>>,
    customiser: Option<Res<'w, crate::customiser::Customiser>>,
    custom_models: Option<Res<'w, crate::custom_models::CustomModels>>,
    mods: Option<Res<'w, crate::modding::ModMenu>>,
    panel: Option<Res<'w, crate::modding::EnabledPanel>>,
    challenge: Option<Res<'w, crate::challenge_map::ChallengeMap>>,
    online: Option<Res<'w, crate::online_menu::OnlineUi>>,
    music: Option<Res<'w, crate::music_ui::MusicUi>>,
    retail: Option<Res<'w, crate::retail_ui::RetailUi>>,
    team: Option<Res<'w, crate::team_select::TeamUi>>,
    party: Option<Res<'w, crate::party_play::PartyPlay>>,
    replay: Option<Res<'w, crate::replay::hud::ReplayHud>>,
}
impl Overlays<'_> {
    pub(crate) fn open(&self) -> bool {
        fn on<T: Resource>(r: &Option<Res<T>>, f: impl Fn(&T) -> bool) -> bool {
            r.as_deref().is_some_and(f)
        }
        on(&self.menu, |m| m.open)
            || on(&self.crossbar, |c| c.runtime.is_some())
            || on(&self.transition, |t| t.busy())
            || on(&self.travel, |t| t.open)
            || on(&self.customiser, |c| c.open)
            || on(&self.custom_models, |c| c.open)
            || on(&self.mods, |m| m.open)
            || on(&self.panel, |p| p.focused)
            || on(&self.challenge, |c| c.active())
            || on(&self.online, |o| o.modal)
            || on(&self.music, |m| m.modal)
            || on(&self.retail, |r| r.modal)
            || on(&self.team, |t| t.modal)
            || on(&self.party, |p| p.modal)
            || on(&self.replay, |r| r.busy())
    }
}

fn press_of(event: &KeyboardInput) -> Option<Press<'_>> {
    if event.state != ButtonState::Pressed {
        return None;
    }
    match event.key_code {
        // A held Enter / Escape repeats; one press is one choice. Backspace and letters repeat.
        KeyCode::Enter | KeyCode::NumpadEnter => (!event.repeat).then_some(Press::Enter),
        KeyCode::Escape => (!event.repeat).then_some(Press::Escape),
        KeyCode::Backspace => Some(Press::Backspace),
        KeyCode::ArrowUp | KeyCode::PageUp => Some(Press::Older),
        KeyCode::ArrowDown | KeyCode::PageDown => Some(Press::Newer),
        _ => event.text.as_deref().map(Press::Text),
    }
}

/// The keyboard, before anything else reads it (customiser::navigation and every menu run after this). While the bar
/// is open, and on the frames it opens and closes, nobody else sees a key (not the Enter that opened it, nor the
/// Escape that closed it, nor a letter a vehicle or the mod panel would take), and the skater gets no pad input.
pub(crate) fn keys(
    mut chat: ResMut<Chat>,
    mut typing: MessageReader<KeyboardInput>,
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    overlays: Overlays,
    input: Option<ResMut<crate::input::ControllerInput>>,
    time: Res<Time<Real>>,
) {
    let free = !overlays.open();
    // C9: taken before the close below, so the Enter that would have sent the line can't answer the popup that
    // closed it.
    let was = chat.bar.is_some();
    // A menu or popup came up over it (a pad's Start), or the session ended: the bar closes unsent, its text kept.
    if (!free || !chat.live)
        && let Some(bar) = chat.bar.take()
    {
        chat.draft = bar.text;
    }
    for event in typing.read() {
        if let Some(press) = press_of(event) {
            chat.press(press, free);
        }
    }
    // The mouse wheel scrolls the open bar's history too: up older, down newer.
    for event in wheel.read() {
        if event.y != 0. {
            chat.press(if event.y > 0. { Press::Older } else { Press::Newer }, free);
        }
    }
    if chat.bar.is_some() && !was {
        chat.opened = time.elapsed_secs();
    }
    if was || chat.bar.is_some() {
        keys.reset_all();
    }
    if let Some(mut input) = input {
        input.chat = chat.bar.is_some();
    }
}

/// The sender's name: the roster's as the session shows it (a copied name numbered), never one the line carries.
pub(crate) fn name_of(members: &[skate_net::roster::Member], actor: u64) -> String {
    crate::player_list::shown_name(members, actor).unwrap_or_else(|| UNNAMED.into())
}
fn line(members: &[skate_net::roster::Member], actor: u64, local: bool, text: String, born: f32) -> Line {
    let host = members.iter().any(|m| m.actor == actor && m.host);
    Line {
        name: name_of(members, actor),
        local,
        host,
        friend: crate::player_list::friend(actor),
        creator: crate::creator::is_creator(actor),
        text,
        born,
    }
}

/// Sends the lines typed (shown as ours at once) and takes the members' lines into the feed.
pub(crate) fn exchange(mut chat: ResMut<Chat>, mut net: ResMut<Multiplayer>, roster: Res<OnlineRoster>, time: Res<Time<Real>>) {
    // Settings > Audio > Text Chat off: no bar, no feed, and what arrives is dropped.
    let was = chat.live;
    chat.live = net.active() && crate::player_list::CHAT.load(std::sync::atomic::Ordering::Relaxed);
    if chat.live && !was {
        chat.hint_until = time.elapsed_secs() + HINT_FOR;
    }
    if !chat.live {
        if let Some(session) = net.session_mut() {
            session.chat.clear();
        }
        if chat.bar.is_some() || !chat.lines.is_empty() || !chat.outbox.is_empty() {
            chat.reset();
        }
        return;
    }
    let now_ms = net.now_ms();
    if let Some(session) = net.session_mut() {
        trade(&mut chat, session, &roster.members, now_ms, time.elapsed_secs());
    }
}

/// [`exchange`] on `session` at `now_ms` (its clock) and `now` (the feed's).
pub(crate) fn trade(chat: &mut Chat, session: &mut skate_net::lobby::Session, members: &[skate_net::roster::Member], now_ms: u64, now: f32) {
    // Audit 10-24: a line past the rate (5 in 5 s) waits for the window, in order, instead of vanishing without a word;
    // past a window's worth waiting, the rest are dropped (a spammer).
    let mut waiting = Vec::new();
    // User 10-25: slurs are masked before a line leaves (chat_filter) and as one arrives; swearing only as it shows,
    // by this player's Text Chat (Filtered).
    let swears = crate::player_list::CHAT_FILTER.load(std::sync::atomic::Ordering::Relaxed);
    for text in std::mem::take(&mut chat.outbox) {
        if !waiting.is_empty() {
            waiting.push(text);
        } else if let Some(text) = session.say(&crate::chat_filter::filter(&text, false), now_ms) {
            chat.push(line(members, session.local, true, crate::chat_filter::filter(&text, swears), now));
        } else if skate_net::chat::clean(&text).is_some() {
            waiting.push(text);
        }
    }
    waiting.truncate(skate_net::chat::RATE);
    chat.outbox = waiting;
    // C6: a muted or blocked sender, or one Chat From: Friends leaves out, never reaches the feed.
    for (actor, text) in session.chat.drain(..).filter(|(actor, _)| crate::player_list::reads(*actor)).collect::<Vec<_>>() {
        // Online safety 10-26 (review 10-24 R3): by the Steam ID Steam vouches for, where there is one. A rejoin is a
        // new actor, and each brought a fresh budget into the shared glyph cache (text_fallback, never cleared).
        let sender = members.iter().find(|m| m.actor == actor).map_or(0, |m| m.steam);
        let seen = chat.rare.entry(if sender != 0 { sender } else { actor }).or_default();
        let text = budget(seen, &crate::chat_filter::filter(&text, swears));
        chat.push(line(members, actor, false, text, now));
    }
}

pub(crate) struct OnlineChatPlugin;
impl Plugin for OnlineChatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Chat>()
            .add_systems(PreUpdate, keys.after(bevy::input::InputSystems).before(crate::customiser::navigation))
            .add_systems(Update, exchange.after(crate::online_roster::track));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, born: f32) -> Line {
        Line { name: "Ann".into(), text: text.into(), born, ..default() }
    }

    #[test]
    fn the_bar_types_sends_and_cancels() {
        let mut chat = Chat { live: true, ..default() };
        chat.press(Press::Enter, false);
        assert!(chat.bar.is_none(), "a menu owns the keyboard");
        chat.press(Press::Enter, true);
        for press in [Press::Text("hey"), Press::Text("\u{7}"), Press::Backspace, Press::Text("!!")] {
            chat.press(press, true);
        }
        assert_eq!(chat.bar.as_ref().unwrap().text, "he!!");
        chat.press(Press::Enter, true);
        assert_eq!((chat.bar.is_none(), chat.outbox.as_slice()), (true, &["he!!".to_owned()][..]));
        // Escape cancels; an empty line just closes; typing stops at MAX_CHARS.
        chat.press(Press::Enter, true);
        chat.press(Press::Text("never sent"), true);
        chat.press(Press::Escape, true);
        chat.press(Press::Enter, true);
        chat.press(Press::Text("   "), true);
        chat.press(Press::Enter, true);
        assert_eq!(chat.outbox.len(), 1);
        chat.press(Press::Enter, true);
        chat.press(Press::Text(&"x".repeat(200)), true);
        assert_eq!(chat.bar.as_ref().unwrap().text.chars().count(), MAX_CHARS);
        // Single player: no bar.
        let mut offline = Chat::default();
        offline.press(Press::Enter, true);
        assert!(offline.bar.is_none());
    }

    #[test]
    fn the_feed_shows_six_lines_that_fade_after_eight_seconds() {
        let mut chat = Chat::default();
        for n in 0..8 {
            chat.push(line(&n.to_string(), n as f32));
        }
        assert_eq!((chat.lines.len(), chat.history.len()), (FEED, 8), "the feed shows six; the session keeps them all");
        let at = |chat: &Chat, now| chat.visible(now).into_iter().map(|(l, a)| (l.text.clone(), a)).collect::<Vec<_>>();
        assert_eq!(at(&chat, 7.), ["2", "3", "4", "5", "6", "7"].map(|t| (t.to_owned(), 1.)));
        let fading = at(&chat, 2. + SHOW_FOR + FADE_FOR / 2.);
        assert_eq!(fading[0].0, "2");
        assert!((fading[0].1 - 0.5).abs() < 1e-3, "half way through its fade");
        assert_eq!(at(&chat, 7. + SHOW_FOR + FADE_FOR), [], "all gone");
        chat.bar = Some(TextEntry::new("", MAX_CHARS));
        assert_eq!(at(&chat, 100.).len(), 8, "the open bar shows the whole session again, unfaded");
        assert!(at(&chat, 100.).iter().all(|(_, a)| *a == 1.));
        // Scrolling back and forth, and never past the first line.
        chat.press(Press::Older, true);
        chat.press(Press::Older, true);
        assert_eq!(at(&chat, 100.).last().map(|l| l.0.clone()), Some("5".to_owned()));
        chat.press(Press::Newer, true);
        assert_eq!(at(&chat, 100.).last().map(|l| l.0.clone()), Some("6".to_owned()));
        for _ in 0..20 {
            chat.press(Press::Older, true);
        }
        assert_eq!(at(&chat, 100.).len(), 1, "the oldest line stays");
        for n in 8..300 {
            chat.push(line(&(n % 3).to_string(), n as f32));
        }
        assert_eq!(chat.history.len(), HISTORY, "a session keeps at most HISTORY lines");
    }

    #[test]
    fn long_lines_wrap_at_spaces_and_long_words_at_characters() {
        let width = |s: &str| s.chars().count() as f32 * 10.;
        assert_eq!(wrap(&width, 0., "short", 100.), ["short"]);
        assert_eq!(wrap(&width, 40., "aaa bbb ccc", 100.), ["aaa", "bbb ccc"], "the name takes room on the first row");
        assert_eq!(wrap(&width, 0., "abcdefghijklmnop", 100.), ["abcdefghij", "klmnop"]);
    }

    /// Audit 10-24: a 120-character line took up to 5 rows, so six from one player made a column that ran off the top
    /// of the screen. A line takes LINE_ROWS rows at most, the last ending in "...".
    #[test]
    fn a_long_line_takes_four_rows_at_most() {
        let width = |s: &str| s.chars().count() as f32 * 10.;
        let words = "aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii";
        assert_eq!(wrap(&width, 0., words, 100.), ["aaaa bbbb", "cccc dddd", "eeee ffff", "gggg hh..."]);
        let ten = "x".repeat(10);
        assert_eq!(wrap(&width, 40., &"x".repeat(MAX_CHARS), 100.), ["xxxxxx", ten.as_str(), ten.as_str(), "xxxxxxx..."]);
        assert_eq!(wrap(&width, 0., "aaaa bbbb cccc dddd eeee", 100.), ["aaaa bbbb", "cccc dddd", "eeee"], "three rows: whole");
    }

    /// Audit 10-24: one fast typist kept all six feed lines, pushing everyone else's off within a second. A full feed
    /// drops the oldest line of whoever holds the most; with one sender that is still the oldest.
    #[test]
    fn one_fast_typist_cannot_push_everyone_else_out_of_the_feed() {
        let said = |name: &str, text: &str| Line { name: name.into(), text: text.into(), ..default() };
        let mut chat = Chat::default();
        chat.push(said("Bob", "hi"));
        chat.push(said("Cat", "yo"));
        for n in 0..10 {
            chat.push(said("Spam", &n.to_string()));
        }
        assert_eq!(chat.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["hi", "yo", "6", "7", "8", "9"]);
    }

    #[test]
    fn a_line_is_named_from_the_roster() {
        let members = vec![skate_net::roster::Member { actor: 20, name: "Real Name".into(), steam: 0, host: false }];
        assert_eq!(name_of(&members, 20), "Real Name");
        assert_eq!(name_of(&members, 99), UNNAMED);
    }

    /// Two sessions over the real wire: a line typed by the guest shows at once on its side and reaches the host under
    /// the name the guest's `@player` gives, once.
    #[test]
    fn a_typed_line_reaches_the_other_player_under_their_roster_name() {
        use skate_net::{
            lobby::{Info, Session},
            roster::{self, PLAYER_KEY, Player},
        };
        let info = |id| Info { id, map: 1, rig: 2, physics: 3, appearance: 4 };
        let (mut host, mut guest) = (Session::new(7, info(10), None), Session::new(7, info(20), Some(1)));
        host.publish_application(PLAYER_KEY, Player { name: "Host Name".into(), steam: 0 }.encode(), 0);
        guest.publish_application(PLAYER_KEY, Player { name: "Guest Name".into(), steam: 0 }.encode(), 0);
        let pump = |host: &mut Session, guest: &mut Session, now: &mut u64| {
            for _ in 0..20 {
                *now += 50;
                for p in host.service(*now) {
                    guest.receive(1, &p.data, *now);
                }
                for p in guest.service(*now) {
                    host.receive(2, &p.data, *now);
                }
            }
        };
        let mut now = 0;
        pump(&mut host, &mut guest, &mut now);
        let (mut at_host, mut at_guest) = (Chat { live: true, ..default() }, Chat { live: true, ..default() });
        for press in [Press::Enter, Press::Text("gg "), Press::Enter] {
            at_guest.press(press, true);
        }
        let members = roster::members(&guest);
        trade(&mut at_guest, &mut guest, &members, now, 1.);
        pump(&mut host, &mut guest, &mut now);
        for _ in 0..2 {
            let members = roster::members(&host);
            trade(&mut at_host, &mut host, &members, now, 2.);
        }
        let line = |local, born| Line { name: "Guest Name".into(), local, text: "gg".into(), born, ..default() };
        assert_eq!(at_host.lines, [line(false, 2.)]);
        assert_eq!(at_guest.lines, [line(true, 1.)], "ours shows at once, under our name");
        // User 10-25: a slur is masked before it leaves, so even a receiver without the filter never gets it (ROT13
        // here, as in chat_filter).
        let slur: String = "avttre".chars().map(|c| (b'a' + (c as u8 - b'a' + 13) % 26) as char).collect();
        let typed = format!("ok {slur}");
        for press in [Press::Enter, Press::Text(&typed), Press::Enter] {
            at_guest.press(press, true);
        }
        let members = roster::members(&guest);
        trade(&mut at_guest, &mut guest, &members, now, 3.);
        pump(&mut host, &mut guest, &mut now);
        assert_eq!(host.chat.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), ["ok ******"], "masked on the wire");
        assert_eq!(at_guest.lines.back().map(|l| l.text.as_str()), Some("ok ******"), "and in our own feed");
    }

    /// Audit 10-24: a 6th line within 5 s vanished without a word (gone for us and everyone). It now waits for the
    /// rate's window and goes then; past a window's worth waiting, the rest are dropped (a spammer).
    #[test]
    fn a_line_typed_past_the_rate_waits_and_goes_when_the_window_frees() {
        use skate_net::lobby::{Info, Session};
        let mut session = Session::new(7, Info { id: 20, map: 1, rig: 2, physics: 3, appearance: 4 }, Some(1));
        let mut chat = Chat { live: true, ..default() };
        for n in 0..6 + skate_net::chat::RATE + 2 {
            for press in [Press::Enter, Press::Text(&format!("line {n}")), Press::Enter] {
                chat.press(press, true);
            }
        }
        trade(&mut chat, &mut session, &[], 1_000, 1.);
        assert_eq!(chat.lines.len(), skate_net::chat::RATE, "five go now");
        trade(&mut chat, &mut session, &[], 3_000, 2.);
        assert_eq!(chat.lines.len(), skate_net::chat::RATE, "the rest wait");
        trade(&mut chat, &mut session, &[], 6_000, 3.);
        let texts: Vec<&str> = chat.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts.last(), Some(&"line 9"), "the next five went once the window freed: {texts:?}");
        trade(&mut chat, &mut session, &[], 12_000, 4.);
        assert_eq!(chat.lines.back().map(|l| l.text.as_str()), Some("line 9"), "past a window's worth: dropped");
    }

    fn key(key_code: KeyCode, text: Option<&str>) -> KeyboardInput {
        KeyboardInput {
            key_code,
            logical_key: bevy::input::keyboard::Key::Enter,
            state: ButtonState::Pressed,
            text: text.map(Into::into),
            repeat: false,
            window: Entity::PLACEHOLDER,
        }
    }
    fn press(app: &mut App, code: KeyCode, text: Option<&str>) {
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(code);
        app.world_mut().write_message(key(code, text));
        app.update();
    }

    /// Enter opens the bar only in a session with nothing else open; while it is open no other system sees a key and
    /// the skater gets no pad input; Escape closes it without opening the Start menu.
    #[test]
    fn enter_opens_only_when_free_and_typing_holds_every_other_input() {
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .add_message::<bevy::input::mouse::MouseWheel>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Time<Real>>()
            .init_resource::<crate::input::ControllerInput>()
            .insert_resource(Chat { live: true, ..default() })
            .init_resource::<crate::online_menu::OnlineUi>()
            .add_systems(Update, keys);
        app.world_mut().resource_mut::<crate::online_menu::OnlineUi>().modal = true;
        press(&mut app, KeyCode::Enter, None);
        assert!(app.world().resource::<Chat>().bar.is_none(), "a popup is up");
        assert!(app.world().resource::<ButtonInput<KeyCode>>().just_pressed(KeyCode::Enter), "the popup gets its Enter");
        app.world_mut().resource_mut::<crate::online_menu::OnlineUi>().modal = false;
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().reset_all();
        press(&mut app, KeyCode::Enter, None);
        assert!(app.world().resource::<Chat>().bar.is_some());
        assert!(!app.world().resource::<ButtonInput<KeyCode>>().just_pressed(KeyCode::Enter), "nobody else takes it");
        assert!(app.world().resource::<crate::input::ControllerInput>().chat, "the skater is held");
        press(&mut app, KeyCode::KeyW, Some("w"));
        assert!(!app.world().resource::<ButtonInput<KeyCode>>().pressed(KeyCode::KeyW), "a vehicle never sees W");
        assert_eq!(app.world().resource::<Chat>().bar.as_ref().unwrap().text, "w");
        press(&mut app, KeyCode::Escape, None);
        assert!(app.world().resource::<Chat>().bar.is_none());
        assert!(!app.world().resource::<ButtonInput<KeyCode>>().just_pressed(KeyCode::Escape), "no Start menu");
        assert!(!app.world().resource::<crate::input::ControllerInput>().chat);
        // Out of a session the bar never opens.
        app.world_mut().resource_mut::<Chat>().live = false;
        press(&mut app, KeyCode::Enter, None);
        assert!(app.world().resource::<Chat>().bar.is_none());
        // C9: a popup raised mid-line closes the bar, and the Enter meant to send the line doesn't answer it.
        app.world_mut().resource_mut::<Chat>().live = true;
        press(&mut app, KeyCode::Enter, None);
        app.world_mut().resource_mut::<crate::online_menu::OnlineUi>().modal = true;
        press(&mut app, KeyCode::Enter, None);
        assert!(app.world().resource::<Chat>().bar.is_none());
        assert!(!app.world().resource::<ButtonInput<KeyCode>>().just_pressed(KeyCode::Enter), "the popup never sees it");
    }

    /// Audit 10-24: a popup, menu or session move closes the bar (C9), and the half-typed line went with it. It comes
    /// back the next time the bar opens; Escape still throws it away.
    #[test]
    fn a_half_typed_line_comes_back_after_an_interruption() {
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .add_message::<bevy::input::mouse::MouseWheel>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Time<Real>>()
            .init_resource::<crate::input::ControllerInput>()
            .insert_resource(Chat { live: true, ..default() })
            .init_resource::<crate::online_menu::OnlineUi>()
            .add_systems(Update, keys);
        let bar = |app: &App| app.world().resource::<Chat>().bar.as_ref().map(|b| b.text.clone());
        press(&mut app, KeyCode::Enter, None);
        press(&mut app, KeyCode::KeyH, Some("hel"));
        app.world_mut().resource_mut::<crate::online_menu::OnlineUi>().modal = true;
        app.update();
        assert_eq!(bar(&app), None, "C9: the popup closes it");
        app.world_mut().resource_mut::<crate::online_menu::OnlineUi>().modal = false;
        press(&mut app, KeyCode::Enter, None);
        assert_eq!(bar(&app).as_deref(), Some("hel"));
        // A session move (the session drops for a moment) keeps it too.
        {
            let mut chat = app.world_mut().resource_mut::<Chat>();
            chat.reset();
            chat.live = true;
        }
        press(&mut app, KeyCode::Enter, None);
        assert_eq!(bar(&app).as_deref(), Some("hel"));
        press(&mut app, KeyCode::Escape, None);
        press(&mut app, KeyCode::Enter, None);
        assert_eq!(bar(&app).as_deref(), Some(""), "Escape threw it away");
    }

    #[test]
    fn one_sender_brings_in_only_so_many_rare_characters() {
        let mut seen = BTreeSet::new();
        let flood: String = (0..RARE_EACH as u32 + 10).filter_map(|i| char::from_u32(0x4e00 + i)).collect();
        let kept = budget(&mut seen, &format!("hi é {flood}"));
        assert!(kept.starts_with("hi é ") && kept.ends_with("??????????") && seen.len() == RARE_EACH);
        assert_eq!(budget(&mut seen, "\u{4e00} ok"), "\u{4e00} ok", "the ones already in still show");
    }

    #[test]
    fn muted_senders_and_non_friends_stay_out_of_the_feed() {
        use crate::player_list::Shield;
        let members = vec![
            skate_net::roster::Member { actor: 1, name: "Ann".into(), steam: 5, host: true },
            skate_net::roster::Member { actor: 2, name: "ann".into(), steam: 6, host: false },
        ];
        let mut shield = Shield::default();
        shield.judge(&members, 99, &[skate_net::presence::Friend { id: 5, name: "Ann".into(), online: true, connect: None }]);
        assert!(shield.reads(1, true, true) && shield.reads(2, true, false));
        assert!(!shield.reads(2, true, true) && !shield.reads(1, false, false), "Chat From: Friends; Text Chat off");
        let person = crate::player_list::Person { name: "ann".into(), steam: 6, actor: Some(2), host: false, friend: false, online: true, connect: None, about: Vec::new() };
        shield.pick(crate::player_list::PlayerAction::Mute, &person);
        shield.judge(&members, 99, &[]);
        assert!(!shield.reads(2, true, false), "muted");
        assert_eq!((name_of(&members, 1), name_of(&members, 2)), ("Ann".into(), "ann #2".into()), "a copied name is numbered");
    }
}
