//! Scripting console UI + command execution glue.
use super::*;

impl App {

    // --- Scripting console (Rhai) — see `script.rs`. ---

    /// Draw the console as a resizable bottom panel (when open) and run any submitted
    /// line. Added to the panel layout before the viewport, so the 3D view fills the
    /// space above it (not a floating window).
    pub(super) fn draw_console(&mut self, ui: &mut egui::Ui) {
        if !self.console_open {
            return;
        }
        if let Some(src) = crate::script::console::show(ui, &mut self.console_open, &mut self.console) {
            self.run_script(&src);
            ui.ctx().request_repaint();
        }
    }

    /// Evaluate `source` as a Rhai script: echo it, route `print`/`list` output and
    /// errors into the console, apply the produced commands on the UI thread, then
    /// record one undo checkpoint. (Called by the console and the
    /// `MOLAR_VIS_DEBUG_SCRIPT` hook.)
    pub(super) fn run_script(&mut self, source: &str) {
        use crate::script::{ConsoleLine, LineKind};
        self.console.lines.push(ConsoleLine { kind: LineKind::Input, text: source.to_string() });
        let summary = self.scene_summary();
        // Evaluate in the persistent REPL session so `let` bindings survive across
        // console lines (`let m = mol(0)` then, next line, `m.rep(0)…`).
        let outcome = self.script.eval(source, summary);
        self.console.lines.extend(outcome.output);
        for cmd in outcome.commands {
            if let Err(e) = self.execute_command(cmd) {
                self.console.lines.push(ConsoleLine { kind: LineKind::Error, text: e });
            }
        }
        self.view_dirty = true;
        // One checkpoint for the whole script (no-op if it changed no document state,
        // e.g. a camera/frame-only script — those are view state, not in EditState).
        self.history.maybe_record(EditState::capture(&self.scene));
    }

    /// Apply one script [`Command`](crate::script::Command). `Load` needs the App's
    /// loader (and the filesystem, native-only); every scene-mutating command is
    /// delegated to [`script::apply_scene_command`]. Returns a console message on failure.
    pub(super) fn execute_command(&mut self, cmd: crate::script::Command) -> Result<(), String> {
        match cmd {
            crate::script::Command::Load(path) => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let raw = data::load_with(&path, &self.settings.behavior.bond_params())?;
                    self.add_loaded(raw);
                    Ok(())
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = path;
                    Err("load() is not available in the browser".to_string())
                }
            }
            other => crate::script::apply_scene_command(
                &mut self.scene,
                &mut self.camera,
                &self.rep_defaults,
                other,
            ),
        }
    }

    /// A one-line-per-rep listing of the scene, returned by the script `list()`.
    pub(super) fn scene_summary(&self) -> String {
        if self.scene.molecules.is_empty() {
            return "(no molecules)".to_string();
        }
        let mut s = String::new();
        for (mi, m) in self.scene.molecules.iter().enumerate() {
            let frames = m.trajectory.n_frames();
            s.push_str(&format!(
                "[{mi}] {} — {} atoms{}{}\n",
                m.name,
                m.n_atoms,
                if frames > 1 { format!(", {frames} frames") } else { String::new() },
                if m.visible { "" } else { " (hidden)" },
            ));
            for (ri, rep) in m.reps.iter().enumerate() {
                s.push_str(&format!(
                    "    rep {ri}: {} / {} / \"{}\"{}\n",
                    rep.kind.label(),
                    rep.color.label(),
                    rep.sel_text,
                    if rep.visible { "" } else { " (hidden)" },
                ));
            }
        }
        s.trim_end().to_string()
    }
}
