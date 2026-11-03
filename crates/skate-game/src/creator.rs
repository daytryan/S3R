//! Not retail (user 10-25): the creator tag. One Steam account, the game's creator ([`CREATOR_STEAM_ID`]), reads
//! "Name - Creator" in a rainbow sweep ([`colour`], user 10-26; was gold) in the Player List, over its skater and in
//! chat; nobody else can show it.
//!
//! - The game holds only an Ed25519 public key ([`PUBLIC_KEY`]). The secret key exists on the creator's PC alone,
//!   sealed with Windows DPAPI for that Windows user (`%LOCALAPPDATA%\Skate3RustEngine\creator.key`), never in the
//!   repository or a build.
//! - Every player publishes a random challenge for its session (`roster::NONCE_KEY`). The creator's game signs each
//!   one with the creator's Steam ID, the session (a Steam session's is its Steam lobby) and both actors
//!   (`roster::CREATOR_KEYS`), only while Steam says it runs as that ID.
//! - Each game checks the answer to its own challenge: a proof seen once never passes again (another session, another
//!   player), and nothing a host relays can be forged. Online safety 10-26: the session is signed too, or a modified
//!   host with a second account in the creator's lobby could carry a real answer into its own. A Steam-verified ID
//!   must be the creator's too, so the tag shows in Steam sessions only.
//!
//! Names can't imitate the tag: `roster::reserved` keeps "Creator" (and the other marks) out of names, and a name only
//! ever draws in the plain colour (apt_text::literal, monochrome glyphs).
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use std::{
    collections::BTreeSet,
    sync::{Mutex, OnceLock},
};

/// steamcommunity.com/id/752476534785648 (user 10-25).
pub(crate) const CREATOR_STEAM_ID: u64 = 76561198325542928;
/// The creator key's public half (made 10-25, scratchpad creator/make_creator_key.py).
const PUBLIC_KEY: &str = "845b7232d34e07914c1ec2e5dd1b2a2bb63366c4014b04c3acdce6e97aa61487";
/// DPAPI entropy the key file was sealed with.
const ENTROPY: &[u8] = b"Skate 3 Remaster creator key v1";
/// What a signature covers starts with this, so no other signed message can pass for a proof.
const DOMAIN: &[u8] = b"S3R creator tag v2\0";
/// The sweep's steps a second: the tag's colour changes this often, so the Player List redraws this often while it
/// shows the tag (apt_scene::draw_key), not every frame.
const STEPS: f32 = 20.;
/// Seconds the sweep takes round the colour circle.
const SWEEP: f32 = 4.;

/// The tag's colour now (user 10-26: "RGB sweep ... instead of the gold"): one colour for the whole tag, the same
/// moment everywhere it shows, going round the rainbow.
pub(crate) fn colour() -> [u8; 3] {
    rainbow(step() as f32 / (STEPS * SWEEP))
}

/// Where the sweep is: a new value [`STEPS`] times a second.
pub(crate) fn step() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    (START.get_or_init(std::time::Instant::now).elapsed().as_secs_f32() * STEPS) as u64
}

/// The colour `turn` of the way round the hue circle (0 red, 1/3 green, 2/3 blue) at three-quarters saturation, so
/// every hue reads on the HUD's dark plates (pure blue doesn't): one channel always 255, one always 64.
fn rainbow(turn: f32) -> [u8; 3] {
    let h = turn.rem_euclid(1.) * 6.;
    let x = 1. - (h % 2. - 1.).abs();
    let rgb = match h as u32 {
        0 => [1., x, 0.],
        1 => [x, 1., 0.],
        2 => [0., 1., x],
        3 => [0., x, 1.],
        4 => [x, 0., 1.],
        _ => [1., 0., x],
    };
    rgb.map(|c: f32| (255. * (0.25 + 0.75 * c)).round() as u8)
}

/// Whether `c` is one of the sweep's colours (online_hud's tests: no other name colour is).
#[cfg(test)]
pub(crate) fn on_sweep(c: [u8; 3]) -> bool {
    c.contains(&255) && c.contains(&64)
}
/// The mark after the name, like " - Host".
pub(crate) const MARK: &str = " - Creator";
/// Answers a record holds: 14 x 72 bytes fits lobby::MAX_APP_VALUE, and the four records cover a full lobby.
const PER_RECORD: usize = 14;
const ENTRY: usize = 8 + 64;

pub(crate) type Nonce = [u8; 16];

/// A fresh challenge (the OS's random source).
pub(crate) fn nonce() -> Nonce {
    let mut n = [0; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut n).expect("system random source");
    n
}

/// The challenge a record holds, if it is one.
pub(crate) fn parse_nonce(value: &[u8]) -> Option<Nonce> {
    value.try_into().ok()
}

pub(crate) fn public_key() -> [u8; 32] {
    std::array::from_fn(|i| u8::from_str_radix(&PUBLIC_KEY[2 * i..2 * i + 2], 16).expect("hex key"))
}

fn message(session: u64, creator_actor: u64, viewer: u64, nonce: &Nonce) -> Vec<u8> {
    let (creator, session, actor, viewer) =
        (CREATOR_STEAM_ID.to_le_bytes(), session.to_le_bytes(), creator_actor.to_le_bytes(), viewer.to_le_bytes());
    [DOMAIN, &creator, &session, &actor, &viewer, nonce].concat()
}

/// The creator's key, opened once: `None` on every PC but the creator's (no file, another Windows user, or a key that
/// is not [`PUBLIC_KEY`]'s).
pub(crate) fn key() -> Option<&'static Ed25519KeyPair> {
    static KEY: OnceLock<Option<Ed25519KeyPair>> = OnceLock::new();
    KEY.get_or_init(|| {
        let file = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Skate3RustEngine/creator.key");
        let seed = unseal(&std::fs::read(file).ok()?)?;
        Ed25519KeyPair::from_seed_and_public_key(&seed, &public_key()).ok()
    })
    .as_ref()
}

#[cfg(not(windows))]
fn unseal(_: &[u8]) -> Option<Vec<u8>> {
    None
}

/// DPAPI for the current Windows user (the launcher's playtime key does the same), with [`ENTROPY`].
#[cfg(windows)]
fn unseal(sealed: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};
    let input = CRYPT_INTEGER_BLOB { cbData: sealed.len() as u32, pbData: sealed.as_ptr() as *mut u8 };
    let entropy = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
    };
    if ok == 0 || out.pbData.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    unsafe { LocalFree(out.pbData as _) };
    Some(bytes)
}

/// The records (`roster::CREATOR_KEYS` order) answering each viewer's challenge, signed by `key` for the creator
/// playing as `creator_actor` in `session`. Viewers past what the records hold are left out.
pub(crate) fn answers(key: &Ed25519KeyPair, session: u64, creator_actor: u64, viewers: &[(u64, Nonce)]) -> Vec<Vec<u8>> {
    let mut records: Vec<Vec<u8>> = viewers
        .chunks(PER_RECORD)
        .take(skate_net::roster::CREATOR_KEYS.len())
        .map(|chunk| {
            chunk
                .iter()
                .flat_map(|(viewer, nonce)| {
                    let signature = key.sign(&message(session, creator_actor, *viewer, nonce));
                    [&viewer.to_le_bytes()[..], signature.as_ref()].concat()
                })
                .collect()
        })
        .collect();
    // Every key is written, so a record for viewers who left is emptied rather than kept.
    records.resize(skate_net::roster::CREATOR_KEYS.len(), Vec::new());
    records
}

/// Whether `records` (an actor's `roster::CREATOR_KEYS` values) answer `viewer`'s challenge `nonce` in `session` under
/// `public` ([`public_key`]): `creator_actor` is the creator.
pub(crate) fn verify<'a>(
    public: &[u8],
    records: impl IntoIterator<Item = &'a [u8]>,
    session: u64,
    creator_actor: u64,
    viewer: u64,
    nonce: &Nonce,
) -> bool {
    let message = message(session, creator_actor, viewer, nonce);
    records.into_iter().flat_map(|r| r.chunks_exact(ENTRY)).any(|entry| {
        entry[..8] == viewer.to_le_bytes() && UnparsedPublicKey::new(&ED25519, public).verify(&message, &entry[8..]).is_ok()
    })
}

/// The session actors shown as the creator (online_roster checks them; the Player List, name tags and chat ask).
static VERIFIED: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());

pub(crate) fn is_creator(actor: u64) -> bool {
    VERIFIED.lock().unwrap_or_else(|e| e.into_inner()).contains(&actor)
}

/// Replaces the verified set; whether it changed.
pub(crate) fn set_verified(actors: BTreeSet<u64>) -> bool {
    let mut verified = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    let changed = *verified != actors;
    *verified = actors;
    changed
}

/// Whether this PC holds the creator key matching the game's public key (a self-check, never the secret itself).
#[allow(dead_code)] // a read-only tap for tools
pub(crate) fn holds_key() -> bool {
    key().is_some_and(|k| k.public_key().as_ref() == public_key().as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> (Ed25519KeyPair, Vec<u8>) {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let key = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public = key.public_key().as_ref().to_vec();
        (key, public)
    }

    #[test]
    fn the_game_holds_the_public_half_of_the_creators_key() {
        // make_creator_key.py signed this with the sealed secret: the embedded key is that key's public half.
        let signature: Vec<u8> = (0..64)
            .map(|i| u8::from_str_radix(&"700264df7bcf569bd80fb8cdbdfccc0e61376b36d211e7c088dc4c8a336678ebf43318681fdd69145656be27901e8cc19b274b0c59a979c8a3cfc588914e500e"[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        assert!(UnparsedPublicKey::new(&ED25519, public_key()).verify(b"S3R creator self-test", &signature).is_ok());
    }

    #[test]
    fn only_an_answer_to_your_own_challenge_proves_the_creator() {
        let (key, public) = test_key();
        let (alice, bob) = (11_u64, 12_u64);
        let (na, nb) = (nonce(), nonce());
        const LOBBY: u64 = 109775240000000001;
        let records = answers(&key, LOBBY, 7, &[(alice, na), (bob, nb)]);
        assert_eq!(records.len(), skate_net::roster::CREATOR_KEYS.len());
        let check = |records: &[Vec<u8>], creator, viewer, nonce: &Nonce| {
            verify(&public, records.iter().map(Vec::as_slice), LOBBY, creator, viewer, nonce)
        };
        assert!(check(&records, 7, alice, &na) && check(&records, 7, bob, &nb));
        assert!(!check(&records, 7, alice, &nb), "another player's answer");
        assert!(!check(&records, 7, alice, &nonce()), "an old session's answer (a new challenge)");
        assert!(!check(&records, 8, alice, &na), "moved onto another actor");
        assert!(!check(&records, 7, 13, &na), "a viewer it never answered");
        let mut tampered = records.clone();
        tampered[0][20] ^= 1;
        assert!(!check(&tampered, 7, alice, &na), "a changed signature");
        let (other, _) = test_key();
        let forged = answers(&other, LOBBY, 7, &[(alice, na)]);
        assert!(!check(&forged, 7, alice, &na), "signed by any other key");
        let records_in = |lobby| verify(&public, records.iter().map(Vec::as_slice), lobby, 7, alice, &na);
        assert!(!records_in(LOBBY + 1), "online safety 10-26: a real answer carried into another lobby");
        assert!(!verify(&public_key(), records.iter().map(Vec::as_slice), LOBBY, 7, alice, &na), "the game's key never matches another's");
        assert!(!check(&[vec![1, 2, 3]], 7, alice, &na), "garbage");
    }

    #[test]
    fn a_full_lobby_fits_the_records() {
        let (key, public) = test_key();
        let viewers: Vec<(u64, Nonce)> = (1..skate_net::lobby::MAX_PLAYERS as u64).map(|v| (v, nonce())).collect();
        let records = answers(&key, 5, 99, &viewers);
        assert!(records.iter().all(|r| r.len() <= skate_net::lobby::MAX_APP_VALUE));
        assert!(viewers.iter().all(|(v, n)| verify(&public, records.iter().map(Vec::as_slice), 5, 99, *v, n)));
        assert!(answers(&key, 5, 99, &[]).iter().all(Vec::is_empty), "nobody to answer: every record emptied");
    }

    /// User 10-26: the tag sweeps the rainbow, bright enough on every hue.
    #[test]
    fn the_tag_sweeps_the_rainbow() {
        assert_eq!(rainbow(0.), [255, 64, 64]);
        assert_eq!(rainbow(1. / 3.), [64, 255, 64]);
        assert_eq!(rainbow(2. / 3.), [64, 64, 255]);
        assert_eq!(rainbow(1.), rainbow(0.), "round and round");
        let colours: BTreeSet<[u8; 3]> = (0..240).map(|i| rainbow(i as f32 / 240.)).collect();
        assert!(colours.len() > 200 && colours.iter().all(|&c| on_sweep(c)), "{colours:?}");
        assert!(on_sweep(colour()));
    }

    #[test]
    fn the_verified_set_reports_changes() {
        // Actor ids no other test uses: the set is global.
        let (a, b) = (0xC0DE_0000_0001, 0xC0DE_0000_0002);
        assert!(set_verified([a].into()));
        assert!(is_creator(a) && !is_creator(b));
        assert!(!set_verified([a].into()));
        assert!(set_verified(BTreeSet::new()));
        assert!(!is_creator(a));
    }
}
