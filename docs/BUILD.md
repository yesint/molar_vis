# molar_vis — Build, test & tech stack

> Reference doc for [molar_vis](../CLAUDE.md). Split out of the master `CLAUDE.md` for on-demand reading — see it for the project overview, build quick-start, and the full docs index.

## Build / run / test

```sh
cargo build
cargo run -p molar_vis -- tests/2lao.pdb            # one molecule
cargo run -p molar_vis -- a.pdb a.xtc               # VMD-style: a.pdb + a.xtc traj = ONE molecule
cargo run -p molar_vis -- -m a.pdb a.xtc -m b.pdb   # `-m` starts a new molecule → two molecules
cargo test -p molar_vis_core
cargo build -p molar_vis_core --target wasm32-unknown-unknown   # WASM-readiness check (now green)
cargo build -p molar_vis_py                                     # native Python module (compile check)
wasm-pack build crates/molar_vis_js --target web --out-dir web/pkg   # browser JS API (M27)

cargo run -p molar_vis --features scripting          # + the in-app Rhai console (M31)
cargo test -p molar_vis_core --features scripting    # 96 tests (92 without)
```

**Feature flags.** The one feature is **`scripting`** (M31, **off by default**): the in-app Rhai
console. Every crate has a pass-through of it (`molar_vis`, `molar_vis_web`, `molar_vis_py`,
`molar_vis_js` → `molar_vis_core/scripting`). Verify **both** configurations after touching
anything near `script*` or the `App` console fields — with it off, `rhai` is out of the
dependency graph entirely (`cargo tree -p molar_vis | grep rhai` → nothing).

**Native Python module** (`crates/molar_vis_py`, M26): `import molar_vis` to drive the viewer
from Python/Jupyter (see that crate + the *Native Python module* notes below). Build/install with
maturin into an active venv (pyo3 0.27 builds for CPython up to 3.14 via
`PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`):

```sh
python -m venv .venv && source .venv/bin/activate && pip install maturin numpy
cd crates/molar_vis_py && maturin develop --release   # builds + installs as `molar_vis`
python -c "import molar_vis as mv; s=mv.System('tests/2lao.pdb'); v=mv.spawn(); m=v.add_mol(s); m.add_rep(s('protein'), style='cartoon', color='ss'); import time; time.sleep(30)"
```

Headless verification on this dev box used a scratch venv + `spectacle -b -n -a -o out.png` to
screenshot the window opened from Python. NB: `pkill -f molar_vis-ui` matches its own shell command
line — kill the python process by PID instead.

**Browser JavaScript API** (`crates/molar_vis_js`, M27): the wasm-bindgen face of the viewer — the
web half of the dual-host plan, mirroring `molar_vis_py` so the same script reads almost identically
in Python and JS. A surrounding web page does `import init, { start, System } from "./pkg/molar_vis.js"`,
`await init()`, `const vis = start("canvas_id")`, then the same `add_mol`/`add_rep`/setters/view-controls
as Python. Built with **wasm-pack** (not trunk — a bin can't export an importable ES module). Headless
verification on this dev box used **chromium**: serve a host page + the `pkg/` over `python -m http.server`
(ES modules + wasm need HTTP, not `file://`), run `chromium-browser --headless --no-sandbox --disable-gpu
--virtual-time-budget=25000 --dump-dom <url>`, and read a result the page writes into the DOM (the API
surface — init/start/parse/select/add_mol/add_rep/setters — runs synchronously, independent of the async
WebGL render, so it's verifiable headlessly even without a GPU; only the pixels need a human glance).

- Test assets in `tests/`: `2lao.pdb` (1911 atoms), `2lao_cg.pdb` (238-residue martinized 2lao,
  mixed α/β — the committed **CG cartoon** fixture; regenerate per `tests/README.md` with
  `martinize2`), `large_375k.gro` (375,548 atoms, generated — **not in git**; regenerate per
  `tests/README.md` with `gmx genconf`). `cg.pdb` (a Martini membrane bundle, all-helix; ~4 MB) is a
  committed CG check fixture.
- Dev machine is **Wayland**. Prefer `MOLAR_VIS_DEBUG_SAVE_UI` + `_HIDDEN=1` (above) over any
  external screenshot: `spectacle -b -n -a -o out.png` grabs the **active** window, which is often
  *not* the app (it has captured the user's browser instead), and `-f` full-screen captures blank on
  this compositor. If a real window is ever unavoidable, capture it *immediately* — a fresh window
  only holds focus for a moment.
- Headless verification env hooks (native only): `MOLAR_VIS_DEBUG_REP=vdw|licorice|ballstick|lines|cartoon|surface`
  (+ `MOLAR_VIS_DEBUG_SURF=1` logs surface grid stats),
  `MOLAR_VIS_DEBUG_SEL="<selection>"`,
  `MOLAR_VIS_DEBUG_COLOR=element|chain|resid|resname|index|beta|secstruct|charge`,
  `MOLAR_VIS_DEBUG_ALLCOLORS=1` (one rep per color scheme, cycling styles — shows every icon),
  `MOLAR_VIS_DEBUG_ORBIT=<deg>`, `MOLAR_VIS_DEBUG_ORTHO=1`,
  `MOLAR_VIS_DEBUG_CUEMODE=linear|exp|exp2` (set the depth-cue falloff curve + bump strength so it
  shows in a screenshot),
  `MOLAR_VIS_DEBUG_AO[=strength]` (enable screen-space ambient occlusion),
  `MOLAR_VIS_DEBUG_SHADOW[=strength]` (enable real-time cast shadows) +
  `MOLAR_VIS_DEBUG_SHADOW_SOFT=<0..1>` (set shadow softness — only visible in the ray-traced render),
  `MOLAR_VIS_DEBUG_BG=gradient|white` (set a gradient / white viewport background),
  `MOLAR_VIS_DEBUG_PERSP=1` (force perspective projection) +
  `MOLAR_VIS_DEBUG_ZOOM=<factor>` (dolly out by `factor`),
  `MOLAR_VIS_DEBUG_VIEWMENU=1` (open the view-settings hamburger window at startup),
  `MOLAR_VIS_DEBUG_TRAJ=<path>` (load a trajectory into mol 0, bypassing the dialog) +
  `MOLAR_VIS_DEBUG_FRAME=<n>` (display frame n) + `MOLAR_VIS_DEBUG_TRAJ_FROM/TO/STRIDE=<n>`
  (load range/stride) + `MOLAR_VIS_DEBUG_TRAJ_PLAY=1` (auto-play, exercises the incremental
  update path) + `MOLAR_VIS_DEBUG_BOX=1` (show mol 0's periodic box) +
  `MOLAR_VIS_DEBUG_PBC="px,py,pz"` (set mol 0 first rep's +a/+b/+c periodic image counts + box;
  exercises periodic-image rendering — 2lao has a CRYST1 box) +
  `MOLAR_VIS_DEBUG_SMOOTH=<window>` (set mol 0 first rep's trajectory smoothing window; pair with
  `MOLAR_VIS_DEBUG_TRAJ`) +
  `MOLAR_VIS_DEBUG_PICK=1` (force Click pick mode + pick at the viewport center each frame, so
  the glow/info overlay can be screenshot headlessly; also logs a GPU-vs-CPU pick comparison —
  `pick ok: gpu == cpu == …` — at `RUST_LOG=molar_vis_core=info`) +
  `MOLAR_VIS_DEBUG_SELMODE=residues|boundh` (set the lasso selection-expansion mode; default Atoms) +
  `MOLAR_VIS_DEBUG_PENDING=<selection>` (stage that selection on every **visible** molecule as an
  active/pending selection — exercises the lasso glow highlight + per-molecule accept/discard UI,
  incl. the multi-molecule case, without a mouse drag. Staged through `merge_into_pending`, the
  path the lasso and click-select take, so it also exercises the tree unfolding + scroll-to a real
  capture triggers; *visible* because that is what a lasso can hit — in a group only the shown
  member) + `MOLAR_VIS_DEBUG_ACCEPT_PENDING=1` (then press each stub's ✓, so the committed rep's
  row being unfolded to and scrolled into view is checkable in a `_SAVE_UI` shot) +
  `MOLAR_VIS_DEBUG_AXES=1` (show the VMD-style orientation-axes gizmo) +
  `MOLAR_VIS_DEBUG_MATERIAL=<name>` (set mol 0's first rep material, e.g. Transparent) +
  `MOLAR_VIS_DEBUG_FOCUS=<selection>` (zoom the camera to fit that selection — exercises
  zoom-to-selection) +
  `MOLAR_VIS_DEBUG_SAVE_SESSION=<path>` / `MOLAR_VIS_DEBUG_LOAD_SESSION=<path>` (save the
  startup scene to / replace it from a JSON session file during `App::new` — drives the
  save/load-state round-trip headlessly, since the rfd dialogs can't be; a save→load→save
  round-trip is byte-identical) +
  `MOLAR_VIS_DEBUG_DRAW_REP=<mol>[:<rep>]` (open that rep in **Draw mode**, scoped to its
  selection — so the rep-scoped editing and the **grey-out of the non-active reps** can be checked
  with `MOLAR_VIS_DEBUG_SAVE_IMAGE`; defaults to rep 0) +
  `MOLAR_VIS_DEBUG_SAVE_MOL=<path>` (write mol 0 to a structure file at startup — exercises the
  molar `FileHandler` write + displayed-frame swap path headlessly) +
  `MOLAR_VIS_DEBUG_SAVE_GROUP=<path>` (write group 0's members to one multi-record file — exercises
  the group-save write loop; pair with `MOLAR_VIS_DEBUG_SDF`) +
  `MOLAR_VIS_DEBUG_SAVE_IMAGE=<path>` (+ optional `_W`/`_H`, default 800×600) — render the startup
  scene to a PNG at startup (builds geometry via `rebuild_dirty`, then offscreen render → GPU
  readback → encode), so the "Save image" path is verifiable headlessly without a window +
  `MOLAR_VIS_DEBUG_RAYTRACE=<path>` (+ optional `_W`/`_H`/`_SAMPLES`, default 800×600/128) — same but
  through the **GPU ray tracer** (pair with `MOLAR_VIS_DEBUG_AO=1`/`_SHADOW=1` to see the ray-traced
  AO/shadows, or `MOLAR_VIS_DEBUG_GI=1` for the path-traced global-illumination tier) +
  `MOLAR_VIS_DEBUG_DELFRAMES=1` (open the delete-frames dialog for mol 0 — pair with
  `MOLAR_VIS_DEBUG_TRAJ`) +
  `MOLAR_VIS_DEBUG_SETTINGS=[appearance|rendering|view|reps|behavior]` (open the program-settings
  modal at that tab — `=1`/empty = Appearance — so each tab can be screenshot; the dialog can't be
  mouse-driven headlessly) +
  `MOLAR_VIS_DEBUG_DEFAULTS=1` (use built-in `Settings::default()` and skip the config-file
  read/write, so headless runs are reproducible and never touch the dev's saved config) +
  `MOLAR_VIS_DEBUG_HIDDEN=1` (create the window **invisible** — `ViewportBuilder::with_visible(false)`
  — so an offscreen render never pops a window onto the desktop) +
  `MOLAR_VIS_DEBUG_EXIT=1` (quit at the end of `App::new`, after the file-producing hooks below have
  run but before eframe's event loop presents a frame). **Pair the two** for fully non-interfering
  headless verification: `MOLAR_VIS_DEBUG_HIDDEN=1 MOLAR_VIS_DEBUG_EXIT=1 MOLAR_VIS_DEBUG_SAVE_IMAGE=…`
  writes the PNG offscreen and self-exits in <1 s — no window, no `timeout` needed. (This is the
  **preferred** way to run the SAVE_IMAGE/RAYTRACE/SAVE_SESSION/SAVE_MOL hooks on a live desktop.) +
  `MOLAR_VIS_DEBUG_SAVE_UI=<path>` — write the **whole egui surface** (panels, dialogs, overlays *and*
  the 3D image) to a PNG via `egui::ViewportCommand::Screenshot`, then quit. **Pair with
  `MOLAR_VIS_DEBUG_HIDDEN=1` and egui-panel verification needs no visible window either** — which
  matters because `spectacle -a` grabs whatever window is *active* and has captured the wrong one.
  Unlike the `App::new` hooks it needs real frames (the backend answers the request a frame later,
  and the capture waits out egui's `Area` **fade-in**, which is a function of wall time — too few
  frames and every dialog is caught half transparent, which reads as a theme bug), so it can't be
  combined with `MOLAR_VIS_DEBUG_EXIT=1`; it self-closes when done, so just
  run it under a short `timeout` as a backstop. Implemented in `app/export.rs`
  (`service_debug_ui_capture`, driven at the end of `App::ui`). +
  `MOLAR_VIS_DEBUG_INTERACTIONS=1` (add an `Interactions` rep on mol 0 with a partner rep — mol 1's
  first rep if a second molecule is loaded, else a disjoint-half second rep on mol 0 — and expand its
  panel; exercises the cross-molecule contact detection + dashed-line build; pair with
  `MOLAR_VIS_DEBUG_SAVE_IMAGE`) +
  `MOLAR_VIS_DEBUG_INTERACTIONS_DIALOG=[hbond|hydrophobic|salt|pistacking|pication|halogen]` (open the
  tabbed interaction-settings dialog at that type tab — pair with `MOLAR_VIS_DEBUG_INTERACTIONS=1`) +
  `MOLAR_VIS_DEBUG_THEME=light|dark|system` (pin the theme without touching the saved config, so
  either palette — and the viewport background that follows it — can be screenshot) +
  `MOLAR_VIS_DEBUG_ALIGN_DIALOG=[1|pick]` (open the **Analysis ▸ Align** window; `pick` also enters
  "choose a representation" mode for the source, so the prompt + lit picker button show) +
  `MOLAR_VIS_DEBUG_ALIGN="<src mol>,<sel>,<frame>;<tgt mol>,<sel>,<frame>;<flags>"` (fill that
  dialog and press **Align**, logging the RMSD — flags from `common`/`all`/`whole`/`same`/`rmsd`,
  the last measuring without moving; the target half may be empty with `same`. Runs through the same
  request-building + undo-recording path the buttons do and leaves the dialog open, so a `_SAVE_UI`
  shot shows the readout and a `_SAVE_IMAGE` one shows the moved molecule) +
  `MOLAR_VIS_DEBUG_DOCKING="<protein files>;<ligand files>"` (load a docking result the way the
  dialog's [Load] does — paths comma-separated within each half, e.g.
  `"tests/jak2.pdb,tests/jak2_traj.pdb;tests/jak2_inhs.sd"` — bypassing the file picker) +
  `MOLAR_VIS_DEBUG_DOCKING_POSE=<n>` / `_DOCKING_FRAME=<n>` (then move that side and reconcile,
  logging `pose=… receptor_frame=…`, so the flexible-docking coupling is checkable in each
  direction) + `MOLAR_VIS_DEBUG_DOCKING_DIALOG=1` (open the empty dialog for a `_SAVE_UI` shot) +
  `MOLAR_VIS_DEBUG_CHARGE_KIND=partial|formal` (which charge the `charge` scheme paints — its
  *Color*-tab option; default partial) +
  `MOLAR_VIS_DEBUG_CHARGES=1` (press **[Compute charges]** on every molecule's first rep, as the
  Color tab's button does, and log the outcome — exercises the espaloma path headlessly; pair with
  `MOLAR_VIS_DEBUG_COLOR=charge` + `_SAVE_IMAGE` to see it painted. Needs an SDF/MOL input: espaloma
  requires Kekulé bond orders, so on a PDB it logs the "load an SDF/MOL" advice instead) +
  `MOLAR_VIS_DEBUG_EXPAND_COLOR=1` (expand mol 0's first rep params at the **[Color]** tab, so that
  tab can be screenshot from a window) +
  `MOLAR_VIS_DEBUG_DIHEDRAL[=<mol>]` (enter edit mode with the **DihedralRotate** tool and select the
  first rotatable bond of molecule `<mol>` — default 0 — so the axis + handles overlay can be
  screenshot from a window) +
  `MOLAR_VIS_DEBUG_DIHEDRAL_ROTATE=<deg>` (also twist that bond's J-side by `<deg>`° up front, so a
  `MOLAR_VIS_DEBUG_SAVE_IMAGE` render shows the rotated geometry headlessly) +
  `MOLAR_VIS_DEBUG_SCRIPT="<rhai source>"` (or `@path` to a file, native; **requires
  `--features scripting`**) — runs a console script at startup through the same path the console uses,
  and opens the console window, so a command's effect (e.g. `mol(0).rep(0).set_color("chain")`) + the
  echoed output can be screenshot headlessly. It and `_CHARGES` run **before** the offscreen-render
  hooks, so their effect lands in a `_SAVE_IMAGE` render too. Generate a
  quick test trajectory with the Python snippet that wrote `tests/2lao_traj.pdb` (multi-MODEL, **not
  in git**).

## Tech stack (working versions)

eframe / egui / egui-wgpu **0.34.3**, wgpu **29.0.3**, egui-phosphor **0.12** (icon font),
glam **0.32** (GPU/camera math), nalgebra **0.34** (molar boundary), bytemuck **1.25**,
molar **2.1** (**git dep** `git = "https://github.com/yesint/molar.git"`,
`default-features=false` → `Float=f32`; pulls `powersasa` transitively from git; **edition 2024,
MSRV 1.85** — see the *molar 2.1 API* notes below), molar_ff **2.1** (same git dep, feature
`espaloma` — GAFF/GAFF2 typing + espaloma partial charges; **native-only dependency**, it bundles a
~600 kB ONNX model and pulls `tract`),
**egui-stylesheet 0.1** (the theme sheets — our own published crate, developed in the sibling repo
`../egui-stylesheet` outside this workspace; it pulls `toml` and turns on egui's `serde` feature),
rhai **1** (**optional**, behind the `scripting` feature: `default-features=false,
features=["std"]` — pure-Rust embedded scripting language for the console; builds for wasm).
**`molar_vis_py` only** (native Python module, M26):
pyo3 **0.27** (`extension-module`) + `molar_python` (rlib, the pymolar bindings) + winit **0.30**,
built as a wheel with **maturin**. GROMACS 2026.1 available as `gmx`.

**Installable** — molar and powersasa come from GitHub (no sibling checkouts, no
`[patch]`). `Cargo.lock` pins the resolved git revisions. **`egui-stylesheet`** is a plain
**crates.io** dep (`"0.1"`) — it's published, repo `github.com/yesint/egui-stylesheet`, local
checkout at `../egui-stylesheet`. To develop it alongside molar_vis, temporarily add a
`[patch.crates-io] egui-stylesheet = { path = "../egui-stylesheet" }` — but don't commit it; release
a new version and bump the requirement here instead. To develop molar/powersasa
locally, temporarily add a `[patch."…powersasa-llm.git"] powersasa = { path = "…" }`
and/or point `molar` at a local path — but don't commit those. **The user's local molar
checkout is at `../molar`** (i.e. `/home/semen/work/Projects/molar`; a git clone of
`github.com/yesint/molar` on `master`) — edit it there, `cargo build`/commit/push it,
then bump molar_vis's `rev` in the root `Cargo.toml` + `Cargo.lock`. The `molar` crate is
`../molar/molar`, the PyO3 bindings `../molar/molar_python`; the local dev `[patch]` points
at those two paths.

