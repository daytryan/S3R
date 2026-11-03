//! The host binary over its pipes, started as the game starts it: a clean environment and `--data`.

use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use skate2_physics_protocol::{
    Boot, EngineMessage, HostMessage, PROTOCOL_VERSION, Pad, Place, read_message, write_message,
};

#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC"]
fn the_binary_answers_hello_boot_and_ticks() {
    let disc = std::env::var_os("S2HOST_TEST_DISC").expect("set S2HOST_TEST_DISC to the Skate 2 disc folder");
    let system_root = std::env::var_os("SystemRoot").expect("SystemRoot");
    // An empty working folder of its own, so the test can see that the host writes nothing there (the port keeps its
    // saves and settings under the working folder when SKATE2_SAVE_ROOT is unset).
    let work = Scratch(std::env::temp_dir().join(format!("skate2-physics-host-pipe-{}", std::process::id())));
    std::fs::create_dir_all(&work.0).expect("make the working folder");

    // The host keeps each boot's port files in a fresh folder under the temp folder; without TEMP, Windows falls back to
    // the Windows folder, which it cannot write.
    let temp = std::env::temp_dir();
    let mut child = Command::new(env!("CARGO_BIN_EXE_skate2-physics-host"))
        .env_clear()
        .env("SystemRoot", system_root)
        .env("TEMP", &temp)
        .env("TMP", &temp)
        .arg("--data")
        .arg(&disc)
        .current_dir(&work.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start the host");
    let mut to_host = BufWriter::new(child.stdin.take().expect("stdin"));
    let mut from_host = BufReader::new(child.stdout.take().expect("stdout"));
    let mut send = |message: &EngineMessage| write_message(&mut to_host, message).expect("write to the host");

    send(&EngineMessage::Hello { version: PROTOCOL_VERSION });
    send(&EngineMessage::Boot(Boot::default()));
    match read_message(&mut from_host) {
        Ok(HostMessage::Ready { boot_ms }) => println!("Ready after {boot_ms} ms"),
        other => panic!("expected Ready, got {other:?}"),
    }
    for tick in 1..=60 {
        send(&EngineMessage::Tick(Pad::default()));
        match read_message(&mut from_host) {
            Ok(HostMessage::Frame(frame)) => assert_eq!(frame.tick, tick),
            other => panic!("tick {tick}: expected a Frame, got {other:?}"),
        }
    }
    // A placement has no answer of its own: the next frames carry its generation, then say it has settled.
    send(&EngineMessage::Place(Place { position: [817.62, -61.34, 946.58], yaw: 1.0, ..Place::default() }));
    let mut placed = Vec::new();
    for tick in 61..=66 {
        send(&EngineMessage::Tick(Pad::default()));
        match read_message(&mut from_host) {
            Ok(HostMessage::Frame(frame)) => placed.push(frame.placed),
            other => panic!("tick {tick}: expected a Frame, got {other:?}"),
        }
    }
    println!("placed over the 6 ticks after a Place: {placed:?}");
    let generation = placed[0].expect("the frame after a Place carries it").0;
    assert!(placed.iter().all(|placed| placed.is_some_and(|(at, _)| at == generation)), "{placed:?}");
    assert_eq!(placed.last(), Some(&Some((generation, true))), "the placement did not settle within 6 ticks");
    send(&EngineMessage::Shutdown);
    let status = child.wait().expect("wait for the host");
    assert!(status.success(), "the host exited with {status}");

    let left: Vec<_> = std::fs::read_dir(&work.0)
        .expect("read the working folder")
        .map(|entry| entry.expect("a folder entry").path())
        .collect();
    assert!(left.is_empty(), "the host wrote into its working folder: {left:?}");
}

/// A folder of the test's own, removed when the test ends, pass or fail.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
