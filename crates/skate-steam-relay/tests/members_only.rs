//! Online safety Q2: in a lobby the relay sends only to, and takes only from, the lobby's members.
//! ponytail: a source guard; the checks are inline in the relay's loop (no function to call from a test). Extract
//! them into one `allowed(directory, peer, id)` and test that instead when the relay is next touched.
const MAIN: &str = include_str!("../src/main.rs");

fn squash(s: &str) -> String {
    s.split_whitespace().collect()
}

#[test]
fn the_relay_sends_to_and_accepts_from_lobby_members_only() {
    let main = squash(MAIN);
    for (what, check) in [
        ("send", "(directory.lobby.is_some()&&!directory.members.contains(&target))"),
        ("receive", "(directory.lobby.is_some()&&!directory.members.contains(&id))"),
        ("send outside a lobby", "(directory.lobby.is_none()&&peer!=0&&peer!=target)"),
        ("receive outside a lobby", "(directory.lobby.is_none()&&peer!=0&&id!=peer)"),
        ("receive from ourselves", "||id==own"),
    ] {
        assert!(main.contains(check), "{what}: {check} is gone from the relay");
    }
    // Each check ends in `continue` (the datagram is dropped), not in a log line.
    for check in ["!directory.members.contains(&target))", "!directory.members.contains(&id))"] {
        let at = main.find(check).unwrap() + check.len();
        assert!(main[at..].starts_with("{continue;}"), "{check} must drop the datagram");
    }
}
