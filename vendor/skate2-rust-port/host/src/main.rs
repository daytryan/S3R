//! The host binary: one session of the port, answering over stdin and stdout. Not tested in play.
//!
//! It is the headless test tool (Amendment A): the game does not start it, build it or ship it. The game runs the same
//! loop on a thread of its own, with the messages as values (`crates/skate-game/src/skate2_physics/sim.rs`).
//!
//! The game sends `Hello`, then `Boot`, then one `Tick` per 60 Hz tick, a `Place` whenever it puts the skater down,
//! and `Shutdown` at the end. The host answers `Boot` with `Ready` and each `Tick` with its `Frame`, after a `Log` for
//! each line the tick wrote (`Host::take_logs`). A `Place` has no answer of its own: the frames after it carry its
//! generation and, once it has settled, `true` in `placed`. `SetGestures`, `SetMode`, `SetAmbient` and `SetProfile`
//! have no answer either: they take effect on the next tick. On any error it sends `Failed` and exits with 1. A boot on
//! a data folder that lacks an entry its `manifest.json` lists sends `Missing("<archive> <key>")` instead, and exits
//! with 1.

use std::ffi::OsString;
use std::io::{Read, Write, stdin, stdout};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use skate2_physics_host::{DataError, Host, HostError, unclean_environment};
use skate2_physics_protocol::{EngineMessage, HostMessage, PROTOCOL_VERSION, read_message, write_message};

/// The folder after `--data`.
fn data_dir(mut args: impl Iterator<Item = OsString>) -> Option<PathBuf> {
    args.find(|arg| arg.as_os_str() == "--data")?;
    args.next().map(PathBuf::from)
}

fn main() -> ExitCode {
    let mut out = stdout().lock();

    // `vars_os`, not `vars`: `vars` panics on a name or value that is not Unicode, which would end the host
    // without telling the game why.
    let vars = std::env::vars_os().map(|(name, value)| {
        (name.to_string_lossy().into_owned(), value.to_string_lossy().into_owned())
    });
    if let Some(name) = unclean_environment(vars) {
        // If the game has already gone, there is nobody to tell.
        let _ = write_message(&mut out, &HostMessage::Failed(format!("environment: {name}")));
        return ExitCode::from(2);
    }

    let Some(data) = data_dir(std::env::args_os().skip(1)) else {
        let _ = write_message(&mut out, &HostMessage::Failed("usage: skate2-physics-host --data DIR".into()));
        return ExitCode::from(1);
    };

    match serve(&data, &mut stdin().lock(), &mut out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Stop::Failed(why)) => {
            let _ = write_message(&mut out, &HostMessage::Failed(why));
            ExitCode::from(1)
        }
        Err(Stop::Missing) => ExitCode::from(1),
    }
}

/// Why `serve` ended before `Shutdown`.
enum Stop {
    /// Tell the game with `Failed`.
    Failed(String),
    /// The game has been told already, with `Missing`.
    Missing,
}

impl From<String> for Stop {
    fn from(why: String) -> Self {
        Self::Failed(why)
    }
}

impl From<&str> for Stop {
    fn from(why: &str) -> Self {
        Self::Failed(why.to_owned())
    }
}

/// Answers the game's messages until `Shutdown`.
fn serve(data: &Path, input: &mut impl Read, out: &mut impl Write) -> Result<(), Stop> {
    match receive(input)? {
        EngineMessage::Hello { version } if version == PROTOCOL_VERSION => {}
        EngineMessage::Hello { version } => return Err(format!("protocol {version}, host {PROTOCOL_VERSION}").into()),
        other => return Err(format!("expected Hello, got {other:?}").into()),
    }
    let mut host = None;
    loop {
        match receive(input)? {
            EngineMessage::Boot(boot) if host.is_none() => {
                let started = Instant::now();
                host = Some(match Host::boot(data, &boot) {
                    Ok(booted) => booted,
                    Err(HostError::Data(DataError::Missing(entry))) => {
                        send(out, &HostMessage::Missing(entry))?;
                        return Err(Stop::Missing);
                    }
                    Err(why) => return Err(why.to_string().into()),
                });
                let boot_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
                send(out, &HostMessage::Ready { boot_ms })?;
            }
            EngineMessage::Tick(pad) => {
                let host = host.as_mut().ok_or("Tick before Boot")?;
                let frame = host.tick(pad).map_err(|why| why.to_string())?;
                for line in host.take_logs() {
                    send(out, &HostMessage::Log(line))?;
                }
                send(out, &HostMessage::Frame(Box::new(frame)))?;
            }
            EngineMessage::Place(place) => {
                host.as_mut().ok_or("Place before Boot")?.place(&place).map_err(|why| why.to_string())?;
            }
            EngineMessage::SetGestures(slots) => {
                host.as_mut().ok_or("SetGestures before Boot")?.set_gestures(slots).map_err(|why| why.to_string())?;
            }
            EngineMessage::SetMode(mode) => {
                host.as_mut().ok_or("SetMode before Boot")?.set_mode(mode).map_err(|why| why.to_string())?;
            }
            EngineMessage::SetAmbient(enabled) => {
                host.as_mut().ok_or("SetAmbient before Boot")?.set_ambient(enabled).map_err(|why| why.to_string())?;
            }
            EngineMessage::SetProfile(profile) => {
                host.as_mut().ok_or("SetProfile before Boot")?.set_profile(profile);
            }
            EngineMessage::Shutdown => return Ok(()),
            other => return Err(format!("unexpected {other:?}").into()),
        }
    }
}

fn receive(input: &mut impl Read) -> Result<EngineMessage, String> {
    read_message(input).map_err(|why| format!("protocol: {why}"))
}

fn send(out: &mut impl Write, message: &HostMessage) -> Result<(), String> {
    write_message(out, message).map_err(|why| format!("pipe: {why}"))
}
