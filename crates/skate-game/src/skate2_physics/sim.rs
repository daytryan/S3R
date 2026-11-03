//! The Skate 2 sim thread and the `Skate2Physics` resource that follows it (Task 14, Amendments A and F). Not tested in
//! play.
//!
//! One program (Amendment A): the port's session lives on a thread of the game's, `skate2-physics`, which runs the
//! host binary's message loop (`host/src/main.rs`'s `serve`, Task 3) over `std::sync::mpsc` channels, with the
//! protocol's messages as values. The game's end is a `Link`.
//!
//! How it cannot freeze the game:
//! - The game's update never waits more than `TIMEOUT` (250 ms) for the thread: a tick waits at most that, a boot is
//!   polled without blocking (and given up after `BOOT_TIMEOUT`), and nothing ever joins the thread. The boot runs one
//!   warm-up tick before it answers `Ready` (`Host::warm_up`), so the first tick the game waits for is an ordinary one.
//! - A panic in the port is caught at the top of the thread (`catch_unwind`; the game's profiles unwind) and sent as
//!   `HostMessage::Failed` before the thread ends. A thread that ends in any other way closes its channel, which is
//!   `HostError::Exited`. With the game's own panic hook (`crash_report.rs`, a backtrace and a save drain on the
//!   panicking thread) the `Failed` may come after 250 ms, so a port panic in the shipped game is often seen as a
//!   `Timeout`; the hook still writes the panic's text.
//! - A stuck thread cannot be killed. Its `Link` is dropped, which sets its cancel flag and sends `Shutdown`: a boot
//!   gives up at its next poll (every 5 ms) and the message loop ends before its next message. A thread stuck inside
//!   one port call is abandoned.
//!
//! What it cannot catch: these end the whole game, not the thread: a stack overflow (the thread has 16 MiB, as the
//! port's own sim thread), a panic while a panic unwinds (a port `Drop` that panics), an allocation failure
//! (`handle_alloc_error` aborts) and `std::process::abort`. A panic on one of the port's own worker threads (animation
//! jobs, `career save`, `sk8 stream read`, `sk8 ground decode`) ends only that worker; the sim then stops answering,
//! which is a `Timeout`.
//!
//! Amendment F (the user, 10-21): a failure never turns Skate 2 off by itself. The saved setting is never written here.
//! A timeout, an exit or a failure restarts the sim (`HostState::Restarting`) with the live `Boot` (the switch's, with
//! every later setting sent); while it restarts after driving, the skater stays Skate 2's (`drives_local`), held at
//! `last_good` until the switch (Task 19) places it back there (`replace_due`). Missing data and a `SKATE2_*` variable
//! cannot heal and are not retried. Three failed attempts in a row (a boot or a tick, with no `HEALTHY_DRIVING` of
//! driving between them) stop the retries until the next switch (`start`).
//!
//! One sim at a time: the port's generators (`HOLDRAND`, `global_next`), its save folder and its worker queues are
//! process-wide (Task 3). Every sim thread holds the process's one sim slot for its whole life (`SlotGuard`, released
//! when the thread ends, a panic included), and a `Link` starts only when the slot is free. So there is never more
//! than one sim thread, and at most one abandoned one: while it still holds the slot, `start` and a restart wait
//! without blocking, and a wait of `SLOT_WAIT` counts as one failed attempt.
#![cfg_attr(not(test), allow(dead_code))] // Task 19's switch drives these; until then only the tests do

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use skate2_physics_host::{DataError, Host, unclean_environment};
use skate2_physics_protocol::{Boot, EngineMessage, Frame, HostMessage, PROTOCOL_VERSION, Pad};

/// The longest the game waits for the thread: a tick's frame.
pub const TIMEOUT: Duration = Duration::from_millis(250);
/// The longest a boot may take, its warm-up tick included (the port's boot takes about 2.4 s).
pub const BOOT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a start or a restart waits for the last sim thread to free the slot before that counts as a failed
/// attempt (it goes on waiting after that).
pub const SLOT_WAIT: Duration = Duration::from_secs(10);
/// Driving this long without a failure starts the failure count over (Amendment F).
pub const HEALTHY_DRIVING: Duration = Duration::from_secs(60);
/// Failed attempts in a row that stop the retries.
pub const MAX_FAILURES: u32 = 3;
/// The sim thread's stack: the port's own app gives its sim thread 16 MiB (`live_skating_client.rs` 3564).
const STACK: usize = 16 << 20;

/// How many port sims `Link::spawn_in` started in this process (the test bodies of `spawn_with` are not counted).
static SPAWNED: AtomicUsize = AtomicUsize::new(0);
/// How many port boots ended by their cancel flag (`Host::boot_with`), for the tests.
static CANCELLED_BOOTS: AtomicUsize = AtomicUsize::new(0);
/// The one sim slot: set while a sim thread lives.
static SLOT: AtomicBool = AtomicBool::new(false);

/// How many port sims have been started in this process.
pub fn spawned() -> usize {
    SPAWNED.load(Ordering::SeqCst)
}

/// How many port boots have ended by their cancel flag in this process.
pub fn cancelled_boots() -> usize {
    CANCELLED_BOOTS.load(Ordering::SeqCst)
}

/// Whether no sim thread holds the slot.
pub fn slot_free() -> bool {
    !SLOT.load(Ordering::Acquire)
}

/// The sim slot, held by a sim thread for its whole life; dropping it frees the slot, on a panic too.
struct SlotGuard(());

impl SlotGuard {
    fn take() -> Option<Self> {
        SLOT.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).ok().map(|_| Self(()))
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        SLOT.store(false, Ordering::Release);
    }
}

/// The flag a `Link` sets when it is dropped: the thread's boot and message loop stop at their next poll.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    fn set(&self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Why a tick or a boot got no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// No answer within the budget.
    Timeout,
    /// The thread ended: its channel is closed.
    Exited,
    /// The port failed or panicked; the text says why.
    Failed(String),
    /// The Skate 2 data lacks an entry: `"<archive> <key>"`.
    Missing(String),
}

impl HostError {
    /// The words the notices use for it.
    fn why(&self) -> String {
        match self {
            Self::Timeout => "it did not answer".into(),
            Self::Exited => "it closed".into(),
            Self::Failed(_) => "an error in the Skate 2 code".into(),
            Self::Missing(entry) => format!("missing {entry}"),
        }
    }
}

/// Why `Link::spawn` started nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The game's environment holds this `SKATE2_*` variable, which the port would read (Amendment A).
    Environment(String),
    /// The last sim thread has not ended yet: it still holds the slot.
    Busy,
    /// The system would not start the thread.
    Thread(String),
}

/// The game's end of a sim thread.
pub struct Link {
    tx: Sender<EngineMessage>,
    /// In a `Mutex` only so the resource is `Sync`; it is reached through `&mut self` alone, which never locks.
    rx: Mutex<Receiver<HostMessage>>,
    cancel: Cancel,
}

impl Link {
    /// Starts the port's sim on `data`: refuses, starting nothing, while the game's environment holds a `SKATE2_*`
    /// variable or the last sim thread still holds the slot; else starts the thread `skate2-physics` and sends it
    /// `Hello` and `Boot`. `poll_ready` says when it has booted.
    pub fn spawn(data: &Path, boot: &Boot) -> Result<Self, Refused> {
        // `vars_os`, read lossy: `vars` panics on a name or value that is not Unicode.
        let vars = std::env::vars_os()
            .map(|(name, value)| (name.to_string_lossy().into_owned(), value.to_string_lossy().into_owned()));
        Self::spawn_in(data, boot, vars)
    }

    /// `spawn` with the environment given (the tests never set their own).
    pub fn spawn_in(
        data: &Path,
        boot: &Boot,
        vars: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, Refused> {
        if let Some(name) = unclean_environment(vars) {
            return Err(Refused::Environment(name));
        }
        let data = data.to_owned();
        let mut link = Self::start(move |messages, replies, cancel| serve(&data, &messages, &replies, &cancel))?;
        SPAWNED.fetch_add(1, Ordering::SeqCst);
        // A send fails only once the thread has ended, which `poll_ready` then reports.
        let _ = link.send(EngineMessage::Hello { version: PROTOCOL_VERSION });
        let _ = link.send(EngineMessage::Boot(boot.clone()));
        Ok(link)
    }

    /// The same slot, thread, panic guard, channels and cancel flag as `spawn`, around `body` instead of the port. For
    /// the tests.
    pub fn spawn_with(
        body: impl FnOnce(Receiver<EngineMessage>, Sender<HostMessage>, Cancel) + Send + 'static,
    ) -> Result<Self, Refused> {
        Self::start(body)
    }

    /// A link over channels the caller holds the other ends of, with no thread (and no slot). For the tests.
    pub fn from_channels(tx: Sender<EngineMessage>, rx: Receiver<HostMessage>) -> Self {
        Self { tx, rx: Mutex::new(rx), cancel: Cancel::default() }
    }

    fn start(
        body: impl FnOnce(Receiver<EngineMessage>, Sender<HostMessage>, Cancel) + Send + 'static,
    ) -> Result<Self, Refused> {
        let slot = SlotGuard::take().ok_or(Refused::Busy)?;
        let (tx, messages) = channel();
        let (replies, rx) = channel();
        let cancel = Cancel::default();
        let flag = cancel.clone();
        // If the thread does not start, the closure is dropped with the slot in it, which frees the slot.
        std::thread::Builder::new()
            .name("skate2-physics".into())
            .stack_size(STACK)
            .spawn(move || {
                // Freed when this thread ends, after the panic report below.
                let _slot = slot;
                let report = replies.clone();
                // A panic in the port ends this thread, not the game: the game hears it as `Failed`.
                if let Err(panic) = catch_unwind(AssertUnwindSafe(move || body(messages, replies, flag))) {
                    let _ = report.send(HostMessage::Failed(format!("panic: {}", panic_text(panic.as_ref()))));
                }
            })
            .map_err(|why| Refused::Thread(why.to_string()))?;
        Ok(Self { tx, rx: Mutex::new(rx), cancel })
    }

    /// Whether the sim has booted, without blocking: `None` while it boots, `Some(Ok(boot_ms))` once it is ready,
    /// `Some(Err(..))` when the boot failed (`Missing` names the entry). Log lines are passed to the game's log.
    pub fn poll_ready(&mut self) -> Option<Result<u32, HostError>> {
        loop {
            let message = match self.receiver().try_recv() {
                Ok(message) => message,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => return Some(Err(HostError::Exited)),
            };
            match message {
                HostMessage::Ready { boot_ms } => return Some(Ok(boot_ms)),
                HostMessage::Log(line) => info!("SKATE2 PHYSICS {line}"),
                other => return Some(Err(refusal(other))),
            }
        }
    }

    /// Runs one tick with `pad` held and waits for its frame, at most `TIMEOUT`, the first tick after `Ready`
    /// included. Log lines that come before the frame are passed to the game's log within the same budget.
    pub fn tick(&mut self, pad: Pad) -> Result<Frame, HostError> {
        self.send(EngineMessage::Tick(pad))?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.receiver().recv_timeout(left) {
                Ok(HostMessage::Frame(frame)) => return Ok(*frame),
                Ok(HostMessage::Log(line)) => info!("SKATE2 PHYSICS {line}"),
                Ok(other) => return Err(refusal(other)),
                Err(RecvTimeoutError::Timeout) => return Err(HostError::Timeout),
                Err(RecvTimeoutError::Disconnected) => return Err(HostError::Exited),
            }
        }
    }

    /// Sends a message that has no answer (`Place`, `SetGestures`, `SetMode`, `SetAmbient`, `SetProfile`).
    pub fn send(&mut self, message: EngineMessage) -> Result<(), HostError> {
        self.tx.send(message).map_err(|_| HostError::Exited)
    }

    fn receiver(&mut self) -> &mut Receiver<HostMessage> {
        self.rx.get_mut().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Link {
    /// Sets the cancel flag, sends `Shutdown` and detaches the thread; nothing waits for it. A boot gives up at its
    /// next poll and the message loop ends before its next message, and the thread frees the slot as it ends. A
    /// thread stuck inside one port call cannot be stopped or killed (Rust has no way to): it keeps the slot until
    /// that call returns, or for good.
    fn drop(&mut self) {
        self.cancel.set();
        let _ = self.tx.send(EngineMessage::Shutdown);
    }
}

/// The error an answer stands for when it is not the one asked for.
fn refusal(message: HostMessage) -> HostError {
    match message {
        HostMessage::Failed(text) => HostError::Failed(text),
        HostMessage::Missing(entry) => HostError::Missing(entry),
        HostMessage::Ready { .. } => HostError::Failed("the sim said Ready again".into()),
        HostMessage::Frame(frame) => HostError::Failed(format!("a frame (tick {}) before Ready", frame.tick)),
        HostMessage::Log(line) => HostError::Failed(format!("unexpected log line {line:?}")),
    }
}

/// A panic's message, as the default hook prints it.
fn panic_text(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into())
}

/// Why `serve_messages` ended before `Shutdown`.
enum Stop {
    /// Tell the game with `Failed`.
    Failed(String),
    /// The game has been told already, with `Missing`.
    Missing,
    /// The game's end is gone (or cancelled the sim): there is nobody to tell.
    Gone,
}

/// The sim thread's loop: the host binary's `serve` (Task 3) with the messages as values, then `Failed` for an error.
fn serve(data: &Path, messages: &Receiver<EngineMessage>, replies: &Sender<HostMessage>, cancel: &Cancel) {
    if let Err(Stop::Failed(why)) = serve_messages(data, messages, replies, cancel) {
        let _ = replies.send(HostMessage::Failed(why));
    }
}

/// Answers the game's messages until `Shutdown` or the cancel flag: `Boot` with `Ready` (or `Missing`) after one
/// warm-up tick, each `Tick` with its `Log` lines and then its `Frame`.
fn serve_messages(
    data: &Path,
    messages: &Receiver<EngineMessage>,
    replies: &Sender<HostMessage>,
    cancel: &Cancel,
) -> Result<(), Stop> {
    let receive = || match messages.recv() {
        Ok(_) if cancel.is_set() => Err(Stop::Gone),
        Ok(message) => Ok(message),
        Err(_) => Err(Stop::Gone),
    };
    let send = |message| replies.send(message).map_err(|_| Stop::Gone);
    let before_boot = |what: &str| Stop::Failed(format!("{what} before Boot"));
    let port = |why: skate2_physics_host::HostError| Stop::Failed(why.to_string());
    match receive()? {
        EngineMessage::Hello { version } if version == PROTOCOL_VERSION => {}
        EngineMessage::Hello { version } => {
            return Err(Stop::Failed(format!("protocol {version}, host {PROTOCOL_VERSION}")));
        }
        other => return Err(Stop::Failed(format!("expected Hello, got {other:?}"))),
    }
    let mut host: Option<Host> = None;
    loop {
        match receive()? {
            EngineMessage::Boot(boot) if host.is_none() => {
                // No data folder at all (the pipeline's skate2-physics stage never ran): a missing-data notice, not
                // "an error in the Skate 2 code" (user 10-25, a dev install without the stage).
                if !data.is_dir() {
                    send(HostMessage::Missing("the folder private/skate2-physics".into()))?;
                    return Err(Stop::Missing);
                }
                let started = Instant::now();
                let mut booted = match Host::boot_with(data, &boot, &cancel.0) {
                    Ok(booted) => booted,
                    Err(skate2_physics_host::HostError::Data(DataError::Missing(entry))) => {
                        send(HostMessage::Missing(entry))?;
                        return Err(Stop::Missing);
                    }
                    Err(skate2_physics_host::HostError::Cancelled) => {
                        CANCELLED_BOOTS.fetch_add(1, Ordering::SeqCst);
                        return Err(Stop::Gone);
                    }
                    Err(why) => return Err(port(why)),
                };
                // Set during the boot's last stage, after its own last check: still a boot its cancel flag ended.
                if cancel.is_set() {
                    CANCELLED_BOOTS.fetch_add(1, Ordering::SeqCst);
                    return Err(Stop::Gone);
                }
                // Inside the boot's budget: the game's first tick is then an ordinary one, held to `TIMEOUT`.
                booted.warm_up().map_err(port)?;
                for line in booted.take_logs() {
                    send(HostMessage::Log(line))?;
                }
                host = Some(booted);
                let boot_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
                send(HostMessage::Ready { boot_ms })?;
            }
            EngineMessage::Tick(pad) => {
                let session = host.as_mut().ok_or_else(|| before_boot("Tick"))?;
                let frame = session.tick(pad).map_err(port)?;
                for line in session.take_logs() {
                    send(HostMessage::Log(line))?;
                }
                send(HostMessage::Frame(Box::new(frame)))?;
            }
            EngineMessage::Place(place) => {
                host.as_mut().ok_or_else(|| before_boot("Place"))?.place(&place).map_err(port)?;
            }
            EngineMessage::SetGestures(slots) => {
                host.as_mut().ok_or_else(|| before_boot("SetGestures"))?.set_gestures(slots).map_err(port)?;
            }
            EngineMessage::SetMode(mode) => {
                host.as_mut().ok_or_else(|| before_boot("SetMode"))?.set_mode(mode).map_err(port)?;
            }
            EngineMessage::SetAmbient(enabled) => {
                host.as_mut().ok_or_else(|| before_boot("SetAmbient"))?.set_ambient(enabled).map_err(port)?;
            }
            EngineMessage::SetProfile(profile) => {
                host.as_mut().ok_or_else(|| before_boot("SetProfile"))?.set_profile(profile);
            }
            EngineMessage::Shutdown => return Ok(()),
            other => return Err(Stop::Failed(format!("unexpected {other:?}"))),
        }
    }
}

/// One attempt to bring a sim up: waiting for the slot (`link` is `None`), then booting.
pub struct Attempt {
    link: Option<Link>,
    /// When this step (the wait, or the boot) began.
    since: Instant,
}

/// What the sim is doing.
pub enum HostState {
    /// Not started (the default, and always with the switch off).
    Off,
    /// The switch's first attempt; Skate 3 physics drives meanwhile.
    Starting(Attempt),
    /// Booted; the switch places the skater and then calls `Skate2Physics::drive`.
    Ready(Link),
    /// Skate 2 physics drives the skater.
    Driving(Link),
    /// After a failure (Amendment F): a fresh sim is on its way.
    Restarting(Attempt),
    /// Stopped, with the reason, until the next switch.
    Failed(String),
}

const LOADING: &str = "Loading Skate 2 physics...";
const RELOADING: &str = "Skate 2 physics hit a problem and is reloading...";

/// Starts a sim on the Skate 2 data with a `Boot`.
pub type Spawner = Box<dyn FnMut(&Path, &Boot) -> Result<Link, Refused> + Send + Sync>;
/// Now, for the boot timeout, the slot wait and the healthy-driving reset.
pub type Clock = Box<dyn Fn() -> Instant + Send + Sync>;

/// The Skate 2 physics switch's state: the sim and the notice the player sees. Not tested in play.
#[derive(Resource)]
pub struct Skate2Physics {
    pub state: HostState,
    /// The notice to show, if any.
    pub notice: Option<String>,
    /// The story mode or an online session wants Skate 3 physics now (Task 19).
    pub force_skate3: bool,
    /// The game is in free play, which picks the sim's game mode (Task 19).
    pub free_play: bool,
    /// The last frame applied while driving: where a restart holds the skater and places it back, and where Skate 3
    /// takes it over if the retries stop.
    pub last_good: Option<Box<Frame>>,
    /// A restart after driving is under way or done: the switch places the skater back at `last_good` before it
    /// calls `drive` (Task 19).
    pub replace_due: bool,
    /// The last failure, which is in the log too.
    pub last_failure: Option<HostError>,
    /// The retries stopped (or the data was missing) while Skate 2 had the skater: it stays held (`drives_local`, so
    /// no engine tick runs on the port's state) until the switch hands it back to Skate 3 and clears this (Task 19).
    pub hand_back_due: bool,
    /// Failed attempts in a row (Amendment F's cap).
    failures: u32,
    /// When driving began, for the healthy-driving reset.
    driving_since: Option<Instant>,
    /// The live boot: the data and the switch's `Boot` with every setting sent since (`send`). A restart boots with
    /// it.
    booted_with: Option<(PathBuf, Boot)>,
    spawner: Spawner,
    clock: Clock,
}

impl Default for Skate2Physics {
    fn default() -> Self {
        Self::with(Box::new(Link::spawn), Box::new(Instant::now))
    }
}

impl Skate2Physics {
    /// Off, starting its sims with `spawner` and reading the time from `clock`.
    pub fn with(spawner: Spawner, clock: Clock) -> Self {
        Self {
            state: HostState::Off,
            notice: None,
            force_skate3: false,
            free_play: false,
            last_good: None,
            replace_due: false,
            last_failure: None,
            hand_back_due: false,
            failures: 0,
            driving_since: None,
            booted_with: None,
            spawner,
            clock,
        }
    }

    /// The Skate 2 data: the `--skate2-physics` folder, else `<asset_root>/private/skate2-physics` (the stage's).
    pub(crate) fn data_dir(config: &crate::config::Config) -> PathBuf {
        config.skate2_physics_dir.clone().unwrap_or_else(|| config.asset_root.join("private").join("skate2-physics"))
    }

    /// Whether Skate 2 physics has the skater: while driving, and while a restart after driving holds it.
    pub fn drives_local(&self) -> bool {
        match self.state {
            HostState::Driving(_) => true,
            HostState::Restarting(_) | HostState::Ready(_) => self.replace_due,
            HostState::Failed(_) => self.hand_back_due,
            HostState::Off | HostState::Starting(_) => false,
        }
    }

    /// The live `Boot` a restart would boot with, if the switch has started a sim.
    pub fn live_boot(&self) -> Option<&Boot> {
        self.booted_with.as_ref().map(|(_, boot)| boot)
    }

    /// The switch: boots a sim on `data` with `boot` when there is none (`Off` or `Failed`), and starts the failure
    /// count over. If the last sim thread still holds the slot, the boot waits for it (`update`).
    pub fn start(&mut self, data: &Path, boot: &Boot) {
        if !matches!(self.state, HostState::Off | HostState::Failed(_)) {
            return;
        }
        self.failures = 0;
        self.driving_since = None;
        self.last_failure = None;
        self.last_good = None;
        self.replace_due = false;
        self.hand_back_due = false;
        self.booted_with = Some((data.to_owned(), boot.clone()));
        self.notice = Some(LOADING.into());
        let now = (self.clock)();
        self.follow(Attempt { link: None, since: now }, now, HostState::Starting);
    }

    /// Follows a start or a restart. Call once per frame; it never blocks.
    pub fn update(&mut self) {
        let now = (self.clock)();
        match std::mem::replace(&mut self.state, HostState::Off) {
            HostState::Starting(attempt) => self.follow(attempt, now, HostState::Starting),
            HostState::Restarting(attempt) => self.follow(attempt, now, HostState::Restarting),
            other => self.state = other,
        }
    }

    /// One step of an attempt, which `wrap` puts back as `Starting` or `Restarting`: start the sim once the slot is
    /// free, then follow its boot.
    fn follow(&mut self, mut attempt: Attempt, now: Instant, wrap: fn(Attempt) -> HostState) {
        let Some(mut link) = attempt.link.take() else {
            let Some((data, boot)) = self.booted_with.clone() else {
                // Not reached: every attempt follows a `start`.
                self.state = HostState::Failed("no boot to start the sim with".into());
                return;
            };
            match (self.spawner)(&data, &boot) {
                Ok(link) => {
                    attempt.link = Some(link);
                    attempt.since = now;
                    self.state = wrap(attempt);
                }
                Err(Refused::Busy) if now.duration_since(attempt.since) >= SLOT_WAIT => {
                    warn!("SKATE2 PHYSICS the last sim thread has not ended within {} s", SLOT_WAIT.as_secs());
                    self.state = wrap(attempt);
                    // Still loading: no sim of this switch has failed, so the player is not told one did.
                    let loading = self.notice.as_deref() == Some(LOADING);
                    self.fail(HostError::Timeout);
                    if loading && matches!(self.state, HostState::Restarting(_)) {
                        self.notice = Some(LOADING.into());
                    }
                }
                Err(Refused::Busy) => self.state = wrap(attempt),
                Err(refused) => {
                    self.state = wrap(attempt);
                    self.refused(refused);
                }
            }
            return;
        };
        match link.poll_ready() {
            Some(Ok(boot_ms)) => {
                self.ready(boot_ms);
                self.state = HostState::Ready(link);
            }
            Some(Err(error)) => {
                attempt.link = Some(link);
                self.state = wrap(attempt);
                self.fail(error);
            }
            None if now.duration_since(attempt.since) > BOOT_TIMEOUT => {
                attempt.link = Some(link);
                self.state = wrap(attempt);
                self.fail(HostError::Timeout);
            }
            None => {
                attempt.link = Some(link);
                self.state = wrap(attempt);
            }
        }
    }

    /// From `Ready` (the skater placed) to `Driving`; false in any other state.
    pub fn drive(&mut self) -> bool {
        if !matches!(self.state, HostState::Ready(_)) {
            return false;
        }
        if let HostState::Ready(link) = std::mem::replace(&mut self.state, HostState::Off) {
            self.state = HostState::Driving(link);
        }
        self.replace_due = false;
        self.driving_since = Some((self.clock)());
        info!("SKATE2 PHYSICS driving");
        true
    }

    /// A placement that did not settle by the switch's deadline (Task 19): a failed attempt, which restarts the sim
    /// (and counts toward the cap) like any other.
    pub fn fail_placement(&mut self) {
        self.fail(HostError::Failed("the placement did not settle".into()));
    }

    /// Runs one tick when the sim is `Ready` or `Driving` and answers its frame (kept as `last_good` while driving).
    /// `None` with no sim to tick, or on a failure, which restarts the sim.
    pub fn tick(&mut self, pad: Pad) -> Option<Box<Frame>> {
        let (link, driving) = match &mut self.state {
            HostState::Ready(link) => (link, false),
            HostState::Driving(link) => (link, true),
            _ => return None,
        };
        match link.tick(pad) {
            Ok(frame) => {
                let frame = Box::new(frame);
                if driving {
                    self.last_good = Some(frame.clone());
                }
                Some(frame)
            }
            Err(error) => {
                self.fail(error);
                None
            }
        }
    }

    /// Sends a message with no answer to the sim, booting or booted; false with none, or when it has closed (which
    /// restarts it). A setting (`SetProfile`, `SetGestures`, `SetMode`, `SetAmbient`) also goes into the live `Boot`,
    /// sent or not, so a restart boots with it.
    pub fn send(&mut self, message: EngineMessage) -> bool {
        if let Some((_, boot)) = &mut self.booted_with {
            match &message {
                EngineMessage::SetProfile(profile) => boot.profile = *profile,
                EngineMessage::SetGestures(slots) => boot.gestures = *slots,
                EngineMessage::SetMode(mode) => boot.mode = *mode,
                EngineMessage::SetAmbient(enabled) => boot.ambient = *enabled,
                _ => {}
            }
        }
        // A link that is still booting takes it too: the sim reads it after the `Boot`, before the first tick.
        let link = match &mut self.state {
            HostState::Ready(link) | HostState::Driving(link) => link,
            HostState::Starting(Attempt { link: Some(link), .. })
            | HostState::Restarting(Attempt { link: Some(link), .. }) => link,
            _ => return false,
        };
        match link.send(message) {
            Ok(()) => true,
            Err(error) => {
                self.fail(error);
                false
            }
        }
    }

    fn ready(&mut self, boot_ms: u32) {
        info!("SKATE2 PHYSICS ready");
        info!("SKATE2 PHYSICS booted in {boot_ms} ms");
        self.notice = None;
    }

    fn refused(&mut self, refused: Refused) {
        match refused {
            Refused::Environment(name) => {
                warn!("SKATE2 PHYSICS refused: the environment variable {name} is set");
                self.hand_back_due = self.replace_due;
                self.state = HostState::Failed(format!("the environment variable {name} is set"));
                self.notice = Some(format!("Skate 2 physics is off: the environment variable {name} is set."));
            }
            Refused::Busy => self.fail(HostError::Timeout),
            Refused::Thread(why) => self.fail(HostError::Failed(format!("the sim thread did not start: {why}"))),
        }
    }

    /// A failed attempt (Amendment F): never the saved setting. Missing data stops; anything else restarts, unless it
    /// is the third failed attempt in a row (`HEALTHY_DRIVING` of driving starts the count over).
    fn fail(&mut self, error: HostError) {
        let now = (self.clock)();
        let why = error.why();
        match &error {
            HostError::Failed(text) => warn!("SKATE2 PHYSICS failed ({why}): {text}"),
            _ => warn!("SKATE2 PHYSICS failed ({why})"),
        }
        self.last_failure = Some(error.clone());
        // Dropping the old state drops its link: its cancel flag is set, and its thread frees the slot as it ends.
        let old = std::mem::replace(&mut self.state, HostState::Off);
        let was_driving = matches!(old, HostState::Driving(_));
        drop(old);
        // Skate 2 had the skater: Skate 3 takes it back through the switch, never straight from the port's state.
        let had = was_driving || self.replace_due;
        if let HostError::Missing(entry) = &error {
            self.hand_back_due = had;
            self.state = HostState::Failed(why);
            self.notice =
                Some(format!("Skate 2 physics needs the Skate 2 data: missing {entry}. Back on Skate 3 physics."));
            return;
        }
        if self.driving_since.take().is_some_and(|since| now.duration_since(since) >= HEALTHY_DRIVING) {
            self.failures = 0;
        }
        self.failures += 1;
        if self.failures >= MAX_FAILURES {
            error!(
                "SKATE2 PHYSICS keeps failing ({why}): {} failed attempts in a row; no more restarts until the next switch",
                self.failures
            );
            self.notice = Some(format!(
                "Skate 2 physics keeps failing ({why}). Using Skate 3 physics until you switch again; your setting is still Skate 2."
            ));
            self.hand_back_due = had;
            self.state = HostState::Failed(why);
            return;
        }
        self.replace_due |= was_driving;
        self.state = HostState::Restarting(Attempt { link: None, since: now });
        self.notice = Some(RELOADING.into());
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::time::{Duration, Instant};

    use skate2_physics_protocol::{Boot, EngineMessage, Frame, HostMessage, Pad, Place, Profile};

    use super::*;
    use crate::skate2_physics::settings::{Gameplay, Physics};

    /// The tests that read the spawn counter or boot the port (whose generators are process-wide) run one at a time,
    /// and so do the timing tests, so `--test-threads=2` cannot run two of them side by side.
    fn serial() -> MutexGuard<'static, ()> {
        static SERIAL: Mutex<()> = Mutex::new(());
        SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The host's ends of the links a fake spawner made, oldest first, each with the `Boot` it was spawned with.
    type Ends = Arc<Mutex<Vec<(Sender<HostMessage>, Receiver<EngineMessage>, Boot)>>>;

    /// A clock the test moves by hand.
    #[derive(Clone)]
    struct Clock(Arc<Mutex<Instant>>);
    impl Clock {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(Instant::now())))
        }
        fn advance(&self, by: Duration) {
            *self.0.lock().unwrap() += by;
        }
    }

    /// Waits until no sim thread holds the slot (an earlier test's thread may still be ending).
    fn wait_for_slot() {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !slot_free() {
            assert!(Instant::now() < deadline, "a sim thread still holds the slot");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// A resource whose spawner makes channel links (no thread) and keeps the host's ends, on a hand clock.
    fn faked() -> (Skate2Physics, Ends, Clock) {
        let (physics, ends, clock, _busy) = faked_busy();
        (physics, ends, clock)
    }

    /// `faked`, whose spawner is refused with `Busy` while the flag is set, as `Link::spawn` is while the last sim
    /// thread holds the slot.
    fn faked_busy() -> (Skate2Physics, Ends, Clock, Arc<AtomicBool>) {
        let ends: Ends = Arc::default();
        let clock = Clock::new();
        let busy = Arc::new(AtomicBool::new(false));
        let spawned = Arc::clone(&ends);
        let now = clock.clone();
        let refused = Arc::clone(&busy);
        let physics = Skate2Physics::with(
            Box::new(move |_: &Path, boot: &Boot| {
                if refused.load(Ordering::SeqCst) {
                    return Err(Refused::Busy);
                }
                let (to_host, from_engine) = channel();
                let (to_engine, from_host) = channel();
                spawned.lock().unwrap().push((to_engine, from_engine, boot.clone()));
                Ok(Link::from_channels(to_host, from_host))
            }),
            Box::new(move || *now.0.lock().unwrap()),
        );
        (physics, ends, clock, busy)
    }

    /// The host side of link `i` ends, as when its thread ends: its sender is dropped.
    fn kill(ends: &Ends, i: usize) {
        drop(std::mem::replace(&mut ends.lock().unwrap()[i].0, channel().0));
    }

    /// The boot every faked test starts with.
    fn boot() -> Boot {
        Boot { mode: 3, ..Boot::default() }
    }

    /// The host side of link `i` says `message`.
    fn say(ends: &Ends, i: usize, message: HostMessage) {
        ends.lock().unwrap()[i].0.send(message).unwrap();
    }

    fn frame(tick: u32) -> HostMessage {
        HostMessage::Frame(Box::new(Frame { tick, ..Frame::default() }))
    }

    /// Starts the faked resource and brings link 0 to `Driving`, with one good frame (tick 7).
    fn driving() -> (Skate2Physics, Ends, Clock) {
        let (mut physics, ends, clock) = faked();
        physics.start(Path::new("D:/s2"), &boot());
        assert!(matches!(physics.state, HostState::Starting(_)));
        assert_eq!(physics.notice.as_deref(), Some("Loading Skate 2 physics..."));
        assert!(!physics.drives_local(), "Skate 3 physics drives while Skate 2 loads");
        say(&ends, 0, HostMessage::Ready { boot_ms: 2400 });
        physics.update();
        assert!(matches!(physics.state, HostState::Ready(_)), "Ready after the host says so");
        assert!(physics.drive());
        assert!(matches!(physics.state, HostState::Driving(_)));
        say(&ends, 0, frame(7));
        assert_eq!(physics.tick(Pad::default()).map(|f| f.tick), Some(7));
        (physics, ends, clock)
    }

    const RELOADING: &str = "Skate 2 physics hit a problem and is reloading...";

    /// The resource after a failure while driving: restarting, holding the skater at its last good frame.
    fn assert_restarting(physics: &Skate2Physics) {
        assert!(matches!(physics.state, HostState::Restarting(_)), "a failure restarts the sim");
        assert_eq!(physics.notice.as_deref(), Some(RELOADING));
        assert!(physics.drives_local(), "Skate 3 physics must not drive while Skate 2 reloads");
        assert!(physics.replace_due, "the skater is placed back once the new sim is ready");
        assert_eq!(physics.last_good.as_ref().map(|f| f.tick), Some(7), "the last good frame is kept");
    }

    #[test]
    fn a_stuck_sim_restarts_after_250_ms() {
        let _serial = serial();
        // A link whose host never answers: its sender is kept alive, so the channel stays open.
        let (to_host, _from_engine) = channel();
        let (_to_engine, from_host) = channel::<HostMessage>();
        let mut link = Link::from_channels(to_host, from_host);
        let started = Instant::now();
        assert_eq!(link.tick(Pad::default()).err(), Some(HostError::Timeout));
        let took = started.elapsed();
        assert!((Duration::from_millis(250)..Duration::from_millis(350)).contains(&took), "took {took:?}");

        let (mut physics, ends, _clock) = driving();
        let started = Instant::now();
        assert!(physics.tick(Pad::default()).is_none(), "no frame: the host is stuck");
        let took = started.elapsed();
        assert!((Duration::from_millis(250)..Duration::from_millis(350)).contains(&took), "took {took:?}");
        assert_eq!(physics.last_failure, Some(HostError::Timeout));
        assert_restarting(&physics);
        drop(ends);
    }

    #[test]
    fn a_dead_sim_restarts_at_once() {
        let _serial = serial();
        let (to_host, _from_engine) = channel();
        let (to_engine, from_host) = channel::<HostMessage>();
        drop(to_engine);
        let mut link = Link::from_channels(to_host, from_host);
        let started = Instant::now();
        assert_eq!(link.tick(Pad::default()).err(), Some(HostError::Exited));
        assert!(started.elapsed() < Duration::from_millis(50), "took {:?}", started.elapsed());

        let (mut physics, ends, _clock) = driving();
        kill(&ends, 0);
        let started = Instant::now();
        assert!(physics.tick(Pad::default()).is_none());
        assert!(started.elapsed() < Duration::from_millis(50), "took {:?}", started.elapsed());
        assert_eq!(physics.last_failure, Some(HostError::Exited));
        assert_restarting(&physics);
    }

    #[test]
    fn a_panic_in_the_sim_restarts() {
        let _serial = serial();
        // The same thread, panic guard and channels as the port's, around a body that panics on its first tick.
        let body = |messages: Receiver<EngineMessage>, replies: Sender<HostMessage>, _cancel: Cancel| {
            replies.send(HostMessage::Ready { boot_ms: 1 }).unwrap();
            for message in messages {
                if matches!(message, EngineMessage::Tick(_)) {
                    panic!("the sim broke on its tick");
                }
            }
        };
        wait_for_slot();
        let mut link = Link::spawn_with(body).unwrap();
        assert_eq!(wait_ready(&mut link), Ok(1));
        let started = Instant::now();
        match link.tick(Pad::default()) {
            Err(HostError::Failed(text)) => assert!(text.contains("the sim broke on its tick"), "{text}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(started.elapsed() < Duration::from_millis(50), "took {:?}", started.elapsed());

        let clock = Clock::new();
        let now = clock.clone();
        let mut physics = Skate2Physics::with(
            Box::new(move |_: &Path, _: &Boot| Link::spawn_with(body)),
            Box::new(move || *now.0.lock().unwrap()),
        );
        physics.start(Path::new("D:/s2"), &Boot::default());
        // The first link's thread frees the slot as it ends; the start waits for it.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !matches!(physics.state, HostState::Ready(_)) {
            assert!(Instant::now() < deadline, "the body says Ready once the slot is free");
            physics.update();
        }
        assert!(physics.drive());
        let started = Instant::now();
        assert!(physics.tick(Pad::default()).is_none());
        assert!(started.elapsed() < Duration::from_millis(50), "took {:?}", started.elapsed());
        match &physics.last_failure {
            Some(HostError::Failed(text)) => assert!(text.contains("the sim broke on its tick"), "{text}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(matches!(physics.state, HostState::Restarting(_)));
        assert_eq!(physics.notice.as_deref(), Some(RELOADING));
        // The test process goes on: the panic ended the sim thread, not the game.
    }

    fn wait_ready(link: &mut Link) -> Result<u32, HostError> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(ready) = link.poll_ready() {
                return ready;
            }
            assert!(Instant::now() < deadline, "no Ready");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn missing_data_names_the_entry() {
        let (mut physics, ends, _clock) = faked();
        physics.start(Path::new("D:/s2"), &boot());
        say(&ends, 0, HostMessage::Missing("data\\big\\anim.big 0x07138dc7".into()));
        physics.update();
        assert!(matches!(&physics.state, HostState::Failed(why) if why == "missing data\\big\\anim.big 0x07138dc7"));
        assert_eq!(
            physics.notice.as_deref(),
            Some("Skate 2 physics needs the Skate 2 data: missing data\\big\\anim.big 0x07138dc7. Back on Skate 3 physics.")
        );
        assert!(!physics.drives_local());
    }

    #[test]
    fn missing_data_is_not_retried() {
        let (mut physics, ends, clock) = faked();
        physics.start(Path::new("D:/s2"), &boot());
        say(&ends, 0, HostMessage::Missing("data\\big\\anim.big 0x07138dc7".into()));
        for _ in 0..10 {
            physics.update();
            clock.advance(Duration::from_secs(30));
        }
        assert_eq!(ends.lock().unwrap().len(), 1, "a missing entry cannot heal: no new sim");
        assert!(matches!(physics.state, HostState::Failed(_)));
        assert_eq!(physics.last_failure, Some(HostError::Missing("data\\big\\anim.big 0x07138dc7".into())));
    }

    /// User 10-25: an install without the skate2-physics stage said "an error in the Skate 2 code"; no data folder is
    /// missing data.
    #[test]
    fn no_data_folder_is_missing_data() {
        let (to_sim, messages) = channel();
        let (replies, from_sim) = channel();
        to_sim.send(EngineMessage::Hello { version: PROTOCOL_VERSION }).unwrap();
        to_sim.send(EngineMessage::Boot(boot())).unwrap();
        let gone = Path::new("no-such-dir/private/skate2-physics");
        assert!(matches!(serve_messages(gone, &messages, &replies, &Cancel::default()), Err(Stop::Missing)));
        assert!(matches!(from_sim.try_recv(), Ok(HostMessage::Missing(entry)) if entry.contains("skate2-physics")));
    }

    #[test]
    fn a_skate2_variable_refuses_the_switch() {
        let _serial = serial();
        let before = spawned();
        let clock = Clock::new();
        let now = clock.clone();
        let mut physics = Skate2Physics::with(
            Box::new(|data: &Path, boot: &Boot| {
                let vars = [("PATH", "x"), ("SKATE2_PARTY_PLAY", "1")];
                Link::spawn_in(data, boot, vars.map(|(name, value)| (name.to_owned(), value.to_owned())))
            }),
            Box::new(move || *now.0.lock().unwrap()),
        );
        physics.start(Path::new("D:/s2"), &Boot::default());
        assert_eq!(physics.notice.as_deref(), Some("Skate 2 physics is off: the environment variable SKATE2_PARTY_PLAY is set."));
        let refused = "the environment variable SKATE2_PARTY_PLAY is set";
        assert!(matches!(&physics.state, HostState::Failed(why) if why == refused));
        assert!(!physics.drives_local());
        for _ in 0..5 {
            clock.advance(Duration::from_secs(61));
            physics.update();
        }
        assert_eq!(spawned(), before, "no sim thread was started, and none is retried");
    }

    #[test]
    fn nothing_starts_with_the_switch_off() {
        let _serial = serial();
        let before = spawned();
        let root = std::env::temp_dir().join(format!("sk8-s2sim-off-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(crate::difficulty::Difficulty::path(&root).parent().unwrap()).unwrap();
        Gameplay::default().save(&root).unwrap();
        assert_eq!(Gameplay::load(&root).physics, Physics::Skate3, "the saved setting is Skate 3");
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, crate::skate2_physics::Skate2PhysicsPlugin));
        for _ in 0..10 {
            app.update();
        }
        let physics = app.world().resource::<Skate2Physics>();
        assert!(matches!(physics.state, HostState::Off));
        assert!(physics.notice.is_none() && !physics.drives_local());
        assert!(!physics.force_skate3 && !physics.free_play && physics.last_good.is_none());
        assert_eq!(spawned(), before, "no skate2-physics thread was started");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_sim_restarts_and_keeps_the_setting() {
        let root = std::env::temp_dir().join(format!("sk8-s2sim-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(crate::difficulty::Difficulty::path(&root).parent().unwrap()).unwrap();
        Gameplay { physics: Physics::Skate2, ..Gameplay::default() }.save(&root).unwrap();

        let (mut physics, ends, _clock) = driving();
        say(&ends, 0, HostMessage::Failed("tick 8: the frame did not complete".into()));
        assert!(physics.tick(Pad::default()).is_none());
        assert_eq!(physics.last_failure, Some(HostError::Failed("tick 8: the frame did not complete".into())));
        assert_restarting(&physics);
        assert_eq!(physics.tick(Pad::default()).map(|f| f.tick), None, "nothing ticks while restarting");

        // The next update starts a fresh sim (the old link had no thread to wait for).
        physics.update();
        assert_eq!(ends.lock().unwrap().len(), 2, "a fresh sim was started");
        assert_restarting(&physics);
        assert_eq!(ends.lock().unwrap()[1].2, boot(), "it boots with what the first one booted with");

        say(&ends, 1, HostMessage::Ready { boot_ms: 2300 });
        physics.update();
        assert!(matches!(physics.state, HostState::Ready(_)));
        assert!(physics.drives_local() && physics.replace_due, "held until the skater is placed back");
        assert!(physics.notice.is_none());
        assert!(physics.drive());
        assert!(matches!(physics.state, HostState::Driving(_)));
        assert!(!physics.replace_due);
        say(&ends, 1, frame(1));
        assert_eq!(physics.tick(Pad::default()).map(|f| f.tick), Some(1));
        assert!(physics.send(EngineMessage::SetMode(1)), "the new sim takes messages");
        assert_eq!(ends.lock().unwrap()[1].1.try_iter().last(), Some(EngineMessage::SetMode(1)));

        assert_eq!(Gameplay::load(&root).physics, Physics::Skate2, "the failure did not touch the saved setting");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn three_failures_stop_retrying() {
        let (mut physics, ends, clock) = driving();
        // A failure after 60 s of driving starts the count over, so these never stop it.
        for (n, gap) in [(1, 61), (2, 61)] {
            kill(&ends, n - 1);
            assert!(physics.tick(Pad::default()).is_none(), "failure {n}");
            assert_eq!(physics.last_failure, Some(HostError::Exited));
            physics.update();
            say(&ends, n, HostMessage::Ready { boot_ms: 1 });
            physics.update();
            assert!(physics.drive(), "restart {n} drives again");
            clock.advance(Duration::from_secs(gap));
        }
        assert_eq!(ends.lock().unwrap().len(), 3);

        // Now three failed attempts in a row: a Failed after 61 s of driving (the count starts over at 1), then two
        // boots that fail.
        say(&ends, 2, HostMessage::Failed("tick 9: one".into()));
        assert!(physics.tick(Pad::default()).is_none());
        assert!(matches!(physics.state, HostState::Restarting(_)));
        clock.advance(Duration::from_secs(10));
        physics.update();
        say(&ends, 3, HostMessage::Failed("boot: two".into()));
        physics.update();
        assert!(matches!(physics.state, HostState::Restarting(_)), "the second failed attempt still retries");
        clock.advance(Duration::from_secs(10));
        physics.update();
        kill(&ends, 4);
        physics.update();
        assert_eq!(physics.last_failure, Some(HostError::Exited));
        assert!(
            matches!(&physics.state, HostState::Failed(why) if why == "it closed"),
            "the third failed attempt in a row stops the retries"
        );
        assert_eq!(
            physics.notice.as_deref(),
            Some("Skate 2 physics keeps failing (it closed). Using Skate 3 physics until you switch again; your setting is still Skate 2.")
        );
        assert!(physics.hand_back_due && physics.drives_local(), "held until the switch hands the skater back");
        physics.hand_back_due = false;
        assert!(!physics.drives_local(), "Skate 3 physics drives after the hand-back");
        assert_eq!(physics.last_good.as_ref().map(|f| f.tick), Some(7), "where Skate 3 takes the skater over");
        for _ in 0..10 {
            clock.advance(Duration::from_secs(30));
            physics.update();
        }
        assert_eq!(ends.lock().unwrap().len(), 5, "no more retries");

        // The next switch starts over.
        physics.start(Path::new("D:/s2"), &boot());
        assert!(matches!(physics.state, HostState::Starting(_)));
        assert_eq!(ends.lock().unwrap().len(), 6);
    }

    #[test]
    fn the_first_tick_after_ready_is_held_to_250_ms() {
        let _serial = serial();
        let (to_host, _from_engine) = channel();
        let (to_engine, from_host) = channel();
        let mut link = Link::from_channels(to_host, from_host);
        to_engine.send(HostMessage::Ready { boot_ms: 2400 }).unwrap();
        assert_eq!(link.poll_ready(), Some(Ok(2400)));
        // The host keeps its end open and never answers the tick.
        let started = Instant::now();
        assert_eq!(link.tick(Pad::default()).err(), Some(HostError::Timeout));
        let took = started.elapsed();
        assert!((Duration::from_millis(250)..Duration::from_millis(350)).contains(&took), "took {took:?}");
        drop(to_engine);
    }

    #[test]
    fn three_boot_timeouts_stop_the_retries() {
        let (mut physics, ends, clock) = faked();
        physics.start(Path::new("D:/s2"), &boot());
        // No sim ever says Ready.
        for attempt in 1..=3 {
            clock.advance(BOOT_TIMEOUT + Duration::from_secs(1));
            physics.update();
            assert_eq!(physics.last_failure, Some(HostError::Timeout), "attempt {attempt}");
            if attempt < 3 {
                assert!(matches!(physics.state, HostState::Restarting(_)), "attempt {attempt} is retried");
                physics.update();
                assert_eq!(ends.lock().unwrap().len(), attempt + 1, "the next sim starts");
            }
        }
        assert!(matches!(&physics.state, HostState::Failed(why) if why == "it did not answer"));
        assert_eq!(
            physics.notice.as_deref(),
            Some("Skate 2 physics keeps failing (it did not answer). Using Skate 3 physics until you switch again; your setting is still Skate 2.")
        );
        for _ in 0..10 {
            clock.advance(Duration::from_secs(60));
            physics.update();
        }
        assert_eq!(ends.lock().unwrap().len(), 3, "no more retries");
    }

    #[test]
    fn a_restart_waits_for_the_slot() {
        let (mut physics, ends, clock, busy) = faked_busy();
        physics.start(Path::new("D:/s2"), &boot());
        say(&ends, 0, HostMessage::Ready { boot_ms: 1 });
        physics.update();
        assert!(physics.drive());
        say(&ends, 0, frame(7));
        assert!(physics.tick(Pad::default()).is_some());

        // The failed sim's thread still holds the slot: the restart waits, holding the skater, and starts nothing.
        busy.store(true, Ordering::SeqCst);
        kill(&ends, 0);
        assert!(physics.tick(Pad::default()).is_none());
        for _ in 0..5 {
            clock.advance(Duration::from_secs(1));
            physics.update();
        }
        assert_eq!(ends.lock().unwrap().len(), 1, "no new sim while the last one holds the slot");
        assert_restarting(&physics);
        assert_eq!(physics.last_failure, Some(HostError::Exited));
        busy.store(false, Ordering::SeqCst);
        physics.update();
        assert_eq!(ends.lock().unwrap().len(), 2, "the new sim starts once the slot is free");

        // A slot still held after 10 s is one failed attempt, and the wait goes on.
        say(&ends, 1, HostMessage::Ready { boot_ms: 1 });
        physics.update();
        assert!(physics.drive());
        clock.advance(HEALTHY_DRIVING);
        busy.store(true, Ordering::SeqCst);
        kill(&ends, 1);
        assert!(physics.tick(Pad::default()).is_none());
        clock.advance(SLOT_WAIT - Duration::from_secs(1));
        physics.update();
        assert_eq!(physics.last_failure, Some(HostError::Exited), "9 s of waiting is not a failure yet");
        clock.advance(Duration::from_secs(1));
        physics.update();
        assert_eq!(physics.last_failure, Some(HostError::Timeout), "10 s of waiting for the slot is a failed attempt");
        assert!(matches!(physics.state, HostState::Restarting(_)), "the second failed attempt in a row still waits");
        assert_eq!(ends.lock().unwrap().len(), 2);
        busy.store(false, Ordering::SeqCst);
        physics.update();
        assert_eq!(ends.lock().unwrap().len(), 3);
    }

    #[test]
    fn a_slot_wait_on_the_first_start_keeps_the_loading_notice() {
        let (mut physics, ends, clock, busy) = faked_busy();
        busy.store(true, Ordering::SeqCst);
        physics.start(Path::new("D:/s2"), &boot());
        clock.advance(SLOT_WAIT);
        physics.update();
        assert_eq!(physics.last_failure, Some(HostError::Timeout), "the wait still counts as a failed attempt");
        assert_eq!(physics.notice.as_deref(), Some("Loading Skate 2 physics..."), "no sim of this switch has failed");
        busy.store(false, Ordering::SeqCst);
        physics.update();
        assert_eq!(ends.lock().unwrap().len(), 1, "the sim starts once the slot is free");
    }

    #[test]
    fn a_sim_holds_the_slot_until_it_ends() {
        let _serial = serial();
        wait_for_slot();
        let freed_within = |limit: Duration| {
            let started = Instant::now();
            while !slot_free() {
                assert!(started.elapsed() < limit, "the slot is still held after {limit:?}");
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        // A sim that answers nothing and ends when its channel closes.
        let waits = |messages: Receiver<EngineMessage>, _: Sender<HostMessage>, _: Cancel| for _ in messages {};
        let first = Link::spawn_with(waits).unwrap();
        assert!(!slot_free());
        assert_eq!(Link::spawn_with(waits).err(), Some(Refused::Busy), "one sim at a time");
        let before = spawned();
        let port = Link::spawn_in(Path::new("D:/s2"), &Boot::default(), std::iter::empty());
        assert_eq!(port.err(), Some(Refused::Busy), "the port's sim waits for the slot too");
        assert_eq!(spawned(), before, "and nothing was started");
        drop(first);
        freed_within(Duration::from_millis(200));

        // A sim that panics frees it as well, after its report.
        let mut panics = Link::spawn_with(|_, _, _| panic!("the sim broke")).unwrap();
        freed_within(Duration::from_millis(200));
        match panics.poll_ready() {
            Some(Err(HostError::Failed(text))) => assert!(text.contains("the sim broke"), "{text}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn a_cancelled_boot_exits() {
        let _serial = serial();
        wait_for_slot();
        // A stand-in for the port's boot, which polls every 5 ms and stops once its cancel flag is set
        // (`Host::boot_with`). It never reads its messages, so only the flag can stop it.
        let link = Link::spawn_with(|_messages, _replies, cancel: Cancel| {
            while !cancel.is_set() {
                std::thread::sleep(Duration::from_millis(5));
            }
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(!slot_free(), "the boot is still running");
        let dropped = Instant::now();
        drop(link);
        while !slot_free() {
            assert!(dropped.elapsed() < Duration::from_millis(200), "the boot did not stop at its next poll");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_restart_boots_with_the_live_settings() {
        let (mut physics, ends, _clock) = driving();
        let profile = Profile { camera_angle: 1, invert_x: true, invert_y: true, grind_transparency: false };
        assert!(physics.send(EngineMessage::SetProfile(profile)));
        assert!(physics.send(EngineMessage::SetGestures([4, 5, 6, 34])));
        assert!(physics.send(EngineMessage::Place(Place::default())), "a placement is not a setting");
        kill(&ends, 0);
        assert!(physics.tick(Pad::default()).is_none());
        // A setting changed while there is no sim to send it to still reaches the next boot.
        assert!(!physics.send(EngineMessage::SetMode(1)));
        physics.update();
        let expected = Boot { profile, gestures: [4, 5, 6, 34], mode: 1, ..boot() };
        assert_eq!(ends.lock().unwrap()[1].2, expected, "the restart boots with the live settings");
        assert_eq!(physics.live_boot(), Some(&expected));
    }

    #[test]
    #[ignore = "needs the Skate 2 data: S2HOST_TEST_DISC"]
    fn a_cancelled_port_boot_exits() {
        let _serial = serial();
        wait_for_slot();
        let root = std::env::var_os("S2HOST_TEST_DISC").expect("set S2HOST_TEST_DISC to a Skate 2 data folder");
        let before = cancelled_boots();
        let link =
            Link::spawn(Path::new(&root), &Boot::default()).expect("the environment holds no SKATE2_ variable");
        // The port's boot takes about 2.4 s: drop the link in the middle of it.
        std::thread::sleep(Duration::from_millis(300));
        let dropped = Instant::now();
        drop(link);
        while !slot_free() {
            assert!(dropped.elapsed() < Duration::from_secs(5), "the cancelled boot is still running");
            std::thread::sleep(Duration::from_millis(5));
        }
        println!("the slot was free {:?} after the drop", dropped.elapsed());
        assert_eq!(cancelled_boots(), before + 1, "the boot ended by its cancel flag, not by finishing");
    }

    #[test]
    fn the_data_folder_is_the_flag_else_the_private_assets() {
        let mut config = crate::config::Config {
            asset_root: "C:/game/assets".into(),
            verification_capture: None,
            map: None,
            map_path: None,
            difficulty: Default::default(),
            check_assets: false,
            start_paused: false,
            multiplayer: Default::default(),
            map_fingerprint: 0,
            teleport: None,
            hero_clip: None,
            skate2_physics_dir: None,
            physics_override: None,
        };
        assert_eq!(Skate2Physics::data_dir(&config), Path::new("C:/game/assets/private/skate2-physics"));
        config.skate2_physics_dir = Some("D:/s2".into());
        assert_eq!(Skate2Physics::data_dir(&config), Path::new("D:/s2"));
    }

    #[test]
    #[ignore = "needs the Skate 2 data: S2HOST_TEST_DISC"]
    fn the_sim_runs_in_the_game() {
        let _serial = serial();
        wait_for_slot();
        let root = std::env::var_os("S2HOST_TEST_DISC").expect("set S2HOST_TEST_DISC to a Skate 2 data folder");
        let before = spawned();
        let started = Instant::now();
        let mut link =
            Link::spawn(Path::new(&root), &Boot::default()).expect("the environment holds no SKATE2_ variable");
        assert_eq!(spawned(), before + 1);
        let boot_ms = loop {
            if let Some(ready) = link.poll_ready() {
                break ready.expect("the port boots");
            }
            assert!(started.elapsed() < Duration::from_secs(10), "Ready within 10 s");
            std::thread::sleep(Duration::from_millis(5));
        };
        println!("Ready after {:?} (the port's boot: {boot_ms} ms)", started.elapsed());
        let mut ticks = Vec::new();
        let mut times = Vec::new();
        for _ in 0..60 {
            let tick = Instant::now();
            ticks.push(link.tick(Pad::default()).expect("a frame").tick);
            times.push(tick.elapsed());
        }
        assert_eq!(ticks, (1..=60).collect::<Vec<_>>());
        let max = times.iter().max().unwrap();
        // The boot's warm-up tick ran before Ready, so the first tick the game waits for is an ordinary one.
        println!("tick 1: {:?}; ticks 2-60: max {:?}, mean {:?}", times[0], times[1..].iter().max().unwrap(),
            times[1..].iter().sum::<Duration>() / 59);
        assert!(*max < TIMEOUT, "every tick, the first included, within {TIMEOUT:?}: max {max:?}");
        // Amendment A: the port's own files are in the host's folder, never in the game's working folder.
        let cwd = std::env::current_dir().unwrap();
        let files = skate2_physics_host::port_files();
        assert_eq!(files.len(), 3, "{files:?}");
        for file in &files {
            assert!(!file.starts_with(&cwd), "{} is in the working folder", file.display());
            assert!(file.starts_with(std::env::temp_dir()), "{} is not in the host's folder", file.display());
        }
        println!("port files: {files:?}");
    }

    /// xorshift64, the soak's seeded generator (no new dependency).
    struct Xorshift(u64);

    impl Xorshift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        /// `0..n`.
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        /// One chance in `n`.
        fn one_in(&mut self, n: u64) -> bool {
            self.below(n) == 0
        }
        /// `-1.0..1.0`.
        fn axis(&mut self) -> f32 {
            (self.next() >> 40) as f32 / 8_388_608.0 - 1.0
        }
    }

    /// The soak's player: one behaviour held for a stretch of 1 to 6 s, then another. Its pads are the engine's
    /// `RawInput`, which the test sends through the game's own `input::pad`.
    struct Player {
        rng: Xorshift,
        behaviour: u64,
        left: u32,
        t: u32,
        stick: [f32; 2],
        flick: [f32; 2],
        grab: bool,
        dpad: u16,
    }

    impl Player {
        const A: u16 = 0x1000;
        const Y: u16 = 0x8000;
        /// A, B, X, Y, LB, RB, both stick clicks and the D-pad.
        const MASH: u16 = 0xF3CF;

        fn new(seed: u64) -> Self {
            let rng = Xorshift(seed);
            Self { rng, behaviour: 0, left: 0, t: 0, stick: [0.0; 2], flick: [0.0; 2], grab: false, dpad: 1 }
        }

        fn next(&mut self) -> crate::input::RawInput {
            if self.left == 0 {
                self.behaviour = self.rng.below(7);
                self.left = 60 + self.rng.below(300) as u32;
                self.t = 0;
                self.stick = [self.rng.axis(), self.rng.axis()];
                self.dpad = 1 << self.rng.below(4);
            }
            self.left -= 1;
            let t = self.t;
            self.t += 1;
            let mut raw = crate::input::RawInput::default();
            match self.behaviour {
                // Idle: nothing held, so a bail's native respawn runs out.
                0 => {}
                // Push and steer: A for 12 ticks of every 40, the left stick held to one side.
                1 => {
                    if t % 40 < 12 {
                        raw.buttons = Self::A;
                    }
                    raw.left = [self.stick[0], 0.0];
                }
                // Carve and powerslide: the left stick held, the right stick hard across for 30 ticks of every 90.
                2 => {
                    raw.left = self.stick;
                    if t % 90 >= 60 {
                        raw.right = [self.stick[0].signum(), 0.0];
                    }
                    if t % 50 < 12 {
                        raw.buttons = Self::A;
                    }
                }
                // Flick tricks: a push, then an antic (right stick down) and a flick in a random direction, now and
                // then a grab (a trigger) held after it.
                3 => {
                    let phase = t % 60;
                    if phase == 20 {
                        self.flick = [self.rng.axis(), self.rng.axis().abs().max(0.5)];
                        self.grab = self.rng.one_in(2);
                    }
                    match phase {
                        0..=11 => raw.buttons = Self::A,
                        20..=22 => raw.right = [self.flick[0] * 0.3, -1.0],
                        24..=25 => raw.right = self.flick,
                        26..=49 if self.grab => raw.triggers = [1.0, self.flick[0].abs()],
                        _ => {}
                    }
                }
                // Button mash: each button about one tick in eight, the triggers now and then, the sticks moved now
                // and then.
                4 => {
                    raw.buttons = (self.rng.next() & self.rng.next() & self.rng.next()) as u16 & Self::MASH;
                    if self.rng.one_in(4) {
                        raw.triggers = [self.rng.axis().abs(), self.rng.axis().abs()];
                    }
                    if self.rng.one_in(10) {
                        self.stick = [self.rng.axis(), self.rng.axis()];
                    }
                    raw.left = self.stick;
                }
                // Off the board: Y, a walk with the left stick, a D-pad gesture every 70 ticks, then Y to get back on.
                5 => {
                    if t < 4 || self.left < 4 {
                        raw.buttons = Self::Y;
                    } else {
                        raw.left = self.stick;
                        if t % 70 >= 30 && t % 70 < 34 {
                            raw.buttons = self.dpad;
                        }
                    }
                }
                // Chaos: everything random every tick.
                _ => {
                    raw.buttons = (self.rng.next() & self.rng.next()) as u16 & Self::MASH;
                    raw.triggers = [self.rng.axis().abs(), self.rng.axis().abs()];
                    raw.left = [self.rng.axis(), self.rng.axis()];
                    raw.right = [self.rng.axis(), self.rng.axis()];
                }
            }
            raw
        }
    }

    /// Amendment F's soak gate: 30 minutes of sim time (108,000 ticks at 60 Hz) through the game's own `Link` (its
    /// thread, channels and 250 ms tick budget) on the Skate 2 data, with a seeded random player (`Player`) whose pads
    /// go through the game's `input::pad`. About every 3,000 ticks it places the skater at a spot of the spike's NSV
    /// run, as the switch does: a teleport (ending the line), a switch-on (with a speed) or a restart (at rest), the
    /// pad neutral until it settles, held to the switch's `PLACE_DEADLINE`. Bails respawn natively. Any tick error,
    /// panic, non-finite deck, root or bone value, tick out of sequence or placement past its deadline fails it. Not
    /// tested in play.
    #[test]
    #[ignore = "needs the Skate 2 data: S2HOST_TEST_DISC; runs about 10 minutes"]
    fn the_sim_survives_a_30_minute_soak() {
        use crate::skate2_physics::input::pad;
        use crate::skate2_physics::switch::{PLACE_DEADLINE, Placing};

        const TICKS: u32 = 108_000;
        const SEED: u64 = 0x5ca7_e2f0_5a4c_0001;
        /// The spike's NSV run (spike-report): the spawn `F_WF_Waterfront`, the ollie, the kickflip landing, the wall
        /// (headless.rs `TARGET`) and the respawn, each facing along the run.
        const SPOTS: [([f32; 3], f32); 5] = [
            ([786.09, -59.34, 882.53], 0.3547),
            ([799.01, -60.55, 912.89], 0.3547),
            ([811.75, -61.34, 936.85], 0.5),
            ([817.62, -61.34, 946.58], 1.0),
            ([818.57, -61.30, 953.17], 1.0),
        ];
        const CATEGORIES: [&str; 8] =
            ["invalid", "ground", "air", "grind", "wipeout", "teleport", "off board", "off board air"];

        let _serial = serial();
        wait_for_slot();
        let root = std::env::var_os("S2HOST_TEST_DISC").expect("set S2HOST_TEST_DISC to a Skate 2 data folder");
        let started = Instant::now();
        let mut link =
            Link::spawn(Path::new(&root), &Boot::default()).expect("the environment holds no SKATE2_ variable");
        let boot_ms = loop {
            if let Some(ready) = link.poll_ready() {
                break ready.expect("the port boots");
            }
            assert!(started.elapsed() < BOOT_TIMEOUT, "Ready within {BOOT_TIMEOUT:?}");
            std::thread::sleep(Duration::from_millis(5));
        };
        println!("soak: Ready after {:?} (the port's boot: {boot_ms} ms), seed {SEED:#x}", started.elapsed());

        let mut player = Player::new(SEED);
        let mut placing: Option<Placing> = None;
        let (mut next_place, mut placed_at) = (3_000, 0);
        let (mut placed, mut settled, mut slowest_settle) = (0u32, 0u32, 0u32);
        let mut seen = [0u64; CATEGORIES.len()];
        let (mut entered, mut last_category) = ([0u32; CATEGORIES.len()], 0u8);
        let (mut longest, mut longest_at, mut total) = (Duration::ZERO, 0, Duration::ZERO);
        let mut last_tick = 0;
        for n in 1..=TICKS {
            // As the switch sends it: neutral while a placement lands.
            let held = if placing.is_some() { Pad::default() } else { pad(player.next(), false) };
            let sent = Instant::now();
            let frame = link.tick(held).unwrap_or_else(|why| panic!("tick {n}: {why:?}"));
            let took = sent.elapsed();
            total += took;
            if took > longest {
                (longest, longest_at) = (took, n);
            }
            assert_eq!(frame.tick, last_tick + 1, "tick {n}: frame tick {} after {last_tick}", frame.tick);
            last_tick = frame.tick;
            let values = frame.deck.iter().chain(&frame.root).chain(frame.bones.iter().flatten());
            assert!(
                values.chain([&frame.speed]).all(|value| value.is_finite()),
                "tick {n}: a non-finite deck, root, bone or speed value (deck {:?}, speed {})",
                frame.deck,
                frame.speed
            );
            let category = usize::from(frame.category);
            assert!(category < CATEGORIES.len(), "tick {n}: category {category}");
            seen[category] += 1;
            if frame.category != last_category {
                entered[category] += 1;
                last_category = frame.category;
            }
            if let Some(placement) = placing.as_mut() {
                match placement.follow(&frame) {
                    Some(true) => {
                        settled += 1;
                        slowest_settle = slowest_settle.max(n - placed_at);
                        placing = None;
                    }
                    Some(false) => panic!(
                        "tick {n}: the placement sent after tick {placed_at} did not settle within {PLACE_DEADLINE} \
                         ticks (placed {:?}, category {category})",
                        frame.placed
                    ),
                    None => {}
                }
            } else if n >= next_place {
                let (position, yaw) = SPOTS[player.rng.below(SPOTS.len() as u64) as usize];
                let place = match player.rng.below(3) {
                    // A teleport while driving.
                    0 => Place { position, yaw, velocity: [0.0; 3], end_line: true },
                    // A switch-on mid-run, carrying the board's speed (0 to 8 m/s along the heading).
                    1 => {
                        let speed = 4.0 + 4.0 * player.rng.axis();
                        let velocity = [speed * yaw.sin(), 0.0, speed * yaw.cos()];
                        Place { position, yaw, velocity, end_line: false }
                    }
                    // A placement back after a restart, at rest.
                    _ => Place { position, yaw, velocity: [0.0; 3], end_line: false },
                };
                link.send(EngineMessage::Place(place)).unwrap_or_else(|why| panic!("tick {n}: Place: {why:?}"));
                placing = Some(Placing::new(frame.tick, true));
                placed += 1;
                placed_at = n;
                next_place = n + 2_500 + player.rng.below(1_000) as u32;
            }
            if n % 10_000 == 0 {
                println!(
                    "soak: tick {n}: deck {:?} {} placements {placed} (settled {settled}) wipeouts {} max {longest:?} \
                     mean {:?}",
                    frame.deck,
                    CATEGORIES[category],
                    entered[4],
                    total / n
                );
            }
        }
        println!(
            "soak: {TICKS} ticks in {:?}: 0 failures; tick time max {longest:?} (tick {longest_at}), mean {:?}",
            started.elapsed(),
            total / TICKS
        );
        println!("soak: placements {placed}, settled {settled}, slowest {slowest_settle} ticks");
        for (i, name) in CATEGORIES.iter().enumerate() {
            println!("soak: {name}: {} ticks, entered {} times", seen[i], entered[i]);
        }
        assert_eq!(settled + u32::from(placing.is_some()), placed, "every placement but a last one in flight settled");
    }
}
