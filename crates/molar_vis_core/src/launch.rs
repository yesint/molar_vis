//! Application launch parameters and the eframe entry point.

use std::path::PathBuf;

use crate::app::App;

/// Parameters handed to the viewer at startup. Built by the binary from argv
/// (and later by a web shell from URL params), so this struct is the single
/// platform-agnostic launch surface.
#[derive(Debug, Default, Clone)]
pub struct AppLaunch {
    /// Files to load on startup, **grouped per molecule** (VMD-style). Each inner
    /// `Vec` is one molecule: the first file provides the topology, and all frames of
    /// the group's files form the trajectory. See [`parse_file_args`].
    pub files: Vec<Vec<PathBuf>>,
    /// Docking startup uses the same receptor/pose loader as the docking dialog.
    pub docking: Option<DockingLaunch>,
}

/// Files supplied by `-r`/`--receptor` and `-l`/`--ligand`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockingLaunch {
    pub receptor: Vec<PathBuf>,
    pub ligands: Vec<PathBuf>,
}

/// Parse normal molecule arguments or a docking result. Both docking sides are
/// required; mixing docking flags with ordinary molecule groups is rejected.
/// Repeated docking flags append files, and `--` ends option parsing.
pub fn parse_launch_args<I, S>(args: I) -> Result<AppLaunch, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    #[derive(Clone, Copy)]
    enum Side {
        Files,
        Receptor,
        Ligands,
    }
    let mut side = Side::Files;
    let mut ordinary = Vec::new();
    let mut current = Vec::new();
    let mut molecule_flag = false;
    let mut receptor = Vec::new();
    let mut ligands = Vec::new();
    let mut docking = false;
    let mut options = true;
    let mut pending: Option<&str> = None;
    for arg in args {
        let arg = arg.as_ref();
        if options && arg == "--" {
            options = false;
            continue;
        }
        let flag = if options {
            if arg == "-r" || arg == "--receptor" {
                Some((Side::Receptor, "-r"))
            } else if arg == "-l" || arg == "--ligand" {
                Some((Side::Ligands, "-l"))
            } else {
                None
            }
        } else {
            None
        };
        if let Some((next, name)) = flag {
            if let Some(previous) = pending {
                return Err(format!("{previous} requires at least one file"));
            }
            side = next;
            docking = true;
            pending = Some(name);
            continue;
        }
        if options && (arg == "-m" || arg == "--molecule") {
            if docking {
                return Err("cannot mix -m with docking flags -r/-l".into());
            }
            molecule_flag = true;
            if !current.is_empty() {
                ordinary.push(std::mem::take(&mut current));
            }
            continue;
        }
        if options && arg.to_string_lossy().starts_with('-') {
            return Err(format!(
                "unknown option {}; use -- before a filename starting with '-'",
                arg.to_string_lossy()
            ));
        }
        match side {
            Side::Files => current.push(PathBuf::from(arg)),
            Side::Receptor => receptor.push(PathBuf::from(arg)),
            Side::Ligands => ligands.push(PathBuf::from(arg)),
        }
        pending = None;
    }
    if let Some(flag) = pending {
        return Err(format!("{flag} requires at least one file"));
    }
    if docking {
        if !ordinary.is_empty() || !current.is_empty() || molecule_flag {
            return Err("cannot mix ordinary molecule files with docking flags -r/-l".into());
        }
        if receptor.is_empty() {
            return Err("docking requires -r RECEPTOR_FILES...".into());
        }
        if ligands.is_empty() {
            return Err("docking requires -l LIGAND_FILES...".into());
        }
        Ok(AppLaunch {
            files: vec![],
            docking: Some(DockingLaunch { receptor, ligands }),
        })
    } else {
        if !current.is_empty() {
            ordinary.push(current);
        }
        Ok(AppLaunch {
            files: ordinary,
            docking: None,
        })
    }
}

/// Parse VMD-style command-line file arguments into per-molecule groups.
///
/// Files are grouped into molecules by the `-m` (or `--molecule`) flag: each `-m`
/// starts a new molecule and every file up to the next `-m` belongs to it. Within a
/// group the **first** file provides the topology, and **all** frames of the group's
/// files form the trajectory (a multi-MODEL/trajectory first file contributes all of
/// its frames, like VMD's `mol new` / `mol addfile`). With no `-m` at all, every file
/// forms a single molecule. So `-m a.pdb a.xtc -m b.pdb` → two molecules, the first
/// carrying `a.xtc` as a trajectory. Pure logic (no IO), so it's WASM-safe and
/// unit-tested. (The actual frame loading lives in `App::new`.)
pub fn parse_file_args<I, S>(args: I) -> Vec<Vec<PathBuf>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    let mut current: Vec<PathBuf> = Vec::new();
    for arg in args {
        let a = arg.as_ref();
        if a == "-m" || a == "--molecule" {
            // Start a new molecule; flush the one in progress (an empty `-m`, e.g. a
            // leading or doubled flag, just opens a fresh group).
            if !current.is_empty() {
                groups.push(std::mem::take(&mut current));
            }
        } else {
            current.push(PathBuf::from(a));
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

/// eframe wgpu configuration that opts the device into conservative early
/// depth-test when the adapter supports it. The sphere/cylinder fragment shaders
/// write analytic `frag_depth`, which normally forces late-Z → every overlapping
/// fragment is shaded (heavy overdraw on close-up VDW/licorice). The
/// `SHADER_EARLY_DEPTH_TEST` feature (native, Vulkan/GLES 3.1+) lets them keep
/// early-Z via `@early_depth_test(greater_equal)`; we request it **only when the
/// adapter advertises it**, and the renderer injects the attribute only when the
/// device ended up with the feature — so unsupported adapters (and WebGL2/wasm,
/// which never run this path) fall back to the plain late-Z shaders. We mirror
/// eframe's default limits (incl. the `max_texture_dimension_2d = 8192` bump), but
/// allow vertex buffers up to the adapter's supported size for large systems.
/// Shared by the native binary and the
/// Python module (both spawn the viewer with `eframe::run_native`).
#[cfg(not(target_arch = "wasm32"))]
pub fn early_z_wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
    use eframe::wgpu;
    use std::sync::Arc;

    WgpuConfiguration {
        wgpu_setup: WgpuSetup::CreateNew(WgpuSetupCreateNew {
            device_descriptor: Arc::new(|adapter: &wgpu::Adapter| {
                let base_limits = if adapter.get_info().backend == wgpu::Backend::Gl {
                    wgpu::Limits::downlevel_webgl2_defaults()
                } else {
                    wgpu::Limits::default()
                };
                let mut required_features = wgpu::Features::empty();
                if adapter
                    .features()
                    .contains(wgpu::Features::SHADER_EARLY_DEPTH_TEST)
                {
                    required_features |= wgpu::Features::SHADER_EARLY_DEPTH_TEST;
                }
                if crate::performance::enabled() {
                    required_features |= adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
                }
                wgpu::DeviceDescriptor {
                    label: Some("molar_vis wgpu device"),
                    required_features,
                    required_limits: wgpu::Limits {
                        max_texture_dimension_2d: 8192,
                        max_buffer_size: adapter.limits().max_buffer_size,
                        ..base_limits
                    },
                    ..Default::default()
                }
            }),
            ..WgpuSetupCreateNew::without_display_handle()
        }),
        ..Default::default()
    }
}

/// Launch the native viewer window. Returns once the window is closed.
/// Native-only: the web build uses `eframe::WebRunner` from a wasm entry point.
#[cfg(not(target_arch = "wasm32"))]
pub fn run(launch: AppLaunch) -> eframe::Result<()> {
    // `MOLAR_VIS_DEBUG_HIDDEN=1` creates the window **invisible** so headless
    // verification runs (e.g. `MOLAR_VIS_DEBUG_SAVE_IMAGE`, which renders offscreen
    // during `App::new`) never pop a window onto the user's desktop. Pair with
    // `MOLAR_VIS_DEBUG_EXIT=1` (see `App::new`) to quit right after the offscreen work,
    // before the event loop ever presents a frame.
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("molar_vis")
        .with_inner_size([1200.0, 800.0]);
    if std::env::var_os("MOLAR_VIS_DEBUG_HIDDEN").is_some() {
        viewport = viewport.with_visible(false);
    }
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: early_z_wgpu_options(),
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "molar_vis",
        native_options,
        Box::new(move |cc| {
            // File/render verification hooks require synchronous initialization.
            if std::env::var_os("MOLAR_VIS_DEBUG_EXIT").is_some() {
                Ok(Box::new(App::new(cc, launch)?) as Box<dyn eframe::App>)
            } else {
                Ok(Box::new(crate::app::startup::NativeStartup::new(cc, launch)?)
                    as Box<dyn eframe::App>)
            }
        }),
    )
}

/// Launch the viewer in a browser, rendering into the `<canvas id="molar_vis_canvas">`
/// of the host page. wasm-only entry point (the `molar_vis_web` binary calls this);
/// the native build uses [`run`]. Starts eframe's `WebRunner` (wgpu backend, with a
/// WebGL2 fallback) on the async executor and returns immediately.
#[cfg(target_arch = "wasm32")]
pub fn run_web() {
    use wasm_bindgen::JsCast as _;

    // Route Rust panics and `log` output to the browser console. Info level:
    // shows eframe/wgpu adapter + backend selection and any warnings/errors,
    // without naga's very chatty per-expression shader-compilation debug logs.
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    log::info!("molar_vis: starting web runner");

    let web_options = eframe::WebOptions::default();
    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window()
            .and_then(|w| w.document())
            .expect("no document");
        let canvas = document
            .get_element_by_id("molar_vis_canvas")
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("page must contain a <canvas id=\"molar_vis_canvas\">");

        let result = eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| {
                    let mut app = App::new(cc, AppLaunch::default())?;
                    // Open to a bundled molecule so the demo isn't an empty viewport.
                    app.load_demo();
                    Ok(Box::new(app))
                }),
            )
            .await;

        // Surface a startup failure both to the console and into the page (the
        // `#loading` element), so the cause is visible without opening devtools.
        if let Err(e) = result {
            let msg = format!("molar_vis failed to start: {e:?}");
            log::error!("{msg}");
            if let Some(el) = document.get_element_by_id("loading") {
                el.set_text_content(Some(&msg));
            }
        } else {
            log::info!("molar_vis: web runner started");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::parse_file_args;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "requires native GPU; allocates the reported 460 MiB line buffer"]
    fn native_device_accepts_large_geometry_buffers() {
        use eframe::egui_wgpu::WgpuSetup;
        pollster::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default())
                .await.unwrap();
            let WgpuSetup::CreateNew(setup) = super::early_z_wgpu_options().wgpu_setup else {
                panic!("expected the native device configuration");
            };
            let descriptor = (setup.device_descriptor)(&adapter);
            assert_eq!(descriptor.required_limits.max_buffer_size, adapter.limits().max_buffer_size);
            const REPORTED_LINE_BUFFER_BYTES: u64 = 482_740_656;
            if adapter.limits().max_buffer_size < REPORTED_LINE_BUFFER_BYTES {
                eprintln!("adapter cannot support the reported line buffer size");
                return;
            }
            let (device, _) = adapter.request_device(&descriptor).await.unwrap();
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("large-lines-regression"),
                size: REPORTED_LINE_BUFFER_BYTES,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            assert_eq!(buffer.size(), REPORTED_LINE_BUFFER_BYTES);
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        });
    }

    #[test]
    fn dash_m_groups_into_separate_molecules() {
        // The spec example: two molecules, the first with a trajectory.
        assert_eq!(
            parse_file_args(["-m", "a.pdb", "a.xtc", "-m", "b.pdb"]),
            vec![vec![p("a.pdb"), p("a.xtc")], vec![p("b.pdb")]]
        );
    }

    #[test]
    fn no_dash_m_is_one_molecule_with_states() {
        assert_eq!(
            parse_file_args(["a.pdb", "b.pdb", "c.pdb"]),
            vec![vec![p("a.pdb"), p("b.pdb"), p("c.pdb")]]
        );
    }

    #[test]
    fn implicit_first_group_then_explicit() {
        assert_eq!(
            parse_file_args(["a.pdb", "-m", "b.pdb"]),
            vec![vec![p("a.pdb")], vec![p("b.pdb")]]
        );
    }

    #[test]
    fn empty_and_stray_flags() {
        assert!(parse_file_args(Vec::<String>::new()).is_empty());
        // Leading/doubled/trailing `-m` just open (empty → ignored) groups.
        assert_eq!(parse_file_args(["-m", "-m", "a.pdb", "-m"]), vec![vec![p("a.pdb")]]);
    }
}

#[cfg(test)]
mod docking_cli_tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn docking_lists_support_multiple_files_and_either_order() {
        for args in [
            vec!["-r", "protein.pdb", "protein.xtc", "-l", "a.sd", "b.pdb"],
            vec![
                "--ligand",
                "a.sd",
                "b.pdb",
                "--receptor",
                "protein.pdb",
                "protein.xtc",
            ],
            vec![
                "-r",
                "protein.pdb",
                "-l",
                "a.sd",
                "-r",
                "protein.xtc",
                "-l",
                "b.pdb",
            ],
        ] {
            let launch = parse_launch_args(args).unwrap();
            assert!(launch.files.is_empty());
            assert_eq!(
                launch.docking.unwrap(),
                DockingLaunch {
                    receptor: paths(&["protein.pdb", "protein.xtc"]),
                    ligands: paths(&["a.sd", "b.pdb"]),
                }
            );
        }
    }

    #[test]
    fn malformed_docking_arguments_are_rejected() {
        for args in [
            vec!["-r"],
            vec!["-l", "poses.sd"],
            vec!["-r", "protein.pdb"],
            vec!["-r", "-l", "poses.sd"],
            vec!["a.pdb", "-r", "b.pdb", "-l", "c.sd"],
            vec!["-m", "-r", "b.pdb", "-l", "c.sd"],
            vec!["-r", "b.pdb", "-l", "c.sd", "-m", "d.pdb"],
            vec!["--typo"],
        ] {
            assert!(parse_launch_args(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn ordinary_loading_is_preserved_and_double_dash_keeps_paths_literal() {
        for args in [
            vec![],
            vec!["a.pdb", "a.xtc"],
            vec!["-m", "-m", "a.pdb", "-m"],
            vec!["a.pdb", "--molecule", "b.pdb"],
        ] {
            let launch = parse_launch_args(&args).unwrap();
            assert_eq!(launch.files, parse_file_args(&args));
            assert!(launch.docking.is_none());
        }
        assert_eq!(
            parse_launch_args(["--", "-m", "-r"]).unwrap().files,
            vec![paths(&["-m", "-r"])]
        );
        assert_eq!(
            parse_launch_args(["-r", "a.pdb", "-l", "--", "-poses.sd"])
                .unwrap()
                .docking
                .unwrap()
                .ligands,
            paths(&["-poses.sd"])
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_paths_need_not_be_utf8() {
        use std::os::unix::ffi::OsStringExt;
        let path = std::ffi::OsString::from_vec(b"pose\xff.sd".to_vec());
        let launch = parse_launch_args([
            std::ffi::OsString::from("-r"),
            "protein.pdb".into(),
            "-l".into(),
            path.clone(),
        ])
        .unwrap();
        assert_eq!(launch.docking.unwrap().ligands, vec![PathBuf::from(path)]);
    }
}
