//! Not retail (user 10-25): the server browser, under Online Challenge Map. Every public session on this build (Steam
//! lists no private lobby; the relay drops hidden ones and those whose host picked Hide from Server Browser:
//! directory::listed, UNLISTED_KEY) in the retail Player List (`online/friends`, player_list); a row is the session's
//! name (else its map), mode, players and Steam's ping estimate to its host, our own first and marked. A opens a
//! session's details: Join (as a friend's Join Game: online_session::join_requests; greyed on our own) over its map
//! and description, greyed.
use crate::{multiplayer::Multiplayer, player_list};
use skate_net::directory::{PAGE_SIZE, Row};

/// The crossbar row (under Online Challenge Map) and the list's title; no retail string fits.
pub(crate) const TITLE: &str = "#Server Browser";
/// The list's one row when no session is listed.
pub(crate) const NONE: &str = "#No public sessions found";
/// Our own session's mark (host or guest: the session we are in).
const OURS: &str = "Your Session";

/// A browse's pages: as many as the relay asks Steam for (BROWSE_MAX, user 10-27: 100).
const MAX_PAGES: usize = skate_net::directory::BROWSE_MAX.div_ceil(PAGE_SIZE);
/// An empty browser looks again this often (a session hosted meanwhile shows up). A knob.
const EMPTY_AGAIN: f32 = 10.;
/// One more look this long after an answer whose pings Steam had not measured yet (seconds after the relay starts). A
/// knob.
const PING_AGAIN: f32 = 5.;
/// A browser listing sessions looks again this often: a session that closed goes, one hosted since comes. A knob.
const LIST_AGAIN: f32 = 15.;

/// Not retail (user 10-27): the order after our own session; LB / RB step through them, round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Sort {
    #[default]
    LowestPing,
    HighestPing,
    FewestPlayers,
    MostPlayers,
}
impl Sort {
    const ALL: [Sort; 4] = [Sort::LowestPing, Sort::HighestPing, Sort::FewestPlayers, Sort::MostPlayers];
    /// The next sort (RB), or the one before (LB).
    pub fn step(self, forward: bool) -> Self {
        let at = Self::ALL.iter().position(|&s| s == self).unwrap_or(0);
        Self::ALL[(at + if forward { 1 } else { Self::ALL.len() - 1 }) % Self::ALL.len()]
    }
    fn words(self) -> &'static str {
        match self {
            Sort::LowestPing => "Lowest Ping",
            Sort::HighestPing => "Highest Ping",
            Sort::FewestPlayers => "Fewest Players",
            Sort::MostPlayers => "Most Players",
        }
    }
    /// A session before `b`: a ping Steam has not measured goes last either way; ties go to the fuller (by ping),
    /// then the lower ping.
    fn order(self, a: &Row, b: &Row) -> std::cmp::Ordering {
        let ping = |r: &Row| r.ping.unwrap_or(u32::MAX);
        let fuller = b.players.cmp(&a.players);
        match self {
            Sort::LowestPing => ping(a).cmp(&ping(b)).then(fuller),
            Sort::HighestPing => (a.ping.is_none(), std::cmp::Reverse(a.ping)).cmp(&(b.ping.is_none(), std::cmp::Reverse(b.ping))).then(fuller),
            Sort::FewestPlayers => a.players.cmp(&b.players).then(ping(a).cmp(&ping(b))),
            Sort::MostPlayers => fuller.then(ping(a).cmp(&ping(b))),
        }
    }
}

#[derive(Default)]
pub(crate) struct Browser {
    /// The list's order (kept while the game runs).
    pub sort: Sort,
    /// Y's words: only sessions whose name or map holds them (case aside) show; cleared when the browser opens.
    pub search: String,
    /// Every page of the last browse (`Multiplayer::browser_rows` holds one at a time).
    rows: Vec<Row>,
    /// The pages of the browse under way, read so far; they replace `rows` once all are in.
    fresh: Vec<Row>,
    /// The page that browse waits for.
    awaiting: Option<usize>,
    /// `Multiplayer::browser_answers` read so far.
    seen: u64,
    /// When the last browse started.
    at: f32,
    /// The look again for pings is spent.
    pinged: bool,
    /// Why the last browse failed (Steam's words), shown while there is no session to list.
    error: Option<String>,
}
impl Browser {
    /// The browser opened: a fresh look from the first page.
    pub fn open(&mut self, net: &mut Multiplayer, now: f32) {
        *self = Self { seen: net.browser_answers, sort: self.sort, ..Self::default() };
        self.browse(net, now);
    }
    /// X (user 10-27): look again now, unless a look is under way.
    pub fn refresh(&mut self, net: &mut Multiplayer, now: f32) {
        if self.awaiting.is_none() {
            self.browse(net, now);
        }
    }
    fn browse(&mut self, net: &mut Multiplayer, now: f32) {
        self.at = now;
        self.fresh.clear();
        self.awaiting = net.browse(0).then_some(0);
        if self.awaiting.is_none() {
            self.error = Some(net.browser_status.clone());
        }
    }
    /// Each frame while open: reads an answered page and asks for the next, and looks again when due.
    pub fn step(&mut self, net: &mut Multiplayer, now: f32) {
        if net.browser_answers != self.seen {
            self.seen = net.browser_answers;
            if self.awaiting.is_some() {
                let next = if net.browser_error {
                    self.error = Some(net.browser_status.clone());
                    None
                } else {
                    self.error = None;
                    self.read(net.browser_page, net.browser_total, &net.browser_rows)
                };
                self.awaiting = match next {
                    Some(page) if net.browse(page) => Some(page),
                    _ => None,
                };
                // Done (or stopped): what was read replaces the list, unless a failed first page read nothing.
                if self.awaiting.is_none() && !(self.fresh.is_empty() && net.browser_error) {
                    self.finish();
                }
            }
        }
        if self.awaiting.is_none() {
            let again = if self.rows.is_empty() {
                EMPTY_AGAIN
            } else if !self.pinged && self.rows.iter().any(|r| r.ping.is_none()) {
                PING_AGAIN
            } else {
                LIST_AGAIN
            };
            if now - self.at >= again {
                self.pinged |= !self.rows.is_empty();
                self.browse(net, now);
            }
        }
    }
    /// One answered page of `total` lobbies: its rows join the browse's (a lobby that moved up a page is not read
    /// twice). The next page to ask for, if any.
    fn read(&mut self, page: usize, total: usize, rows: &[Row]) -> Option<usize> {
        for row in rows {
            if !self.fresh.iter().any(|r| r.id == row.id) {
                self.fresh.push(row.clone());
            }
        }
        let next = page + 1;
        (next * PAGE_SIZE < total && next < MAX_PAGES).then_some(next)
    }
    /// A browse done: its rows replace the list's unless they show the same sessions as they are (a ping a few ms
    /// off aside), so a look again moves neither the list nor its cursor.
    fn finish(&mut self) {
        let shown = |r: &Row| (r.id, r.players, r.capacity, r.map.clone(), r.mode.clone(), r.name.clone(), r.description.clone(), r.ping.is_some());
        let fresh = std::mem::take(&mut self.fresh);
        if !self.rows.iter().map(shown).eq(fresh.iter().map(shown)) {
            self.rows = fresh;
        }
    }
    /// The list's rows: a header with the sort (and the search), then our own session (`room`), then the rest in the
    /// sort's order, those the search leaves out gone; with none at all, one header saying why. `text` looks a retail
    /// string up (None: missing). A host's own words are filtered as chat is (Settings > Audio).
    pub fn rows(&self, room: Option<u64>, text: impl Fn(&str) -> Option<String>) -> Vec<player_list::Row> {
        let header = |label: String| player_list::Row { label, header: true, person: None };
        if self.rows.is_empty() {
            let why = match (&self.error, self.awaiting) {
                (_, Some(_)) => "#Searching for sessions...".to_owned(),
                (Some(error), None) => crate::apt_text::literal(error),
                (None, None) => NONE.to_owned(),
            };
            return vec![header(why)];
        }
        let swears = player_list::CHAT_FILTER.load(std::sync::atomic::Ordering::Relaxed);
        let own_words = |words: &str| crate::chat_filter::filter(words.trim(), swears);
        let search = self.search.trim().to_lowercase();
        let found = |r: &Row| {
            search.is_empty() || Some(r.id) == room || [own_words(&r.name), r.map.replace('_', " ")].iter().any(|w| w.to_lowercase().contains(&search))
        };
        let mut sessions: Vec<&Row> = self.rows.iter().filter(|r| found(r)).collect();
        sessions.sort_by(|a, b| (Some(a.id) != room).cmp(&(Some(b.id) != room)).then_with(|| self.sort.order(a, b)));
        let mut state = format!("Sort: {}", self.sort.words());
        if !search.is_empty() {
            state += &format!(" - Search: \"{}\"", self.search.trim());
        }
        // LB / RB step the sort: their buttons lead its row (the legend has room for four).
        let sort = crate::apt_text::glyph_first("R_Bumper", &crate::apt_text::literal(&state));
        let mut out = vec![header(crate::apt_text::glyph_first("L_Bumper", &sort))];
        if sessions.is_empty() {
            out.push(header(crate::apt_text::literal(&format!("No sessions match \"{}\"", self.search.trim()))));
        }
        out.extend((sessions.into_iter())
            .map(|r| {
                let ours = Some(r.id) == room;
                let map = r.map.replace('_', " ");
                let title = Some(own_words(&r.name)).filter(|n| !n.is_empty()).unwrap_or_else(|| map.clone());
                let mut words = vec![title.clone()];
                words.extend(mode(&r.mode, &text));
                words.push(format!("{}/{}", r.players, r.capacity));
                // Our own: no ping to ourselves, and the mark.
                if ours {
                    words.push(OURS.into());
                } else {
                    words.extend(r.ping.map(|ms| format!("{ms} ms")));
                }
                // The details under Join: the map (when a name leads the row), then the description.
                let mut about: Vec<String> = (title != map).then_some(map).into_iter().collect();
                about.extend(crate::session_details::wrap(&own_words(&r.description), crate::session_details::LINE));
                let person = player_list::Person {
                    name: title,
                    steam: 0,
                    actor: None,
                    host: false,
                    friend: false,
                    online: true,
                    // We are in our own.
                    connect: (!ours).then(|| skate_net::presence::connect_string(r.id, &r.map)),
                    about,
                };
                player_list::Row { label: crate::apt_text::literal(&words.join(" - ")), header: false, person: Some(person) }
            }));
        out
    }
}

#[cfg(test)]
impl Browser {
    /// Tests: a browser whose last browse read `rows`.
    pub(crate) fn with_rows(rows: Vec<Row>) -> Self {
        Self { rows, ..Self::default() }
    }
}

/// A lobby's mode (the relay's: its challenge-type filter, else its Online Challenge Map tab) in the game's words.
fn mode(mode: &str, text: &impl Fn(&str) -> Option<String>) -> Option<String> {
    match mode {
        filter if filter.starts_with("ID_MAP_FILTER_") => text(filter),
        // Every location is freeskate, the Skate 2 / Skate 1 tabs' too (online_session::intent).
        "freeskate" | "skate2" | "skate1" => text("ID_FREESKATE_LOBBY"),
        "team" | "career" | "versus" => text("ID_FREESKATE_CHALLENGE_LOBBY"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(id: u64, players: usize, ping: Option<u32>, mode: &str) -> Row {
        Row {
            id,
            map: "New_San_Vanelona".into(),
            players,
            capacity: 6,
            compatible: true,
            ping,
            mode: mode.into(),
            name: String::new(),
            description: String::new(),
        }
    }
    fn text(id: &str) -> Option<String> {
        match id {
            "ID_FREESKATE_LOBBY" => Some("Freeskate".into()),
            "ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE" => Some("Spot Battle".into()),
            _ => None,
        }
    }
    /// Labels with apt_text::literal's mark taken off.
    fn labels(rows: &[player_list::Row]) -> Vec<&str> {
        rows.iter().map(|r| r.label.strip_prefix("#\u{1}").unwrap_or(&r.label)).collect()
    }
    #[test]
    fn every_page_is_read_once_then_listed() {
        let mut b = Browser::default();
        let page = |ids: std::ops::Range<u64>| ids.map(|id| row(id, 1, Some(40), "freeskate")).collect::<Vec<_>>();
        assert_eq!(b.read(0, 12, &page(1..6)), Some(1));
        // A lobby gone between the pages: the second starts one later, so 6 comes again.
        assert_eq!(b.read(1, 11, &page(6..11)), Some(2));
        assert_eq!(b.read(2, 11, &page(11..12)), None, "the last page");
        assert_eq!(b.fresh.iter().map(|r| r.id).collect::<Vec<_>>(), (1..12).collect::<Vec<_>>());
        let mut many = Browser::default();
        assert_eq!(many.read(MAX_PAGES - 1, 1000, &page(1..6)), None, "Steam's page cap");
    }
    #[test]
    fn rows_name_map_mode_players_and_ping_ours_first_then_lowest_ping() {
        let b = Browser {
            rows: vec![row(1, 2, None, "freeskate"), row(2, 3, Some(80), "ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE"), row(3, 5, Some(30), "x"), row(4, 1, Some(30), "")],
            ..Browser::default()
        };
        let rows = b.rows(Some(4), text);
        assert_eq!(labels(&rows), [
            "<B:L_Bumper> <B:R_Bumper> Sort: Lowest Ping",
            "New San Vanelona - 1/6 - Your Session",
            "New San Vanelona - 5/6 - 30 ms",
            "New San Vanelona - Spot Battle - 3/6 - 80 ms",
            "New San Vanelona - Freeskate - 2/6",
        ], "the sort (LB / RB on its row: the legend has no room for a fifth button), then our own session (4) first, without a ping; an unknown mode reads as none");
        assert_eq!(rows[1].person.as_ref().map(|p| p.connect.is_none()), Some(true), "no joining the session we are in");
        assert_eq!(rows[2].person.as_ref().and_then(|p| p.connect.as_deref()), Some("sk8:3:New_San_Vanelona"));
        assert!(rows[0].header && rows[1..].iter().all(|r| !r.header));
    }
    /// User 10-27: LB / RB step round four sorts; ours stays first; a ping Steam hasn't measured goes last either way.
    #[test]
    fn sorts_order_the_rest_after_our_own_session() {
        let mut b = Browser { rows: vec![row(1, 2, None, ""), row(2, 5, Some(80), ""), row(3, 1, Some(30), ""), row(4, 3, Some(50), "")], ..Browser::default() };
        let ids = |b: &Browser| b.rows(Some(4), text)[1..].iter().map(|r| r.label.clone()).collect::<Vec<_>>();
        let order = |b: &Browser| {
            let by: Vec<String> = ids(b);
            by.iter().map(|l| ["5/6", "1/6", "2/6", "3/6"].iter().position(|p| l.contains(p)).unwrap()).collect::<Vec<_>>()
        };
        // Positions in ["5/6" (id 2), "1/6" (id 3), "2/6" (id 1), "3/6" (id 4, ours)].
        assert_eq!(order(&b), [3, 1, 0, 2], "Lowest Ping: ours, 30, 80, unmeasured");
        b.sort = b.sort.step(true);
        assert_eq!((b.sort, order(&b)), (Sort::HighestPing, vec![3, 0, 1, 2]), "ours, 80, 30, unmeasured");
        b.sort = b.sort.step(true);
        assert_eq!((b.sort, order(&b)), (Sort::FewestPlayers, vec![3, 1, 2, 0]));
        b.sort = b.sort.step(true);
        assert_eq!((b.sort, order(&b)), (Sort::MostPlayers, vec![3, 0, 2, 1]));
        assert_eq!(b.sort.step(true), Sort::LowestPing, "round");
        assert_eq!(Sort::LowestPing.step(false), Sort::MostPlayers, "LB goes back round");
        assert_eq!(labels(&b.rows(Some(4), text))[0], "<B:L_Bumper> <B:R_Bumper> Sort: Most Players");
    }
    /// User 10-27: Y's words keep the sessions whose name or map holds them, case aside, and our own; none says so.
    #[test]
    fn a_search_keeps_matching_names_and_our_own() {
        let park = Row { name: "Ryan's Park".into(), ..row(1, 2, Some(40), "") };
        let other = Row { map: "University".into(), ..row(2, 2, Some(50), "") };
        let mut b = Browser { rows: vec![park, other, row(3, 1, None, "")], search: " PARK ".into(), ..Browser::default() };
        assert_eq!(labels(&b.rows(Some(3), text)), ["<B:L_Bumper> <B:R_Bumper> Sort: Lowest Ping - Search: \"PARK\"", "New San Vanelona - 1/6 - Your Session", "Ryan's Park - 2/6 - 40 ms"]);
        b.search = "univ".into();
        assert_eq!(labels(&b.rows(None, text))[1..], ["University - 2/6 - 50 ms"], "a map matches too");
        b.search = "nowhere".into();
        assert_eq!(labels(&b.rows(None, text)), ["<B:L_Bumper> <B:R_Bumper> Sort: Lowest Ping - Search: \"nowhere\"", "No sessions match \"nowhere\""]);
        b.search = "<B:A>".into();
        assert!(labels(&b.rows(None, text))[0].ends_with("Search: \"<\u{2060}B:A>\""), "typed words never draw a button");
    }
    #[test]
    fn a_named_session_leads_with_its_name_and_details_its_map_and_description() {
        let named = Row { name: " Ryan's Park ".into(), description: "Chill freeskate at the park, all welcome".into(), ..row(7, 2, Some(55), "freeskate") };
        let b = Browser { rows: vec![named, row(8, 1, Some(60), "")], ..Browser::default() };
        let rows = b.rows(None, text);
        assert_eq!(labels(&rows)[1..], ["Ryan's Park - Freeskate - 2/6 - 55 ms", "New San Vanelona - 1/6 - 60 ms"]);
        let person = rows[1].person.as_ref().unwrap();
        assert_eq!(person.name, "Ryan's Park", "the details screen's title");
        assert_eq!(person.about, ["New San Vanelona", "Chill freeskate at the park,", "all welcome"]);
        assert!(rows[2].person.as_ref().unwrap().about.is_empty(), "the map already leads the row");
    }
    #[test]
    fn a_look_again_keeps_the_list_unless_a_session_changed() {
        let mut b = Browser { rows: vec![row(1, 2, Some(40), "freeskate")], ..Browser::default() };
        b.fresh = vec![row(1, 2, Some(43), "freeskate")];
        b.finish();
        assert_eq!(b.rows[0].ping, Some(40), "a ping a few ms off moves nothing");
        b.fresh = vec![row(1, 3, Some(43), "freeskate")];
        b.finish();
        assert_eq!(b.rows[0].players, 3, "a player joined");
        b.fresh = vec![row(1, 3, Some(43), "freeskate"), row(2, 1, None, "")];
        b.finish();
        assert_eq!(b.rows.len(), 2, "a session hosted since");
        b.fresh = vec![];
        b.finish();
        assert!(b.rows.is_empty(), "every session closed");
    }
    #[test]
    fn an_empty_browser_says_why() {
        let mut b = Browser { awaiting: Some(0), ..Browser::default() };
        assert_eq!(labels(&b.rows(None, text)), ["#Searching for sessions..."]);
        b.awaiting = None;
        assert_eq!(labels(&b.rows(None, text)), ["#No public sessions found"]);
        b.error = Some("Steam request timed out".into());
        assert_eq!(labels(&b.rows(None, text)), ["Steam request timed out"]);
        assert!(b.rows(None, text)[0].header);
    }
}
