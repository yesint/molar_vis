//! Browser trajectory streaming: parse frames **incrementally** from an in-memory
//! buffer (the file picker hands us the chosen file's bytes as a `Vec<u8>`).
//!
//! wasm has no background threads, so instead of the native reader thread we keep
//! a [`FileHandler`] over a `Cursor` alive across frames and read a *batch* per
//! `ui()` call ([`TrajStream::next_batch`]) — frames stream into the trajectory and
//! the UI never blocks. The frame walk mirrors the native loader (skip to the
//! wanted frame, read it, check the atom count), honoring [`LoadOptions`].

use molar::prelude::*;

use crate::trajectory::LoadOptions;

/// An in-progress incremental trajectory read from an in-memory buffer.
pub struct TrajStream {
    fh: FileHandler,
    opts: LoadOptions,
    /// Next file-frame index to read (advances by `stride`).
    target: usize,
    expected_atoms: usize,
    /// Set once the end of the requested range / file is reached.
    pub done: bool,
}

impl TrajStream {
    /// Open a stream over `bytes` (format taken from `name`'s extension). Frames
    /// must match `expected_atoms` (the molecule's atom count).
    pub fn new(
        name: &str,
        bytes: Vec<u8>,
        opts: LoadOptions,
        expected_atoms: usize,
    ) -> Result<Self, String> {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let fh = FileHandler::from_reader(&ext, std::io::Cursor::new(bytes))
            .map_err(|e| format!("can't open {name}: {}", chain(&e)))?;
        Ok(Self {
            fh,
            target: opts.from,
            opts,
            expected_atoms,
            done: false,
        })
    }

    /// Read up to `max` wanted frames; returns them (possibly empty) and sets
    /// `done` when the requested range / end of file is reached. On a read error
    /// `done` is set and the error returned.
    pub fn next_batch(&mut self, max: usize) -> Result<Vec<State>, String> {
        let stride = self.opts.stride.max(1);
        let mut out = Vec::new();
        while out.len() < max {
            if matches!(self.opts.to, Some(to) if self.target > to) {
                self.done = true;
                break;
            }
            match self.fh.skip_to_frame(self.target) {
                Ok(()) => {}
                Err(e) if is_eof(&e) => {
                    self.done = true;
                    break;
                }
                Err(e) => {
                    self.done = true;
                    return Err(chain(&e));
                }
            }
            match self.fh.read_state() {
                Ok(state) => {
                    if state.coords.len() != self.expected_atoms {
                        self.done = true;
                        return Err(format!(
                            "frame {} has {} atoms but the molecule has {}",
                            self.target,
                            state.coords.len(),
                            self.expected_atoms
                        ));
                    }
                    out.push(state);
                }
                Err(e) if is_eof(&e) => {
                    self.done = true;
                    break;
                }
                Err(e) => {
                    self.done = true;
                    return Err(chain(&e));
                }
            }
            self.target += stride;
        }
        Ok(out)
    }
}

fn is_eof(e: &FileIoError) -> bool {
    matches!(e.kind(), FileFormatError::Eof)
}

/// Flatten an error's `source()` chain into one message (molar nests
/// `FileIoError` → `FileFormatError` → handler error).
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
