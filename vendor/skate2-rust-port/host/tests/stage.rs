//! The `skate2-physics` stage's output (the cut-down copy of the Skate 2 disc) against the disc tests: every test that
//! runs on a disc runs again on the copy. Ignored by default. Not tested in play.
//!
//! Set `S2HOST_TEST_DISC` to the stage output (`.../assets/private/skate2-physics`, the folder with `manifest.json`),
//! and `S2HOST_TEST_MIN` and `S2HOST_TEST_RWCM` as for the other disc tests. `S2HOST_TEST_READS` is not used here: the
//! tests run on the copy log into a folder of their own, so the full disc's read logs (which the stage's entry list is
//! made from) are not mixed with theirs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use skate2_physics_host::data;

/// The other test binaries, with the fewest tests each must run (the tests of Tasks 3-8 and 22: 16 headless, 5 city, 1 pipe).
const SUITES: [(&str, usize); 3] = [("headless", 16), ("city", 5), ("pipe", 1)];

/// The newest `deps/<name>-<hash>` test binary beside this one.
fn suite(name: &str) -> PathBuf {
    let this = std::env::current_exe().expect("this test binary");
    let deps = this.parent().expect("its folder");
    let mut found: Vec<(SystemTime, PathBuf)> = fs::read_dir(deps)
        .expect("the deps folder")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file = entry.file_name().to_string_lossy().into_owned();
            let hash = file.strip_prefix(&format!("{name}-"))?.strip_suffix(std::env::consts::EXE_SUFFIX)?;
            (hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| {
                (entry.metadata().and_then(|meta| meta.modified()).unwrap_or(SystemTime::UNIX_EPOCH), entry.path())
            })
        })
        .collect();
    found.sort();
    found.pop().unwrap_or_else(|| panic!("no {name} test binary in {}: build the host tests first", deps.display())).1
}

/// `N` of "test result: ok. N passed; ..." in a test binary's output.
fn passed(output: &str) -> usize {
    let line = output.lines().rev().find(|line| line.starts_with("test result: ")).unwrap_or("");
    line.split("ok. ").nth(1).and_then(|rest| rest.split(" passed").next()).and_then(|count| count.trim().parse().ok()).unwrap_or(0)
}

#[test]
#[ignore = "needs the stage output: S2HOST_TEST_DISC, S2HOST_TEST_MIN, S2HOST_TEST_RWCM (and the other test binaries built)"]
fn the_stage_output_runs_the_disc_tests() {
    let copy = PathBuf::from(std::env::var_os("S2HOST_TEST_DISC").expect("set S2HOST_TEST_DISC to the stage output"));
    assert!(copy.join(data::MANIFEST).is_file(), "{} has no manifest.json: it is not the stage output", copy.display());
    let listed = data::check(&copy).unwrap_or_else(|why| panic!("{}: {why}", copy.display()));
    println!("manifest: {listed} entries, all there");

    let reads = std::env::temp_dir().join(format!("skate2-physics-host-stage-reads-{}", std::process::id()));
    let _ = fs::remove_dir_all(&reads);
    fs::create_dir_all(&reads).expect("make the read log folder");
    for (name, fewest) in SUITES {
        let exe = suite(name);
        let output = Command::new(&exe)
            .args(["--ignored", "--test-threads=2"])
            .env("S2HOST_TEST_READS", &reads)
            .output()
            .unwrap_or_else(|why| panic!("{}: {why}", exe.display()));
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let tail = |text: &str| text.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        assert!(
            output.status.success(),
            "{name} on the stage output failed ({}):\n{}\n{}",
            output.status,
            tail(&text),
            tail(&String::from_utf8_lossy(&output.stderr))
        );
        let count = passed(&text);
        println!("{name}: {count} passed on the stage output");
        assert!(count >= fewest, "{name}: {count} tests ran, at least {fewest} are expected");
    }

    // What the tests read from the copy: no path read found nothing in it (the copy lost no read).
    let (mut reads_made, mut misses) = (0_usize, Vec::new());
    for file in fs::read_dir(&reads).expect("the read logs").filter_map(Result::ok) {
        for line in fs::read_to_string(file.path()).expect("a read log").lines() {
            if let Some(path) = line.strip_prefix("!notfound\t") {
                misses.push(format!("{}: {path}", file.file_name().to_string_lossy()));
            } else {
                reads_made += 1;
            }
        }
    }
    println!("reads over the stage output: {reads_made} lines; paths read and not found: {}", misses.len());
    misses.sort();
    misses.dedup();
    assert!(misses.is_empty(), "paths the copy lacks ({}):\n{}", misses.len(), misses.join("\n"));
    let _ = fs::remove_dir_all(Path::new(&reads));
}
