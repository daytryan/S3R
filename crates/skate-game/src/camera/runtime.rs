//! Complete normal camera owner: subject publications, stock graph, CameraMan,
//! swept world queries and retained query history. The simulation calls advance
//! once per completed physical/animation publication; rendering only presents it.
use super::{
    collision::CameraCollision,
    graph::CameraGraph,
    graph_subject::CameraGraphEnvironment,
    settings, shake_data,
    shot_data::StockShots,
    subject::{CameraSubjectSnapshot, SubjectPublisher},
    trajectory::{CameraTrajectory, TrajectoryResult},
};
use bevy::prelude::Resource;
use skate_core::{
    camera::{
        CameraFrame, CameraMan, CompassSettings, ManagerSettings, MovingObstacleProvider,
        ShakeSamples, SimulationRateRequest, SlowMotionSettings,
    },
    physics::board_world::BoardWorld,
    point_graph::PointGraph,
};
use skate_data::collections::Collections;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// Camera Style choices: the data folder under `private/camera-styles`, written by
/// skate2port/old_cameras.py ("" = the stock Skate 3 camera).
pub(crate) const STYLES: [&str; 3] = ["", "skate2", "skate1"];
/// ponytail: one process-wide selection that every load reads (startup, map loads,
/// the settings row); thread it through Config if styles ever differ per player.
pub(crate) static STYLE: AtomicUsize = AtomicUsize::new(0);
/// Settings > Controls > Camera Shake; off zeroes the rig's shake multipliers.
pub(crate) static SHAKE: AtomicBool = AtomicBool::new(true);

#[derive(Resource)]
pub(crate) struct CameraRuntime {
    /// The STYLES index this runtime was loaded for.
    pub style: usize,
    manager: CameraMan,
    subject: SubjectPublisher,
    graph: CameraGraph,
    shots: StockShots,
    settings: ManagerSettings,
    compass_settings: CompassSettings,
    shakes: [ShakeSamples; 2],
    trajectories: [TrajectoryResult; 3],
    pub frame: Option<CameraFrame>,
    /// Immutable physical subject publication consumed by the camera graph.
    /// Rendering may inspect this snapshot for diagnostics without rebuilding
    /// subject fields from mutable skater state.
    pub latest_subject: Option<CameraSubjectSnapshot>,
    /// Native message order, including End followed by Begin in one graph tick.
    /// The simulation schedule drains these after advance.
    pub simulation_rate_requests: Vec<SimulationRateRequest>,
    /// The manager's grind-ledge side bytes (+37..+39), physics/camera_output.rs.
    pub ledge: crate::physics::camera_output::LedgeSide,
}

#[cfg(test)]
#[path = "tests/runtime.rs"]
mod tests;
impl CameraRuntime {
    /// The selected STYLE; the stock camera stands in if that style's data fails to
    /// load, keeping `style` as requested so the setting does not retry every frame.
    pub fn load(root: &Path) -> Result<Self, String> {
        let style = STYLE.load(Ordering::Relaxed).min(STYLES.len() - 1);
        Self::load_style(root, style).or_else(|error| {
            if style == 0 {
                return Err(error);
            }
            bevy::log::warn!(
                "Camera style {} unavailable, using the Skate 3 camera: {error}",
                STYLES[style]
            );
            Ok(Self {
                style,
                ..Self::load_style(root, 0)?
            })
        })
    }

    fn load_style(root: &Path, style: usize) -> Result<Self, String> {
        // Another game's camera: its shots, rig settings and graph over the stock data.
        let folder = (style != 0).then(|| root.join("private/camera-styles").join(STYLES[style]));
        // perf 10-18: the stock camera reads the shared vault; an overlay needs its own copy.
        let data = match &folder {
            Some(folder) => {
                let mut data = Collections::load(root)?;
                data.overlay(&folder.join("collections.json"))?;
                std::sync::Arc::new(data)
            }
            None => Collections::shared(root)?,
        };
        let values = data
            .words::<32>("slowmotion_controller", "default", "timescale")?
            .map(f32::from_bits);
        let slow_motion = SlowMotionSettings {
            timescale: PointGraph {
                x: core::array::from_fn(|i| values[i]),
                y: core::array::from_fn(|i| values[i + 16]),
            },
            fps_at_scale_one: data.float("slowmotion_controller", "default", "fps_at_scale_one")?,
        };
        let graph = CameraGraph::load(
            &folder
                .as_deref()
                .unwrap_or(&root.join("private/stock/data/script/camera"))
                .join("Default_cameragraph.stategraph"),
            slow_motion,
        )?;
        let samples = |name: &str| -> Result<ShakeSamples, String> {
            let path = root.join("private/stock/data/camera").join(name);
            shake_data::parse(
                &std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?,
            )
        };
        Ok(Self {
            style,
            manager: CameraMan::new(),
            subject: SubjectPublisher::new(),
            graph,
            shots: StockShots::from_collections(&data)?,
            settings: settings::manager_settings(&data)?,
            compass_settings: settings::compass_settings(&data)?,
            shakes: [samples("1.shk")?, samples("2.shk")?],
            trajectories: core::array::from_fn(|_| TrajectoryResult::new()),
            frame: None,
            latest_subject: None,
            simulation_rate_requests: Vec::new(),
            ledge: Default::default(),
        })
    }

    /// Rebuild from another style's data mid-game (the Camera Angle setting).
    pub fn switch_style(&mut self, style: usize, root: &Path) {
        STYLE.store(style, Ordering::Relaxed);
        match Self::load(root) {
            Ok(mut loaded) => {
                bevy::log::info!("Camera style: {:?}", STYLES[loaded.style]);
                // Off-board input steers relative to the last completed camera frame
                // every tick; keep the one on screen until the new camera completes one.
                loaded.frame = self.frame;
                // A fresh rig starts at the world origin's heading, in front of the
                // skater, and swings round for seconds. Reset it the way a teleport or
                // respawn does, so it starts in place behind the skater.
                loaded.manager.state.request_reset();
                *self = loaded;
            }
            Err(error) => {
                bevy::log::error!(
                    "Camera style {style} failed to load, keeping the current camera: {error}"
                );
                self.style = style; // do not retry every frame
            }
        }
    }

    /// The rig starts again behind the skater, as at a teleport or respawn (Party Play's handover).
    /// Every caller hands the camera to another subject, whose tick count is its own (run12: a Skate 1 AI rider's 0
    /// against the player's 11544 failed the monotonic check and stopped the rider), so both tick checks start over:
    /// this one and the subject publisher's (run14: "subject publication is not monotonic: previous=3671, current=0"
    /// stopped Shingo again). INFERRED: the publisher's damped COM, anchors and compass start over with the rig.
    pub fn reset_rig(&mut self) {
        self.manager.state.request_reset();
        self.latest_subject = None;
        self.subject = SubjectPublisher::new();
    }

    /// A minimized window is 0x0, so width / height is NaN; that reached the
    /// field of view and ended the game with "non-finite frame". Keep the last
    /// real shape until the window has one again.
    pub fn set_aspect_ratio(&mut self, value: f32) {
        if value.is_finite() && value > 0. {
            self.manager.state.aspect_ratio = value;
        }
    }
    pub fn selected_shot(&self) -> &str {
        &self.manager.shots.current().name
    }

    pub fn advance(
        &mut self,
        dt: f32,
        snapshot: CameraSubjectSnapshot,
        world: &BoardWorld,
        query_gravity: [f32; 4],
        environment: &CameraGraphEnvironment,
        moving: &mut impl MovingObstacleProvider,
    ) -> Result<CameraFrame, String> {
        if let Some(previous) = self.latest_subject.as_ref()
            && snapshot.tick <= previous.tick
        {
            return Err(format!(
                "Camera received non-monotonic subject tick: previous={}, current={}",
                previous.tick, snapshot.tick,
            ));
        }
        self.latest_subject = Some(snapshot);
        let mut subject = self
            .subject
            .publish(snapshot, &self.manager, self.compass_settings)?;
        let mut settings = self.settings;
        if !SHAKE.load(Ordering::Relaxed) {
            let shake = &mut settings.shake;
            (shake.translation_multiplier, shake.rotation_multiplier, shake.dutch_multiplier) = (0., 0., 0.);
        }
        self.manager.prepare(&subject, settings);
        let requests = self.graph.update(
            dt,
            &mut self.manager,
            &subject,
            snapshot.graph,
            environment,
            &self.shots,
        )?;
        self.simulation_rate_requests.extend(requests);
        let [a, b, c] = &mut self.trajectories;
        let mut trajectories = [a, b, c].map(|result| CameraTrajectory {
            world,
            gravity: query_gravity,
            result,
        });
        let mut collision = CameraCollision::new(world);
        let frame = self.manager.update(
            dt,
            &mut subject,
            settings,
            [&self.shakes[0], &self.shakes[1]],
            &mut trajectories,
            moving,
            &mut collision,
        )?;
        if let Some(error) = collision.error {
            return Err(error);
        }
        if let Some(error) = self.trajectories.iter().find_map(|v| v.error.as_ref()) {
            return Err(error.clone());
        }
        if !frame.position.iter().all(|v| v.is_finite())
            || !frame.basis.columns.iter().flatten().all(|v| v.is_finite())
            || !frame.field_of_view_degrees.is_finite()
        {
            return Err(format!(
                "Normal gameplay camera produced a non-finite frame: frame={frame:?}; lens_length={:?}; aspect_ratio={:?}; subject_transform={:?}; skeleton_root={:?}; ground_normal={:?}; launch_position={:?}; landing_position={:?}",
                self.manager.shots.interpolated.lens_length,
                self.manager.state.aspect_ratio,
                subject.rig.transform,
                subject.rig.skeleton_root,
                subject.ground_normal,
                subject.launch_position,
                subject.landing_position,
            ));
        }
        self.frame = Some(frame);
        Ok(frame)
    }
}
