//! Scriptable commands and their application to the scene.
//!
//! [`Command`] is the vocabulary of scene mutations a script (or a menu action) can ask
//! for, and [`apply_scene_command`] performs one — doing the *same* field-set +
//! dirty-flag the GUI does, so the change converges on the normal `rebuild_dirty` path
//! with no second render branch. Kept free of `App` so it is unit-testable without a GPU.
//!
//! This layer is **always compiled**: the app's own menu actions and the Python
//! (`molar_vis_py`) / JavaScript (`molar_vis_js`) hosts drive the viewer through it.
//!
//! The in-app **Rhai console** on top of it — the [`engine`] and [`console`] modules — is
//! behind the non-default `scripting` feature. Rhai and its console are a convenience for
//! interactive use, not something every embedding wants to pay for, so an embedder that
//! drives the viewer from its own host language can leave them out entirely.
//!
//! Pure-Rust + WASM-safe; only the `load()` command is native-gated (in
//! `App::execute_command`).

pub mod command;
#[cfg(feature = "scripting")]
pub mod console;
#[cfg(feature = "scripting")]
pub mod engine;

pub use command::{parse_color, parse_material, Command, RepRef};
#[cfg(feature = "scripting")]
pub use console::{ConsoleLine, LineKind, ScriptConsole};
#[cfg(feature = "scripting")]
pub use engine::ScriptSession;

/// Apply a **scene-mutating** command (everything except [`Command::Load`], which
/// needs the App's loader and is handled in `App::execute_command`). Performs the
/// same field-set + dirty-flag the GUI does, so the change converges on the normal
/// `rebuild_dirty` path. Returns a message to show in the console on a bad index /
/// value / selection. Kept free of `App` so it's unit-testable without a GPU.
pub fn apply_scene_command(
    scene: &mut crate::scene::Scene,
    camera: &mut crate::camera::Camera,
    rep_defaults: &crate::settings::RepDefaults,
    cmd: Command,
) -> Result<(), String> {
    use crate::scene::EvalError;
    match cmd {
        Command::Load(_) => Err("load() must be applied by the app".to_string()),
        Command::Select { mol, rep, text } => {
            let (mi, ri) = resolve_rep(scene, mol, rep)?;
            // Pre-validate for an immediate error; an empty match is a (non-fatal)
            // warning the field flags, so let it through.
            if let Err(EvalError::Invalid { message, .. }) =
                scene.molecules[mi].data.evaluate(&text)
            {
                return Err(message);
            }
            let r = &mut scene.molecules[mi].reps[ri];
            r.sel_text = text;
            r.sel_dirty = true;
            Ok(())
        }
        Command::Color { mol, rep, method } => {
            let cm = parse_color(&method).ok_or_else(|| format!("unknown color scheme '{method}'"))?;
            let (mi, ri) = resolve_rep(scene, mol, rep)?;
            let r = &mut scene.molecules[mi].reps[ri];
            r.color = cm;
            r.geom_dirty = true;
            Ok(())
        }
        Command::Style { mol, rep, kind } => {
            let k = crate::geometry::RepKind::from_name(&kind)
                .ok_or_else(|| format!("unknown style '{kind}'"))?;
            let (mi, ri) = resolve_rep(scene, mol, rep)?;
            let r = &mut scene.molecules[mi].reps[ri];
            r.kind = k;
            r.params = crate::geometry::RepParams::for_kind(k);
            r.geom_dirty = true;
            Ok(())
        }
        Command::Material { mol, rep, name } => {
            let mat = parse_material(&name).ok_or_else(|| format!("unknown material '{name}'"))?;
            let (mi, ri) = resolve_rep(scene, mol, rep)?;
            let r = &mut scene.molecules[mi].reps[ri];
            r.material = mat;
            r.geom_dirty = true;
            Ok(())
        }
        Command::AddRep { mol } => {
            let mi = resolve_mol(scene, mol)?;
            scene.molecules[mi]
                .reps
                .push(crate::scene::Representation::from_defaults(rep_defaults));
            Ok(())
        }
        Command::DeleteRep { mol, rep } => {
            let (mi, ri) = resolve_rep(scene, mol, RepRef::Index(rep))?;
            scene.molecules[mi].reps.remove(ri);
            Ok(())
        }
        Command::ShowMol { mol, visible } => {
            let mi = resolve_mol(scene, mol)?;
            scene.molecules[mi].visible = visible;
            Ok(())
        }
        Command::Frame { mol, index } => {
            let mi = resolve_mol(scene, mol)?;
            let m = &mut scene.molecules[mi];
            let n = m.trajectory.n_frames();
            if n == 0 {
                return Err("molecule has no trajectory".to_string());
            }
            m.trajectory.set_current(index.min(n - 1));
            m.apply_current_frame();
            Ok(())
        }
        Command::Play { mol, on } => {
            let mi = resolve_mol(scene, mol)?;
            scene.molecules[mi].trajectory.set_playing(on);
            Ok(())
        }
        Command::Focus { mol, text } => {
            let mi = resolve_mol(scene, mol)?;
            let m = &scene.molecules[mi];
            let (_, sel) = m.data.evaluate(&text).map_err(|e| match e {
                EvalError::Empty => "selection matched no atoms".to_string(),
                EvalError::Invalid { message, .. } => message,
            })?;
            let (min, max) = m.sel_bbox(&sel);
            camera.focus_bbox(min, max);
            Ok(())
        }
    }
}

/// Validate a molecule index.
fn resolve_mol(scene: &crate::scene::Scene, mol: usize) -> Result<usize, String> {
    if scene.molecules.is_empty() {
        return Err("no molecules loaded".to_string());
    }
    if mol >= scene.molecules.len() {
        return Err(format!("no molecule {mol} (have {})", scene.molecules.len()));
    }
    Ok(mol)
}

/// Resolve `(molecule, rep)` indices, mapping `RepRef::Last` → the molecule's last rep.
fn resolve_rep(
    scene: &crate::scene::Scene,
    mol: usize,
    rep: RepRef,
) -> Result<(usize, usize), String> {
    let mi = resolve_mol(scene, mol)?;
    let n = scene.molecules[mi].reps.len();
    let ri = match rep {
        RepRef::Index(i) => i,
        RepRef::Last => n
            .checked_sub(1)
            .ok_or_else(|| format!("molecule {mi} has no representations"))?,
    };
    if ri >= n {
        return Err(format!("molecule {mi} has no representation {ri} (have {n})"));
    }
    Ok((mi, ri))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_parser_matches_labels() {
        assert_eq!(parse_color("Chain"), Some(crate::color::ColorMethod::Chain));
        assert_eq!(parse_color("ss"), Some(crate::color::ColorMethod::SecStruct));
        assert!(parse_color("bogus").is_none());
    }

    fn load_scene() -> crate::scene::Scene {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb");
        let raw = crate::data::load(std::path::Path::new(path)).expect("load 2lao.pdb");
        let mut scene = crate::scene::Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        scene.selected_mol = Some(0);
        scene
    }

    /// Commands mutate the rep + set the right dirty flag; `AddRep` + a `Last`-targeted
    /// style grows the list and styles the new rep; selection resolves against molar's
    /// grammar. Built from [`Command`]s directly — this layer is what the app's menus and
    /// the Python/JS hosts use, so its test must not depend on the Rhai front end (the
    /// script→command half is covered in `engine`'s own tests).
    #[test]
    fn commands_mutate_the_scene() {
        let mut scene = load_scene();
        let mut camera = crate::camera::Camera::default();
        let defaults = crate::settings::RepDefaults::default();
        let apply = |scene: &mut crate::scene::Scene,
                         camera: &mut crate::camera::Camera,
                         cmds: Vec<Command>| {
            for cmd in cmds {
                apply_scene_command(scene, camera, &defaults, cmd).expect("command ok");
            }
        };

        let r0 = RepRef::Index(0);
        apply(
            &mut scene,
            &mut camera,
            vec![
                Command::Style { mol: 0, rep: r0, kind: "vdw".into() },
                Command::Color { mol: 0, rep: r0, method: "chain".into() },
                Command::Select { mol: 0, rep: r0, text: "name CA".into() },
            ],
        );
        let rep = &scene.molecules[0].reps[0];
        assert_eq!(rep.color, crate::color::ColorMethod::Chain);
        assert_eq!(rep.kind, crate::geometry::RepKind::Vdw);
        assert!(rep.geom_dirty);
        assert_eq!(rep.sel_text, "name CA");
        assert!(rep.sel_dirty);
        // `evaluate` returns Err(Empty) for a zero-atom match, so Ok ⇒ matched ≥1 atom.
        scene.molecules[0].data.evaluate(&rep.sel_text)
            .expect("name CA evaluates to a non-empty selection");

        // AddRep appends; a `Last`-targeted Style then hits the rep just added.
        apply(
            &mut scene,
            &mut camera,
            vec![
                Command::AddRep { mol: 0 },
                Command::Style { mol: 0, rep: RepRef::Last, kind: "cartoon".into() },
            ],
        );
        assert_eq!(scene.molecules[0].reps.len(), 2);
        assert_eq!(scene.molecules[0].reps[1].kind, crate::geometry::RepKind::Cartoon);

        // Bad value / index → clean Err, not a panic.
        assert!(apply_scene_command(
            &mut scene,
            &mut camera,
            &defaults,
            Command::Color { mol: 0, rep: RepRef::Index(0), method: "bogus".into() },
        )
        .is_err());
        assert!(apply_scene_command(
            &mut scene,
            &mut camera,
            &defaults,
            Command::Color { mol: 9, rep: RepRef::Index(0), method: "chain".into() },
        )
        .is_err());
    }
}
