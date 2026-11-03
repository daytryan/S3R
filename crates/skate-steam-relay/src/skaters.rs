//! Friends' Skaters on Steam (decision Q2a): each player's boss look is ONE Steam Workshop item of app 480 with
//! the tag [`TAG`], friends-only visibility, no content or preview, and the skater as the item's metadata (returned
//! by queries, so nothing is downloaded and the publisher can be offline). Friends read it with
//! CreateQueryUserUGCRequest(friend, Published) + SetReturnMetadata.
//!
//! Sans-IO: [`Skaters`] decides, the `--presence` loop (presence.rs) runs its [`Cmd`]s on Steam and feeds the
//! results back as [`Event`]s, so the tests drive it with a mocked Steam.
use skate_net::presence::{MAX_ALL_FRIENDS, MAX_NAME, MAX_SKATER, Report, SKATER_PAGE};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

pub const TAG: &str = "s3r-skater";
/// A friend lookup that has not answered by then reports "None" (a late answer still lands).
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(15);
/// A publish step (lookup, create, update) that has not answered by then is retried from a fresh lookup.
pub const PUBLISH_TIMEOUT: Duration = Duration::from_secs(60);
/// After a failed publish step.
pub const RETRY: Duration = Duration::from_secs(30);
/// A friend's answer is reused this long.
pub const CACHE: Duration = Duration::from_secs(60);
/// Friend lookups in flight at once.
pub const IN_FLIGHT: usize = 4;

/// One of a user's published Workshop items, as a query returned it.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: u64,
    pub owner: u64,
    pub tagged: bool,
    pub metadata: Option<String>,
}
#[derive(Debug, PartialEq)]
pub enum Cmd {
    /// CreateQueryUserUGCRequest(user, Published, tag) with metadata.
    Query(u64),
    Create,
    Update { item: u64, metadata: String },
    Delete(u64),
}
pub enum Event {
    Queried(u64, Result<Vec<Item>, String>),
    /// (item, needs the Workshop legal agreement).
    Created(Result<(u64, bool), String>),
    /// Needs the Workshop legal agreement.
    Updated(Result<bool, String>),
}

#[derive(PartialEq)]
enum Step {
    Lookup,
    Create,
    Update(String),
}

pub struct Skaters {
    own: u64,
    /// Our item, once looked up or created.
    item: Option<u64>,
    looked_up: bool,
    /// What Steam holds on our item.
    published: Option<String>,
    want: Option<String>,
    busy: Option<(Instant, Step)>,
    retry_at: Option<Instant>,
    agreement: bool,
    queue: VecDeque<u64>,
    asked: HashMap<u64, Instant>,
    cache: HashMap<u64, (Instant, Option<String>)>,
    pub cmds: Vec<Cmd>,
    pub reports: Vec<Report>,
}

/// Friends' Skaters' friend list in report pages: every friend, names without control characters, capped.
pub fn friend_pages(friends: impl IntoIterator<Item = (u64, String)>) -> Vec<Report> {
    let rows: Vec<(u64, String)> = friends
        .into_iter()
        .take(MAX_ALL_FRIENDS)
        .map(|(id, name)| {
            let clean: String = name.chars().filter(|c| !c.is_control()).collect();
            (id, skate_net::roster::truncate(&clean, MAX_NAME).to_owned())
        })
        .collect();
    let total = rows.len() as u32;
    if rows.is_empty() {
        return vec![Report::SkaterFriends { first: 0, total, friends: vec![] }];
    }
    rows.chunks(SKATER_PAGE)
        .enumerate()
        .map(|(i, page)| Report::SkaterFriends {
            first: (i * SKATER_PAGE) as u32,
            total,
            friends: page.to_vec(),
        })
        .collect()
}

impl Skaters {
    pub fn new(own: u64) -> Self {
        Self {
            own,
            item: None,
            looked_up: false,
            published: None,
            want: None,
            busy: None,
            retry_at: None,
            agreement: false,
            queue: VecDeque::new(),
            asked: HashMap::new(),
            cache: HashMap::new(),
            cmds: Vec::new(),
            reports: Vec::new(),
        }
    }

    pub fn publish(&mut self, metadata: String) {
        if metadata.len() <= MAX_SKATER && !metadata.contains('\0') {
            self.want = Some(metadata);
        }
    }

    /// The list opened: cached friends answer now, the rest are looked up.
    pub fn list(&mut self, friends: &[u64], now: Instant) {
        for &friend in friends.iter().take(MAX_ALL_FRIENDS) {
            self.lookup(friend, now, false);
        }
    }
    /// The player is looking at this friend: answer from the cache or look them up first.
    pub fn fetch(&mut self, friend: u64, now: Instant) {
        self.lookup(friend, now, true);
    }
    fn lookup(&mut self, friend: u64, now: Instant, first: bool) {
        if friend == 0 || friend == self.own {
            return;
        }
        if let Some((at, skater)) = self.cache.get(&friend) {
            if now.duration_since(*at) < CACHE {
                self.reports.push(Report::Skater { friend, skater: skater.clone() });
                return;
            }
        }
        if self.asked.contains_key(&friend) {
            return;
        }
        if let Some(i) = self.queue.iter().position(|&f| f == friend) {
            if !first {
                return;
            }
            self.queue.remove(i);
        }
        if self.queue.len() >= MAX_ALL_FRIENDS {
            return;
        }
        if first {
            self.queue.push_front(friend);
        } else {
            self.queue.push_back(friend);
        }
    }

    pub fn event(&mut self, event: Event, now: Instant) {
        match event {
            Event::Queried(user, result) if user == self.own => {
                if !matches!(self.busy, Some((_, Step::Lookup))) {
                    return;
                }
                self.busy = None;
                match result {
                    Ok(items) => {
                        // One item per user: keep the first created, delete any extra.
                        let mut mine: Vec<Item> = items.into_iter().filter(|i| i.tagged && i.owner == self.own && i.id != 0).collect();
                        mine.sort_by_key(|i| i.id);
                        for extra in mine.iter().skip(1) {
                            self.cmds.push(Cmd::Delete(extra.id));
                        }
                        self.item = mine.first().map(|i| i.id);
                        self.published = mine.first().and_then(|i| i.metadata.clone());
                        self.looked_up = true;
                    }
                    Err(_) => self.retry_at = Some(now + RETRY),
                }
            }
            Event::Queried(friend, result) => {
                self.asked.remove(&friend);
                // ponytail: one result page (50) per friend; one item per user, so the first tagged one is it.
                let skater = result.ok().and_then(|items| {
                    let mut theirs: Vec<Item> = items.into_iter().filter(|i| i.tagged && i.owner == friend).collect();
                    theirs.sort_by_key(|i| i.id);
                    theirs.into_iter().next()?.metadata.filter(|m| m.len() <= MAX_SKATER)
                });
                self.cache.insert(friend, (now, skater.clone()));
                self.reports.push(Report::Skater { friend, skater });
            }
            Event::Created(result) => {
                if matches!(self.busy, Some((_, Step::Create))) {
                    self.busy = None;
                }
                match result {
                    Ok((id, agreement)) if id != 0 => {
                        self.item = Some(id);
                        self.looked_up = true;
                        self.agreement |= agreement;
                    }
                    other => {
                        self.failed(other.err().unwrap_or_default());
                        // Maybe made anyway: look again before creating another.
                        self.looked_up = false;
                        self.retry_at = Some(now + RETRY);
                    }
                }
            }
            Event::Updated(result) => {
                let Some((_, Step::Update(sent))) = self.busy.take() else { return };
                match result {
                    Ok(agreement) => {
                        self.published = Some(sent);
                        self.agreement |= agreement;
                        self.reports.push(Report::SkaterPublished { needs_agreement: self.agreement });
                    }
                    Err(e) => {
                        self.failed(e);
                        // Deleted on the web, or Steam refused: start over from a lookup.
                        self.item = None;
                        self.looked_up = false;
                        self.retry_at = Some(now + RETRY);
                    }
                }
            }
        }
    }

    fn failed(&mut self, error: String) {
        let text = format!("Could not publish your skater: {error}");
        self.reports.push(Report::Error(skate_net::roster::truncate(&text, 500).to_owned()));
    }

    /// Timeouts, then the next Steam calls.
    pub fn step(&mut self, now: Instant) {
        let late: Vec<u64> = self.asked.iter().filter(|(_, at)| now.duration_since(**at) >= QUERY_TIMEOUT).map(|(f, _)| *f).collect();
        for friend in late {
            self.asked.remove(&friend);
            self.reports.push(Report::Skater { friend, skater: None });
        }
        while self.asked.len() < IN_FLIGHT {
            let Some(friend) = self.queue.pop_front() else { break };
            self.asked.insert(friend, now);
            self.cmds.push(Cmd::Query(friend));
        }
        if self.busy.as_ref().is_some_and(|(at, _)| now.duration_since(*at) >= PUBLISH_TIMEOUT) {
            // A create may still land: look before creating again.
            self.busy = None;
            self.looked_up = false;
        }
        if self.busy.is_some() || self.retry_at.is_some_and(|at| now < at) {
            return;
        }
        let Some(want) = self.want.clone() else { return };
        let step = if !self.looked_up {
            self.cmds.push(Cmd::Query(self.own));
            Step::Lookup
        } else if self.published.as_ref() == Some(&want) {
            return;
        } else if let Some(item) = self.item {
            self.cmds.push(Cmd::Update { item, metadata: want.clone() });
            Step::Update(want)
        } else {
            self.cmds.push(Cmd::Create);
            Step::Create
        };
        self.busy = Some((now, step));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ME: u64 = 76561198000000001;
    fn mine(id: u64, metadata: &str) -> Item {
        Item { id, owner: ME, tagged: true, metadata: Some(metadata.into()) }
    }
    fn take(s: &mut Skaters) -> Vec<Cmd> {
        std::mem::take(&mut s.cmds)
    }

    #[test]
    fn first_publish_looks_up_then_creates_then_updates_once() {
        let (mut s, t) = (Skaters::new(ME), Instant::now());
        s.publish("look-a".into());
        s.step(t);
        assert_eq!(take(&mut s), [Cmd::Query(ME)]);
        s.event(Event::Queried(ME, Ok(vec![])), t);
        s.step(t);
        assert_eq!(take(&mut s), [Cmd::Create]);
        s.event(Event::Created(Ok((55, true))), t);
        s.step(t);
        assert_eq!(take(&mut s), [Cmd::Update { item: 55, metadata: "look-a".into() }]);
        s.step(t);
        assert!(take(&mut s).is_empty(), "one step at a time");
        s.event(Event::Updated(Ok(false)), t);
        assert_eq!(s.reports.pop(), Some(Report::SkaterPublished { needs_agreement: true }));
        // The same look again (a game restart, an unchanged file): nothing goes to Steam.
        s.publish("look-a".into());
        s.step(t);
        assert!(take(&mut s).is_empty(), "no duplicate publish");
        // A new look updates the same item; it never creates a second one.
        s.publish("look-b".into());
        s.step(t);
        assert_eq!(take(&mut s), [Cmd::Update { item: 55, metadata: "look-b".into() }]);
    }

    #[test]
    fn an_existing_item_is_reused_and_extras_deleted() {
        let (mut s, t) = (Skaters::new(ME), Instant::now());
        s.publish("same".into());
        s.step(t);
        take(&mut s);
        let foreign = Item { id: 1, owner: 9, tagged: true, metadata: None };
        let untagged = Item { id: 2, owner: ME, tagged: false, metadata: None };
        s.event(Event::Queried(ME, Ok(vec![mine(40, "same"), foreign, untagged, mine(30, "same")])), t);
        assert_eq!(take(&mut s), [Cmd::Delete(40)], "the oldest (30) stays");
        s.step(t);
        assert!(take(&mut s).is_empty(), "Steam already holds this look");
    }

    #[test]
    fn publish_timeouts_and_failures_never_create_twice() {
        let (mut s, t) = (Skaters::new(ME), Instant::now());
        s.publish("x".into());
        s.step(t);
        s.event(Event::Queried(ME, Ok(vec![])), t);
        s.step(t);
        assert_eq!(take(&mut s), [Cmd::Query(ME), Cmd::Create]);
        // Create never answers: after the timeout we look again (it may have landed) rather than create.
        let later = t + PUBLISH_TIMEOUT;
        s.step(later);
        assert_eq!(take(&mut s), [Cmd::Query(ME)]);
        s.event(Event::Queried(ME, Ok(vec![mine(77, "old")])), later);
        s.step(later);
        assert_eq!(take(&mut s), [Cmd::Update { item: 77, metadata: "x".into() }]);
        // Update refused (item deleted on the web): wait, then start over from a lookup.
        s.event(Event::Updated(Err("FileNotFound".into())), later);
        s.step(later + Duration::from_secs(1));
        assert!(take(&mut s).is_empty(), "backs off");
        s.step(later + RETRY);
        assert_eq!(take(&mut s), [Cmd::Query(ME)]);
        // Oversized or NUL looks are never sent to Steam (its setter would refuse and the crate would panic).
        let mut s = Skaters::new(ME);
        s.publish("x".repeat(MAX_SKATER + 1));
        s.publish("a\0b".into());
        s.step(t);
        assert!(take(&mut s).is_empty());
    }

    #[test]
    fn friends_are_looked_up_four_at_a_time_cached_and_timed_out() {
        let (mut s, t) = (Skaters::new(ME), Instant::now());
        let friends: Vec<u64> = (1..=6).collect();
        s.list(&friends, t);
        s.fetch(6, t);
        s.step(t);
        assert_eq!(take(&mut s), [Cmd::Query(6), Cmd::Query(1), Cmd::Query(2), Cmd::Query(3)], "the hilighted one first");
        // Friend 6 (offline or not, it makes no difference to Steam) published; friend 1 has nothing.
        s.event(Event::Queried(6, Ok(vec![Item { id: 9, owner: 6, tagged: true, metadata: Some("look".into()) }])), t);
        s.event(Event::Queried(1, Ok(vec![Item { id: 8, owner: 1, tagged: false, metadata: Some("not ours".into()) }])), t);
        assert_eq!(s.reports, [Report::Skater { friend: 6, skater: Some("look".into()) }, Report::Skater { friend: 1, skater: None }]);
        s.reports.clear();
        s.step(t);
        assert_eq!(take(&mut s), [Cmd::Query(4), Cmd::Query(5)]);
        // 2 and 3 never answer: "None" at the timeout.
        s.step(t + QUERY_TIMEOUT);
        let mut late: Vec<u64> = s.reports.iter().map(|r| match r { Report::Skater { friend, skater: None } => *friend, _ => 0 }).collect();
        late.sort();
        assert_eq!(late, [2, 3, 4, 5]);
        s.reports.clear();
        // Reopened within the cache time: answered at once, no Steam call; later, looked up again.
        s.list(&[6], t + Duration::from_secs(5));
        assert_eq!(s.reports, [Report::Skater { friend: 6, skater: Some("look".into()) }]);
        s.step(t + Duration::from_secs(5));
        assert!(!take(&mut s).contains(&Cmd::Query(6)));
        s.list(&[6], t + CACHE);
        s.step(t + CACHE);
        assert!(take(&mut s).contains(&Cmd::Query(6)));
        // A friend's oversized metadata (cannot come from Steam, but the cap holds) is "None".
        s.event(Event::Queried(6, Ok(vec![Item { id: 9, owner: 6, tagged: true, metadata: Some("x".repeat(MAX_SKATER + 1)) }])), t);
        assert_eq!(s.reports.pop(), Some(Report::Skater { friend: 6, skater: None }));
    }

    #[test]
    fn friend_pages_cover_everyone_and_fit_the_protocol() {
        let names = ["Zoë", "Влад", "スケーター", "🛹 Shredder", "tab\there"];
        let pages = friend_pages((0..250u64).map(|i| (i + 1, names[i as usize % names.len()].to_string())));
        assert_eq!(pages.len(), 3);
        let mut all = Vec::new();
        for page in &pages {
            let text = skate_net::directory::encode(page);
            let Some(Report::SkaterFriends { first, total, friends }) = skate_net::presence::report(&text) else { panic!("page refused") };
            assert_eq!((first as usize, total), (all.len(), 250));
            all.extend(friends);
        }
        assert_eq!(all.len(), 250, "offline friends included: nothing filters by state");
        assert_eq!(all[1].1, "Влад");
        assert_eq!(all[3].1, "🛹 Shredder");
        assert_eq!(all[4].1, "tabhere", "control characters dropped");
        assert_eq!(friend_pages(Vec::new()), [Report::SkaterFriends { first: 0, total: 0, friends: vec![] }]);
    }
}
