//! Skate 3's leader paths (stage 1 Task 31): the career export's `ai/<row>.json` (tools/skate3_ai_paths.py: the
//! row's `AIPATH_Challenge_ch_<row>.rx2` from miscload.big, nodes at 60 Hz) as the engine's AI paths
//! ([`crate::skate2_rider::Path`], the same per-node fields), the path selection a leader starts and chains with
//! (TU3 0x8246c7f8, ai_runtime.md 2) and the follow rubber band (TU3 0x8246f300 / 0x82470830, ai_runtime.md 1).
//! The tricks are Skate 3's EScorableID rows, [`TRICK_COUNT`] of them; Skate 2's recordings stop at 269.
use crate::skate2_rider::Path as AiPath;
use bevy::math::Vec3;
use std::path::Path;

/// EScorableID's rows (TU3 table 0x820862A8, 332 x 24 bytes; skate_core IDENTIFIERS): a Skate 3 trick index is below.
pub const TRICK_COUNT: usize = 332;
const _: () = assert!(TRICK_COUNT == skate_core::scoring::catalog::IDENTIFIERS.len());

/// ai_skater/default: the band's 3D distances (Hash_0FCC7FAFFA888C9A 5.0, Hash_50442DAE22B63160 15.0) and the speed
/// term's weight (Hash_DCD5DE4BE3FC0942 0.125).
const BAND_LO: f32 = 5.;
const BAND_HI: f32 = 15.;
const BAND_GAIN: f32 = 0.125;
/// The selection radius (ai_skater/default Hash_4F87E7A70DA11691, 4.0 m; 0x82458968 tests dist^2 <= 16, 3D).
const RADIUS: f32 = 4.;

#[derive(serde::Deserialize)]
struct Export {
    row: String,
    ticks_per_second: u32,
    paths: Vec<AiPath>,
}

/// The row's leader paths, in the file's order. Refused: another row's file, another tick rate, or a path whose
/// per-node arrays are empty or of different lengths (the driver indexes them by node).
pub fn load(root: &Path, row: &str) -> Result<Vec<AiPath>, String> {
    let file = root.join("ai").join(format!("{row}.json"));
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let export: Export = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    if export.row != row || export.ticks_per_second != 60 {
        return Err(format!("{}: row {} at {} ticks/s", file.display(), export.row, export.ticks_per_second));
    }
    for p in &export.paths {
        let n = p.pos.len();
        let lengths = [p.step.len(), p.ticks.len(), p.state.len(), p.board.len(), p.skater.len(), p.widths.len(), p.flags.len()];
        if n == 0 || lengths.iter().any(|&l| l != n) {
            return Err(format!("{}: path {} has uneven nodes", file.display(), p.name));
        }
    }
    Ok(export.paths)
}

/// Selection 0x8246c7f8 from `at` (the AI at StartPath, or the current path's last node at its end, 0x8246d4e8): the
/// paths whose first node is within 4 m, in file order; with none, the one with the nearest first node (0x82458c20).
/// Every FTL path's class mask is 0x3fffffffffffffff (ai_runtime.md 2), so the class test is left out. Several
/// candidates are scored by the driver (0x8246c230: turn, random, crowding, player distance).
pub fn candidates(paths: &[AiPath], at: Vec3) -> Vec<usize> {
    let first = |i: usize| Vec3::from_array(paths[i].pos[0]).distance_squared(at);
    let near: Vec<usize> = (0..paths.len()).filter(|&i| first(i) <= RADIUS * RADIUS).collect();
    if !near.is_empty() {
        return near;
    }
    (0..paths.len()).min_by(|&a, &b| first(a).total_cmp(&first(b))).into_iter().collect()
}

/// The FTL rubber band: the leader's path speed for a player `distance_3d` from it (3D, 0x8246f398-f448), negative
/// with the player behind the leader (dir . (player - leader) < 0, 0x8246f3d8). No change within 5 m, then a ramp to
/// +/-1 at 15 m (0x8246f4c4), weighted 0.125: `base_speed x (1 + 0.125 x factor)` (0x82470830; its 0.3 term is 0 on
/// every traced path), so 87.5 % with the player far behind, 112.5 % far ahead.
pub fn rubber_band(distance_3d: f32, base_speed: f32) -> f32 {
    let factor = ((distance_3d.abs() - BAND_LO) / (BAND_HI - BAND_LO)).clamp(0., 1.);
    base_speed * (1. + BAND_GAIN * factor.copysign(distance_3d))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ai_runtime.md 1's table: 100 % within 5 m, linear to 87.5 % (behind) / 112.5 % (ahead) at 15 m, held beyond.
    #[test]
    fn the_rubber_band_holds_5_to_15_m_within_12_5_percent() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        for d in [0., 3., -3., 5., -5.] {
            assert_eq!(rubber_band(d, 8.), 8., "{d}");
        }
        assert!(close(rubber_band(10., 8.), 8.5));
        assert!(close(rubber_band(-10., 8.), 7.5));
        assert!(close(rubber_band(15., 8.), 9.));
        assert!(close(rubber_band(-15., 8.), 7.));
        assert!(close(rubber_band(400., 8.), 9.));
        assert!(close(rubber_band(-400., 8.), 7.));
        for i in -200..=200 {
            let s = rubber_band(i as f32 * 0.5, 10.);
            assert!((8.75 - 1e-4..=11.25 + 1e-4).contains(&s), "{i}: {s}");
        }
    }

    /// The export (SKATE3_CAREER_DIR): ftl_univ_02's 7 paths chain 11 -> 2 -> 3 -> 7 -> 8 -> 9 -> 10 (71.1 s) from the
    /// startlocator (0.94 m); ftl_univ_03's 10 start by the fallback at 3 (6.54 m), then 3 -> {12, 13} -> 5 -> ... -> 11
    /// (91 s), 12 and 13 branching to each other at node 0; each chain's last path falls back to its own start
    /// (ai.md 4, ai_runtime.md 2). Skate 3's tricks above Skate 2's 269 rows are used.
    #[test]
    #[ignore = "requires the skate3-career export (SKATE3_CAREER_DIR)"]
    fn the_ftl_paths_chain_as_ai_md_says() {
        let root = std::path::PathBuf::from(std::env::var_os("SKATE3_CAREER_DIR").expect("set SKATE3_CAREER_DIR"));
        let rows: [(&str, usize, [f32; 3], &[&str], f32); 2] = [
            ("ftl_univ_02", 7, [310.06, 74.12, -439.29], &["11", "2", "3", "7", "8", "9", "10"], 71.1),
            ("ftl_univ_03", 10, [118.15, 64.16, -231.35], &["3", "12", "5", "6", "7", "8", "9", "10", "11"], 91.0),
        ];
        let mut above = false;
        for (row, count, start, chain, seconds) in rows {
            let paths = load(&root, row).unwrap();
            assert_eq!(paths.len(), count, "{row}");
            let id = |n: &str| paths.iter().position(|p| p.name == format!("ch_{row}_{n}")).unwrap_or_else(|| panic!("{row}: {n}"));
            let end = |n: &str| Vec3::from_array(*paths[id(n)].pos.last().unwrap());
            let names = |c: Vec<usize>| c.into_iter().map(|i| paths[i].name.rsplit('_').next().unwrap().to_owned()).collect::<Vec<_>>();
            assert_eq!(names(candidates(&paths, Vec3::from_array(start))), [chain[0]], "{row}: start");
            for w in chain.windows(2) {
                let want: &[&str] = if (row, w[0]) == ("ftl_univ_03", "3") { &["12", "13"] } else { &[w[1]] };
                assert_eq!(names(candidates(&paths, end(w[0]))), want, "{row}: after {}", w[0]);
            }
            let last = chain[chain.len() - 1];
            assert_eq!(names(candidates(&paths, end(last))), [last], "{row}: the end falls back");
            let ticks: u32 = chain.iter().map(|n| paths[id(n)].ticks.iter().sum::<u32>()).sum();
            assert!((ticks as f32 / 60. - seconds).abs() < 0.1, "{row}: {ticks} ticks");
            for p in &paths {
                for t in &p.tricks {
                    assert!(t.trick == -1 || (0..TRICK_COUNT as i32).contains(&t.trick), "{}: {}", p.name, t.trick);
                    above |= t.trick >= 269;
                }
                let branches: Vec<(usize, Vec<&str>)> = p.branches.iter().map(|b| (b.node, b.to.iter().map(|l| l.path.as_str()).collect())).collect();
                match p.name.rsplit('_').next().unwrap() {
                    "12" if row == "ftl_univ_03" => assert_eq!(branches, [(0, vec!["ch_ftl_univ_03_13"])]),
                    "13" if row == "ftl_univ_03" => assert_eq!(branches, [(0, vec!["ch_ftl_univ_03_12"])]),
                    _ => assert!(branches.is_empty(), "{}", p.name),
                }
            }
        }
        assert!(above, "a Skate 3 trick past Skate 2's 269 rows");
    }
}
