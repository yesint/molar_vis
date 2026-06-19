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

use molar::prelude::{Pos, State};

/// How playback behaves at the ends of the trajectory.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
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
    /// Frames to advance per playback step (skip during playing; `1` = every frame).
    pub play_step: usize,
    /// UI: zoom the scrub slider to a ±25-frame window around the current frame
    /// (only meaningful for long trajectories). Transient view state, not serialized.
    pub slider_zoom: bool,
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
            play_step: 1,
            slider_zoom: false,
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

    /// Delete the inclusive frame range `[from, to]` (clamped to valid indices),
    /// then clamp `current` into the remaining range. Returns how many frames were
    /// removed.
    pub fn delete_range(&mut self, from: usize, to: usize) -> usize {
        let n = self.frames.len();
        if n == 0 || from > to || from >= n {
            return 0;
        }
        let to = to.min(n - 1);
        let removed = to - from + 1;
        self.frames.drain(from..=to);
        self.clamp_current();
        removed
    }

    /// Keep every `stride`-th frame (0, stride, 2·stride, …) and drop the rest,
    /// then clamp `current`. `stride <= 1` is a no-op. Returns how many frames were
    /// removed.
    pub fn decimate(&mut self, stride: usize) -> usize {
        if stride <= 1 {
            return 0;
        }
        let before = self.frames.len();
        let mut i = 0usize;
        self.frames.retain(|_| {
            let keep = i % stride == 0;
            i += 1;
            keep
        });
        self.clamp_current();
        before - self.frames.len()
    }

    /// Clamp `current` to a valid index after the frame set shrank.
    fn clamp_current(&mut self) {
        self.current = self.current.min(self.frames.len().saturating_sub(1));
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
        // Advance by `play_step` frames per tick (skip frames during playback).
        let stepv = self.dir.signum() * self.play_step.max(1) as i32;
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
                        // A multi-frame step can overshoot — land on the end, then stop.
                        let end = if next < 0 { 0 } else { (n - 1) as usize };
                        moved |= self.current != end;
                        self.current = end;
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

    /// Trajectory smoothing: a transient blend of the frames around `current`,
    /// returned as an owned `State` (coords only — the box is taken as-is from the
    /// current frame). **Computed at render time and dropped after the geometry
    /// build; nothing is stored.** `window` is the odd smoothing window (`1` = off);
    /// adjacent frames are weighted by **Savitzky–Golay** (local-polynomial)
    /// coefficients, and the window is shrunk symmetrically toward the trajectory
    /// ends so it degrades gracefully (no smoothing exactly at the first/last frame).
    /// Returns `None` when there's nothing to smooth (`window ≤ 1`, `< 3` frames, or
    /// a hard end) — callers then render the raw current frame.
    pub fn smoothed_state(&self, window: u32) -> Option<State> {
        let n = self.frames.len();
        if window <= 1 || n < 3 {
            return None;
        }
        let half = (window as usize - 1) / 2;
        // Symmetric shrink at the ends: keep the window centred but smaller.
        let m = half.min(self.current).min(n - 1 - self.current);
        if m == 0 {
            return None;
        }
        let coeffs = sg_center_coeffs(m);
        let n_atoms = self.frames[self.current].coords.len();
        let mut coords = vec![Pos::origin(); n_atoms];
        for (k, &c) in coeffs.iter().enumerate() {
            let frame = &self.frames[self.current - m + k];
            for a in 0..n_atoms {
                coords[a].coords += frame.coords[a].coords * c;
            }
        }
        Some(State {
            coords,
            pbox: self.frames[self.current].pbox.clone(),
            ..Default::default()
        })
    }
}

/// Savitzky–Golay center-point smoothing coefficients for a symmetric window of
/// half-width `m` (window `2m+1`, offsets `−m..=m`), summing to 1. Degree-2
/// (quadratic) for `m ≥ 2`; a plain boxcar average for `m == 1` (a quadratic
/// through 3 points interpolates exactly = no smoothing, so drop to degree 0).
fn sg_center_coeffs(m: usize) -> Vec<f32> {
    let w = 2 * m + 1;
    if m <= 1 {
        return vec![1.0 / w as f32; w];
    }
    let mf = m as f32;
    let denom = (2.0 * mf - 1.0) * (2.0 * mf + 1.0) * (2.0 * mf + 3.0);
    (0..w)
        .map(|k| {
            let i = k as f32 - mf;
            3.0 * (3.0 * mf * mf + 3.0 * mf - 1.0 - 5.0 * i * i) / denom
        })
        .collect()
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
    fn sg_coeffs_sum_to_one_and_match_known() {
        // Boxcar at window 3.
        let c3 = sg_center_coeffs(1);
        assert!((c3.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(c3.iter().all(|&w| (w - 1.0 / 3.0).abs() < 1e-5));
        // Classic 5-point quadratic Savitzky–Golay: (-3, 12, 17, 12, -3) / 35.
        let c5 = sg_center_coeffs(2);
        assert!((c5.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        let want = [-3.0, 12.0, 17.0, 12.0, -3.0].map(|v: f32| v / 35.0);
        for (a, b) in c5.iter().zip(want.iter()) {
            assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        }
    }

    #[test]
    fn smoothed_state_blends_frames() {
        use molar::prelude::Pos;
        // One atom whose x is 0, 10, 2 across three frames (frame 1 is "noisy").
        let mut t = traj(3);
        for (i, &x) in [0.0f32, 10.0, 2.0].iter().enumerate() {
            t.frames[i].coords = vec![Pos::new(x, 0.0, 0.0)];
        }
        t.current = 1;
        // Window 1 → no smoothing.
        assert!(t.smoothed_state(1).is_none());
        // Window 3 at the middle frame → boxcar average (0+10+2)/3 = 4.
        let s = t.smoothed_state(3).expect("smoothed");
        assert!((s.coords[0].x - 4.0).abs() < 1e-4, "{}", s.coords[0].x);
        // At a hard end the window shrinks to nothing → no smoothing.
        t.current = 0;
        assert!(t.smoothed_state(3).is_none());
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
    fn delete_range_removes_and_clamps_current() {
        let mut t = traj(10);
        t.current = 8;
        assert_eq!(t.delete_range(3, 5), 3); // remove indices 3,4,5
        assert_eq!(t.frames.len(), 7);
        assert_eq!(t.current, 6); // clamped to new last index
        // Out-of-range / inverted ranges are no-ops.
        assert_eq!(t.delete_range(100, 200), 0);
        assert_eq!(t.delete_range(5, 2), 0);
        assert_eq!(t.frames.len(), 7);
        // Deleting everything reverts to no frames (falls back to the structure).
        assert_eq!(t.delete_range(0, 6), 7);
        assert_eq!(t.frames.len(), 0);
        assert_eq!(t.current, 0);
    }

    #[test]
    fn decimate_keeps_every_nth() {
        let mut t = traj(10);
        t.current = 9;
        assert_eq!(t.decimate(3), 6); // keep 0,3,6,9 → drop 6
        assert_eq!(t.frames.len(), 4);
        assert_eq!(t.current, 3);
        assert_eq!(t.decimate(1), 0); // no-op
        assert_eq!(t.frames.len(), 4);
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
