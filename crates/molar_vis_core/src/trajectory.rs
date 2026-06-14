//! In-memory MD trajectory storage and VMD-style playback state for a molecule.
//!
//! A [`Trajectory`] holds all loaded frames (`molar::State`s) plus the playback
//! cursor and animation parameters. Frame 0 is the molecule's static structure
//! coordinates (seeded by `scene::Molecule::seed_frame0`); trajectory frames
//! loaded from a file are appended after it, and multiple loads concatenate.
//!
//! This module is pure data + logic with no IO or platform dependencies, so it
//! compiles unchanged for `wasm32`. The actual frame reading lives in the
//! native-only `data::traj_loader`; the wasm path feeds frames in via the same
//! [`LoadMsg`] channel from a worker.

use molar::prelude::State;

/// How playback behaves at the ends of the trajectory.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LoopMode {
    /// Stop when an end is reached.
    Once,
    /// Wrap around to the other end and keep playing.
    #[default]
    Loop,
}

/// A molecule's loaded frames plus its playback state.
pub struct Trajectory {
    /// All frames, in display order. Empty until the first load seeds frame 0
    /// with the structure coordinates (the static structure otherwise lives only
    /// in the `System`, with no [`Trajectory`] entry).
    pub frames: Vec<State>,
    /// Index of the currently displayed frame.
    pub current: usize,
    /// Whether playback is currently running.
    pub playing: bool,
    /// Playback direction: `+1` forward, `-1` backward.
    pub dir: i32,
    pub loop_mode: LoopMode,
    /// Target playback rate, frames per second.
    pub speed_fps: f32,
    /// Wall-clock seconds accumulated toward the next frame advance.
    accum: f64,
}

impl Default for Trajectory {
    fn default() -> Self {
        Self {
            frames: Vec::new(),
            current: 0,
            playing: false,
            dir: 1,
            loop_mode: LoopMode::Loop,
            speed_fps: 15.0,
            accum: 0.0,
        }
    }
}

impl Trajectory {
    pub fn n_frames(&self) -> usize {
        self.frames.len()
    }

    /// Whether playback controls should be shown (more than one frame).
    pub fn has_playback(&self) -> bool {
        self.frames.len() > 1
    }

    /// Simulation time of the current frame, if any.
    pub fn current_time(&self) -> Option<f32> {
        self.frames.get(self.current).map(|s| s.time)
    }

    /// Set the current frame, clamped to a valid index. Returns whether it moved.
    pub fn set_current(&mut self, i: usize) -> bool {
        if self.frames.is_empty() {
            return false;
        }
        let new = i.min(self.frames.len() - 1);
        let moved = new != self.current;
        self.current = new;
        moved
    }

    /// Step by `delta` frames (e.g. `-1`/`+1`), honoring [`LoopMode`] at the ends.
    /// Returns whether the current frame moved.
    pub fn step(&mut self, delta: i32) -> bool {
        if self.frames.len() < 2 {
            return false;
        }
        let n = self.frames.len() as i32;
        let next = self.current as i32 + delta;
        let target = match self.loop_mode {
            LoopMode::Loop => next.rem_euclid(n),
            LoopMode::Once => next.clamp(0, n - 1),
        } as usize;
        let moved = target != self.current;
        self.current = target;
        moved
    }

    /// Start or stop playback (resets the time accumulator).
    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
        self.accum = 0.0;
    }

    /// Advance playback by `dt` seconds of wall-clock time. Returns whether the
    /// displayed frame changed (so the caller re-applies the state and re-renders).
    /// In [`LoopMode::Once`], playback stops when an end is reached.
    pub fn tick(&mut self, dt: f64) -> bool {
        if !self.playing || self.frames.len() < 2 || self.speed_fps <= 0.0 {
            return false;
        }
        self.accum += dt;
        let period = 1.0 / self.speed_fps as f64;
        let n = self.frames.len() as i32;
        let stepv = self.dir.signum();
        let mut moved = false;
        while self.accum >= period {
            self.accum -= period;
            let next = self.current as i32 + stepv;
            match self.loop_mode {
                LoopMode::Loop => {
                    self.current = next.rem_euclid(n) as usize;
                    moved = true;
                }
                LoopMode::Once => {
                    if next < 0 || next >= n {
                        self.playing = false;
                        self.accum = 0.0;
                        break;
                    }
                    self.current = next as usize;
                    moved = true;
                }
            }
        }
        moved
    }
}

/// Which frames to keep when reading a trajectory file.
#[derive(Clone, Copy, Debug)]
pub struct LoadOptions {
    /// First file frame to keep (0-based).
    pub from: usize,
    /// Last file frame to keep, inclusive; `None` = read to end of file.
    pub to: Option<usize>,
    /// Keep every `stride`-th frame (clamped to `>= 1`; `1` = every frame).
    pub stride: usize,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self { from: 0, to: None, stride: 1 }
    }
}

/// Synchronous vs background frame reading (the load dialog's choice).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LoadMode {
    /// Read everything before returning (blocks the UI).
    #[default]
    Sync,
    /// Read on a background worker, streaming frames to the UI as they arrive.
    Async,
}

/// A message from a background trajectory loader to the UI thread.
pub enum LoadMsg {
    /// One kept frame.
    Frame(State),
    /// Reading finished normally (end of file / requested range).
    Done,
    /// Reading aborted with this message.
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn traj(n: usize) -> Trajectory {
        let mut t = Trajectory::default();
        t.frames = (0..n).map(|_| State::default()).collect();
        t
    }

    #[test]
    fn set_current_clamps() {
        let mut t = traj(3);
        assert!(t.set_current(2));
        assert_eq!(t.current, 2);
        assert!(!t.set_current(99)); // clamped to 2, no move
        assert_eq!(t.current, 2);
    }

    #[test]
    fn step_loops_and_clamps() {
        let mut t = traj(3);
        t.loop_mode = LoopMode::Loop;
        t.current = 2;
        t.step(1);
        assert_eq!(t.current, 0); // wrapped
        t.loop_mode = LoopMode::Once;
        t.current = 2;
        assert!(!t.step(1)); // clamped at end, no move
        assert_eq!(t.current, 2);
    }

    #[test]
    fn tick_advances_at_speed_and_stops_in_once() {
        let mut t = traj(3);
        t.speed_fps = 10.0; // one frame every 0.1 s
        t.loop_mode = LoopMode::Once;
        t.set_playing(true);
        assert!(!t.tick(0.05)); // not enough accumulated
        assert_eq!(t.current, 0);
        assert!(t.tick(0.06)); // crosses 0.1 s -> advance one
        assert_eq!(t.current, 1);
        assert!(t.tick(0.2)); // advances to 2, then end stops playback
        assert_eq!(t.current, 2);
        assert!(!t.playing);
    }

}
