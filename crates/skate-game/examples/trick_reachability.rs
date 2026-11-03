//! Which of the 332 retail scorables can this port actually credit?
//!
//! The retail trick vocabulary is already complete in `skate_core::scoring::catalog`, and the
//! graph VM executes every authored skater behaviour, so "a trick is missing" cannot be answered
//! by reading the catalog. It is answered by asking four independent questions per scorable and
//! reporting where the answers disagree:
//!
//!   * named-by-graph -- does a `ScoringTrick` / `ScoringGrabs` / `ScoringHandPlants` leaf in a
//!     compiled graph publish this name?
//!   * augmented -- is it credited by a `SetScoreAugmentation` flag bit instead of by name?
//!   * grind-named -- does `grind_names::lookup` publish this scorable id?
//!   * observed-live -- has it ever been credited on a `SCORE_TRICK` line under `logs/`?
//!
//! Read the **compiled** `.stategraph` only, never the `MotionGraphIncludes/` source XML. The
//! graph compiler has already expanded every `$MACRO$`; the XML has not, and it also carries
//! authored leaves that never reach a shipped graph. Reading it would report unreachable tricks
//! as reachable and mangle the rest of the names on the way.
//!
//!     cargo run -p skate-game --example trick_reachability -- [assets dir] [logs dir]

#[path = "../src/physics/grind_names.rs"]
mod grind_names;

use skate_core::animation::skeleton_input::name::encode;
use skate_core::scoring::catalog::IDENTIFIERS;
use skate_data::collections::Collections;
use skate_data::scoring::ScoringData;
use skate_data::state_graph::{
    StateGraph,
    attributes::Attributes,
    binding::{Binding, Node, OperationKind},
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// `AttributeName` is a faithful port type and carries no `Ord`, so its public five-word
/// encoding is the map key here rather than growing a derive in `skate-core` for one report.
type Encoded = [u32; 5];

const DEFAULT_ASSETS: &str = r"C:\s3\installations\70eda9dc4644496d81ae73af95ff4285\assets";

const GRAPHS: [&str; 2] = [
    "private/stock/data/state/MotionGraph_OnBoard.stategraph",
    "private/stock/data/state/ActionGraph_OnBoard.stategraph",
];

/// Attribute keys that publish a scorable name, keyed by the behaviour's registration string.
/// Verbatim from `motion_tricks::Operation::parse` (`ScoringTrick`, `ScoringHandPlants`) and
/// `motion_stock_gameplay::Operation::parse` (`ScoringGrabs`) -- the runtime's own spelling,
/// including the lower-case `handplantname` beside the camel-case `grabName`.
///
/// The four directional keys hold *complete* names (`FSGrab_LEFT`), not suffixes:
/// `select_grab_score` returns one of the five whole strings, so treating each as an
/// independently published name is exactly what the runtime does.
const NAMING_LEAVES: [(&str, &[&str]); 3] = [
    ("ScoringTrick", &["trick"]),
    ("ScoringGrabs", &["grabName", "up", "left", "down", "right"]),
    (
        "ScoringHandPlants",
        &["handplantname", "up", "left", "down", "right"],
    ),
];

/// `SetScoreAugmentation`'s source enum (82BBFB28, decoded in `motion_native::Operation::parse`)
/// against the catalog ids those augmentations credit. The two orders differ -- the augmentation
/// index is *not* a scorable id -- so the mapping is written out rather than inferred. These six
/// are credited by a flag bit in `ScorePacket::set`, never by a published name, which is why a
/// name-only audit reports powerslides, manuals and reverts as unreachable.
const AUGMENTATIONS: [(&str, Option<usize>); 10] = [
    ("FSPowerslide", Some(0)),
    ("BSPowerslide", Some(1)),
    ("FSRevert", Some(4)),
    ("BSRevert", Some(5)),
    ("NoseManual", Some(2)),
    ("TailManual", Some(3)),
    ("Wipeout", None),
    ("Landing", None),
    ("RideIdle", None),
    ("Switching", None),
];

/// Credited by bare id at `EndAirTrick` 82DA6260 and carrying no authored record at all.
/// `catalog.rs` documents this; it is a design fact, not a gap.
const METRICS: [usize; 7] = [129, 130, 131, 132, 133, 237, 253];

/// The S2-era generic grind scorables. They appear in no graph and in no `grind_names` table --
/// 82DEE508 publishes skating ids and scoring ids separately, and S3 credits the specific
/// orientation-bearing ids instead. Informational, not a work item.
const GENERIC_GRINDS: std::ops::RangeInclusive<usize> = 38..=53;

#[derive(Default, Clone)]
struct Sighting {
    /// (graph file, dotted state path, behaviour, attribute, authored spelling)
    sites: Vec<(&'static str, String, &'static str, &'static str, String)>,
    /// True once any site survives `Binding`'s enabled propagation.
    enabled: bool,
}

struct GraphScan {
    elements: usize,
    behaviours: usize,
    scoring_leaves: usize,
}

fn state_path(binding: &Binding, mut state: usize) -> String {
    let mut parts = vec![binding.states[state].name.clone()];
    while let Some(parent) = binding.states[state].parent {
        parts.push(binding.states[parent].name.clone());
        state = parent;
    }
    parts.reverse();
    parts.join(".")
}

fn scan_graph(
    file: &'static str,
    graph: &StateGraph,
    names: &mut BTreeMap<Encoded, Sighting>,
    augmented: &mut BTreeSet<usize>,
) -> Result<GraphScan, String> {
    let binding = Binding::from_graph(graph).map_err(|e| e.to_string())?;
    let mut scan = GraphScan {
        elements: graph.elements.len(),
        behaviours: 0,
        scoring_leaves: 0,
    };
    for operation in &binding.operations {
        if operation.kind != OperationKind::Behavior {
            continue;
        }
        scan.behaviours += 1;
        // Native lookup semantics: 32-bit key hash, first record wins on a duplicate key.
        let attributes = Attributes::new(&graph.elements[operation.element].attributes);
        if operation.name == "SetScoreAugmentation" {
            for key in ["augment", "mirrorAugment"] {
                let Some(text) = attributes.text(key).filter(|t| !t.is_empty()) else {
                    continue;
                };
                if let Some((_, Some(id))) = AUGMENTATIONS.iter().find(|(n, _)| *n == text) {
                    augmented.insert(*id);
                }
            }
            continue;
        }
        let Some((leaf, keys)) = NAMING_LEAVES.iter().find(|(n, _)| *n == operation.name) else {
            continue;
        };
        scan.scoring_leaves += 1;
        let owner = match operation.parent {
            Node::State(owner) => state_path(&binding, owner),
            _ => String::from("<not a state>"),
        };
        for key in *keys {
            let Some(text) = attributes.text(key).filter(|t| !t.is_empty()) else {
                continue;
            };
            // A surviving template would silently report its trick as unnamed. The compiled
            // graphs carry none today; fail loudly if an asset revision ever ships one.
            assert!(
                !text.contains('$'),
                "{file}: unexpanded template `{text}` at {owner} ({leaf} {key})"
            );
            let entry = names.entry(encode(text.as_bytes()).0).or_default();
            entry
                .sites
                .push((file, owner.clone(), leaf, key, text.to_owned()));
            entry.enabled |= operation.enabled != 0;
        }
    }
    Ok(scan)
}

/// Enumerate the whole chromosome space: 2*2*2*2*4*6 = 384, the table size. Going through
/// `lookup` rather than the `pub(super)` tables keeps to the module's intended API and exercises
/// its bounds check on the way.
fn grind_scorables() -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    for a in 0..2 {
        for b in 0..2 {
            for c in 0..2 {
                for d in 0..2 {
                    for e in 0..4 {
                        for f in 0..6 {
                            if let Some(name) = grind_names::lookup([a, b, c, d, e, f])
                                && name.scorable_id >= 0
                            {
                                out.insert(name.scorable_id as usize);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// Ids the shipped game can never credit, each with its retail evidence. Everything else with a
/// record must carry a `trick_completeness` proof.
const RETAIL_UNCREDITED: [(std::ops::RangeInclusive<usize>, &str); 1] = [(
    67..=69,
    "gnd_* is published only by `ScoringGrabs` (GroundGrabs/*.xml), whose 8258F9B8 sets the grab \
     bit 0x02000000; the scorer's lookup 82DAC5D0..5FC drops ids 67..69 whenever that bit is set. \
     Pinned by `held_ground_grabs_are_named_but_never_credited`",
)];

/// `SCORE_TRICK proof test=<fn> id=<id>` lines that `tests/trick_completeness.rs` appends to
/// `TRICK_PROOF_LOG`: the first test proving each id.
fn proofs(logs: &Path) -> BTreeMap<usize, String> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(logs) else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in text.lines().filter(|l| l.starts_with("SCORE_TRICK proof ")) {
            let field = |key: &str| line.split_whitespace().find_map(|f| f.strip_prefix(key));
            if let (Some(test), Some(Ok(id))) = (field("test="), field("id=").map(str::parse)) {
                out.entry(id).or_insert_with(|| test.to_owned());
            }
        }
    }
    out
}

/// `SCORE_TRICK` lines from `scoring_runtime.rs`. Both the air and the ground emitter spell the
/// field `id={}` directly after `slot={}`, so one split is unambiguous. Count sightings rather
/// than presence: a single sighting is weak evidence and the report should be able to say so.
fn observed(logs: &Path) -> (BTreeMap<usize, usize>, usize) {
    let mut counts = BTreeMap::new();
    let mut files = 0;
    let Ok(entries) = std::fs::read_dir(logs) else {
        return (counts, files);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("log" | "err")
        ) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut seen_here = false;
        for line in text
            .lines()
            .filter(|l| l.contains("SCORE_TRICK") && !l.starts_with("SCORE_TRICK proof "))
        {
            for field in line.split_whitespace() {
                if let Some(value) = field.strip_prefix("id=")
                    && let Ok(id) = value.parse::<usize>()
                {
                    *counts.entry(id).or_insert(0) += 1;
                    seen_here = true;
                }
            }
        }
        if seen_here {
            files += 1;
        }
    }
    (counts, files)
}

/// The four answers for one scorable, plus the verdict they add up to.
struct Row {
    id: usize,
    identifier: &'static str,
    class: u32,
    score_type: usize,
    points: Option<i32>,
    named: bool,
    enabled: bool,
    augmented: bool,
    grind: bool,
    seen: usize,
    proof: Option<String>,
    verdict: &'static str,
    tags: String,
    site: String,
}

const PROVEN: &str = "proven";
const UNCREDITED: &str = "retail never credits";
const NAMED_UNSEEN: &str = "named, not yet seen";
const UNNAMED: &str = "points, unnamed";
const NO_RECORD: &str = "no record";
const METRIC: &str = "metric";

fn main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let assets = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ASSETS));
    // Not canonicalised: the report prints a fixed relative label so two machines produce a
    // byte-identical file.
    let logs = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../logs"));

    let collections = Collections::load(&assets)?;
    let scoring = ScoringData::load(&collections)?;

    let mut names: BTreeMap<Encoded, Sighting> = BTreeMap::new();
    let mut augmented: BTreeSet<usize> = BTreeSet::new();
    let mut scans = Vec::new();
    for file in GRAPHS {
        let graph = StateGraph::load(&assets.join(file)).map_err(|e| e.to_string())?;
        scans.push((file, scan_graph(file, &graph, &mut names, &mut augmented)?));
    }
    let grinds = grind_scorables();
    let (live, log_files) = observed(&logs);
    let proven = proofs(&logs);

    let rows: Vec<Row> = IDENTIFIERS
        .iter()
        .enumerate()
        .map(|(id, &(identifier, class, score_type))| {
            let sighting = names.get(&encode(identifier.as_bytes()).0);
            let named = sighting.is_some();
            let enabled = sighting.is_some_and(|s| s.enabled);
            let augmented = augmented.contains(&id);
            let grind = grinds.contains(&id);
            let seen = live.get(&id).copied().unwrap_or(0);
            let proof = proven.get(&id).cloned();
            let points = scoring.by_id(id).map(|d| d.points);
            let reachable = named || augmented || grind;
            let verdict = if METRICS.contains(&id) {
                METRIC
            } else if reachable && proof.is_some() {
                PROVEN
            } else if RETAIL_UNCREDITED.iter().any(|(ids, _)| ids.contains(&id)) {
                UNCREDITED
            } else if reachable {
                NAMED_UNSEEN
            } else if points.is_some() {
                UNNAMED
            } else {
                NO_RECORD
            };
            let mut tags = Vec::new();
            if augmented {
                tags.push("augment-slot");
            }
            if grind {
                tags.push("grind-table");
            }
            if named && !enabled {
                tags.push("graph-disabled");
            }
            if GENERIC_GRINDS.contains(&id) {
                tags.push("generic-grind");
            }
            if !reachable && points.is_some() && !GENERIC_GRINDS.contains(&id) {
                tags.push("unpublished");
            }
            let site = sighting
                .and_then(|s| s.sites.first())
                .map(|(_, owner, leaf, key, text)| format!("{owner}  {leaf} {key}={text}"))
                .unwrap_or_default();
            Row {
                id,
                identifier,
                class,
                score_type,
                points,
                named,
                enabled,
                augmented,
                grind,
                seen,
                proof,
                verdict,
                tags: tags.join(","),
                site,
            }
        })
        .collect();

    print_report(&rows, &scoring, &names, &scans, log_files, proven.len());

    // A live sighting that the audit calls unreachable is a hole in the audit, not in the game.
    let contradictions: Vec<&Row> = rows
        .iter()
        .filter(|r| r.seen > 0 && matches!(r.verdict, UNNAMED | NO_RECORD))
        .collect();
    if contradictions.is_empty() {
        return Ok(());
    }
    eprintln!("\nCONTRADICTIONS: credited live but classified unreachable");
    for row in &contradictions {
        eprintln!(
            "  {:>3} {:<30} {} sightings, verdict {}",
            row.id, row.identifier, row.seen, row.verdict
        );
    }
    Err(format!(
        "{} scorable(s) were credited live but the audit found no crediting path",
        contradictions.len()
    ))
}

fn print_report(
    rows: &[Row],
    scoring: &ScoringData,
    names: &BTreeMap<Encoded, Sighting>,
    scans: &[(&str, GraphScan)],
    log_files: usize,
    proofs: usize,
) {
    let count = |verdict: &str| rows.iter().filter(|r| r.verdict == verdict).count();
    println!("# Trick reachability -- {} EScorableID entries", rows.len());
    println!();
    println!("Generated by `cargo run -p skate-game --example trick_reachability`.");
    println!(
        "Sources: the compiled stategraphs, the authored VLT scoring records, the `grind_names`"
    );
    println!("tables, every `SCORE_TRICK` line under `logs/`, and the `SCORE_TRICK proof` lines the");
    println!("`trick_completeness` playback tests write (`TRICK_PROOF_LOG=logs/trick-proofs.log`).");
    println!();
    println!("```");
    for (file, scan) in scans {
        println!(
            "{:<46} {:>6} elements {:>6} behaviours {:>4} scoring leaves",
            file.rsplit('/').next().unwrap_or(file),
            scan.elements,
            scan.behaviours,
            scan.scoring_leaves
        );
    }
    println!(
        "{:<46} {:>6} authored definitions",
        "VLT scoring records",
        scoring.definitions.len()
    );
    println!(
        "{:<46} {:>6} distinct authored score names",
        "compiled graph",
        names.len()
    );
    println!(
        "{:<46} {:>6} ids credited across {log_files} log file(s)",
        "playtest logs",
        rows.iter().filter(|r| r.seen > 0).count()
    );
    println!("{:<46} {:>6} ids proven by a playback test", "trick_completeness", proofs);
    println!("```");
    println!();
    println!("| verdict | count |");
    println!("|---|---:|");
    for verdict in [PROVEN, UNCREDITED, NAMED_UNSEEN, UNNAMED, NO_RECORD, METRIC] {
        println!("| {verdict} | {} |", count(verdict));
    }
    println!();
    println!("`proven` means a headless playback test in `src/tests/trick_completeness.rs` drove raw");
    println!("controller samples through the whole chain and the scorer both named the trick and");
    println!("credited its record (repetition count > 0); the `proof` column names that test.");
    println!("`named, not yet seen` has a crediting path but no proof yet. `retail never credits`");
    println!("is named by the graph but dropped by the retail scorer itself:");
    for (ids, why) in RETAIL_UNCREDITED {
        println!("- {}..={}: {why}", ids.start(), ids.end());
    }
    println!();
    println!("The `seen` column counts `SCORE_TRICK` credits in session logs under `logs/`, if any.");
    println!();
    println!("`points, unnamed` has authored points but no crediting path; read its tag before");
    println!("treating it as work, since `generic-grind` marks the S2-era ids S3 superseded.");
    println!(
        "`no record` has no VLT record at all -- `ScoringData::load` skips those, and its own"
    );
    println!("comment names them: enum entries the shipped game never scores. Reaching one would");
    println!("mean inventing a trick retail does not have.");
    println!();
    println!("## Every scorable");
    println!();
    println!(
        "| id | identifier | cls | ty | points | named | aug | grind | seen | verdict | proof | tags | first authored site |"
    );
    println!("|---:|---|---:|---:|---:|---|---|---|---:|---|---|---|---|");
    for row in rows {
        let yes = |b: bool| if b { "yes" } else { "-" };
        println!(
            "| {} | `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            row.id,
            row.identifier,
            row.class,
            row.score_type,
            row.points
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".into()),
            if row.named && !row.enabled {
                "disabled"
            } else {
                yes(row.named)
            },
            yes(row.augmented),
            yes(row.grind),
            row.seen,
            row.verdict,
            row.proof.as_deref().map(|t| format!("`{t}`")).unwrap_or_else(|| "-".into()),
            row.tags,
            row.site
        );
    }

    // The reverse diff: names the graph publishes that no EScorableID slot can receive, so the
    // name lookup finds nothing. For a grind that is inert rather than fatal -- the grind
    // collector credits the chromosome's `scorable_id` from `grind_names::lookup`, not the
    // leaf's spelling -- which is why the backfoot family still scores, under its non-backfoot
    // scorable. Anywhere else, a name here would mean a trick that animates and scores nothing.
    let catalog: BTreeSet<Encoded> = IDENTIFIERS
        .iter()
        .map(|(name, _, _)| encode(name.as_bytes()).0)
        .collect();
    let orphans: Vec<&Sighting> = names
        .iter()
        .filter(|(encoded, _)| !catalog.contains(*encoded))
        .map(|(_, sighting)| sighting)
        .collect();
    println!();
    println!(
        "## Authored names with no EScorableID slot ({})",
        orphans.len()
    );
    println!();
    println!("Published by a scoring leaf but absent from the 332-entry enum, so the name lookup");
    println!(
        "finds nothing. Every one of these is a backfoot (`BF_`) grind variant, and grinds are"
    );
    println!(
        "credited through `grind_names::lookup`'s `scorable_id` rather than through the leaf's"
    );
    println!("spelling -- so they still score, under the non-backfoot scorable. Retail ships the");
    println!("same mismatch; it is not a port defect.");
    println!();
    println!("| authored spelling | leaf | state |");
    println!("|---|---|---|");
    let mut spellings: Vec<(&str, &str, &str)> = orphans
        .iter()
        .filter_map(|s| s.sites.first())
        .map(|(_, owner, leaf, _, text)| (text.as_str(), *leaf, owner.as_str()))
        .collect();
    spellings.sort_unstable();
    for (text, leaf, owner) in spellings {
        println!("| `{text}` | {leaf} | {owner} |");
    }
}
