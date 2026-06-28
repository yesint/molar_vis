//! App construction (App::new) + headless debug presets.
use super::*;
#[cfg(not(target_arch = "wasm32"))]
use super::session_io::*;
use super::loaders::*;
use super::draw::*;

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        launch: AppLaunch,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Program settings: load from the platform config dir (created with defaults
        // on first launch). `MOLAR_VIS_DEBUG_DEFAULTS=1` forces built-in defaults
        // (no file IO) so headless verification is reproducible and never depends on
        // the dev's saved config. WASM has no filesystem, so it always uses defaults.
        let settings = {
            #[cfg(not(target_arch = "wasm32"))]
            {
                if std::env::var("MOLAR_VIS_DEBUG_DEFAULTS").is_ok() {
                    Settings::default()
                } else {
                    Settings::load_or_create()
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                Settings::default()
            }
        };

        crate::theme::apply(&cc.egui_ctx, &settings.appearance);

        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or("wgpu render state unavailable (eframe must use the wgpu backend)")?;
        let renderer = SceneRenderer::new(render_state, &settings.rendering);

        // New-representation defaults come from the settings; VMD's default style is
        // Lines. MOLAR_VIS_DEBUG_REP=vdw|licorice|… still overrides the kind for
        // headless checks.
        let rep_defaults = Self::effective_rep_defaults(&settings);
        let bond_params = settings.behavior.bond_params();

        let mut scene = Scene::default();
        let mut status = String::new();
        // VMD-style command-line grouping: each `launch.files` entry is one molecule's
        // file list — the first file is the structure (topology + frame 0) and any
        // following files load as appended trajectory states. `-m` on the command line
        // starts a new molecule (see `launch::parse_file_args`).
        for group in &launch.files {
            let (structure, extra) = match group.split_first() {
                Some(parts) => parts,
                None => continue,
            };
            let raw = match data::load_with(structure, &bond_params) {
                Ok(raw) => raw,
                Err(e) => {
                    log::error!("{e}");
                    status = e;
                    continue;
                }
            };
            scene.add(raw, &rep_defaults);
            let mol = scene.molecules.last_mut().unwrap();
            mol.trajectory.speed_fps = settings.behavior.traj_fps;
            mol.trajectory.loop_mode = settings.behavior.loop_mode;
            // Build the trajectory from ALL frames in the group, VMD-style: the **first
            // file's frames beyond frame 0** (frame 0 is the structure just loaded — a
            // multi-MODEL/trajectory structure file thus contributes all its frames),
            // then every extra file's frames. `seed_frame0` (idempotent) makes frame 0
            // the structure; only files that actually yield frames are recorded as
            // trajectory loads, so a plain single-frame structure stays static.
            #[cfg(not(target_arch = "wasm32"))]
            {
                let n = mol.n_atoms;
                let mut seeded = false;
                // (path, from): the first file is read from frame 1 (frame 0 is the
                // structure); extra files from frame 0.
                let sources = std::iter::once((structure, 1usize))
                    .chain(extra.iter().map(|p| (p, 0usize)));
                for (path, from) in sources {
                    let opts = LoadOptions { from, to: None, stride: 1 };
                    match data::traj_loader::read_frames_sync(path, &opts, n) {
                        Ok(frames) if !frames.is_empty() => {
                            if !seeded {
                                mol.seed_frame0();
                                seeded = true;
                            }
                            mol.append_frames(frames);
                            mol.traj_loads.push(crate::scene::TrajLoad {
                                path: path.clone(),
                                from,
                                to: None,
                                stride: 1,
                            });
                        }
                        Ok(_) => {} // no frames in this file (e.g. single-MODEL structure)
                        Err(e) => {
                            log::error!("trajectory {}: {e}", path.display());
                            status = e;
                        }
                    }
                }
                if seeded {
                    mol.apply_current_frame();
                }
            }
            #[cfg(target_arch = "wasm32")]
            let _ = extra;
        }
        if !scene.molecules.is_empty() {
            scene.selected_mol = Some(0);
            status = format!("{} molecule(s) loaded", scene.molecules.len());
        } else if status.is_empty() {
            status = "No molecules loaded.".to_string();
        }

        // Verification hook: MOLAR_VIS_DEBUG_SEL=<selection> overrides the initial
        // selection of every molecule's first rep (e.g. "name CA", "protein").
        if let Ok(sel) = std::env::var("MOLAR_VIS_DEBUG_SEL") {
            for mol in &mut scene.molecules {
                if let Some(rep) = mol.reps.first_mut() {
                    rep.sel_text = sel.clone();
                    rep.sel_dirty = true;
                }
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_COLOR sets the first rep's color scheme.
        if let Some(cm) = std::env::var("MOLAR_VIS_DEBUG_COLOR").ok().and_then(|c| {
            match c.to_ascii_lowercase().as_str() {
                "element" => Some(ColorMethod::Element),
                "chain" => Some(ColorMethod::Chain),
                "resid" => Some(ColorMethod::ResId),
                "resname" => Some(ColorMethod::ResName),
                "index" => Some(ColorMethod::Index),
                "beta" => Some(ColorMethod::Beta),
                "secstruct" | "structure" | "ss" => Some(ColorMethod::SecStruct),
                "solid" => Some(ColorMethod::Solid(crate::color::DEFAULT_SOLID)),
                _ => None,
            }
        }) {
            for mol in &mut scene.molecules {
                if let Some(rep) = mol.reps.first_mut() {
                    rep.color = cm;
                }
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_SMOOTH=<window> sets mol 0's first rep
        // trajectory smoothing window (odd; needs MOLAR_VIS_DEBUG_TRAJ to do anything).
        if let Ok(w) = std::env::var("MOLAR_VIS_DEBUG_SMOOTH") {
            if let Ok(w) = w.trim().parse::<u32>() {
                if let Some(rep) = scene.molecules.first_mut().and_then(|m| m.reps.first_mut()) {
                    rep.smooth_window = w.max(1) | 1;
                }
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_MATERIAL sets the first rep's material.
        if let Some(mat) = std::env::var("MOLAR_VIS_DEBUG_MATERIAL").ok().and_then(|m| {
            Material::ALL
                .into_iter()
                .find(|x| x.label().eq_ignore_ascii_case(&m))
        }) {
            for mol in &mut scene.molecules {
                if let Some(rep) = mol.reps.first_mut() {
                    rep.material = mat;
                }
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_ALLCOLORS lays out one rep per color
        // scheme (cycling styles) so every style/color icon is visible at once.
        if std::env::var("MOLAR_VIS_DEBUG_ALLCOLORS").is_ok() {
            for mol in &mut scene.molecules {
                mol.reps.clear();
                for (i, &cm) in ColorMethod::ALL.iter().enumerate() {
                    let mut rep =
                        Representation::new(crate::geometry::RepKind::ALL[i % 4]);
                    rep.color = cm;
                    mol.reps.push(rep);
                }
                mol.selected_rep = Some(0);
            }
        }

        // Verification hook: MOLAR_VIS_DEBUG_TRAJ=<path> loads a trajectory into
        // the first molecule (sync), bypassing the dialog; MOLAR_VIS_DEBUG_FRAME=<n>
        // selects the displayed frame so headless screenshots can confirm motion.
        #[cfg(not(target_arch = "wasm32"))]
        if let Ok(traj_path) = std::env::var("MOLAR_VIS_DEBUG_TRAJ") {
            if let Some(mol) = scene.molecules.first_mut() {
                mol.seed_frame0();
                let envn = |k: &str| std::env::var(k).ok().and_then(|s| s.parse::<usize>().ok());
                let opts = crate::trajectory::LoadOptions {
                    from: envn("MOLAR_VIS_DEBUG_TRAJ_FROM").unwrap_or(0),
                    to: envn("MOLAR_VIS_DEBUG_TRAJ_TO"),
                    stride: envn("MOLAR_VIS_DEBUG_TRAJ_STRIDE").unwrap_or(1),
                };
                match data::traj_loader::read_frames_sync(
                    std::path::Path::new(&traj_path),
                    &opts,
                    mol.n_atoms,
                ) {
                    Ok(frames) => {
                        mol.append_frames(frames);
                        mol.traj_loads.push(crate::scene::TrajLoad {
                            path: std::path::PathBuf::from(&traj_path),
                            from: opts.from,
                            to: opts.to,
                            stride: opts.stride,
                        });
                        let frame = std::env::var("MOLAR_VIS_DEBUG_FRAME")
                            .ok()
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(0);
                        mol.trajectory.set_current(frame);
                        if std::env::var("MOLAR_VIS_DEBUG_TRAJ_PLAY").is_ok() {
                            mol.trajectory.set_playing(true);
                        }
                        mol.apply_current_frame();
                        log::info!(
                            "debug trajectory: {} frames, showing {}",
                            mol.trajectory.n_frames(),
                            mol.trajectory.current
                        );
                    }
                    Err(e) => log::error!("debug trajectory load failed: {e}"),
                }
            }
        }

        // Verification hook: MOLAR_VIS_DEBUG_BOX=1 shows the periodic box on mol 0.
        if std::env::var("MOLAR_VIS_DEBUG_BOX").is_ok() {
            if let Some(mol) = scene.molecules.first_mut() {
                mol.show_box = true;
                mol.box_dirty = true;
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_PBC="px,py,pz" sets the +a/+b/+c periodic
        // image counts on mol 0's first rep (and shows the box), exercising the
        // dynamic-camera image rendering headlessly.
        if let Ok(spec) = std::env::var("MOLAR_VIS_DEBUG_PBC") {
            let n: Vec<u32> = spec.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if let Some(mol) = scene.molecules.first_mut() {
                if let Some(rep) = mol.reps.first_mut() {
                    rep.periodic.pos = [
                        n.first().copied().unwrap_or(0),
                        n.get(1).copied().unwrap_or(0),
                        n.get(2).copied().unwrap_or(0),
                    ];
                    rep.periodic.show_box = true;
                }
            }
        }

        let mut camera = match scene.bbox() {
            Some((min, max)) => Camera::frame_bbox(min, max, settings.view.fill),
            None => Camera::default(),
        };
        // Seed the fresh camera with the user's default view (projection, depth-cue,
        // AO, shadows, background). The debug hooks below override specific fields.
        settings.view.seed_camera(&mut camera);
        if let Ok(deg) = std::env::var("MOLAR_VIS_DEBUG_ORBIT") {
            if let Ok(d) = deg.parse::<f32>() {
                camera.orbit(d, d * 0.4, 1.0);
            }
        }
        if std::env::var("MOLAR_VIS_DEBUG_ORTHO").is_ok() {
            camera.projection = Projection::Orthographic;
        }
        if std::env::var("MOLAR_VIS_DEBUG_PERSP").is_ok() {
            camera.projection = Projection::Perspective;
        }
        // Verification hook: MOLAR_VIS_DEBUG_ZOOM=<factor> dollies out (factor > 1).
        if let Ok(f) = std::env::var("MOLAR_VIS_DEBUG_ZOOM") {
            if let Ok(f) = f.parse::<f32>() {
                camera.distance *= f.max(0.05);
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_CUEMODE=linear|exp|exp2 sets the depth-
        // cue falloff curve (and bumps strength so it's visible in a screenshot).
        if let Ok(m) = std::env::var("MOLAR_VIS_DEBUG_CUEMODE") {
            camera.depth_cue.mode = match m.to_ascii_lowercase().as_str() {
                "exp" => CueMode::Exp,
                "exp2" => CueMode::Exp2,
                _ => CueMode::Linear,
            };
            camera.depth_cue.enabled = true;
            camera.depth_cue.strength = 0.9;
            camera.depth_cue.start = 0.0;
        }
        // Verification hook: MOLAR_VIS_DEBUG_AO[=strength] enables ambient occlusion.
        if let Ok(v) = std::env::var("MOLAR_VIS_DEBUG_AO") {
            camera.ao.enabled = true;
            if let Ok(s) = v.trim().parse::<f32>() {
                camera.ao.strength = s.clamp(0.0, 1.0);
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_SHADOW[=strength] enables cast shadows.
        if let Ok(v) = std::env::var("MOLAR_VIS_DEBUG_SHADOW") {
            camera.shadow.enabled = true;
            if let Ok(s) = v.trim().parse::<f32>() {
                camera.shadow.strength = s.clamp(0.0, 1.0);
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_BG=gradient|white sets the background.
        if let Ok(v) = std::env::var("MOLAR_VIS_DEBUG_BG") {
            match v.trim().to_ascii_lowercase().as_str() {
                "gradient" => camera.background.kind = crate::camera::BgKind::Gradient,
                "white" => camera.background.color = [0.95, 0.95, 0.95, 1.0],
                _ => {}
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_FOCUS=<selection> zooms the camera to
        // fit that selection of mol 0 (exercises the zoom-to-selection path).
        if let Ok(sel_text) = std::env::var("MOLAR_VIS_DEBUG_FOCUS") {
            if let Some(mol) = scene.molecules.first() {
                if let Ok((_, sel)) = mol.data.evaluate(&sel_text) {
                    let (min, max) = mol.sel_bbox(&sel);
                    camera.focus_bbox(min, max);
                }
            }
        }

        // Verification hook: MOLAR_VIS_DEBUG_PENDING=<selection> stages that selection
        // as an active (pending) selection on **every** molecule — exercises the lasso
        // glow + per-molecule accept/discard UI (incl. the multi-molecule case) without
        // simulating a mouse drag.
        if let Ok(sel_text) = std::env::var("MOLAR_VIS_DEBUG_PENDING") {
            for mol in &mut scene.molecules {
                if let Ok((_, sel)) = mol.data.evaluate(&sel_text) {
                    let atoms: Vec<usize> = {
                        let bound = mol.data.bind(&sel);
                        bound.iter_particle().map(|p| p.id).collect()
                    };
                    if atoms.is_empty() {
                        continue;
                    }
                    mol.pending = Some(scene::PendingSelection { sel_text: sel_text.clone(), atoms });
                    mol.reps_open = true;
                    mol.glow_dirty = true;
                }
            }
        }

        let history = History::new(EditState::capture(&scene));

        // Verification hook: MOLAR_VIS_DEBUG_PARAMS=1 opens the first rep's gear panel.
        if std::env::var("MOLAR_VIS_DEBUG_PARAMS").is_ok() {
            if let Some(rep) = scene.molecules.first_mut().and_then(|m| m.reps.first_mut()) {
                rep.params_open = true;
            }
        }

        // Browser file-open + trajectory-load channels (the async pickers send
        // their bytes back here for `ui()` to process).
        #[cfg(target_arch = "wasm32")]
        let (file_tx, file_rx) = std::sync::mpsc::channel::<(String, Vec<u8>)>();
        #[cfg(target_arch = "wasm32")]
        let (traj_tx, traj_rx) = std::sync::mpsc::channel::<(MolId, String, Vec<u8>)>();

        // Compute these before the struct init moves `settings` in.
        let pick_mode = if std::env::var("MOLAR_VIS_DEBUG_PICK").is_ok() {
            PickMode::Click
        } else {
            settings.behavior.pick_mode
        };
        let selection_mode = match std::env::var("MOLAR_VIS_DEBUG_SELMODE").as_deref() {
            Ok("residues") => SelectionMode::Residues,
            Ok("boundh") => SelectionMode::BoundH,
            _ => settings.behavior.selection_mode,
        };

        #[allow(unused_mut)]
        let mut app = Self {
            renderer,
            camera,
            scene,
            settings,
            rep_defaults,
            settings_draft: None,
            settings_tab: SettingsPage::default(),
            last_render_camera: None,
            last_size: [0, 0],
            view_dirty: true,
            status,
            history,
            export_request: None,
            #[cfg(target_arch = "wasm32")]
            pending_capture: None,
            pending_undo_n: None,
            pending_redo_n: None,
            editing_rep: None,
            load_dialog: None,
            delete_frames_dialog: None,
            rename_mol: None,
            loaders: HashMap::new(),
            pick_mode,
            selection_mode,
            lasso_path: Vec::new(),
            last_lens_ndc: None,
            #[cfg(not(target_arch = "wasm32"))]
            hover_pick: None,
            #[cfg(not(target_arch = "wasm32"))]
            last_pick_px: None,
            axes_on: std::env::var("MOLAR_VIS_DEBUG_AXES").is_ok(),
            axes_corner: Corner::BottomRight,
            view_tab: ViewTab::default(),
            view_menu_open: std::env::var("MOLAR_VIS_DEBUG_VIEWMENU").is_ok(),
            view_menu_rect: None,
            #[cfg(target_arch = "wasm32")]
            file_tx,
            #[cfg(target_arch = "wasm32")]
            file_rx,
            #[cfg(target_arch = "wasm32")]
            traj_tx,
            #[cfg(target_arch = "wasm32")]
            traj_rx,
            #[cfg(target_arch = "wasm32")]
            wasm_loaders: HashMap::new(),
            draw: None,
            console_open: false,
            console: crate::script::ScriptConsole::default(),
            script: crate::script::ScriptSession::new(),
            jobs_rx: None,
        };

        // Verification hooks (native): exercise the session save/load round-trip
        // headlessly, since the rfd file dialogs can't be driven in a headless run.
        // MOLAR_VIS_DEBUG_LOAD_SESSION=<path> replaces the scene from a session
        // file; MOLAR_VIS_DEBUG_SAVE_SESSION=<path> writes the current state out.
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Ok(path) = std::env::var("MOLAR_VIS_DEBUG_LOAD_SESSION") {
                app.load_session_from(std::path::Path::new(&path));
            }
            if let Ok(path) = std::env::var("MOLAR_VIS_DEBUG_SAVE_SESSION") {
                app.save_session_to(std::path::Path::new(&path));
            }
            // MOLAR_VIS_DEBUG_SAVE_MOL=<path> writes mol 0 to a structure file
            // (exercises the molar FileHandler write + displayed-frame swap path).
            if let Ok(path) = std::env::var("MOLAR_VIS_DEBUG_SAVE_MOL") {
                if let Some(mol) = app.scene.molecules.first_mut() {
                    match save_displayed(mol, std::path::Path::new(&path), None) {
                        Ok(()) => log::info!("debug: saved molecule to {path}"),
                        Err(e) => log::error!("debug save molecule failed: {e}"),
                    }
                }
            }
            // MOLAR_VIS_DEBUG_SAVE_IMAGE=<path> renders the startup scene to a PNG
            // headlessly — exercising the offscreen render → GPU readback → PNG-encode
            // path without a window/screenshot. Size from _W/_H (default 800×600). We
            // first run `rebuild_dirty` to build + upload geometry (it isn't built until
            // the first `ui()` frame), then capture + block on the readback + save.
            if let Ok(path) = std::env::var("MOLAR_VIS_DEBUG_SAVE_IMAGE") {
                if let Some(rs) = cc.wgpu_render_state.as_ref() {
                    let dim = |k: &str, d: u32| {
                        std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
                    };
                    let (w, h) = (
                        dim("MOLAR_VIS_DEBUG_SAVE_IMAGE_W", 800),
                        dim("MOLAR_VIS_DEBUG_SAVE_IMAGE_H", 600),
                    );
                    app.rebuild_dirty(rs);
                    let aspect = w as f32 / h as f32;
                    let view = app.camera.view();
                    let proj = app.camera.proj(aspect);
                    let cap = app.renderer.capture_begin(
                        rs,
                        w,
                        h,
                        view,
                        proj,
                        app.camera.is_perspective(),
                        app.camera.cue_uniform(),
                        app.camera.ao_uniform(),
                        app.camera.shadow_uniform(),
                        app.camera.background,
                        app.camera.eye_depth_range(),
                        &app.scene,
                    );
                    let _ = rs.device.poll(wgpu::PollType::wait_indefinitely());
                    match cap.read().save(std::path::Path::new(&path)) {
                        Ok(()) => log::info!("debug: saved image ({w}x{h}) to {path}"),
                        Err(e) => log::error!("debug save image failed: {e}"),
                    }
                }
            }
        }

        // Verification hook: MOLAR_VIS_DEBUG_SETTINGS=1 (or =appearance|rendering|
        // view|reps|behavior) opens the program-settings modal at the given tab at
        // startup (it can't be driven by a mouse in a headless run), so each tab can
        // be screenshot. Pair with MOLAR_VIS_DEBUG_DEFAULTS=1 to keep the shown values
        // reproducible regardless of the saved config.
        if let Ok(tab) = std::env::var("MOLAR_VIS_DEBUG_SETTINGS") {
            app.settings_draft = Some(app.settings.clone());
            app.settings_tab = match tab.to_ascii_lowercase().as_str() {
                "rendering" => SettingsPage::Rendering,
                "view" => SettingsPage::View,
                "reps" | "representations" => SettingsPage::Representations,
                "behavior" => SettingsPage::Behavior,
                _ => SettingsPage::Appearance,
            };
        }

        // Verification hook: MOLAR_VIS_DEBUG_DELFRAMES=1 opens the delete-frames
        // dialog for mol 0 (pair with MOLAR_VIS_DEBUG_TRAJ to have frames).
        if std::env::var("MOLAR_VIS_DEBUG_DELFRAMES").is_ok() {
            if let Some(mol) = app.scene.molecules.first() {
                app.delete_frames_dialog = Some(DeleteFramesDialog::new(mol.id));
            }
        }

        // Verification hook: MOLAR_VIS_DEBUG_EDIT_REP=1 opens mol 0's first rep
        // selection field in edit mode, so a selection error's in-field
        // whole-word highlight can be screenshot headlessly.
        if std::env::var("MOLAR_VIS_DEBUG_EDIT_REP").is_ok()
            && app.scene.molecules.first().is_some_and(|m| !m.reps.is_empty())
        {
            app.editing_rep = Some((0, 0));
        }

        // Verification hook: MOLAR_VIS_DEBUG_DRAW=<preset> builds a small molecule
        // through the *same* `Molecule` edit helpers the Draw-mode UI uses (single_atom
        // → add_atom/add_bond with rough coords), turns Draw mode on, gives it a
        // Ball-and-Stick rep, frames the camera, and relaxes it once — so the drawing
        // path can be exercised without a mouse on this headless (Wayland) box. Pair
        // with `RUST_LOG=molar_vis_core=info` to see the atom/bond counts + a bond
        // length after relaxation. Presets: methane, ethane, water, benzene.
        if let Ok(preset) = std::env::var("MOLAR_VIS_DEBUG_DRAW") {
            app.debug_draw_preset(&preset.to_ascii_lowercase());
        }

        // Verification hook: MOLAR_VIS_DEBUG_SCRIPT=<source | @path> runs a Rhai
        // script at startup through the same path the console uses, so a command's
        // effect (e.g. `mol(0).rep(0).set_color("chain")`) can be screenshot headlessly.
        if let Ok(src) = std::env::var("MOLAR_VIS_DEBUG_SCRIPT") {
            #[cfg(not(target_arch = "wasm32"))]
            let src = match src.strip_prefix('@') {
                Some(path) => std::fs::read_to_string(path).unwrap_or_else(|e| {
                    log::error!("debug script file: {e}");
                    String::new()
                }),
                None => src,
            };
            app.run_script(&src);
            app.console_open = true; // show the console (echo + output) for the screenshot
            app.console.focus_input = true;
        }

        Ok(app)
    }

    /// Headless verification: build a known small molecule via the Draw-mode edit
    /// helpers and relax it. Enters Draw mode (so the toolbar/viewport state is
    /// realistic). Logs the result at `info`. Native + wasm safe (pure CPU + molar).
    pub(super) fn debug_draw_preset(&mut self, preset: &str) {
        use crate::minimize::{BondOrder, RelaxKind};
        // (element, x, y, z) seed atoms + (i, j, order) bonds — rough/strained coords
        // (a hand-drawn sketch), all in nm. The minimizer is what makes them sensible.
        let atoms: Vec<(Element, glam::Vec3)>;
        let bonds: Vec<(usize, usize, BondOrder)>;
        match preset {
            "water" => {
                atoms = vec![
                    (Element::O, glam::vec3(0.0, 0.0, 0.0)),
                    (Element::H, glam::vec3(0.10, 0.0, 0.0)),
                    (Element::H, glam::vec3(0.0, 0.10, 0.0)),
                ];
                bonds = vec![(0, 1, BondOrder::Single), (0, 2, BondOrder::Single)];
            }
            "ethane" => {
                atoms = vec![
                    (Element::C, glam::vec3(0.0, 0.0, 0.0)),
                    (Element::C, glam::vec3(0.16, 0.0, 0.0)),
                    (Element::H, glam::vec3(-0.05, 0.09, 0.0)),
                    (Element::H, glam::vec3(-0.05, -0.09, 0.0)),
                    (Element::H, glam::vec3(-0.05, 0.0, 0.09)),
                    (Element::H, glam::vec3(0.21, 0.09, 0.0)),
                    (Element::H, glam::vec3(0.21, -0.09, 0.0)),
                    (Element::H, glam::vec3(0.21, 0.0, -0.09)),
                ];
                bonds = vec![
                    (0, 1, BondOrder::Single),
                    (0, 2, BondOrder::Single),
                    (0, 3, BondOrder::Single),
                    (0, 4, BondOrder::Single),
                    (1, 5, BondOrder::Single),
                    (1, 6, BondOrder::Single),
                    (1, 7, BondOrder::Single),
                ];
            }
            "ethene" => {
                // H2C=CH2 — a double bond to exercise multi-order rendering.
                atoms = vec![
                    (Element::C, glam::vec3(0.0, 0.0, 0.0)),
                    (Element::C, glam::vec3(0.14, 0.0, 0.0)),
                    (Element::H, glam::vec3(-0.06, 0.08, 0.0)),
                    (Element::H, glam::vec3(-0.06, -0.08, 0.0)),
                    (Element::H, glam::vec3(0.20, 0.08, 0.0)),
                    (Element::H, glam::vec3(0.20, -0.08, 0.0)),
                ];
                bonds = vec![
                    (0, 1, BondOrder::Double),
                    (0, 2, BondOrder::Single),
                    (0, 3, BondOrder::Single),
                    (1, 4, BondOrder::Single),
                    (1, 5, BondOrder::Single),
                ];
            }
            "acetylene" => {
                // HC≡CH — a triple bond.
                atoms = vec![
                    (Element::C, glam::vec3(0.0, 0.0, 0.0)),
                    (Element::C, glam::vec3(0.13, 0.0, 0.0)),
                    (Element::H, glam::vec3(-0.10, 0.0, 0.0)),
                    (Element::H, glam::vec3(0.23, 0.0, 0.0)),
                ];
                bonds = vec![
                    (0, 1, BondOrder::Triple),
                    (0, 2, BondOrder::Single),
                    (1, 3, BondOrder::Single),
                ];
            }
            "benzene" => {
                // Six carbons on a rough hexagon (radius ~0.14 nm) — deliberately a bit
                // off so the relax has something to do — with aromatic ring bonds.
                let mut a = Vec::new();
                let mut b = Vec::new();
                let r = 0.135_f32;
                for k in 0..6 {
                    let th = std::f32::consts::TAU * (k as f32) / 6.0;
                    // jitter so it isn't already perfect
                    let rr = r * if k % 2 == 0 { 1.08 } else { 0.92 };
                    a.push((Element::C, glam::vec3(rr * th.cos(), rr * th.sin(), 0.0)));
                }
                for k in 0..6 {
                    b.push((k, (k + 1) % 6, BondOrder::Aromatic));
                }
                atoms = a;
                bonds = b;
            }
            // "methane" and anything unrecognized → methane.
            _ => {
                atoms = vec![
                    (Element::C, glam::vec3(0.0, 0.0, 0.0)),
                    (Element::H, glam::vec3(0.08, 0.08, 0.08)),
                    (Element::H, glam::vec3(-0.08, -0.08, 0.08)),
                    (Element::H, glam::vec3(-0.08, 0.08, -0.08)),
                    (Element::H, glam::vec3(0.08, -0.08, -0.08)),
                ];
                bonds = vec![
                    (0, 1, BondOrder::Single),
                    (0, 2, BondOrder::Single),
                    (0, 3, BondOrder::Single),
                    (0, 4, BondOrder::Single),
                ];
            }
        }
        let Some((first_el, first_pos)) = atoms.first().copied() else {
            return;
        };
        // Create the molecule from the first atom (a drawn molecule is never empty),
        // exactly like the first click of the Atom tool.
        let raw = match data::RawMolecule::single_atom("drawn", first_el.make_atom(), first_pos) {
            Ok(raw) => raw,
            Err(e) => {
                log::error!("debug draw: {e}");
                return;
            }
        };
        let mut session = DrawSession { element: first_el, ..DrawSession::default() };
        self.start_drawn_molecule(raw, &mut session);
        let Some(target) = session.target else { return };
        let Some(mi) = self.scene.molecules.iter().position(|m| m.id == target) else {
            return;
        };
        // Append the rest via the same helpers, then relax to convergence.
        {
            let mol = &mut self.scene.molecules[mi];
            for &(el, pos) in atoms.iter().skip(1) {
                mol.add_atom(&el.make_atom(), pos);
            }
            for &(i, j, order) in &bonds {
                mol.add_bond(i, j, order);
            }
            mol.refresh_bbox();
            mol.perceive_aromaticity(); // detect rings/aromaticity (drives the ring-circle overlay)
            let res = crate::minimize::relax_in_system(
                mol.data.system_mut().expect("drawn molecule is owned"),
                &mol.bonds,
                RelaxKind::Cleanup,
            );
            // A representative bond length after relaxation (the first bond).
            let len0 = mol.bonds.first().map(|bond| {
                let (a, b) = (bond.i1, bond.i2);
                let st = mol.data.state();
                match (st.coords.get(a), st.coords.get(b)) {
                    (Some(pa), Some(pb)) => {
                        glam::vec3(pa.x, pa.y, pa.z).distance(glam::vec3(pb.x, pb.y, pb.z))
                    }
                    _ => f32::NAN,
                }
            });
            log::info!(
                "debug draw '{preset}': {} atoms, {} bonds, relax {} steps (converged={}, fmax={:.4}), bond0 len = {:.4} nm",
                mol.n_atoms,
                mol.bonds.len(),
                res.steps,
                res.converged,
                res.final_force_norm,
                len0.unwrap_or(f32::NAN),
            );
            if let Some(rep) = mol.reps.first_mut() {
                rep.geom_dirty = true;
            }
        }
        // Frame the camera on the drawn molecule and keep the session active.
        let (min, max) = self.scene.molecules[mi].current_bbox();
        self.camera = Camera::frame_bbox(min, max, self.settings.view.fill);
        self.settings.view.seed_camera(&mut self.camera);
        self.last_render_camera = None;
        self.view_dirty = true;
        self.pick_mode = PickMode::Off;
        self.draw = Some(session);
        // A fresh drawn molecule is its own baseline for undo.
        self.history = History::new(EditState::capture(&self.scene));
    }
}
