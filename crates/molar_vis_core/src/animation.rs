//! Camera motion evaluated at explicit time, shared by preview and offline export.
use crate::camera::Camera;
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    Linear,
    #[default]
    Smooth,
}
impl Easing {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "linear" => Ok(Self::Linear),
            "smooth" => Ok(Self::Smooth),
            _ => Err("easing must be 'linear' or 'smooth'".into()),
        }
    }
    fn sample(self, t: f32) -> f32 {
        match self {
            Self::Linear => t,
            Self::Smooth => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// Pose only: changing views does not replace lighting or projection settings.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ViewPose {
    pub target: Vec3,
    pub orientation: Quat,
    pub distance: f32,
    pub scene_radius: f32,
    pub pivot: Option<Vec3>,
    pub clip_offset: Vec3,
}
impl ViewPose {
    pub(crate) fn capture(c: &Camera) -> Self {
        Self {
            target: c.target,
            orientation: c.orientation,
            distance: c.distance,
            scene_radius: c.scene_radius,
            pivot: c.pivot,
            clip_offset: c.clip_offset,
        }
    }
    fn valid(&self) -> bool {
        self.target.is_finite()
            && self.orientation.is_finite()
            && self.orientation.is_normalized()
            && self.distance.is_finite()
            && self.distance > 0.0
            && self.scene_radius.is_finite()
            && self.scene_radius > 0.0
            && self.clip_offset.is_finite()
            && self.pivot.is_none_or(|v| v.is_finite())
    }
    pub(crate) fn apply(self, c: &mut Camera) {
        c.target = self.target;
        c.orientation = self.orientation;
        c.distance = self.distance;
        c.scene_radius = self.scene_radius;
        c.pivot = self.pivot;
        c.clip_offset = self.clip_offset;
    }
}

fn one() -> f32 {
    1.0
}
fn resolution() -> usize {
    256
}

/// IDs in Focus/Unobstructed refer to stable molecule/representation handles.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ViewAction {
    Move {
        #[serde(default)]
        yaw: f32,
        #[serde(default)]
        pitch: f32,
        #[serde(default)]
        roll: f32,
        #[serde(default = "one")]
        zoom: f32,
        #[serde(default)]
        pan: [f32; 2],
    },
    To {
        view: ViewPose,
    },
    Focus {
        mol: u64,
        selection: String,
    },
    Unobstructed {
        targets: Vec<(u64, u64)>,
        #[serde(default = "one")]
        zoom_out: f32,
        #[serde(default = "resolution")]
        resolution: usize,
    },
    /// Switch the shown group member and approach its unobstructed view.
    GroupUnobstructed {
        group: u64,
        member: usize,
        #[serde(default = "one")]
        zoom_out: f32,
        #[serde(default = "resolution")]
        resolution: usize,
    },
    Reset,
    Hold,
}
impl ViewAction {
    pub fn rotation(yaw: f32, pitch: f32) -> Self {
        Self::Move {
            yaw,
            pitch,
            roll: 0.0,
            zoom: 1.0,
            pan: [0.0; 2],
        }
    }
    pub fn zoom(factor: f32) -> Self {
        Self::Move {
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            zoom: factor,
            pan: [0.0; 2],
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        let valid = match self {
            Self::Move {
                yaw,
                pitch,
                roll,
                zoom,
                pan,
            } => {
                [*yaw, *pitch, *roll, *zoom, pan[0], pan[1]]
                    .iter()
                    .all(|v| v.is_finite())
                    && *zoom > 0.0
            }
            Self::To { view } => view.valid(),
            Self::Unobstructed {
                targets,
                zoom_out,
                resolution,
            } => {
                !targets.is_empty()
                    && zoom_out.is_finite()
                    && *zoom_out > 0.0
                    && (1..=65536).contains(resolution)
            }
            Self::Focus { selection, .. } => !selection.trim().is_empty(),
            Self::GroupUnobstructed {
                zoom_out,
                resolution,
                ..
            } => zoom_out.is_finite() && *zoom_out > 0.0 && (1..=65536).contains(resolution),
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err("invalid camera motion parameters".into())
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewStep {
    pub action: ViewAction,
    pub duration: f64,
    #[serde(default)]
    pub easing: Easing,
}
impl ViewStep {
    pub fn validate(&self) -> Result<(), String> {
        self.action.validate()?;
        if !self.duration.is_finite() || self.duration < 0.0 {
            return Err("duration must be finite and non-negative".into());
        }
        Ok(())
    }
}

/// Sample loaded trajectory frames at an explicit rate, clamping at `end`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrajectoryTrack {
    pub mol: u64,
    pub start: usize,
    pub end: usize,
    pub fps: f64,
}
impl TrajectoryTrack {
    pub(crate) fn frame(&self, time: f64) -> usize {
        let offset =
            ((time.max(0.0) * self.fps).floor() as usize).min(self.start.abs_diff(self.end));
        if self.end >= self.start {
            self.start + offset
        } else {
            self.start - offset
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Movie {
    pub steps: Vec<ViewStep>,
    #[serde(default)]
    pub trajectories: Vec<TrajectoryTrack>,
}
impl Movie {
    pub fn from_json(json: &str) -> Result<Self, String> {
        let movie: Self = serde_json::from_str(json).map_err(|e| crate::errors::error_chain(&e))?;
        movie.validate()?;
        Ok(movie)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.steps.is_empty() {
            return Err("movie must contain at least one step".into());
        }
        for s in &self.steps {
            s.validate()?;
        }
        if !self.duration().is_finite() {
            return Err("movie duration is too large".into());
        }
        for t in &self.trajectories {
            if !t.fps.is_finite() || t.fps <= 0.0 {
                return Err("trajectory fps must be finite and positive".into());
            }
        }
        Ok(())
    }
    pub fn duration(&self) -> f64 {
        self.steps.iter().map(|s| s.duration).sum()
    }
}

#[derive(Clone)]
pub(crate) struct Segment {
    pub start: Camera,
    pub end: Camera,
    pub step: ViewStep,
}
impl Segment {
    pub fn sample(&self, t: f64) -> Camera {
        let u = if self.step.duration == 0.0 {
            1.0
        } else {
            (t / self.step.duration).clamp(0.0, 1.0) as f32
        };
        if u <= 0.0 {
            return self.start;
        }
        if u >= 1.0 {
            return self.end;
        }
        let u = self.step.easing.sample(u);
        let mut c = self.start;
        if let ViewAction::Move {
            yaw,
            pitch,
            roll,
            zoom,
            pan,
        } = self.step.action
        {
            // Evaluate from the original pose. This preserves full turns and pivot arcs.
            c.rotate_deg(yaw * u, pitch * u);
            c.roll_deg(roll * u);
            c.zoom_by(zoom.powf(u));
            c.pan_fraction(pan[0] * u, pan[1] * u);
        } else {
            c.orientation = self
                .start
                .orientation
                .slerp(self.end.orientation, u)
                .normalize();
            c.target = self.start.target.lerp(self.end.target, u);
            c.distance = (self.start.distance.ln() * (1.0 - u) + self.end.distance.ln() * u).exp();
            // Keep the wider slab until framing has finished; no sudden intermediate clipping.
            c.scene_radius = self.start.scene_radius.max(self.end.scene_radius);
            let clip_center = (self.start.target + self.start.clip_offset)
                .lerp(self.end.target + self.end.clip_offset, u);
            c.clip_offset = clip_center - c.target;
        }
        c
    }
}

#[derive(Clone)]
pub(crate) struct Timeline {
    pub segments: Vec<Segment>,
    pub duration: f64,
}
impl Timeline {
    pub fn sample(&self, mut time: f64) -> Camera {
        for segment in &self.segments {
            if time < segment.step.duration {
                return segment.sample(time);
            }
            time -= segment.step.duration;
        }
        self.segments.last().expect("validated nonempty movie").end
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Running,
    Completed,
    Cancelled,
    Failed,
}
#[derive(Clone, Debug, Serialize)]
pub struct TaskProgress {
    pub state: TaskState,
    pub completed: u64,
    pub total: u64,
    pub error: Option<String>,
}
struct Shared {
    cancel: AtomicBool,
    progress: Mutex<TaskProgress>,
    #[cfg(not(target_arch = "wasm32"))]
    changed: std::sync::Condvar,
}
/// A thread-safe handle. Waiting never blocks the viewer's event loop.
#[derive(Clone)]
pub struct AnimationHandle(Arc<Shared>);
impl Default for AnimationHandle {
    fn default() -> Self {
        Self::new(0)
    }
}
impl AnimationHandle {
    pub(crate) fn new(total: u64) -> Self {
        Self(Arc::new(Shared {
            cancel: AtomicBool::new(false),
            progress: Mutex::new(TaskProgress {
                state: TaskState::Running,
                completed: 0,
                total,
                error: None,
            }),
            #[cfg(not(target_arch = "wasm32"))]
            changed: std::sync::Condvar::new(),
        }))
    }
    pub fn cancel(&self) {
        let progress = self.0.progress.lock().unwrap();
        if matches!(progress.state, TaskState::Running) {
            self.0.cancel.store(true, Ordering::Release);
        }
    }
    pub(crate) fn cancelled(&self) -> bool {
        self.0.cancel.load(Ordering::Acquire)
    }
    pub fn progress(&self) -> TaskProgress {
        self.0.progress.lock().unwrap().clone()
    }
    pub fn is_done(&self) -> bool {
        !matches!(self.progress().state, TaskState::Running)
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn advance(&self, completed: u64) {
        self.0.progress.lock().unwrap().completed = completed;
    }
    pub(crate) fn finish(&self, result: Result<(), String>) {
        let mut p = self.0.progress.lock().unwrap();
        if !matches!(p.state, TaskState::Running) {
            return;
        }
        p.state = if self.cancelled() {
            TaskState::Cancelled
        } else if result.is_ok() {
            TaskState::Completed
        } else {
            TaskState::Failed
        };
        p.error = result.err();
        #[cfg(not(target_arch = "wasm32"))]
        self.0.changed.notify_all();
    }
    /// Serialize cancellation with the final file replacement on a worker thread.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn complete_when_ready(
        &self,
        poll: impl FnOnce() -> Option<Result<(), String>>,
    ) -> Option<Result<(), String>> {
        let mut progress = self.0.progress.lock().unwrap();
        if self.cancelled() {
            return Some(Err("render cancelled".into()));
        }
        let result = poll()?;
        progress.state = if result.is_ok() {
            TaskState::Completed
        } else {
            TaskState::Failed
        };
        progress.error = result.clone().err();
        self.0.changed.notify_all();
        Some(result)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn wait(&self) -> Result<(), String> {
        let mut p = self.0.progress.lock().unwrap();
        while matches!(p.state, TaskState::Running) {
            p = self.0.changed.wait(p).unwrap();
        }
        match p.state {
            TaskState::Completed => Ok(()),
            TaskState::Cancelled => Err("animation cancelled".into()),
            _ => Err(p.error.clone().unwrap_or_else(|| "animation failed".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_turn_and_pivot_are_preserved() {
        let mut start = Camera::default();
        start.target = Vec3::X;
        start.pivot = Some(Vec3::ZERO);
        let step = ViewStep {
            action: ViewAction::rotation(360.0, 0.0),
            duration: 2.0,
            easing: Easing::Linear,
        };
        let mut end = start;
        end.rotate_deg(360.0, 0.0);
        let s = Segment { start, end, step };
        let middle = s.sample(1.0);
        assert!((middle.target + Vec3::X).length() < 1e-5);
        assert!((middle.target + middle.clip_offset - start.target).length() < 1e-5);
        assert!((s.sample(2.0).target - start.target).length() < 1e-5);
    }
    #[test]
    fn zoom_is_multiplicative_and_sampling_is_stateless() {
        let start = Camera::default();
        let mut end = start;
        end.zoom_by(4.0);
        let s = Segment {
            start,
            end,
            step: ViewStep {
                action: ViewAction::zoom(4.0),
                duration: 2.0,
                easing: Easing::Linear,
            },
        };
        assert!((s.sample(1.0).distance - start.distance / 2.0).abs() < 1e-5);
        s.sample(1.9);
        assert_eq!(s.sample(0.5), s.sample(0.5));
        assert_eq!(s.sample(2.0), end);
    }
    #[test]
    fn tracks_and_invalid_inputs() {
        let t = TrajectoryTrack {
            mol: 1,
            start: 10,
            end: 3,
            fps: 2.0,
        };
        assert_eq!(t.frame(1.5), 7);
        assert_eq!(t.frame(100.0), 3);
        assert!(ViewAction::zoom(0.0).validate().is_err());
        assert!(ViewAction::rotation(f32::NAN, 0.0).validate().is_err());
        assert!(Movie::from_json(r#"{"steps":[]}"#).is_err());
    }
}

#[cfg(test)]
mod timeline_tests {
    use super::*;
    #[test]
    fn saved_view_takes_short_rotation_and_exact_endpoint() {
        let mut start = Camera::default();
        start.orientation = Quat::IDENTITY;
        let mut end = start;
        end.orientation = Quat::from_rotation_z(270.0f32.to_radians());
        end.distance = start.distance / 4.0;
        let segment = Segment {
            start,
            end,
            step: ViewStep {
                action: ViewAction::To {
                    view: ViewPose::capture(&end),
                },
                duration: 2.0,
                easing: Easing::Linear,
            },
        };
        let midpoint = segment.sample(1.0);
        assert!(
            (midpoint.orientation * Vec3::X
                - Quat::from_rotation_z(-45.0f32.to_radians()) * Vec3::X)
                .length()
                < 1e-5
        );
        assert!((midpoint.distance - start.distance / 2.0).abs() < 1e-5);
        assert_eq!(segment.sample(0.0), start);
        assert_eq!(segment.sample(2.0), end);
    }
    #[test]
    fn sequence_boundaries_holds_and_zero_duration_steps() {
        let a = Camera::default();
        let mut b = a;
        b.zoom_by(2.0);
        let mut c = b;
        c.rotate_deg(90.0, 0.0);
        let timeline = Timeline {
            duration: 2.0,
            segments: vec![
                Segment {
                    start: a,
                    end: b,
                    step: ViewStep {
                        action: ViewAction::zoom(2.0),
                        duration: 0.0,
                        easing: Easing::Smooth,
                    },
                },
                Segment {
                    start: b,
                    end: b,
                    step: ViewStep {
                        action: ViewAction::Hold,
                        duration: 1.0,
                        easing: Easing::Linear,
                    },
                },
                Segment {
                    start: b,
                    end: c,
                    step: ViewStep {
                        action: ViewAction::rotation(90.0, 0.0),
                        duration: 1.0,
                        easing: Easing::Smooth,
                    },
                },
            ],
        };
        assert_eq!(timeline.sample(0.0), b);
        assert_eq!(timeline.sample(1.0), b);
        assert_eq!(timeline.sample(2.0), c);
        assert_eq!(timeline.sample(100.0), c);
        assert_eq!(timeline.sample(0.5), b);
    }
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn publication_and_cancellation_have_one_terminal_result() {
        let completed = AnimationHandle::default();
        assert!(
            completed
                .complete_when_ready(|| Some(Ok(())))
                .unwrap()
                .is_ok()
        );
        completed.cancel();
        assert!(!completed.cancelled());
        assert!(matches!(completed.progress().state, TaskState::Completed));
        let cancelled = AnimationHandle::default();
        cancelled.cancel();
        let published = std::sync::atomic::AtomicBool::new(false);
        assert!(
            cancelled
                .complete_when_ready(|| {
                    published.store(true, Ordering::Relaxed);
                    Some(Ok(()))
                })
                .unwrap()
                .is_err()
        );
        assert!(!published.load(Ordering::Relaxed));
        cancelled.finish(Ok(()));
        assert!(matches!(cancelled.progress().state, TaskState::Cancelled));
    }

    #[test]
    fn completion_and_cancellation_wake_waiters() {
        let h = AnimationHandle::default();
        h.cancel();
        h.finish(Ok(()));
        assert!(h.is_done());
        assert!(matches!(h.progress().state, TaskState::Cancelled));
        h.finish(Err("late failure".into()));
        assert!(matches!(h.progress().state, TaskState::Cancelled));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let handle = AnimationHandle::default();
            let waiter = handle.clone();
            let worker = std::thread::spawn(move || waiter.wait());
            handle.finish(Ok(()));
            assert!(worker.join().unwrap().is_ok());
        }
    }
}
