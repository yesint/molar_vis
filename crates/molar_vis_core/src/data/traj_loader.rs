//! Native trajectory frame reading via molar's `FileHandler`.
//!
//! Reads coordinate frames (xtc/trr/dcd/multi-frame pdb/gro/…) into memory,
//! applying the load range/stride and validating that every frame matches the
//! molecule's atom count. Two entry points: a blocking [`read_frames_sync`] and
//! a background [`spawn_async`] that streams frames over a channel.
//!
//! Native-only: opens files from a path and uses `std::thread` for the async
//! path. The wasm build feeds frames through the same [`LoadMsg`] channel from a
//! Web Worker instead (see the wasm trajectory source).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use molar::prelude::*;

use crate::trajectory::{LoadMsg, LoadOptions};

/// Render an error with its full `source()` chain (molar nests
/// `FileIoError` → `FileFormatError` → handler error), so the message is useful.
fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(e) = src {
        s.push_str(": ");
        s.push_str(&e.to_string());
        src = e.source();
    }
    s
}

/// Walk the wanted frames — `from`, `from+stride`, … up to `to` — handing each
/// to `sink`. Between wanted frames we use [`FileHandler::skip_to_frame`], which
/// seeks past skipped frames for random-access formats (xtc/trr/dcd) and falls
/// back to serial reads for the rest — so we don't decompress frames we discard.
/// `sink` returns `false` to stop early (e.g. an async receiver was dropped).
fn walk_frames(
    fh: &mut FileHandler,
    opts: &LoadOptions,
    expected_atoms: usize,
    mut sink: impl FnMut(State) -> bool,
) -> Result<(), String> {
    let stride = opts.stride.max(1);
    let mut target = opts.from;
    loop {
        if matches!(opts.to, Some(to) if target > to) {
            break;
        }
        // Position at `target`. Past end -> clean EOF -> stop.
        match fh.skip_to_frame(target) {
            Ok(()) => {}
            Err(e) if is_eof(&e) => break,
            Err(e) => return Err(chain(&e)),
        }
        match fh.read_state() {
            Ok(state) => {
                check_atoms(&state, target, expected_atoms)?;
                if !sink(state) {
                    return Ok(());
                }
            }
            Err(e) if is_eof(&e) => break,
            Err(e) => return Err(chain(&e)),
        }
        target += stride;
    }
    Ok(())
}

fn is_eof(e: &FileIoError) -> bool {
    matches!(e.kind(), FileFormatError::Eof)
}

/// Read every wanted frame into a `Vec`, blocking until done.
pub fn read_frames_sync(
    path: &Path,
    opts: &LoadOptions,
    expected_atoms: usize,
) -> Result<Vec<State>, String> {
    let mut fh = FileHandler::open(path).map_err(|e| chain(&e))?;
    let mut frames = Vec::new();
    walk_frames(&mut fh, opts, expected_atoms, |st| {
        frames.push(st);
        true
    })?;
    if frames.is_empty() {
        return Err("no frames matched the selected range".to_string());
    }
    Ok(frames)
}

/// Read wanted frames on a background thread, sending each over the returned
/// channel as [`LoadMsg::Frame`], then [`LoadMsg::Done`] (or [`LoadMsg::Error`]).
/// The thread exits early if the receiver is dropped.
pub fn spawn_async(
    path: PathBuf,
    opts: LoadOptions,
    expected_atoms: usize,
) -> Receiver<LoadMsg> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut fh = match FileHandler::open(&path) {
            Ok(fh) => fh,
            Err(e) => {
                let _ = tx.send(LoadMsg::Error(chain(&e)));
                return;
            }
        };
        let frame_tx = tx.clone();
        // `sink` returns false once the receiver is gone, ending the walk.
        let res = walk_frames(&mut fh, &opts, expected_atoms, move |st| {
            frame_tx.send(LoadMsg::Frame(st)).is_ok()
        });
        let _ = match res {
            Ok(()) => tx.send(LoadMsg::Done),
            Err(e) => tx.send(LoadMsg::Error(e)),
        };
    });
    rx
}

fn check_atoms(state: &State, frame: usize, expected: usize) -> Result<(), String> {
    if state.coords.len() != expected {
        Err(format!(
            "frame {frame} has {} atoms but the molecule has {expected}",
            state.coords.len()
        ))
    } else {
        Ok(())
    }
}
