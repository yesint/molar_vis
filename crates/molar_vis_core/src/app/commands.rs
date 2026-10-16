//! Apply shared scene commands on the UI thread.
use super::*;

impl App {
    /// Apply one scene [`Command`](crate::script::Command). `Load` needs the App's
    /// loader (and the filesystem, native-only); every scene-mutating command is
    /// delegated to [`script::apply_scene_command`]. Returns an error on failure.
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
}
