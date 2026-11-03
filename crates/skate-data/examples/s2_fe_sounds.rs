//! Skate 2's front-end one-shots as wavs; the work is `skate_data::s2_fe_sounds` (see there).
//!
//!   cargo run -p skate-data --example s2_fe_sounds -- SKATE2_ROOT S2_VAULT_JSON SCREENS_DIR
use std::path::Path;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, vault, screens] = &args[..] else { return Err("usage: s2_fe_sounds SKATE2_ROOT S2_VAULT_JSON SCREENS_DIR".into()) };
    skate_data::s2_fe_sounds(Path::new(root), Path::new(vault), Path::new(screens))
}
