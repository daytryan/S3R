use super::*;
use crate::map_transition::CurrentMap;
use crate::multiplayer::Online;
use std::path::PathBuf;

const DOWNTOWN_SKATE_LINE: Vec3 = Vec3::new(42.42, -32.01, 536.80);

fn assets() -> Option<PathBuf> {
    let root: PathBuf = std::env::var_os("SKATE3_ASSET_ROOT")?.into();
    MAPS.iter().all(|m| root.join(dirs(m).unwrap().0).join("roads.json").exists()).then_some(root)
}

/// The maps with traffic: Skate 2, Skate 1, then the Skate 3 districts with roads.
const MAPS: [&str; 5] = ["New_San_Vanelona", "Skate_1_San_Vanelona", "DownTown", "Industrial", "University"];

fn roads(root: &Path, map: &str) -> JRoads {
    serde_json::from_slice(&std::fs::read(root.join(dirs(map).unwrap().0).join("roads.json")).unwrap()).unwrap()
}

fn map(name: &str) -> CurrentMap {
    CurrentMap { path: None, name: name.into(), spawn: [0.0; 3], heading: 0.0, generation: 0 }
}

#[test]
fn light_program_matches_the_retail_timeline() {
    // 0x825c77c8: odd controllers start green after 0.5 s all-red, even ones red; S2 17 s, S1 32 s cycle.
    for (green, amber, red, cycle) in [(7.0, 1.0, 0.5, 17.0), (12.0, 2.0, 2.0, 32.0)] {
        let mut l: [Light; 4] = std::array::from_fn(|i| Light::new(i, green, amber, red));
        let mut modes = Vec::new();
        for _ in 0..((cycle * 2.0 * 60.0) as usize) {
            for x in &mut l {
                x.tick();
            }
            modes.push([l[0].mode(), l[1].mode()]);
        }
        let at = |t: f32| modes[(t * 60.0) as usize];
        assert_eq!(at(red + 1.0), [0, 2], "odd green first");
        assert_eq!(at(red + green + 0.5 * amber), [0, 1], "odd amber");
        assert_eq!(at(2.0 * red + green + amber + 1.0), [2, 0], "even green after the all-red");
        assert_eq!(at(cycle - 0.5 * amber), [1, 0], "even amber ends the cycle");
        assert_eq!(at(cycle + red + 1.0), [0, 2], "the cycle repeats");
        assert_eq!(l[2].mode(), l[0].mode());
    }
}

#[test]
fn only_the_traffic_maps_and_only_offline() {
    for name in MAPS {
        assert!(wanted(Some(&map(name)), false).is_some(), "{name} offline");
        assert!(wanted(Some(&map(name)), true).is_none(), "{name} online");
    }
    // Skate 3 maps without a road network, other Skate 1/2 maps, and anything else: no traffic.
    for name in ["SkateSchool", "MegaPark", "DownTownSkatePark", "IndustrialSkatePark", "StartPark", "BlackBoxPark",
        "MaloofMoneyCup", "Skate_2_School", "Test world", "New_San_Vanelona_Park"]
    {
        assert!(wanted(Some(&map(name)), false).is_none(), "{name}");
    }
    assert!(wanted(None, false).is_none());
    // Skate 3 districts read their own data and share one vehicles folder.
    assert_eq!(dirs("University"), Some(("private/traffic/skate3/University", "private/traffic/skate3/vehicles")));
    assert_eq!(dirs("New_San_Vanelona"), Some(("private/traffic/skate2", "private/traffic/skate2/vehicles")));
}

#[test]
fn going_online_or_leaving_the_city_despawns_every_car() {
    for city in ["New_San_Vanelona", "DownTown"] {
        going_online_or_leaving(city);
    }
}

fn going_online_or_leaving(city: &str) {
    let mut app = App::new();
    app.init_resource::<Traffic>().insert_resource(Online(false)).insert_resource(map(city)).add_systems(Update, gate);
    app.update();
    assert!(app.world().resource::<Traffic>().key.is_some());
    let car = app.world_mut().spawn(TrafficCar::default()).id();
    app.world_mut().resource_mut::<Online>().0 = true;
    app.update();
    assert!(app.world().get_entity(car).is_err(), "a car survived going online");
    assert!(app.world().resource::<Traffic>().sim.is_none() && app.world().resource::<Traffic>().key.is_none());

    app.world_mut().resource_mut::<Online>().0 = false;
    app.update();
    let car = app.world_mut().spawn(TrafficCar::default()).id();
    app.world_mut().resource_mut::<CurrentMap>().name = "Test world".into();
    app.update();
    assert!(app.world().get_entity(car).is_err(), "a car survived leaving the city");
    assert!(app.world().resource::<Traffic>().key.is_none());
}

/// Amendment C: the engine's cars are off while Skate 2 physics has the skater (a Skate 2 skater cannot touch them),
/// and come back when the switch goes off. Not tested in play.
#[test]
fn nsv_engine_traffic_stops_under_the_switch() {
    use crate::skate2_physics::Skate2Physics;
    use crate::skate2_physics::sim::{HostState, Link};
    let mut app = App::new();
    app.init_resource::<Traffic>()
        .init_resource::<Skate2Physics>()
        .insert_resource(Online(false))
        .insert_resource(map("New_San_Vanelona"))
        .add_systems(Update, gate);
    app.update();
    assert!(app.world().resource::<Traffic>().key.is_some(), "NSV traffic with the switch off");
    let car = app.world_mut().spawn(TrafficCar::default()).id();
    let (to_host, _from_engine) = std::sync::mpsc::channel();
    let (_to_engine, from_host) = std::sync::mpsc::channel();
    app.world_mut().resource_mut::<Skate2Physics>().state = HostState::Driving(Link::from_channels(to_host, from_host));
    app.update();
    assert!(app.world().get_entity(car).is_err(), "a car survived the switch");
    assert!(app.world().resource::<Traffic>().sim.is_none() && app.world().resource::<Traffic>().key.is_none());
    app.world_mut().resource_mut::<Skate2Physics>().state = HostState::Off;
    app.update();
    assert!(app.world().resource::<Traffic>().key.is_some(), "the cars come back when the switch goes off");
}

#[test]
fn road_network_matches_the_exported_tables() {
    let Some(root) = assets() else { return };
    for game in MAPS {
        let j = roads(&root, game);
        let net = Network::new(&j);
        let mut worst = 0.0f32;
        for p in net.roads.iter().flat_map(|r| &r.pieces) {
            assert!(p.up.y > 0.5, "{game:?} piece up {:?}", p.up);
            // The exported arc table is the chord sum of the same Hermite: confirms basis and tangent scale.
            let built = Curve::build(p.c.p0, p.c.p1, p.c.t0, p.c.t1, p.c.arc.len());
            for (a, b) in built.arc.iter().zip(&p.c.arc) {
                worst = worst.max((a - b).abs() / p.len.max(1.0));
            }
        }
        assert!(worst < 1e-3, "{game:?} arc tables differ by {worst}");
        // Every superroad ends in the road its intersection lists at entry slot a_n.
        let mut bad = 0;
        for (si, s) in net.sups.iter().enumerate() {
            let (Some(n), Some(&last)) = (s.ahead, s.roads.last()) else { continue };
            if j.roads[last].id != j.intersections[n].entries[s.slot].id {
                bad += 1;
                eprintln!("{game:?} superroad {} ends in {}", j.superroads[si].id, j.roads[last].id);
            }
            assert_eq!(j.roads[last].ends[1].id, j.intersections[n].id, "{game:?} last road's forward end");
        }
        assert_eq!(bad, 0);
    }
}

#[test]
fn downtown_skate_line_is_on_the_skate2_network() {
    let Some(root) = assets() else { return };
    let net = Network::new(&roads(&root, "New_San_Vanelona"));
    let (r, p) = net.nearest(DOWNTOWN_SKATE_LINE).expect("no road point within 10 m of Downtown Skate Line");
    let road = &net.roads[r];
    let pose = net.road_pose(road.sup, road.start + road.pieces[p].end - road.pieces[p].len, 0).unwrap();
    assert!(pose.pos.distance(DOWNTOWN_SKATE_LINE) < 20.0, "{:?}", pose.pos);
}

#[derive(Default, Debug)]
struct Stats {
    most: usize,
    spawned: u64,
    despawned: usize,
    entered: usize,
    on_red: usize,
    changes: usize,
    parked: usize,
    min_gap: f32,
    /// The car slot table's high-water mark.
    slots: usize,
}

/// 120 s at 60 Hz with the skater standing at `at`; checks lanes, lights, gaps and the despawn radius each tick.
fn run(root: &Path, game: &str, at: Vec3) -> Stats {
    let mut sim = Sim::load(root, dirs(game).unwrap()).unwrap();
    let mut st = Stats { min_gap: MAX, ..Default::default() };
    let mut changing = std::collections::HashSet::new();
    let mut parked = std::collections::HashSet::new();
    for _ in 0..120 * 60 {
        let before: HashMap<u64, On> = sim.cars.iter().flatten().map(|c| (c.serial, c.on)).collect();
        let old = sim.serial;
        sim.tick(at);
        let now: HashMap<u64, usize> = sim.cars.iter().enumerate().filter_map(|(i, c)| Some((c.as_ref()?.serial, i))).collect();
        st.despawned += before.keys().filter(|s| !now.contains_key(s)).count();
        // Item 29: spawn/despawn churn leaves nothing behind. Every lane-list, intersection-record and parking entry
        // belongs to a live car on that superroad / record (a despawned car's slot is reused, so a stale entry would
        // also steer the car that takes the slot over), and the slot table never outgrows the cars on the road.
        for (si, lanes) in sim.lists.iter().enumerate() {
            for &v in lanes.iter().flatten() {
                assert!(sim.car(v).is_some_and(|c| c.sup == si), "lane entry {v} on superroad {si} outlived its car");
            }
            for &(_, v) in &sim.parking[si] {
                assert!(sim.car(v).is_some_and(|c| c.sup == si), "parking entry {v} on superroad {si} outlived its car");
            }
        }
        for (n, recs) in sim.recs.iter().enumerate() {
            for (r, rec) in recs.iter().enumerate() {
                for &v in rec {
                    assert!(sim.car(v).is_some_and(|c| c.rec == Some((n, r))), "record ({n}, {r}) entry {v} outlived its car");
                }
            }
        }
        st.slots = st.slots.max(sim.cars.len());
        st.most = st.most.max(now.len());
        for (i, c) in sim.cars.iter().enumerate().filter_map(|(i, c)| Some((i, c.as_ref()?))) {
            match c.on {
                On::Road => {
                    let s = &sim.net.sups[c.sup];
                    assert!(c.d >= 0.0 && c.d <= s.len + 1.0, "off the superroad: d {} of {}", c.d, s.len);
                    let lane = c.change.as_ref().map_or(c.lane, |k| k.to);
                    assert!(lane < s.lanes || (lane == s.lanes && s.flags & 1 != 0), "lane {lane} of {}", s.lanes);
                    assert!(sim.lists[c.sup][lane].contains(&i), "car not in its lane list");
                    if c.state == State::Changing {
                        changing.insert(c.serial);
                    }
                    if c.state == State::Parked {
                        parked.insert(c.serial);
                    }
                }
                On::Inter => {
                    let (n, r) = c.rec.expect("on an intersection without a record");
                    assert!(sim.recs[n][r].contains(&i), "not registered on its record");
                    if before.get(&c.serial) == Some(&On::Road) {
                        st.entered += 1;
                        let ln = &sim.net.inters[n].lanes[r];
                        let right = ln.exit == (ln.entry + 3) % 4;
                        if sim.net.inters[n].lights && sim.lights[ln.entry].mode() == 0 && !right && !c.ignores_lights() {
                            st.on_red += 1;
                        }
                    }
                }
            }
            // The despawn pass ran this tick; cars it then spawned (ring + 10 m snap) may sit past it until the next.
            if sim.frame % 4 == 2 && c.serial <= old {
                assert!(c.pose.pos.distance(at) <= DESPAWN + 1.0, "car kept past the despawn radius");
            }
        }
        for (si, s) in sim.net.sups.iter().enumerate() {
            for lane in 0..s.lanes {
                let l = &sim.lists[si][lane];
                for w in l.windows(2) {
                    let (Some(a), Some(b)) = (sim.car(w[0]), sim.car(w[1])) else { continue };
                    if [a, b].iter().all(|c| c.on == On::Road && c.change.is_none()) {
                        st.min_gap = st.min_gap.min(b.d - a.d - 0.5 * (a.len + b.len));
                    }
                }
            }
        }
    }
    assert!(st.slots <= st.most + 1, "car slots {} grew past the most cars at once {}", st.slots, st.most);
    st.spawned = sim.serial;
    st.changes = changing.len();
    st.parked = parked.len();
    st
}

#[test]
fn headless_two_minutes_of_traffic() {
    let Some(root) = assets() else { return };
    let mut all = vec![("New_San_Vanelona", run(&root, "New_San_Vanelona", DOWNTOWN_SKATE_LINE))];
    // The others: stand at the busiest intersection (signalled where the map has lights; University has none).
    for game in &MAPS[1..] {
        let net = Network::new(&roads(&root, game));
        let hub = net.inters.iter().max_by_key(|i| (i.lights, i.lanes.len())).unwrap();
        all.push((game, run(&root, game, hub.lanes[0].c.p0)));
    }
    for (game, st) in &all {
        eprintln!("{game}: {st:?}");
        assert!(st.most > 0 && st.entered > 0 && st.despawned > 0, "{game}: {st:?}");
        assert_eq!(st.on_red, 0, "{game}: a car entered on red");
        assert!(st.min_gap > -0.5, "{game}: cars overlapped by {}", -st.min_gap);
    }
}

/// User 10-14, bug #6: a car's box starts at its chassis floor, clear of the road, so a board can roll under it.
#[test]
fn car_boxes_leave_room_under_the_chassis() {
    let Some(root) = assets() else { return };
    for game in MAPS {
        let sim = Sim::load(&root, dirs(game).unwrap()).unwrap();
        for m in &sim.models {
            let bottom = m.centre.y - m.half.y;
            assert!(bottom > 0.1, "{game} {}: box bottom {bottom}", m.path);
        }
    }
}

/// User 10-14, bug #5: Skate 1 paint follows vehiclematerial.psh, and every Skate 1 car gets a palette colour.
#[test]
fn skate1_cars_are_painted_from_their_palette() {
    // Paint (green under 0.035) takes r * red + b * blue; anything else is untouched.
    let mut px = [200, 5, 0, 255, 90, 90, 90, 255];
    colorize(&mut px, [0.0, 0.5, 1.0], [0.0, 0.0, 1.0]);
    assert_eq!(px, [0, 100, 200, 255, 90, 90, 90, 255]);
    let Some(root) = assets() else { return };
    let game = "Skate_1_San_Vanelona";
    let net = Network::new(&roads(&root, game));
    let hub = net.inters.iter().max_by_key(|i| (i.lights, i.lanes.len())).unwrap().lanes[0].c.p0;
    let mut sim = Sim::load(&root, dirs(game).unwrap()).unwrap();
    for _ in 0..60 {
        sim.tick(hub);
    }
    let cars: Vec<&Car> = sim.cars.iter().flatten().collect();
    assert!(!cars.is_empty() && cars.iter().all(|c| c.paint.is_some()), "{} cars", cars.len());
    let colours: std::collections::HashSet<[u32; 3]> = cars.iter().map(|c| c.paint.unwrap().0.map(f32::to_bits)).collect();
    assert!(colours.len() > 1, "every car the same colour");
}

/// Bug 10-17 #42: Skate 3 keeps at most 23 cars live (budget A), and they come in more than one colour.
#[test]
fn skate3_traffic_keeps_retails_23_cars_in_their_own_colours() {
    assert!(painted("vehicle_chassis") && !painted("vehicle_glass"));
    let Some(root) = assets() else { return };
    for game in ["DownTown", "Industrial", "University"] {
        let net = Network::new(&roads(&root, game));
        let hub = net.inters.iter().max_by_key(|i| i.lanes.len()).unwrap().lanes[0].c.p0;
        let mut sim = Sim::load(&root, dirs(game).unwrap()).unwrap();
        let mut most = 0;
        for _ in 0..600 {
            sim.tick(hub);
            most = most.max(sim.count());
        }
        assert!(most > 0 && most <= SKATE3_LIVE_CARS, "{game}: {most} cars");
        let colours: std::collections::HashSet<[u32; 3]> =
            sim.cars.iter().flatten().filter_map(|c| c.paint).map(|p| p.0.map(f32::to_bits)).collect();
        assert!(colours.len() > 1, "{game}: every car the same colour");
    }
}

/// User 10-14, bug #5: every blended Skate 1 car material is drawn opaque (`cutout`); Skate 3's glass stays blended.
#[test]
fn skate1_car_bodies_are_not_blended() {
    let Some(root) = assets() else { return };
    let mut checked = 0;
    for (dir, want) in [("private/traffic/skate1/vehicles", true), ("private/traffic/skate3/vehicles", false)] {
        for entry in std::fs::read_dir(root.join(dir)).unwrap().flatten() {
            let bytes = std::fs::read(entry.path()).unwrap();
            let n = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
            let json: Value = serde_json::from_slice(&bytes[20..20 + n]).unwrap();
            for m in json["materials"].as_array().into_iter().flatten() {
                if m["alphaMode"] == "BLEND" {
                    assert_eq!(cutout(m["name"].as_str().unwrap()), want, "{:?} {}", entry.path(), m["name"]);
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 0);
}

/// Not retail (user 10-14, bug #8): with a camera, no car appears or vanishes in its view cone.
#[test]
fn cars_spawn_and_despawn_out_of_view() {
    let Some(root) = assets() else { return };
    let mut sim = Sim::load(&root, dirs("New_San_Vanelona").unwrap()).unwrap();
    let eye = DOWNTOWN_SKATE_LINE + Vec3::Y * 2.0;
    let mut seen: HashMap<u64, Vec3> = HashMap::new();
    let (mut spawned, mut kept) = (0, 0);
    for f in 0..90 * 60 {
        // The camera turns slowly, so cars leave and enter the cone.
        let a = f as f32 * 0.0015;
        let fwd = Vec3::new(a.cos(), 0.0, a.sin());
        sim.view = Some((eye, fwd));
        let first = sim.serial == 0;
        sim.tick(DOWNTOWN_SKATE_LINE);
        let now: HashMap<u64, Vec3> = sim.cars.iter().flatten().map(|c| (c.serial, c.pose.pos)).collect();
        let cone = |p: Vec3| (p - eye).normalize_or_zero().dot(fwd) > VIEW_COS + 0.02;
        for (s, p) in &now {
            if !first && !seen.contains_key(s) {
                spawned += 1;
                assert!(!cone(*p), "car {s} spawned in view at {p}");
            }
        }
        for (s, p) in &seen {
            if !now.contains_key(s) && cone(*p) {
                kept += 1;
                assert!(p.distance(DOWNTOWN_SKATE_LINE) > DESPAWN_SEEN - 5.0, "car {s} vanished in view at {p}");
            }
        }
        seen = now;
    }
    assert!(spawned > 0, "the ring still spawns ({kept} in-view despawns past {DESPAWN_SEEN} m)");
}

// ---------------------------------------------------------------- Mission B: the skater meets the cars

/// A Skate 2 street with cars on it: a minute at Downtown Skate Line.
fn busy(root: &Path) -> Sim {
    let mut sim = Sim::load(root, dirs("New_San_Vanelona").unwrap()).unwrap();
    for _ in 0..60 * 60 {
        sim.tick(DOWNTOWN_SKATE_LINE);
    }
    sim
}

fn car(sim: &Sim, serial: u64) -> &Car {
    sim.cars.iter().flatten().find(|c| c.serial == serial).expect("the car is gone")
}

fn car_mut(sim: &mut Sim, serial: u64) -> &mut Car {
    sim.cars.iter_mut().flatten().find(|c| c.serial == serial).expect("the car is gone")
}

#[test]
fn every_car_is_a_box_the_skater_can_hit() {
    let Some(root) = assets() else { return };
    let sim = busy(&root);
    let shapes = sim.shapes();
    assert!(!shapes.is_empty() && shapes.len() >= sim.count());
    let mut length: HashMap<u64, f32> = HashMap::new();
    for (serial, s) in &shapes {
        let c = car(&sim, *serial);
        assert_eq!(Vec3::from_array(s.position), c.pose.pos);
        let q = Quat::from_array(s.rotation);
        assert!((q * Vec3::Z).distance(c.pose.fwd) < 1e-4 && (q * Vec3::X).distance(c.pose.left) < 1e-4, "the car's basis");
        assert!((Vec3::from_array(s.velocity) - c.pose.fwd * c.speed).length() < 1e-4, "it moves with the car");
        // The model's own boxes: its width, standing on the road, end to end along its length.
        assert!((s.half[0] - c.hw).abs() < 1e-3, "{:?} vs {}", s.half, c.hw);
        assert!(s.half[1] > 0.2 && s.offset[1] > 0.0, "{:?} {:?}", s.half, s.offset);
        *length.entry(*serial).or_default() += 2.0 * s.half[2];
    }
    for (serial, l) in length {
        let c = car(&sim, serial);
        assert!((l - c.len).abs() < 1e-3, "boxes {l} m long vs the car's {}", c.len);
    }
}

/// Bug v12 #30: the hood and trunk boxes stand at their drawn height, not the roof's (one roof-high box put an
/// invisible wall up to 0.6 m over the hood).
#[test]
fn car_hoods_stand_lower_than_their_cabins() {
    let Some(root) = assets() else { return };
    let sim = Sim::load(&root, dirs("DownTown").unwrap()).unwrap();
    let sedan = sim.models.iter().find(|m| m.path.ends_with("/fourdoor_sedan_01.glb")).expect("the sedan is in the census");
    let top = |(c, h): &(Vec3, Vec3)| c.y + h.y;
    let roof = sedan.boxes.iter().map(top).fold(f32::MIN, f32::max);
    // +Z is the car's front: its last box is the hood's.
    let hood = sedan.boxes.iter().max_by(|a, b| a.0.z.total_cmp(&b.0.z)).map(top).unwrap();
    assert!(sedan.boxes.len() >= 3, "{:?}", sedan.boxes);
    assert!((roof - (sedan.centre.y + sedan.half.y)).abs() < 0.02, "the cabin box reaches the roof: {roof}");
    assert!(hood < roof - 0.4, "the hood box tops out at {hood}, the roof at {roof}");
}

#[test]
fn a_hit_on_the_nose_brakes_the_car_to_a_stop_and_marks_it_for_the_hall_of_meat() {
    let Some(root) = assets() else { return };
    let mut sim = busy(&root);
    let serial = sim.cars.iter().flatten().find(|c| c.speed > 3.0 && c.state == State::Following).expect("a moving car").serial;
    let (pos, fwd, len) = (car(&sim, serial).pose.pos, car(&sim, serial).pose.fwd, car(&sim, serial).len);
    // The tail: marked, no brake.
    sim.hit(serial, pos - fwd * 0.5 * len, Vec3::ZERO);
    assert_eq!((car(&sim, serial).hit, car(&sim, serial).f4385 & 0x20), (1, 0));
    // 0x82b0a6b8: a contact ahead of the car's centre sets 4385&0x20; 0x82b0c210 then takes all the speed off.
    sim.hit(serial, pos + fwd * 0.5 * len, Vec3::ZERO);
    assert_eq!(car(&sim, serial).f4385 & 0x20, 0x20);
    let speed = car(&sim, serial).speed;
    sim.tick(DOWNTOWN_SKATE_LINE);
    let c = car(&sim, serial);
    assert!(c.accel <= -0.9 * speed && c.speed < speed, "accel {} speed {} -> {}", c.accel, speed, c.speed);
    // 0x823ad3f0: the hit mask is per frame.
    sim.clear_hits();
    assert_eq!(car(&sim, serial).hit, 0);
    // 0x82b0c740: stopped, the brake request clears.
    for _ in 0..60 * 10 {
        sim.tick(DOWNTOWN_SKATE_LINE);
        if car(&sim, serial).speed == 0.0 {
            break;
        }
    }
    assert_eq!(car(&sim, serial).speed, 0.0, "the car stopped");
    assert_eq!(car(&sim, serial).f4385 & 0x20, 0);
}

#[test]
fn hitting_a_parked_car_sets_its_alarm_off() {
    let Some(root) = assets() else { return };
    let mut sim = busy(&root);
    let serial = sim.cars.iter().flatten().next().unwrap().serial;
    let at = car(&sim, serial).pose.pos;
    car_mut(&mut sim, serial).f3420 |= 0x80;
    car_mut(&mut sim, serial).speed = 0.0;
    car_mut(&mut sim, serial).t3696 = 3.0;
    // Profile 543475921FD9E04A (0.1): a touch slower than it leaves the car be.
    sim.hit(serial, at, Vec3::new(0.05, 0.0, 0.0));
    assert_eq!(car(&sim, serial).f3420 & 0x10, 0);
    sim.hit(serial, at, Vec3::new(2.0, 0.0, 0.0));
    let c = car(&sim, serial);
    assert_eq!(c.f3420 & 0x10, 0x10, "alarm");
    assert_eq!((c.t3696, c.t3700), (0.0, 0.0));
    // A moving car has no alarm.
    let moving = sim.cars.iter().flatten().find(|c| c.f3420 & 0x80 == 0).unwrap().serial;
    let at = car(&sim, moving).pose.pos;
    sim.hit(moving, at, Vec3::new(5.0, 0.0, 0.0));
    assert_eq!(car(&sim, moving).f3420 & 0x10, 0);
}

#[test]
fn the_hall_of_meat_sees_the_cars_hit_this_frame_within_eight_metres() {
    let Some(root) = assets() else { return };
    let mut sim = busy(&root);
    let mut cars = sim.cars.iter().flatten().map(|c| (c.serial, c.pose.pos, c.hom));
    let (near, at, hom) = cars.next().unwrap();
    let far = cars.find(|c| c.1.distance(at) > 9.0).expect("a car 9 m away").0;
    sim.skater = at + Vec3::X;
    assert_eq!(sim.hom_hits(), [None; 8]);
    sim.hit(near, at, Vec3::ZERO);
    sim.hit(far, at, Vec3::ZERO);
    assert_eq!(sim.hom_hits()[0], Some((near, hom)));
    assert_eq!(sim.hom_hits()[1..], [None; 7]);
    // Skate 2's two Trucks are HoM types 12 and 13 (hom.json prices them at 2000); the rest score vehicle_points.
    let types: std::collections::BTreeSet<i32> = sim.census.kinds.values().map(|k| k.hom).collect();
    assert!(types.contains(&12) && types.contains(&13), "{types:?}");
}

#[test]
fn online_no_car_has_a_body_and_none_is_hit() {
    let Some(root) = assets() else { return };
    let mut traffic = Traffic { sim: Some(busy(&root)), ..default() };
    let serial = traffic.sim.as_ref().unwrap().cars.iter().flatten().next().unwrap().serial;
    assert!(!car_bodies(&traffic, false, false).is_empty(), "offline the cars collide");
    assert!(car_bodies(&traffic, true, false).is_empty(), "online: no car bodies");
    assert!(car_bodies(&traffic, false, true).is_empty(), "replay: no car bodies");
    // Going online drops the simulation (gate): no bodies, no hits for the Hall of Meat.
    let mut app = App::new();
    app.insert_resource(Online(false)).insert_resource(map("New_San_Vanelona")).init_resource::<Traffic>().add_systems(Update, gate);
    app.update();
    let at = traffic.sim.as_ref().unwrap().cars.iter().flatten().next().unwrap().pose.pos;
    traffic.sim.as_mut().unwrap().skater = at;
    traffic.sim.as_mut().unwrap().hit(serial, at, Vec3::ZERO);
    traffic.key = app.world().resource::<Traffic>().key.clone();
    assert!(traffic.hom_hits()[0].is_some());
    assert!(traffic.voices().is_some_and(|v| !v.is_empty()), "offline the cars are heard");
    app.insert_resource(traffic);
    app.world_mut().resource_mut::<Online>().0 = true;
    app.update();
    let traffic = app.world().resource::<Traffic>();
    assert!(traffic.sim.is_none() && car_bodies(traffic, true, false).is_empty() && car_bodies(traffic, false, false).is_empty());
    assert_eq!(traffic.hom_hits(), [None; 8]);
    assert_eq!(traffic.voices(), None, "online: no car sounds");
}

#[test]
fn the_traffic_audio_hears_the_ten_nearest_cars_within_forty_metres() {
    let Some(root) = assets() else { return };
    let mut sim = busy(&root);
    let (serial, at) = sim.cars.iter().flatten().map(|c| (c.serial, c.pose.pos)).next().unwrap();
    sim.skater = at;
    car_mut(&mut sim, serial).status = 6;
    car_mut(&mut sim, serial).f3420 |= 8;
    let voices = sim.voices();
    let near = sim.cars.iter().flatten().filter(|c| c.pose.pos.distance(at) <= 40.0).count();
    assert_eq!(voices.len(), near.min(10));
    assert_eq!((voices[0].handle, voices[0].flags, voices[0].distance), (sim.handles + serial as u32, 6 << 28 | 1 << 27, 0.0));
    // Another simulation (the next map) hands out other handles.
    assert_ne!(busy(&root).handles, sim.handles);
    assert_eq!(voices[0].position, at.to_array());
    assert!(voices.windows(2).all(|w| w[0].distance <= w[1].distance));
    // The census gives every kind its audio row (old_traffic.py), which picks the engine bank.
    assert!(sim.census.kinds.values().all(|k| k.tune.audio != 0), "a car kind without an audio row");
    assert!(voices.iter().all(|v| v.key != 0));
}

/// Perf 10-12 measurement, per traffic map: Sim::load, the first tick (the 6000-try fill), then the mean cost of a
/// 60 Hz step's traffic work (tick; shapes for the solve; voices for the audio) over 2 minutes with the skater riding
/// along with a car, so the census despawns behind and spawns ahead as on a skate.
/// `cargo test ... traffic_tick_cost -- --ignored --nocapture`
#[test]
#[ignore = "requires the installation; timing"]
fn traffic_tick_cost() {
    use std::time::{Duration, Instant};
    let root = assets().expect("SKATE3_ASSET_ROOT with every traffic map");
    for game in MAPS {
        let net = Network::new(&roads(&root, game));
        let hub = net.inters.iter().max_by_key(|i| (i.lights, i.lanes.len())).unwrap().lanes[0].c.p0;
        let t = Instant::now();
        let mut sim = Sim::load(&root, dirs(game).unwrap()).unwrap();
        let load = t.elapsed();
        let t = Instant::now();
        sim.tick(hub);
        let fill = t.elapsed();
        let (mut tick, mut shapes, mut voices, mut most, mut heard) = (Duration::ZERO, Duration::ZERO, Duration::ZERO, 0, 0);
        let mut follow = None;
        const TICKS: u32 = 120 * 60;
        for _ in 0..TICKS {
            let at = match follow.and_then(|s| sim.cars.iter().flatten().find(|c| c.serial == s)) {
                Some(c) => c.pose.pos,
                None => {
                    let c = sim.cars.iter().flatten().next();
                    follow = c.map(|c| c.serial);
                    c.map_or(hub, |c| c.pose.pos)
                }
            };
            let t = Instant::now();
            sim.tick(at);
            tick += t.elapsed();
            let t = Instant::now();
            std::hint::black_box(sim.shapes());
            shapes += t.elapsed();
            let t = Instant::now();
            heard = heard.max(std::hint::black_box(sim.voices()).len());
            voices += t.elapsed();
            most = most.max(sim.count());
        }
        let us = |d: Duration| d.as_secs_f64() * 1e6 / f64::from(TICKS);
        eprintln!(
            "SKATE_TRAFFIC_COST {game}: load {:.1} ms, fill {:.2} ms, per step: tick {:.1} us, shapes {:.2} us, voices {:.2} us; most cars {most}, heard {heard}, spawned {}",
            load.as_secs_f64() * 1e3,
            fill.as_secs_f64() * 1e3,
            us(tick),
            us(shapes),
            us(voices),
            sim.serial
        );
    }
}

/// The paint copies of a car material that unloaded (its model's last car left, or the map changed) are released.
#[test]
fn paint_copies_of_an_unloaded_model_are_released() {
    use bevy::ecs::system::RunSystemOnce;
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default(), bevy::scene::ScenePlugin))
        .init_asset::<StandardMaterial>()
        .init_asset::<Image>();
    let world = app.world_mut();
    let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
    let [live, gone, a, b] = std::array::from_fn(|_| materials.add(StandardMaterial::default()));
    materials.remove(gone.id());
    let mut traffic = Traffic::default();
    traffic.painted.insert((live.id(), [0; 6]), (a, 0));
    traffic.painted.insert((gone.id(), [0; 6]), (b, 0));
    world.insert_resource(traffic);
    world.run_system_once(show).unwrap();
    let painted = &world.resource::<Traffic>().painted;
    assert_eq!(painted.keys().collect::<Vec<_>>(), [&(live.id(), [0; 6])]);
}

fn paint_assets() -> (Assets<StandardMaterial>, Assets<Image>, Handle<Image>) {
    let mut images = Assets::<Image>::default();
    let atlas = images.add(Image::new(
        bevy::render::render_resource::Extent3d { width: 2, height: 1, depth_or_array_layers: 1 },
        bevy::render::render_resource::TextureDimension::D2,
        vec![200, 5, 0, 128, 90, 90, 90, 255],
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    ));
    (Assets::default(), images, atlas)
}

/// Perf 11-01: Skate 1's body and light materials share one atlas, and a chassis's shine mask has no colour, so each
/// painted texture is made once: the second material in the same colours, or the chassis in new ones, adds no image.
#[test]
fn materials_sharing_an_atlas_share_its_painted_copy() {
    let (mut materials, mut images, atlas) = paint_assets();
    let source = |materials: &mut Assets<StandardMaterial>| {
        materials.add(StandardMaterial { base_color_texture: Some(atlas.clone()), ..default() })
    };
    let [body, light, chassis] = std::array::from_fn(|_| source(&mut materials));
    let mut cache = HashMap::new();
    let (red, blue) = ([0.0, 0.5, 1.0], [0.0, 0.0, 1.0]);
    let texture = |materials: &Assets<StandardMaterial>, h: &Handle<StandardMaterial>| {
        let m = materials.get(h).unwrap();
        (m.base_color_texture.as_ref().map(Handle::id), m.metallic_roughness_texture.as_ref().map(Handle::id))
    };

    let a = paint_copy(&mut cache, &body, false, (red, blue), 1, &mut materials, &mut images).unwrap();
    assert_eq!(images.len(), 2, "one painted atlas");
    let painted = images.get(texture(&materials, &a).0.unwrap()).unwrap().data.clone().unwrap();
    assert_eq!(painted[..4], [0, 100, 200, 128], "the paint texel takes the colours");
    let b = paint_copy(&mut cache, &light, false, (red, blue), 1, &mut materials, &mut images).unwrap();
    assert_eq!((images.len(), texture(&materials, &b).0), (2, texture(&materials, &a).0), "the light shares it");
    let c = paint_copy(&mut cache, &light, false, (blue, red), 2, &mut materials, &mut images).unwrap();
    assert!(images.len() == 3 && texture(&materials, &c).0 != texture(&materials, &a).0, "other colours paint anew");
    let again = paint_copy(&mut cache, &body, false, (red, blue), 3, &mut materials, &mut images);
    assert_eq!(again, Some(a.clone()), "cached");

    let d = paint_copy(&mut cache, &chassis, true, (red, blue), 4, &mut materials, &mut images).unwrap();
    assert_eq!(images.len(), 4, "the body's paint and a new shine mask");
    let e = paint_copy(&mut cache, &chassis, true, (blue, red), 5, &mut materials, &mut images).unwrap();
    assert_eq!(images.len(), 4, "both made already");
    assert_eq!(texture(&materials, &e).1, texture(&materials, &d).1);
    assert!(texture(&materials, &d).1.is_some() && texture(&materials, &d).1 != texture(&materials, &a).1);
}

/// Perf 11-01: past PAINT_CACHE the copies worn longest ago go; a copy a car wore again stays.
#[test]
fn the_paint_cache_lets_the_least_recently_worn_copies_go() {
    let (mut materials, mut images, atlas) = paint_assets();
    let body = materials.add(StandardMaterial { base_color_texture: Some(atlas), ..default() });
    let mut cache = HashMap::new();
    let colours = |i: usize| ([i as f32 / 100.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let first = paint_copy(&mut cache, &body, false, colours(0), 0, &mut materials, &mut images).unwrap();
    for i in 1..PAINT_CACHE {
        paint_copy(&mut cache, &body, false, colours(i), i as u64, &mut materials, &mut images).unwrap();
    }
    let again = paint_copy(&mut cache, &body, false, colours(0), 100, &mut materials, &mut images);
    assert_eq!(again, Some(first), "worn again");
    paint_copy(&mut cache, &body, false, colours(PAINT_CACHE), 101, &mut materials, &mut images).unwrap();
    assert_eq!(cache.len(), PAINT_CACHE);
    let worn: Vec<u64> = cache.values().map(|(_, t)| *t).collect();
    assert!(!worn.contains(&1) && worn.contains(&100) && worn.contains(&101), "car 1's copy went first");
}

/// Perf 11-01: a car model stays loaded after its last car leaves (no reload and repaint when the next one comes), and
/// is let go on a map change.
#[test]
fn a_car_model_stays_loaded_until_the_map_changes() {
    use bevy::ecs::system::RunSystemOnce;
    let Some(root) = assets() else { return };
    let game = "Skate_1_San_Vanelona";
    let net = Network::new(&roads(&root, game));
    let hub = net.inters.iter().max_by_key(|i| (i.lights, i.lanes.len())).unwrap().lanes[0].c.p0;
    let mut sim = Sim::load(&root, dirs(game).unwrap()).unwrap();
    for _ in 0..60 {
        sim.tick(hub);
    }
    let models: std::collections::HashSet<usize> = sim.cars.iter().flatten().map(|c| c.model).collect();
    assert!(!models.is_empty());
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default(), bevy::scene::ScenePlugin))
        .init_asset::<StandardMaterial>()
        .init_asset::<Image>()
        .insert_resource(Online(false))
        .insert_resource(map(game));
    app.world_mut().insert_resource(Traffic { sim: Some(sim), key: Some((game.into(), 0)), ..default() });
    app.world_mut().run_system_once(show).unwrap();
    let held = |w: &World| -> HashMap<usize, AssetId<Scene>> {
        w.resource::<Traffic>().scenes.iter().map(|(m, h)| (*m, h.id())).collect()
    };
    let before = held(app.world());
    assert_eq!(before.keys().copied().collect::<std::collections::HashSet<_>>(), models);

    app.world_mut().resource_mut::<Traffic>().sim.as_mut().unwrap().cars.iter_mut().for_each(|c| *c = None);
    app.world_mut().run_system_once(show).unwrap();
    assert!(app.world().resource::<Traffic>().shown.is_empty(), "every car left");
    assert_eq!(held(app.world()), before, "their models stay loaded");

    app.world_mut().resource_mut::<CurrentMap>().name = "Test world".into();
    app.world_mut().run_system_once(gate).unwrap();
    assert!(app.world().resource::<Traffic>().scenes.is_empty(), "a map change lets them go");
}

#[test]
fn traffic_at_0_5_halves_the_census_count() {
    assert_eq!(census_cap(12, 0.5, usize::MAX), 6);
    assert_eq!(census_cap(7, 0.5, usize::MAX), 3, "floored");
    assert_eq!(census_cap(40, 0.5, SKATE3_LIVE_CARS), SKATE3_LIVE_CARS.min(20), "still within budget A");
}

#[test]
fn zero_clears_the_cars_and_the_career_ignores_the_setting() {
    use crate::skate3_career::freeplay::{Settings, density};
    use crate::skate3_career::mode::GameMode;
    let none = Settings { traffic: 0.0, ..Settings::default() };
    assert_eq!(census_cap(12, density(GameMode::ProfileFreeSkate, &none), usize::MAX), 0);
    assert_eq!(density(GameMode::SinglePlayer, &none), 1.0, "the career ignores Free Play's setting");
}

/// Bug 10-25 #24: a session marker set on a car's roof takes that car away on the return; the others stay.
#[test]
fn a_marker_return_takes_away_the_car_under_it() {
    let Some(root) = assets() else { return };
    let mut sim = busy(&root);
    let count = sim.count();
    let (serial, roof) = {
        let c = sim.cars.iter().flatten().next().expect("a car");
        let m = &sim.models[c.model];
        (c.serial, c.pose.pos + c.pose.up * (m.centre.y + m.half.y + 0.3) + c.pose.fwd * m.centre.z)
    };
    // Well clear of every car: nothing goes.
    sim.clear_at(roof + Vec3::Y * 50.0);
    assert_eq!(sim.count(), count);
    sim.clear_at(roof);
    assert!(sim.cars.iter().flatten().all(|c| c.serial != serial), "the car under the marker is gone");
    assert!(sim.count() >= count.saturating_sub(2) && sim.count() < count, "only what stands there goes");
}

/// v12 list 2: a Skate 3 chassis shines only where its atlas alpha marks it (vehicle_defaultPS scales every specular
/// and environment term by alpha squared); the rest is matte.
#[test]
fn chassis_shine_follows_the_atlas_alpha_squared() {
    let size = bevy::render::render_resource::Extent3d { width: 3, height: 1, depth_or_array_layers: 1 };
    let diffuse = Image::new(
        size,
        bevy::render::render_resource::TextureDimension::D2,
        vec![9, 9, 9, 0, 9, 9, 9, 128, 9, 9, 9, 255],
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    );
    let mask = shine_mask(&diffuse).unwrap();
    let roughness: Vec<u8> = mask.data.unwrap().chunks_exact(4).map(|px| px[1]).collect();
    let half = 1. + (SHINE_ROUGHNESS - 1.) * (128f32 / 255.).powi(2);
    assert_eq!(roughness, [255, (half * 255.).round() as u8, (SHINE_ROUGHNESS * 255.).round() as u8]);
    assert!(shine_masked("vehicle_chassis") && !shine_masked("vehicleMaterialClass"), "Skate 1's alpha is a cutout");
    assert_eq!(finish("vehicle_chassis"), None, "no uniform gloss on the body");
}

/// v12 list 2: riding onto a car's top bails; its sides are ordinary vehicle contacts.
#[test]
fn a_contact_on_a_cars_roof_is_on_top_and_one_on_its_side_is_not() {
    let Some(root) = assets() else { return };
    let sim = busy(&root);
    let c = sim.cars.iter().flatten().next().expect("a car");
    let m = &sim.models[c.model];
    let (centre, half) = m.boxes.iter().copied().max_by(|a, b| (a.0.y + a.1.y).total_cmp(&(b.0.y + b.1.y))).unwrap();
    let at = |x: f32, y: f32, z: f32| c.pose.pos + c.pose.left * x + c.pose.up * y + c.pose.fwd * z;
    assert!(sim.on_top(c.serial, at(0., centre.y + half.y, centre.z)), "the roof");
    assert!(!sim.on_top(c.serial, at(centre.x + half.x, centre.y, centre.z)), "a door");
    assert!(!sim.on_top(c.serial, at(0., centre.y + half.y, centre.z + 50.)), "off the car's length");
}
