//! `Sk8::cLocationManager` over the registered arenas: `RegArena` (`0x82a8eff8`) adds every
//! named `tLocationDesc` of an arena's kind-21 `tLocationDescData` (`AddLocationData`
//! `0x82a8e980`) under `GetHashValue32(name)`, after the entries already held for that hash,
//! and `DRegArena` (`0x82a8e430`) removes them again. The global locator arenas
//! (`cGlobalFileLocationManager::LoadGlobalLocations` `0x82a8f3b0`, every
//! `data\content\global_locators\*.rx2`) register at boot; the streamed simulation arenas
//! follow the world focus.

use crate::{location_arena_plan::LocationArenaPlan, simulation_normal_arena_plan::SimulationNormalArenaPlan};
use skate2_formats::{
    big::BigEntryKey,
    mount::MountSet,
    pegasus::{LOCATION_DESC_DATA_TYPE_ID, LocationDescData, TABLE_OF_CONTENTS_TYPE_ID, TableOfContents},
    rw4::Arena,
};
use std::sync::Arc;

/// `LoadGlobalLocations`' directory.
const GLOBAL_LOCATORS: &str = "data\\content\\global_locators\\";

/// `tSubLocationDesc` bytes: `GetSpawnLocation` (`0x82a8d250`) indexes the sub-locations
/// by 112.
const SUB_LOCATION_SIZE: u32 = 112;

/// One named `tLocationDesc`: its transform and its sub-locations' transforms.
#[derive(Clone, Debug)]
struct Location {
    hash: u32,
    transform: [f32; 16],
    sub_locations: Vec<[f32; 16]>,
}

/// The global locator arenas' locations, then the streamed arenas' by arena, in
/// registration order.
#[derive(Default)]
pub(crate) struct LocationManager {
    global: Vec<Location>,
    arenas: Vec<(u64, Vec<Location>)>,
}

/// The named descriptors of one `tLocationDescData`.
fn push_locations(data: &LocationDescData, locations: &mut Vec<Location>) {
    for desc in &data.locations {
        let Some(name) = desc.name.as_deref() else { continue };
        let first = desc
            .sub_locations_offset
            .and_then(|offset| offset.checked_sub(data.sub_locations_offset))
            .map_or(0, |bytes| bytes / SUB_LOCATION_SIZE);
        let sub_locations = (0..desc.sub_location_count)
            .filter_map(|i| usize::try_from(first + i).ok().and_then(|at| data.sub_locations.get(at)))
            .map(|sub| sub.transform)
            .collect();
        locations.push(Location { hash: crate::challenge::keys::hash_value32(name), transform: desc.transform, sub_locations });
    }
}

fn locations_of(plan: &Arc<SimulationNormalArenaPlan>) -> Result<Vec<Location>, String> {
    let plan = LocationArenaPlan::from_normal_plan(Arc::clone(plan)).map_err(|e| e.to_string())?;
    let mut locations = Vec::new();
    for operation in 0..plan.operations().len() {
        let Some(source) = plan.source_handle(operation) else { continue };
        push_locations(&LocationDescData::parse(source.bytes()).map_err(|e| format!("{e:?}"))?, &mut locations);
    }
    Ok(locations)
}

/// A standalone arena's kind-21 assets, in its first table of contents' order.
fn rx2_locations(bytes: &[u8], locations: &mut Vec<Location>) -> Result<(), String> {
    let resources = Arena::split_file_resources(bytes).map_err(|e| format!("{e:?}"))?;
    let arena = Arena::parse(&resources).map_err(|e| format!("{e:?}"))?;
    let Some(toc) = arena.dictionary.iter().find(|entry| entry.type_id == TABLE_OF_CONTENTS_TYPE_ID) else { return Ok(()) };
    let toc = TableOfContents::parse(arena.object_bytes(&resources, toc).ok_or("table of contents outside the arena")?)
        .map_err(|e| format!("{e:?}"))?;
    for entry in toc.entries.iter().filter(|entry| entry.object_type == LOCATION_DESC_DATA_TYPE_ID) {
        let source = arena.local_entry(entry.object).ok_or("location data outside the arena")?;
        let bytes = arena.object_bytes(&resources, source).ok_or("location data outside the arena")?;
        push_locations(&LocationDescData::parse(bytes).map_err(|e| format!("{e:?}"))?, locations);
    }
    Ok(())
}

impl LocationManager {
    /// `LoadGlobalLocations` (`0x82a8f3b0`): every global locator arena, in the
    /// directory's order.
    pub(crate) fn with_global_locations(mounts: &MountSet) -> Result<Self, String> {
        let mut global = Vec::new();
        for archive in mounts.archives() {
            for entry in &archive.archive().entries {
                let BigEntryKey::Name(name) = &entry.key else { continue };
                let lower = name.to_ascii_lowercase();
                if !lower.starts_with(GLOBAL_LOCATORS) || !lower.ends_with(".rx2") {
                    continue;
                }
                let bytes = archive.read_entry(entry).map_err(|e| format!("{name}: {e}"))?;
                rx2_locations(&bytes, &mut global).map_err(|e| format!("{name}: {e}"))?;
            }
        }
        Ok(Self { global, arenas: Vec::new() })
    }

    /// Follows the registered arenas: a new arena's locations go after those held, a departed
    /// arena's leave.
    pub(crate) fn sync(&mut self, plans: &[Arc<SimulationNormalArenaPlan>]) -> Result<(), String> {
        let ids: Vec<u64> = plans.iter().map(|plan| plan.static_plan().asset().decoded.asset_id).collect();
        self.arenas.retain(|(id, _)| ids.contains(id));
        for (plan, &id) in plans.iter().zip(&ids) {
            if self.arenas.iter().any(|(held, _)| *held == id) {
                continue;
            }
            self.arenas.push((id, locations_of(plan)?));
        }
        Ok(())
    }

    /// `GetLocationInfo` (`0x82a8dc40`): the first entry of the hash (streamed entries are
    /// not high priority).
    fn find(&self, hash: u32) -> Option<&Location> {
        self.global
            .iter()
            .chain(self.arenas.iter().flat_map(|(_, locations)| locations))
            .find(|location| location.hash == hash)
    }

    /// The global locations, the streamed arenas and their locations held.
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        (self.global.len(), self.arenas.len(), self.arenas.iter().map(|(_, l)| l.len()).sum())
    }

    /// `GetNumSpawnLocations` (`0x82a8ddd0`): the location's sub-locations, 0 for an unknown one.
    pub(crate) fn num_spawn_locations(&self, hash: u32) -> usize {
        self.find(hash).map_or(0, |location| location.sub_locations.len())
    }

    /// `IsValidLocation` (`0x82a8de18`).
    pub(crate) fn is_valid(&self, hash: u32) -> bool {
        self.find(hash).is_some()
    }

    /// `GetLocationOrientation` (`0x82a8de48`): the location's transform.
    pub(crate) fn orientation(&self, hash: u32) -> Option<[f32; 16]> {
        self.find(hash).map(|location| location.transform)
    }

    /// `GetSpawnLocation(name, index)` (`0x82a8dce8` → `0x82a8d250`): the index's
    /// sub-location; out of range, the first sub-location (or, without sub-locations, the
    /// location) with its translation moved by (0, 0, -5 index).
    pub(crate) fn spawn_location(&self, hash: u32, index: i32) -> Option<[f32; 16]> {
        let location = self.find(hash)?;
        let sub = usize::try_from(index).ok().and_then(|i| location.sub_locations.get(i));
        if let Some(sub) = sub {
            return Some(*sub);
        }
        let mut transform = location.sub_locations.first().copied().unwrap_or(location.transform);
        #[expect(clippy::cast_precision_loss, reason = "the native (float) conversion")]
        let offset = index as f32 * -5.0;
        transform[14] += offset;
        Some(transform)
    }
}
