# CLAUDE.md — molar_vis

A modern, legacy-free molecular viewer modeled after VMD, in **pure Rust**,
targeting Linux/Windows/macOS/WebAssembly. It builds on **molar** (the user's Rust
molecular library: IO, selections, topology, DSSP) and renders on **eframe/egui +
wgpu** with hand-written WGSL GPU ray-cast impostors.

The user, Semen Yesylevsky, is the author of molar. The full approved plan lives at
`~/.claude/plans/we-are-going-to-rippling-wreath.md`; per-session memory at
`~/.claude/projects/-home-semen-work-Projects-molar-vis/memory/`.

## Build / run / test

```sh
cargo build
cargo run -p molar_vis -- tests/2lao.pdb            # one molecule
cargo run -p molar_vis -- a.pdb a.xtc               # VMD-style: a.pdb + a.xtc traj = ONE molecule
cargo run -p molar_vis -- -m a.pdb a.xtc -m b.pdb   # `-m` starts a new molecule → two molecules
cargo test -p molar_vis_core
cargo build -p molar_vis_core --target wasm32-unknown-unknown   # WASM-readiness check (now green)
```

- Test assets in `tests/`: `2lao.pdb` (1911 atoms), `2lao_cg.pdb` (238-residue martinized 2lao,
  mixed α/β — the committed **CG cartoon** fixture; regenerate per `tests/README.md` with
  `martinize2`), `large_375k.gro` (375,548 atoms, generated — **not in git**; regenerate per
  `tests/README.md` with `gmx genconf`). `cg.pdb` (a Martini membrane bundle, all-helix) is a handy
  CG check but **not in git** (~4 MB, user-supplied).
- Dev machine is **Wayland**; screenshot a running window with `spectacle -b -n -a -o out.png`
  (**`-a` = active window — use this**; `-f` full-screen captures blank on this compositor).
- Headless verification env hooks (native only): `MOLAR_VIS_DEBUG_REP=vdw|licorice|ballstick|lines|cartoon|surface`
  (+ `MOLAR_VIS_DEBUG_SURF=1` logs surface grid stats),
  `MOLAR_VIS_DEBUG_SEL="<selection>"`,
  `MOLAR_VIS_DEBUG_COLOR=element|chain|resid|resname|index|beta|secstruct`,
  `MOLAR_VIS_DEBUG_ALLCOLORS=1` (one rep per color scheme, cycling styles — shows every icon),
  `MOLAR_VIS_DEBUG_ORBIT=<deg>`, `MOLAR_VIS_DEBUG_ORTHO=1`,
  `MOLAR_VIS_DEBUG_CUEMODE=linear|exp|exp2` (set the depth-cue falloff curve + bump strength so it
  shows in a screenshot),
  `MOLAR_VIS_DEBUG_AO[=strength]` (enable screen-space ambient occlusion),
  `MOLAR_VIS_DEBUG_SHADOW[=strength]` (enable real-time cast shadows),
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
  `MOLAR_VIS_DEBUG_PENDING=<selection>` (stage that selection on **every** molecule as an
  active/pending selection — exercises the lasso glow highlight + per-molecule accept/discard UI,
  incl. the multi-molecule case, without a mouse drag) +
  `MOLAR_VIS_DEBUG_AXES=1` (show the VMD-style orientation-axes gizmo) +
  `MOLAR_VIS_DEBUG_MATERIAL=<name>` (set mol 0's first rep material, e.g. Transparent) +
  `MOLAR_VIS_DEBUG_FOCUS=<selection>` (zoom the camera to fit that selection — exercises
  zoom-to-selection) +
  `MOLAR_VIS_DEBUG_SAVE_SESSION=<path>` / `MOLAR_VIS_DEBUG_LOAD_SESSION=<path>` (save the
  startup scene to / replace it from a JSON session file during `App::new` — drives the
  save/load-state round-trip headlessly, since the rfd dialogs can't be; a save→load→save
  round-trip is byte-identical) +
  `MOLAR_VIS_DEBUG_EDIT_REP=1` (open mol 0's first rep selection field in edit mode, so the
  contextual selection-suggestion hint and an invalid selection's in-field red error highlight
  can be screenshot headlessly — pair with `MOLAR_VIS_DEBUG_SEL`) +
  `MOLAR_VIS_DEBUG_SAVE_MOL=<path>` (write mol 0 to a structure file at startup — exercises the
  molar `FileHandler` write + displayed-frame swap path headlessly) +
  `MOLAR_VIS_DEBUG_DELFRAMES=1` (open the delete-frames dialog for mol 0 — pair with
  `MOLAR_VIS_DEBUG_TRAJ`) +
  `MOLAR_VIS_DEBUG_SETTINGS=[appearance|rendering|view|reps|behavior]` (open the program-settings
  modal at that tab — `=1`/empty = Appearance — so each tab can be screenshot; the dialog can't be
  mouse-driven headlessly) +
  `MOLAR_VIS_DEBUG_DEFAULTS=1` (use built-in `Settings::default()` and skip the config-file
  read/write, so headless runs are reproducible and never touch the dev's saved config) +
  `MOLAR_VIS_DEBUG_SCRIPT="<rhai source>"` (or `@path` to a file, native) — runs a console script at
  startup through the same path the console uses, and opens the console window, so a command's effect
  (e.g. `mol(0).rep(0).set_color("chain")`) + the echoed output can be screenshot headlessly. Generate a
  quick test trajectory with the Python snippet that wrote `tests/2lao_traj.pdb` (multi-MODEL, **not
  in git**).

## Tech stack (working versions)

eframe / egui / egui-wgpu **0.34.3**, wgpu **29.0.3**, egui-phosphor **0.12** (icon font),
glam **0.32** (GPU/camera math), nalgebra **0.34** (molar boundary), bytemuck **1.25**,
molar **1.4** (**git dep** `git = "https://github.com/yesint/molar.git"`,
`default-features=false` → `Float=f32`; pulls `powersasa` transitively from git),
rhai **1** (`default-features=false, features=["std"]` — pure-Rust embedded scripting
language for the console; builds for wasm). GROMACS 2026.1 available as `gmx`.

**Installable** — molar and powersasa come from GitHub (no sibling checkouts, no
`[patch]`). `Cargo.lock` pins the resolved git revisions. To develop molar/powersasa
locally, temporarily add a `[patch."…powersasa-llm.git"] powersasa = { path = "…" }`
and/or point `molar` at a local path — but don't commit those.

## Workspace & modules

`crates/molar_vis_core` (library, WASM-safe, all logic) + `crates/molar_vis` (native bin:
argv + logging). **Modern module layout** (`<module>.rs` + `<module>/`, no `mod.rs`).

- `lib.rs` — module decls, `run`/`App` re-exports.
- `launch.rs` — `AppLaunch` (startup files, **grouped per molecule** as `Vec<Vec<PathBuf>>`),
  eframe bootstrap (`Renderer::Wgpu`), and **`parse_file_args`** — the VMD-style command-line file
  grouping (pure logic, WASM-safe, unit-tested): `-m`/`--molecule` starts a new molecule; within a
  group the **first file provides the topology** and **all frames of the group's files form the
  trajectory**. The native bin (`crates/molar_vis/src/main.rs`) parses argv (incl. `-h`/`--help`)
  into the groups; `App::new` loads each group's structure, then appends the **first file's frames
  beyond frame 0** (so a multi-MODEL/trajectory structure file contributes all its frames, like VMD)
  **plus** every extra file's frames via `read_frames_sync` (native-only); files that yield no extra
  frames aren't recorded as trajectory loads, so a plain single-frame structure stays static. So
  `traj.pdb` = one molecule with its full trajectory, `a.pdb a.xtc` = one molecule with a trajectory,
  `-m a.pdb -m b.pdb` = two molecules.
- `app.rs` — `eframe::App`; owns `SceneRenderer`, `Camera`, `Scene`; left panel
  (Scene/Molecules/Representations/Controls) + central viewport; `rebuild_dirty()`
  and the render-skip logic. Holds the `MOLAR_VIS_DEBUG_*` hooks.
- `theme.rs` — `apply(ctx, &AppearanceSettings)`: installs the Phosphor icon font, configures both
  the dark (custom high-contrast) and light styles + the accent/font-scale from settings, and
  `set_theme`s the chosen `ThemeMode` (Dark/Light/System). Called at launch and on a settings change.
- `camera.rs` — quaternion arcball `Camera`. VMD mouse nav (in `app.rs::draw_viewport`):
  LMB orbit · **Shift+LMB `roll`** (screen-plane, about the view axis) · RMB (or MMB)
  `pan` · **Shift+RMB `zoom_drag`** (dolly along view Z) · wheel `zoom_scroll` (**zoom-to-cursor**:
  takes the cursor NDC + aspect and pans `target` so the world point under the cursor stays put —
  the focal-plane half-height is `distance·tan(fov/2)` for both projections, so the offset scales
  with distance). Perspective
  **and** orthographic projection. `frame_bbox`/`focus_bbox` use `fit_distance` (fit the
  bbox's **longest dimension to ~90%** of the viewport; bounding-sphere radius still drives
  near/far). Also owns the view-state knobs the top-bar menu edits: `depth_cue`/`ao`/`shadow`,
  `background` (`Background { Solid|Gradient, color/top/bottom }`) — all `serde(default)`, so
  sessions save/load them for free. `#[derive(PartialEq)]` drives render-skip.
- `color.rs` — CPK element colors → packed RGBA8 (`u32`); `ColorMethod`, `Colorizer`.
- `secstruct.rs` — `SsMap` (per-residue SS keyed by `resindex`), `SsClass` (helix/sheet/coil),
  VMD `ss_color`. Shared by the Cartoon rep and the SecStruct color scheme. **Coarse-grained
  (Martini) path** (`assign_cg_ss`, M22): when the residues are CG `BB` beads (no atomistic `CA`),
  DSSP can't run (it needs the N/CA/C/O backbone), so SS is inferred **geometrically** from the BB
  trace's *virtual bond angle* θ (∠ BBᵢ₋₁,BBᵢ,BBᵢ₊₁) and *virtual dihedral* τ (over four BB) — both
  scale-invariant, so they transfer despite BB spacing (~0.32 nm) ≠ Cα (~0.38 nm): helix
  `θ∈[80,118]°, τ∈[−100,−20]°`; sheet `θ≥122°, τ≥120° | τ≤−150°` (`vangle`/`vdihedral`, windows
  calibrated against mdtraj-DSSP on a martinized α/β protein). A **β-pairing filter** then drops any
  extended residue with **no non-sequential partner BB within 0.6 nm** (CG has no H-bonds, so this
  is what stops inventing spurious strands — lifts strand precision 0.59→0.92), followed by
  single-residue gap-fill and demotion of helix runs <4 / sheet runs <2 to coil.
- `geometry.rs` — `RepKind`, `RepParams` (**per-style enum**), `GeometryData`/`MeshData`;
  `build(system, sel, bonds, params, color)` binds the `Sel` (`system.bind`), reads
  positions/atoms via `iter_particle` (nothing cached), and dispatches on `params`. Spheres
  come from the selected atoms; bonds are half-bond split, colored by each atom. Computes a
  `SsMap` once when the rep is Cartoon or colored by SecStruct. **PBC dashed half-bonds** (gated by
  `build`'s `dashed_pbc` arg — the *Dashed wrap-around bonds* setting; when off, `pbox = None` and
  all bonds draw as plain solid half-bonds): the box is read from the bound (`BoxProvider::get_box`).
  Per bond, a **cheap ½-box pre-test** (`wrap_thresh2` = `(½·shortest lattice vector)²`) skips the
  two `PeriodicBox::closest_image` calls for the non-wrapping majority — a real covalent bond is
  short, so it can only wrap if the atoms sit > ½ box apart in raw coords. A bond that does cross a
  box face is drawn as two **dashed** stubs (`dashes()`) running from each atom **to its partner's
  nearest image** (`half_bond_ends`: `a→b_image`, `b→a_image` — the full bond toward the image, not
  beyond it) — so they cross opposite faces, reach where the partner actually is in the nearest cell,
  and nothing crosses the box interior (no long-line artifact). Non-wrapping bonds use the usual
  solid midpoint split.
  Applies to cylinders (Licorice/BallAndStick) and lines. **Cartoon over PBC** (`cartoon.rs`):
  runs are split at a PBC jump between consecutive Cα (`is_pbc_jump`), so the ribbon never crosses
  the box. A run ending at such a jump is **extended one residue past the face** with a *ghost*
  control point at the across-boundary partner's nearest image (`ghost_of`); the ribbon stays 100%
  opaque up to the box face (`PeriodicBox::is_inside`), then the part **beyond** the face is
  **dashed** — opaque stripe rings with transparent gap rings (`STRIPE_RINGS`, per-ring; matching
  the dashed bonds; no fade). The mesh material stamping in `build` *multiplies* (not overwrites)
  the per-vertex alpha so the transparent gap rings survive.
- `geometry/cartoon.rs` — per-chain spline through Cα using VMD's **modified Catmull-Rom
  basis (slope 1.25, interpolating)** + 12 subdivisions — helices genuinely coil but the
  slope-1.25 tangents make the loops round/smooth (standard CR slope 2 looked angular). SS
  classes are cleaned first: β-bridge → coil and single-residue helix/sheet runs demoted to
  coil (else spurious stubs/arrows). Ribbon orientation = VMD's
  **renormalized cumulative-average perp** (`D=(A×B)×A` from the previous carbonyl, flipped to
  the running `g`, then `g=normalize(g+D)`; the running average is what keeps helix ribbons
  flat — using the raw per-residue normal garbles them). **`g`/`D` must be at Ångström scale**
  (`NM_TO_ANGSTROM`): the average mixes unit `g` with `|D|∝length³`, so nm coords (|D|≈0.02)
  freeze the frame → rippled helices + ~90°-rotated sheets; Å (|D|≈17) is what VMD relies on.
  Only β-strand coords are smoothed
  (`(2·CAᵢ+CAᵢ₋₁+CAᵢ₊₁)/4`); helix/coil keep raw Cα. Elliptical cross-section (width axis =
  perp, thickness axis = tangent×perp) morphing by `SsClass` (helix=sheet flat ribbon, coil
  tube); emits indexed `MeshData`. Mirrors VMD `draw_cartoon_ribbons`. **β-arrowheads**
  (`arrow_regions`/`width_at`): per contiguous sheet run, a sharp barb (a width discontinuity at
  the base) flaring to `arrow_base` then a linear taper to a point at the strand's last Cα (then
  ramping back up into the following coil) — the only departure from the original ellipse path.
  (A degenerate/zero normal — failed frame, arrow tip — is guarded in `mesh.wgsl` so it doesn't
  `normalize`→NaN→white on NVIDIA.) Every emitted vertex is tagged with its source `resindex` in
  `MeshData::vert_res` (parallel to `vertices`, not uploaded) so the selection glow can extract a
  given residue's ribbon segment from the *exact* parent mesh (`cartoon_cache` + `cartoon_submesh`).
  **Coarse-grained (Martini) helices** (M22; `cg` path, detected by `BB` beads + no `CA`): a CG
  backbone has no carbonyl to orient the ribbon, and the BB beads spiral the helix axis at
  ~100°/residue (3.66 res/turn, 0.55 nm pitch, ~0.18 nm radius — measured), so the all-atom
  carbonyl-frame machinery can't apply (every backbone-derived flat frame either twists into a
  candy-screw or goes edge-on). Instead the helix is a **flat ribbon wrapped on the helix
  cylinder's surface**: (1) collapse the spiralling BB trace onto a smooth local **axis** (windowed
  centroid over ~a turn + a helix-only Laplacian low-pass, clamped to the run); (2) per residue the
  outward **radial** (raw BB − axis, ⟂ the axis tangent) is the ribbon **normal** (broad face out),
  and the centerline rides the cylinder at `axis + radius·radial` (`cg_helix_ribbon`; `radius` = the
  helix's own mean BB-to-axis distance × `RADIUS_SCALE` 1.25 ≈ the all-atom Cα helix radius). The
  **phase** comes from a parallel-transported frame (`e1`), the measured angle unwrapped to
  monotonic, then made **uniform** by linear interpolation **anchored to the measured phase at both
  ends** — equal turns in the middle, but endpoints pinned to the real backbone so the coil/sheet
  connect without a detour (a least-squares slope put the end turn on the wrong side of the cylinder
  → a weird ribbon "extension"). **Helix-interior** segments are evaluated as an **analytic helix**
  (`cg_helix_sample`: a CR spline on the *smooth axis* — well-spaced, no overshoot — plus the
  analytic rotation `radius·(cosφ·e1+sinφ·e2)`), *not* by CR-splining the ~3.7 surface control
  points per turn (which overshoots → overlapping turns). **Helix↔coil boundary** segments
  (`cg_boundary_centerline`) use a **Hermite** whose helix-side tangent is the true spiral tangent
  (`hermite`), so the ribbon flows out of the last turn straight into the coil tube instead of a CR
  spline swinging back and laying a doubled stub over the last turn. The ribbon **half-width tapers**
  from full to the coil radius over ~2 residues at each run end (`cg_res_width`, smoothstep) so the
  flat tape blends into the thin loop tube. β-sheets keep the SC1-oriented arrow ribbon; coil stays
  a round tube. The CG data (axis/`e1`/phase/radius per residue) is carried on `RunCtx` for the
  analytic sample. Verified on `tests/2lao_cg.pdb` (α/β) and a Martini membrane bundle from many
  angles. **Flat-ribbon shading** (`emit`, applies to **all-atom too**): a flat cross-section
  (half-thickness ≪ half-width — helix/sheet) gets a **constant ±normal on its two broad faces**
  (crisp flat tape) rather than the elliptical normal, which fans ~180° across the broad face and
  shades the ribbon like a domed lens (foreshortened helix turns then read as solid blobs); round
  cross-sections (coil tube) keep the smooth ellipse.
- `scene.rs` — `Scene { molecules, selected_mol, trash }`, `Molecule` (molar `System` +
  guessed `bonds` + bbox + `reps`; the `System` is the single source of per-atom data),
  `Representation` (kind / params / `sel_text` (editable buffer) / `expr: SelectionExpr`
  (compiled) / `sel: Sel` (evaluated) / `periodic: PeriodicParams` (image counts + Self/Box,
  in `EditState`) / visible / dirty flags / `RepGpu`), `evaluate()`
  (text → `SelectionExpr` → `Sel`). `Molecule` also owns a `trajectory: Trajectory` and the
  `seed_frame0`/`append_frames`/`push_frame`/`apply_current_frame` methods (see *molar integration*),
  plus a `source: MoleculeSource` (`File(path)`/`Bytes{name}`) and `traj_loads: Vec<TrajLoad>`
  (the trajectory files loaded into it, in order) — both for session save/load (see `session.rs`).
- `session.rs` — **save/load visualization state** (M13). `Session { format, version, view:
  ViewState, molecules: Vec<MolSession> }`, serialized to JSON. The design goal is
  *extensible-without-ceremony*: the per-rep document is serialized through the **same**
  `history::RepState` undo/redo uses, so a new undoable rep field is saved/loaded **for free**
  (no second site to update); the only manual seam is global `ViewState` (camera + view-toolbar
  toggles) via `App::view_state`/`apply_view_state`. Every field is `#[serde(default)]` →
  forward/back-compatible (unknown fields ignored, missing ones default), so older/newer files
  still load. Molecules are referenced **by source path** (reloaded from disk), not embedded —
  embedding atoms is the separate "save molecules to file" roadmap item. `MolSession` carries
  source / reps (`RepState`) / visibility / show_box / `traj_loads` / `current_frame`. Pure
  data + serde (no IO, WASM-safe); the native `Session` menu (New/Save/Load) + rfd dialogs +
  `std::fs` + scene-reload live in `app.rs`: `save_session`/`load_session` → `_to`/`_from`
  workers; `new_session` (+ shared `reset_document`) starts an empty scene; `apply_session`
  reloads each molecule via `data::load`, rebuilds reps, replays trajectories with
  `read_frames_sync`, applies the view state, and resets the undo history — loading a session (or
  New) = opening a document, not an undo step.
  `SsAlgorithm` (foreign, no serde) rides a `#[serde(remote)]` shim in `history.rs`; `Camera`
  derives serde via glam's `serde` feature.
- `settings.rs` — **persistent program settings** (M21). `Settings { format, version, appearance,
  rendering, view, reps, behavior }`, serialized to JSON in the platform config dir
  (`directories::ProjectDirs::from("","","molar_vis")` → `~/.config/molar_vis/settings.json` on
  Linux). These are the launch-time defaults that used to be hardcoded: `AppearanceSettings`
  (theme mode / font scale / accent — `theme.rs`), `RenderingSettings` (SSAA / shadow-map res —
  `render.rs`), `ViewDefaults` (projection / depth-cue / AO / shadow / background / fit-fraction,
  seeded onto a **new** scene's camera via `ViewDefaults::seed_camera`), `RepDefaults` (new-rep
  style / color / material / selection / surface-quality — `Representation::from_defaults`),
  `BehaviorSettings` (mouse sensitivity, default pick/selection mode, trajectory fps/loop,
  bond-guessing thresholds + **periodic search** → `data::BondParams`, and **`dashed_pbc_bonds`** —
  the only live render toggle here, applied by marking all reps `geom_dirty` on Save). Same design
  as `session.rs`: pure data + serde,
  WASM-safe, every field `#[serde(default)]` with `Default` impls reproducing the **exact** old
  constants (a fresh config = old behavior); forward/back-compatible. Native IO
  (`load_or_create`/`save`/`config_path`, `#[cfg(not(wasm))]`) creates the file with defaults on
  first launch, and on a parse error backs the bad file up to `*.bak` and resets. The browser keeps
  settings in memory (no filesystem). The dialog UI + apply logic live in `app.rs` (cogwheel
  button → `draw_settings_dialog`; `apply_settings`); the **app-global** knobs (theme, SSAA, shadow
  map) apply live on Save, the **new-document defaults** (view/rep/behavior) are read when the next
  scene/molecule is created and never mutate the open document. The dialog is a **free, movable
  `egui::Window`** (not a centered `Modal` — a Modal re-centers each frame so its top jumps as the
  per-tab content height changes; a top-anchored fixed-width Window grows/shrinks only at the
  **bottom**), closed via Save / Cancel / Escape. 4 round-trip/default/compat tests.
- `script.rs` (+ `script/{command,console}.rs`) — **in-app Rhai scripting console** (M24). A
  togglable Console — a **resizable bottom `Panel::bottom`** (View menu → `[x] Console`; the input
  field auto-focuses on open via `console.focus_input`), *not* a floating window. The input row is a
  nested `Panel::bottom` (keeps the outer panel at its set height — computing a scroll height from
  `available_height` instead fed back and blew the panel to full size); the field is `add_sized`
  with the Run (↵) button to its right (a plain row — a `right_to_left` + INFINITY-width field also
  broke the sizing), close is the phosphor `X`. The user types **Rhai** commands in a **fluent,
  object-oriented** style:
  `mol(i)` → a `MolHandle`, `.rep(j)` → a `RepHandle`, with chaining
  (`mol(0).rep(0).set_style("vdw").set_color("chain").select("protein")`;
  `mol(0).add_rep("cartoon").set_color("ss")`). **Command-queue binding**: the handles are
  lightweight (a molecule index + a [`RepRef`] + a clone of a shared `Rc<RefCell<Vec<Command>>>`);
  Rhai closures can't borrow `&mut App`, so the handle methods **push** `Command`s
  (`script/command.rs`) during eval and never touch the scene; `print`/`debug`/`list()` route text
  into an output buffer. **`RepRef {Index, Last}`** lets `add_rep` return a handle to the
  just-appended rep (`Last` resolves to the molecule's last rep at apply time) so further `.set_*`
  chain onto it. `evaluate_script(source, summary)` builds a *local* `Engine` (operation/call/expr-
  depth limits; `register_type_with_name` for the two handles), runs, and returns
  `EvalOutcome { commands, output }`. `App::run_script` echoes the line, appends the output, applies
  each command, then records **one** undo checkpoint.
  `apply_scene_command(scene, camera, rep_defaults, cmd)` (the testable, GPU-free seam) does the
  *same field-set + dirty-flag the GUI does* for every command except `Load` — `select` → `sel_text`
  + `sel_dirty`, `set_color`/`set_style`/`set_material` → `geom_dirty`, `add_rep`/`delete_rep`/
  `show`/`hide`, `frame`/`play`/`pause`, `focus` → `camera.focus_bbox` — converging on the normal
  `rebuild_dirty` path with no new render branch (`resolve_rep` maps `RepRef::Last` → last index);
  `App::execute_command` handles `Load` (native `data::load_with` + `add_loaded`; wasm → "not
  available") and delegates the rest. Enum args (color/style/material) ride as raw strings, parsed
  (with `parse_color`/`parse_material`/`RepKind::from_name`) in `apply_scene_command` so a bad value
  is one clean console error. `mol(i)`/`load(path)`/`list()` are the only free functions; `list()`
  reflects the **pre-script** scene summary. Pure-Rust + WASM-safe (the console runs in the browser;
  only `load()` is native-gated). 5 unit tests
  (parse→commands, chaining/loops, syntax-error-not-panic, color-parser, apply→scene). `script/console.rs`
  is pure UI (`ScriptConsole` state + `show(ui, …)` builds the bottom panel — drawn in the panel
  sequence *before* `draw_viewport` so the 3D view fills the space above it; scrollback fills the
  middle with the **input row pinned to the bottom via a nested `Panel::bottom`** so the prompt stays
  visible/editable at any height; Enter via the rename-dialog focus idiom, ↑/↓ history recall, ✕
  close). See M24.
- `suggest.rs` — **selection-input assistance** for the rep selection field (M14). `SelHints`
  (distinct chains / resnames / names + resid/resindex/index ranges, computed once from the
  static topology and cached per molecule on `App::sel_hints`); `SelHints::hint_for(text)` finds
  the **last grammar keyword** in the text and returns a one-line hint (`chains: A B C R`,
  `resid: 2..120`, `index: 0..N`, capped value lists with `… (+N)`). `parse_sel_error(raw)` parses
  molar's parse-error string (`"syntax error: \n<text>\n----^\nExpected <…>"`) into a concise
  message + the **caret char-offset** the `^` points at. Pure logic, WASM-safe. The field draw
  (`app.rs::sel_text_edit`) uses a `TextEdit` **layouter** to paint the text from the caret offset
  to the end **red** (in-place error highlight); the hint renders under the focused field
  (`active_hint` in `draw_reps_for`).
- `trajectory.rs` — `Trajectory { frames: Vec<State>, current, playing, loop_mode, speed_fps, … }`
  (`n_frames`/`has_playback`/`set_current`/`step`/`tick`), `LoadOptions {from,to,stride}`,
  `LoadMode {Sync,Async}`, `LoadMsg {Frame,Done,Error}`. Pure data + playback math, **WASM-safe**.
- `data.rs` + `data/loader.rs` (`RawMolecule`: System + guessed bonds + bbox; positions/
  radii are transient, used only for bond guessing) + `data/bonds.rs` (VDW-fraction filter)
  + `data/traj_loader.rs` (**native-only**, `#[cfg(not(wasm))]`: `read_frames_sync`/`spawn_async`).
- `render.rs` — `SceneRenderer`: offscreen color + `Depth32Float` targets (Strategy A) **plus
  Weighted-Blended OIT `accum` (RGBA16F) + `reveal` (R16F) targets** (in `Targets`, with an
  `oit_bind_group` for the resolve), **dynamic-offset** camera UBO (bind group 0; an array of
  `CameraUniform` at `CAMERA_STRIDE`=256 — entry 0 is the base camera, one extra per **periodic
  image** = base view × `Mat4::from_translation(i·a+j·b+k·c)`, grown/`make_camera_bind_group`'d as
  needed), sphere/cylinder/line/**mesh** pipelines (each `[opaque, oit, glow]` — index `GLOW=2`
  is additive cyan, depth-test `≤`, no depth-write) + a fullscreen **`composite_pipeline`**
  (`oit_bgl`), `RepGpu` (per-rep buffers; mesh = vertex + u32 index buffer; buffers carry
  `COPY_DST`; `has_geometry()`), `upload()` (recreate buffers), **`update()`** (in-place
  `write_buffer` when element counts match, for coords-only frame changes), `render_scene()` (builds
  the per-image camera list + `images[mol][rep]` = camera indices, then up to **4 passes**: opaque →
  OIT → composite → **glow** (`draw_glow` draws each molecule's `glow_gpu` for the active-selection
  highlight; skipped when none); `draw_reps` loops a rep's images, selecting each image's camera by
  **dynamic offset** — same geometry buffers re-drawn shifted, **no data duplication**; the box
  wireframe is replicated at each image cell of any rep with periodic `Box` on, + the molecule-level
  box at entry 0), `texture_id()`. Plus `render/{sphere,cylinder,line,mesh,camera_uniform}.rs` and
  `render/shaders/*.wgsl` (incl. `oit_composite.wgsl`; lit shaders carry `fs_main` + `fs_oit` +
  `fs_glow`; the `build_pipeline`s take `depth_compare`). The cartoon mesh writes real depth and
  interleaves correctly with the impostors.
- `pick.rs` — atom picking (`PickMode {Off, Click, Lasso}`, `PickHit` (carries the hit `mol` +
  atom `id`), `cursor_ray`, `ray_sphere`, `effective_radius`, `pick` = CPU ray-cast; native hover
  uses the GPU id-buffer instead — `hit_for_atom` rebuilds a `PickHit` from the decoded
  `(mol, rep, atom)`) **and lasso selection** (`lasso_select`,
  `point_in_polygon`, `index_selection_string`, `LassoSelection`). Hit-tests the cursor/lasso
  against atoms **as displayed** (smoothed + periodic images, sharing `PeriodicParams::offsets`
  with the renderer) and reports the atom's **real** stored coordinate. Both hover-pick and lasso
  share `atom_in_rep(kind, name)` — the **style-specific contribution filter**: a Cartoon rep is
  hit only on its **backbone** atoms (`cartoon_atom`: N/CA/C/O + terminal OT1/OT2/OXT — what the
  ribbon is built from, never side chains); every other style hits all selected atoms (Lines
  included, via its isolated-atom crosses). Drives the hover-info overlay
  (`draw_pick_overlay`/`draw_glow_ring` in `app.rs`). The lasso result is staged as a molecule's
  active (pending) selection, highlighted by a GPU glow pass (not an egui overlay) — see *active
  selection* under M11. **`SelectionMode` + `expand_selection`** (toolbar dropdown next to the pick
  selector; `App::selection_mode`): how a lasso/hover expands its raw hits per molecule — `Atoms`
  (exact), `Residues` (any hit residue selected whole), or `BoundH` (hit **heavy** atoms + the H
  bonded to them via the guessed `bonds`; a hit H whose heavy atom isn't selected is dropped).
  `Residues` grows each hit by **walking outward by atom index** (down then up) while `resindex`
  holds — residues are contiguous index runs, so this is O(residue size), never a full-system scan
  (`system.topology().get_atom(i)` is identity-indexed). Applied to each lasso gesture's hits in
  `finish_lasso` *before* the set op, and to the hovered atom in `draw_viewport` (Residues →
  whole-residue highlight). `BoundH` is lasso-only (`App::effective_selection_mode` falls back to
  Atoms for hover).
- `spatial.rs` — `AtomGrid`: a uniform spatial grid of atom positions for **ray-neighborhood**
  queries (`atoms_near_ray`), the inverse of `within`/`dist point` — the cursor is a *line*, and a
  line spans the box so molar's `dist line` is brute O(N). The grid (mirroring molar's distance-search
  grid, minus the periodic part: bin into `extent/dims` cells, flat `x + y·dx + z·dx·dy`) walks only
  the cells in the ray's R-tube (sub-cell march + R-skirt, dedup), so a query is O(tube + nearby), not
  O(N). Pure logic, WASM-safe; 3 unit tests.
- **Hover detail lens** (QoL, `app.rs` + `scene.rs` `HoverDetail`): in Hover mode, the **front-facing
  residues** under the cursor **view line** of a visible **Cartoon/Surface** molecule are shown as a
  distance-faded **CPK ball-and-stick** aid over the ribbon/surface — to hint *where the atoms are*. It
  is **driven by the cursor ray, NOT a pick hit** (`draw_viewport` triggers it whenever the cursor is
  in the viewport, picking the molecule with the most atoms in the tube), so it appears **between**
  atoms / in surface dimples too — that's the whole point. A lazily-built, frame/geom-invalidated
  `Molecule::hover_grid` (`AtomGrid`) holds the lens **seed** atoms (which residues the line passes
  near): **Cartoon → the N–CA–C chain trace** (no carbonyl/terminal backbone oxygens — what the ribbon
  traces); **Surface → solvent-exposed only** (per-atom SASA `bound.sasa().areas() > 0.01 nm²`, not
  deep-buried atoms). The query (`AtomGrid::atoms_near_ray_t`, which returns each hit's signed `t`
  along the ray) keeps only the seeds on the **near (camera-facing) half** along the ray (`t ≤
  midpoint` of the hit `t`-range — so the far side no longer bleeds through the cleared-depth overlay)
  and **expands them to whole residues** (`pick::expand_selection` Residues), so complete front
  residues poke through. `build_hover_detail` builds Ball-and-Stick (Element color) and `fade_by_ray`
  sets each element's alpha by perpendicular distance to the ray (opaque on-axis → 0 at the fade
  radius, **R·1.8** — widened past the R-tube selection radius so whole residues' side chains stay
  visible). Stored in `Molecule::hover_detail` / `hover_detail_gpu` (rebuilt in `rebuild_dirty` when
  the cursor moves), drawn last (`draw_hover_detail`, render pass 5) with the opaque pipelines over the
  composite with a **freshly cleared depth** — so it reveals the atoms *over* the ribbon/surface
  (depth-testing the scene would let the opaque geometry occlude the very atoms being exposed) while
  still self-occluding correctly; the near-half filter is what keeps it from also revealing the *back*
  surface the cleared depth would otherwise expose. Trajectory caveat: the grid/eval use the displayed
  frame's coords (grid invalidated per frame).

## Key architecture

- **Strategy A rendering** — the 3D scene is drawn into our *own* offscreen color +
  depth textures, then composited into egui as an `Image`. egui's render pass has no
  depth attachment; this gives full depth control for impostors.
- **Anti-aliasing = SSAA** (`SSAA` in `render.rs`, default 2×) — the offscreen targets are
  allocated at `SSAA×` the viewport (clamped to `max_texture_dimension_2d`); egui's existing
  `FilterMode::Linear` downsamples into the 1× image rect (a 2×2 box average). This smooths
  **everything**, crucially the **impostor silhouettes** (decided per-pixel by `discard`, so MSAA
  can't touch them) as well as the cartoon mesh and lines — no MSAA targets / depth-resolve / OIT
  rework. The camera viewport param (`params.yz`) stays at the **logical** size so fat-line pixel
  widths come out correct after the downsample (a 2× target with logical viewport → line is `w`
  final px). Cost: `SSAA²`× fragments per re-render; idle still 0 GPU (render-skip unchanged).
- **Impostors** — spheres & cylinders are GPU ray-cast in fragment shaders that write
  analytic `frag_depth`, so they occlude correctly against each other (and, later, the
  cartoon mesh). The camera uniform carries a perspective flag: perspective uses an
  eye-ray from the origin; **orthographic uses a parallel ray with origin on the camera
  plane (z=0)** so the near hit has t>0 (a past bug black-screened ortho). Lines are
  **screen-space fat-line quads** — WebGPU only rasterizes 1px `LineList`, so each segment
  (a pair of `LineVertex`, which now carries a per-vertex `width` px) is reinterpreted as
  **instanced** data (stride = 2 verts) and drawn as a `TriangleStrip` quad expanded
  perpendicular to the segment by `width` px in `line.wgsl` (uses the viewport size carried
  in the camera uniform's `params.yz`); width stays constant in pixels at any zoom, like VMD.
  Half-bond coloring = two half-segments per bond, colored by each endpoint atom.
- **Depth cueing (fog)** — fog fades all geometry toward the background (`BG` in
  `render.rs`, also the clear color) by eye-space distance, with three VMD-style falloff
  **`CueMode`s** (matching the OpenGL fog equations): **Linear**, **Exp** (`1−e^(−k·t)`), **Exp²**
  (`1−e^(−(k·t)²)`), all normalized to reach full fog at the far plane so switching modes keeps the
  far-fog at `strength` and only changes the ramp shape. The camera uniform carries
  `cue = [near, far, strength, mode]` (eye-space, derived per frame by `Camera::cue_uniform`
  from `distance`/`scene_radius` + the scene-relative `DepthCue { enabled, start, strength, mode }`
  on `Camera`) + `fog_color`. Every fragment shader applies the shared `apply_fog(color, eye_z)`
  (computes normalized depth `t∈[0,1]`, selects the curve by `cue.w`); line/mesh pass eye-space `z`
  as a varying, the impostors use their ray hit. Lives in `Camera` so its `PartialEq` re-renders on
  change; the top-view-toolbar depth-cue popup has the **mode tabs** (`tab_bar`) + Strength/Start
  sliders and stays open until you click outside/the button (`CloseOnClickOutside`).
  `MOLAR_VIS_DEBUG_CUEMODE=linear|exp|exp2` sets it headlessly.
- **Ambient occlusion (SSAO)** — a fullscreen pass (`render/ssao.rs` + `shaders/ssao.wgsl`) inserted
  after the opaque pass: it reads the scene **depth** (the depth target now carries `TEXTURE_BINDING`
  so it's sampleable; impostors' analytic `frag_depth` makes it exact), reconstructs view-space
  positions via the inverse projection, and estimates occlusion **without normals** — for each pixel
  it counts neighbours (a fixed golden-angle spiral kernel, world-radius scaled to screen by the
  projection) that sit *in front* of it in view space, so creases/contacts darken but flat surfaces
  don't self-shadow. The AO factor is written back with a **multiply blend** (`result = dst×ao`)
  onto the opaque color before the OIT composite — no extra targets, no separate blur (the 2× SSAA
  downsample smooths the mild banding from the unrotated kernel). Settings live in `Camera::ao`
  (`Ao { enabled, strength, radius }`, off by default) → re-renders via `PartialEq`, serialized in
  sessions; `Camera::ao_uniform` feeds the pass `[radius, bias, strength, enabled]`. **Gated to full
  WebGPU** (`ssao_pipeline: Option`, built only when `oit_enabled`): WebGL2 can't reliably sample
  the depth texture, so it skips SSAO rather than risk a startup shader-compile failure. Works on
  both impostors (VDW) and meshes (surface/cartoon). `MOLAR_VIS_DEBUG_AO[=strength]` enables it.
- **Cast shadows (real-time shadow mapping, deferred)** — VMD's ray-traced shadows, but real-time.
  An extra **shadow pass** (pass 0, before opaque, only when `Camera::shadow.enabled`) renders the
  opaque geometry from a **key light** into a fixed `2048²` `Depth32Float` shadow map
  (`shadow_depth_view`); a throwaway color target (`shadow_color_view`) lets us **reuse the existing
  opaque pipelines** for the depth fill (impostors compute correct light-space analytic `frag_depth`
  because the light camera is just another `CameraUniform` entry — ortho, `perspective=false` — so
  **no depth-only pipeline variants are needed**; `draw_shadow_casters` draws spheres/cylinders/mesh
  only — lines/box don't cast). The light is directional (`SHADOW_LIGHT_DIR_VIEW`, a view-space
  upper-right key off the view axis so shadows fall on camera-visible surfaces — a near-camera
  headlight would hide them); its **orthographic frustum is fit to the scene's bounding sphere**,
  recovered from `view` + `depth_range`. The shadow is then applied **deferred in the AO pass**: the
  SSAO shader already reconstructs each pixel's view-space position, so it also projects it to the
  light's clip space (`shadow_matrix = light_proj·light_view·inv_view`, carried in `SsaoUniform`),
  does a 3×3 PCF `textureSampleCompareLevel` against the shadow map, and folds the result into the
  same multiply-blend (`output = ao × shadow_factor`). So **no lit-shader changes and no new
  pipelines** — one extra geometry pass + a shadow sample in the existing fullscreen pass. The AO
  pass now runs when *either* AO or shadows are on (AO strength 0 when AO is off). `Camera::shadow`
  (`Shadow { enabled, strength }`, off by default, serialized) → `shadow_uniform` = `[strength,
  bias, enabled, _]`. **Gated to full WebGPU** like SSAO (shares `ssao_pipeline`; WebGL2 skips it).
  Periodic images aren't baked into the shadow map (rare combo), so they may be mis-shadowed.
  `MOLAR_VIS_DEBUG_SHADOW[=strength]` enables it. Verified on VDW (impostors) + surface (mesh),
  alone and combined with AO.
- **Background** — `Camera::background` (`Background { kind: Solid|Gradient, color, top, bottom }`,
  serialized, drives re-render via `PartialEq`). The opaque pass clears to `background.clear_color()`;
  for a gradient, a fullscreen pass (`render/background.rs` + `shaders/background.wgsl`) is drawn
  **first inside the opaque pass** (color only, `depth_compare = Always`, no depth-write, so it sits
  behind the geometry without perturbing the depth the SSAO/shadow passes read). Depth-cue fog fades
  geometry toward `background.fog_color()` (the solid color, or the gradient midpoint) — passed to
  `CameraUniform` in place of the old `BG` const. `MOLAR_VIS_DEBUG_BG=gradient`.
- **Scene graph** — N molecules × M reps. Each rep has a molar **selection string**
  compiled to atom indices (`compile_selection` → `system.select`). Geometry is built
  only for selected atoms (and bonds whose endpoints are both selected).
- **Dirty flags & render-skip** — `rep.sel_dirty` (recompile selection), `rep.geom_dirty`
  (rebuild + reupload geometry). `app.rebuild_dirty()` processes them each frame.
  `render_scene` runs **only** when geometry changed, the camera moved (`Camera`
  `PartialEq` vs `last_render_camera`), the viewport resized, or `view_dirty`
  (visibility/structure). No continuous repaint → **idle = 0 GPU**; egui repaints on input.

## molar integration notes

- Coordinates and `atom.vdw()` are in **nanometers** — do all geometry/camera/clip in nm.
- `const _: () = assert!(size_of::<molar::Float>()==4)` in the loader guards f32.
- The `System` is kept alive per molecule and is the single source of per-atom data
  (positions, elements, radii). Each rep keeps a compiled `SelectionExpr`
  (`SelectionExpr::new(text)`, stores the text via `get_str()`) and the evaluated `Sel`
  (`system.select(&expr)`). Read coords by binding: `system.bind(&sel)` → `SelBound` →
  `iter_particle()` (`Particle { id, atom, pos }`). `scene::evaluate` returns
  `Result<_, EvalError>` distinguishing the two molar failure modes: **`Empty`** (valid
  syntax, 0 atoms — molar errors via `SelectionError::Empty*`; the GUI treats it as a
  non-destructive *warning*: `rep.sel_empty=true`, drop geometry/render nothing, keep the
  text, flag the field with a red border + right-justified "⚠ 0!" via `mark_empty_selection`)
  vs **`Invalid`** (syntax/other error → `rep.sel_error`, shown in red below the field,
  keeps prior geometry).
- **Disjoint bind (molar `SelBoundParts`):** `system.bind_with_state(&sel, &state)` binds a
  selection using the system's **topology** but coordinates from an **external** `State` (e.g.
  a trajectory frame) — no copy into the System. `geometry::build` takes the bound (generic
  over the providers) so frames render by reference. `System::state()`/`topology()` borrow the
  parts. (molar addition; `SelBound` is System-coupled and unchanged.)
- Selection grammar incl.: `all`, `protein`, `backbone`, `water`, `name`, `resid`,
  `resindex`, `resname`, `index`, `chain`, `within …`.
- **Trajectory (M7, implemented):** per-molecule `Trajectory { frames: Vec<State>, current,
  playing, … }` (`trajectory.rs`). Frame 0 = the structure coords (`Molecule::seed_frame0`,
  via the `set_state(State::new_fake(n))` swap trick); loaded frames append; multiple loads
  concatenate. **Frame changes are zero-copy**: `Molecule::apply_current_frame` does NOT copy
  the frame into the System — it just sets dirty flags; `rebuild_dirty` reads the frame by
  reference via `bind_with_state(sel, &frames[current])`. **Trajectory smoothing** (per-rep
  `smooth_window`, odd, 1=off; Traj tab): when >1, `rebuild_dirty` binds a **transient**
  `Trajectory::smoothed_state(window)` instead of the raw frame — a Savitzky–Golay (local
  polynomial) blend of the nearby frames' coords (window shrunk symmetrically at the ends; box
  taken as-is), computed at build time and dropped after (a render-time coord transform, *nothing
  stored* — same philosophy as periodic images). Routing per rep: `dynamic` →
  `sel_dirty` (re-eval selection — those molecules *do* get the frame `set_state`'d in, since
  selection eval reads the System's own state); Cartoon/SecStruct with `ss_per_frame` →
  `geom_dirty` (SS may restructure); otherwise → **`coords_dirty`** (incremental). `Sel`s stay
  valid (topology unchanged). Loading: `data/traj_loader.rs` (native, threads)
  walks wanted frames `from, from+stride, …≤to` via `FileHandler::skip_to_frame(target)` +
  `read_state` — skipped frames are **seeked over, not decompressed** (random-access for
  xtc/trr/dcd via the in-molar generic seek, serial fallback for pdb/gro/xyz) — validating
  atom count per frame; sync (blocking) or async
  (`spawn_async` → `mpsc` channel drained each `ui()`). VMD-style control bar + slider in
  `app.rs` (`draw_traj_bar`), Load dialog is an `egui::Modal` (rfd file picker). Trajectory is
  **not** in `EditState` (view state, like the camera).
- **Per-frame rebuild paths (`rebuild_dirty`):** `geom_dirty` = full structural rebuild
  (selection/style/color/params, or SS restructure) → recompute SS into `rep.ss_cache`, build,
  `renderer.upload` (recreate buffers). `coords_dirty` = coordinates-only frame change → build
  reusing the cached SS (**no DSSP**), then `renderer.update` writes the new data into the
  **existing** GPU buffers in place (`queue.write_buffer`, no realloc) when element counts match
  (else recreates). Buffers carry `COPY_DST`. So scrubbing/playing avoids both per-frame DSSP
  and per-frame buffer reallocation. Per-rep **`ss_per_frame`** toggle (settings **Traj**
  tab, Cartoon / SecStruct only; in `EditState`) forces DSSP recompute every frame when
  motion changes SS.
- Bonds aren't in GRO (partial in PDB); guessed **once at load** (`distance_search_single` +
  `dist < 0.6*(vdw_i+vdw_j)`; `BondParams` = factor/cutoff/min_dist/**periodic**) and never
  recomputed on a frame change. **Periodic bond search is opt-in** (`BondParams.periodic`, the
  *Periodic search* setting — off by default): only then does `bonds::guess` use
  `distance_search_single_pbc` + minimum-image scoring to find covalent bonds across a box face in a
  wrapped structure. The PBC search is **much slower** (scans neighbouring cells), so the default
  non-periodic path keeps large-structure loads fast; a non-wrapped protein gets the same bonds
  either way (wrapping bonds are then rendered as dashed PBC half-bonds — see `geometry.rs`).
- Secondary structure for M6 cartoon: `molar::Dssp` (10-variant `SS` enum).

## Conventions & gotchas

- CPU-side indices are `usize` (`bonds: Vec<[usize;2]>`, `sel_indices: Vec<usize>`);
  colors are packed `u32` RGBA8. No GPU index buffers yet (instances carry data).
- Default new-molecule rep = **Lines** (VMD-authentic).
- **egui 0.34.3 here uses the newer API**: implement `App::ui(&mut self, ui, frame)`
  (not `update`); panels via `Panel::left(id)` / `.show_inside`; `global_style` /
  `set_global_style`.
- **wgpu 29 descriptors**: `PipelineLayoutDescriptor.bind_group_layouts: &[Option<&BGL>]`;
  `immediate_size` (replaces `push_constant_ranges`); `multiview_mask: Option<NonZero<u32>>`;
  `DepthStencilState { depth_write_enabled: Option<bool>, depth_compare: Option<_> }`;
  `RenderPassColorAttachment.depth_slice`; `RenderPassDescriptor.multiview_mask`.
- The theme sets `visuals.override_text_color`, so **frameless buttons show no hover
  feedback** — use `selectable_label` (frameless-resting, highlights on hover) or framed
  widgets for clickable icons.
- Icons: `egui_phosphor::regular::{EYE, EYE_SLASH, TRASH, COPY, PLUS, PERSPECTIVE, CUBE}`;
  the font is installed in `theme::apply` via `egui_phosphor::add_to_fonts`.
- **Wayland IME workaround** (`defuse_broken_ime` at the top of `App::ui`, Linux-gated):
  recent Wayland compositors make winit stream `Ime(Disabled)` + deliver typed chars as
  `Ime(Commit(..))` with no `Enabled`/`Preedit`, which egui 0.34.3 mishandles so text
  fields accept only the **first** character (paste/backspace still work). We rewrite
  `Ime(Commit)`→`Text` and drop stray `Ime` events. No-op on X11; macOS/Windows untouched.
  See `mod ime_workaround_tests` and the [[wayland-ime-textinput-workaround]] memory.

## UI layout

**Left panel** = a **menu bar** + the molecule list directly (no `Scene`/`Molecules`
collapsing headers; global scene controls live in the top view toolbar, below).
**Menu bar** (`draw_menu_bar`, an `egui::MenuBar` — the old inline toolbar of buttons is gone,
every global action now lives in a menu): three drop-downs — with **hover-switching** (once one
menu is open, moving the pointer onto a sibling top-level button opens that one). egui 0.34's
`MenuBar` only opens a top-level menu on **click** (the `bar` flag merely picks `MenuButton` vs
`SubMenuButton`), so the hover-switch is added by hand: each menu button's `Response` is collected,
and when any bar popup `is_id_open`, a hover over a *different* button calls `Popup::open_id` (which
closes the others — at most one popup is open per viewport) + a `request_repaint` (it takes effect
next frame). The menus —
- **Molecule** — **Draw** (toggle the interactive sketch mode, `toggle_draw`; a checkable
  `selectable_label`) · **Load…** (`App::open_structure` — native `rfd` picker / wasm file picker
  filtered to topology+coords formats pdb/ent/gro/xyz/tpr; loads via `data::load`, `scene.add`s a new
  molecule, frames the camera on the first one, undoable via the normal checkpoint).
- **Session** (`STACK`; native only — the wasm build has no filesystem to reload molecule sources
  from) — **New** (`App::new_session` — drop all molecules + reset camera/history to an empty
  document), **Save…** (`App::save_session`), **Load…** (`App::load_session`), saving/loading the
  whole visualization state as a JSON session (see `session.rs`).
- **Edit** — **Undo** / **Redo** (single step, each labelled with the next action's
  `describe_change` and a `shortcut_text`; the old `▼` **cumulative** undo/redo dropdown is gone, but
  Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y still repeat — `History::undo_n`/`redo_n`/`undo_len`/`redo_len`
  remain as test-only/API machinery) · **Settings…** (`GEAR_SIX`) opening the program-settings window
  (`App::draw_settings_dialog`; see `settings.rs` / M21).
- **View** — **`[x] Console`** (a `CHECK_SQUARE`/`SQUARE`-marked toggle of `console_open` — the Rhai
  scripting console bottom panel; opening it sets `console.focus_input` so the input grabs focus; see
  `script.rs` / M24).

Then one **molecule row** each:
expand-caret + **name** (the atom/frame counts are no longer shown inline — they're a **hover
tooltip** on the name: `N atoms / M frames`) + **Load-trajectory** (`FOLDER_OPEN`, left of the
name), right-justified **add-rep** · **zoom-to-molecule** (`MAGNIFYING_GLASS_PLUS` →
`Camera::focus_bbox`) · eye · a **per-molecule menu** (`LIST` hamburger, replacing the old
standalone trash/box buttons): **Save molecule…** (`FLOPPY_DISK` → `save_molecule`, native),
**Rename…** (`PENCIL_SIMPLE` → `rename_mol` + the `draw_rename_dialog` modal; edits `mol.name`,
persisted in sessions via `MolSession.name`), **Show periodic box** checkbox (`mol.show_box`),
**Delete frames…** (`SCISSORS` → the delete-frames modal; enabled only with a loaded
trajectory), **Delete molecule** (`TRASH`). A **two-row trajectory bar** appears below when
>1 frame (row 1: play · frame/total · fps · loop · **slider-zoom** toggle (±25-frame window,
enabled >50 frames) · **step** = playback skip per tick; row 2: first · back · full-width scrub
slider · forward · last); reps listed (indented) when the molecule caret is open. The
**Load-trajectory** modal's *Last frame* is a **text field** (empty = read to EOF), not a checkbox.

**Top view toolbar** (`draw_view_toolbar`, an `egui::Panel::top("view_toolbar")` *above*
the viewport — a real panel, **not** a floating `Area` over the 3D image; spans the central
area right of the left panel, added in `ui()` between the left panel and `draw_viewport`).
Left-aligned **selection controls**, then a right-aligned (`Layout::right_to_left`) **hamburger**
opening the view-settings menu:
**selection** — a **`Selection mode`-labelled pick-mode dropdown** (`Off` default / `Click` / `Lasso` —
see `pick.rs` / M11; **`Click`** hovers to show the atom's identity/glow (as before) and **on click
selects** the hovered atom/residue — merging it into the molecule's **active (pending) selection**
via the same op as the lasso (plain = replace, **Shift** = add, **Ctrl/⌘** = subtract;
`merge_into_pending`), expanded per the `Atoms`/`Residues` scope; in `Lasso` an LMB drag accumulates
`App::lasso_path` and **Alt+LMB orbits** (rotate the view without leaving Lasso mode), the polygon is
drawn as a cyan polyline, and on release `finish_lasso` stages the enclosed atoms — both paths feed
the same `Molecule::pending` (*not* a rep yet) glowing highlight + minimal accept/discard UI;
**two-step**, so accepting is the only undoable part) and — **only when the selection mode isn't
`Off`** — a **`Scope` dropdown** (`Atoms`/`Residues`/`Bound H` — how a hit expands;
`App::selection_mode`, see `pick::expand_selection`; `Bound H` is lasso-only, hidden in `Click`). In
`Click`/`Lasso` mode, while a modifier is held a **modifier hint** (add/subtract, + rotate for
Lasso-Alt) is drawn as a **floating overlay on the 3D viewport** (a top-center pill,
`draw_modifier_hint_overlay` in `draw_viewport`) — *not* a toolbar row, so it never resizes the view.
**view-settings hamburger** (`LIST`, right-aligned) — toggles a **`Window`** (`App::view_menu_open`,
`view_settings_window`; **not** a `Popup` — a Popup's `CloseOnClickOutside` fights the nested
click-to-open dropdowns/color pickers below, which was the bug), positioned under the button
(`Align2::RIGHT_TOP` pivot). It **closes on a click outside it** — tested against the window's rect
**as drawn the _previous_ frame** (`App::view_menu_rect`), **not** this frame's rect (nor
`ctx.layer_id_at`, which reads the same just-updated area state). The window is right-pivoted, so
clicking a tab switches `view_tab` and `Window::show` *immediately* re-lays-out for the new tab in the
same frame; a narrower tab moves the left edge right, so the freshly-updated rect no longer covers the
leftmost tab the click landed on → the menu wrongly closed (this fooled an earlier "fix" that swapped
`rect` for `layer_id_at` — both reflect the post-relayout geometry; the real fix is to test against
the geometry the user actually clicked, i.e. last frame's rect). Still kept open while a child popup
is open (`egui::Popup::is_any_open`) and on clicks on the hamburger itself (`anchor`). Tabs via the shared
`tab_bar`: **Camera / Lighting / Scene** (`App::view_tab: ViewTab`), each rendered by
`view_tab_camera/lighting/scene`:
  - **Camera**: **Projection** two **icon-only** `selectable_label`s (Persp/Ortho glyphs, tooltips;
    orthographic is the default) + a **Depth cue** group (`egui::Frame::group`): a **Type** dropdown
    (None / Linear / Exp / Exp²) that **opens on click, downward** (an `egui::Popup::menu`; None ⇄
    `enabled=false`) + **Strength** / **Start** rows, each a `slider_with_edit` (a `Slider` + a
    `DragValue` edit box).
  - **Lighting**: **Ambient occlusion** (enable + Strength/Radius; `Camera::ao`) + **Cast shadows**
    (enable + Strength; `Camera::shadow`).
  - **Scene**: an **Axes** group with a monitor-like **screen widget** (`draw_axes_widget`,
    hand-laid-out: a rectangle showing a **live mini downsampled render of the scene** (the
    `renderer.texture_id()` painted into the rect), an on/off **checkbox in its center** (on a
    translucent backing so it reads over the render), and a corner **radio outside each of the four
    corners** = where the gizmo is anchored (`Corner`, drawn onto the 3D image by `draw_axes_overlay`);
    a **Background** group (Solid/Gradient radios + `color_submenu` swatches — a `Button`-swatch that
    **opens on click, downward** a `Popup::menu` (`CloseOnClickOutside`) with an inline
    `color_picker_color32`, linear↔Color32 via `egui::Rgba` for WYSIWYG; `Camera::background`).
Toolbar buttons use the **`overlay_button` helper** (a fixed-height framed button, glyph **centered
by ink bounds** `Galley::mesh_bounds`, not the font line-box); the **`toolbar_label`** helper draws
the `Selection mode`/`Scope` labels with the **same ink-centering** so they line up with the buttons next
to them. Dropdowns hang off `egui::Popup::menu(&resp)`.

Each rep is a **two-row block** (`ui.vertical`; the whole block is the reorder drop target
via `dnd_hover_payload`/`dnd_release_payload`):
- **Row 1**: **drag handle** (`DOTS_SIX_VERTICAL` in `dnd_drag_source(payload=index)`) ·
  **selection field** (fills width; focusing sets `editing_rep` and expands it to a
  full-width editor, collapsing on Enter/blur) · right-justified compact actions
  (`Layout::right_to_left` + `compact_actions`): **zoom-to-selection** (`MAGNIFYING_GLASS_PLUS`
  → `Camera::focus_bbox` on the rep's `sel` bbox) · eye · duplicate · **save selection to file**
  (`FLOPPY_DISK` → `save_rep_selection`, native; just left of trash) · trash. The rep's
  **selection error** (if any) is shown in red on the next line, aligned under the field — and
  the **erroring span of the text is painted red in-place** (a `sel_text_edit` layouter colors
  from the molar caret offset to the end; see `suggest.rs`). Editing the field (`resp.changed()`)
  immediately **clears the stale message / red highlight / empty flag** (`clear_sel_feedback`),
  recomputed on commit. While the field is focused, a faint **suggestion hint** for the keyword
  being typed (e.g. `chains: A B C R`, `resid: 2..120`) appears under it (`active_hint`, from the
  cached `SelHints`), **truncated with `…`** (`Label::truncate`) so a long value list stays on one line.
- **Row 2** (a **settings caret** — `CARET_RIGHT`/`CARET_DOWN`, where the drag handle is in
  row 1 — toggles `params_open`; then) **style** dropdown · **color** dropdown · **material**
  dropdown (`material_picker`: button = a small shaded-sphere icon faded by opacity; the popup is a
  **grid of material previews** — each `material_cell` renders a **two-sphere-and-bond fragment**
  shaded with that material as an `egui::Mesh` (per-vertex Blinn-Phong via `preview_shade`, matching
  the lit shaders: `base·(amb+dif·N·L)+spec·(N·H)^exp` + outline + opacity-as-alpha;
  `push_preview_sphere`/`push_preview_bond`), so Glossy/Metal/Diffuse/Glass/Ghost/AO… read
  distinctly). The expanded settings
  panel (`draw_rep_params`) is **tabbed** — **[Style]** (per-style geometry params: VDW
  *Sphere scale*, Lines *Line width (px)*, Licorice/Ball-and-Stick radii, Cartoon ribbon
  dims, Surface probe/quality/smoothing + SS-algorithm + Defaults; every style now has at
  least one tunable so Defaults is always shown), **[Traj]** (`draw_traj_tab`: *Update every
  frame* = `rep.dynamic`; *Recompute SS every frame* = `ss_per_frame` for Cartoon/SecStruct;
  *Smooth window* = `rep.smooth_window` — odd (1=off, 3,5,7…; a half-width `DragValue` shown as the
  window via `custom_formatter`), trajectory smoothing; sets `coords_dirty`), **[Periodic]** (`draw_periodic_tab`, **only shown when the
  molecule has a box** — gated by `mol.system.state().pbox.is_some()`: *Self* / *Box* checkboxes
  + six `spin_u32` spinboxes −x/+x/−y/+y/−z/+z (a `DragValue` flanked by `−`/`+` step buttons,
  range 0..=8) giving the image counts along ±a,±b,±c; these
  are render-only so the tab returns a `view_dirty` bool instead of setting `geom_dirty`); tab in
  `rep.settings_tab: SettingsTab`. The tab bar uses the shared **`tab_bar(ui, &mut current, &[(T,
  label)…])`** helper — the **app-default tab style** (underline tabs: selected = bold + accent
  underline, others weak/clickable), reused by every tabbed UI (rep settings, the delete-frames
  dialog, …) so they stay consistent. Style and color are **icon+text** buttons built by the shared
  `picker_button(label, draw_icon)` helper (drawn glyph + label + caret → `egui::Popup::menu`
  of icon+label rows). `paint_style_icon` draws each `RepKind`; `paint_color_icon` draws each
  `ColorMethod` (Element = CPK dots, Chain = interlocking colored links, ResID =
  backbone-with-residues diagram, ResName = "ALA" on rainbow, Index = "123" colored digits,
  Beta = "B" on rainbow, **Solid = a filled swatch of the chosen color**). The `Solid` row is a
  **submenu** (`egui::containers::menu::SubMenu`, ⏵): hovering opens a panel with a preset
  swatch grid (`SOLID_SWATCHES`, `swatch_button`) + a full `color_picker_color32` (the submenu uses
  `CloseOnClickOutside` so dragging the picker doesn't dismiss it).

History labels via `describe_change` ("edit selection", "change coloring",
"reorder representations", …). FPS in the footer.

## Milestone status

- ✅ M0 scaffold + offscreen triangle
- ✅ M1 molar load + VDW sphere impostors (analytic frag_depth)
- ✅ M2 arcball camera + VMD mouse nav
- ✅ M3 bonds → Licorice / Ball-and-Stick / Lines (cylinder impostors, half-bond lines)
- ✅ M4 multi-molecule / multi-rep scene + selection strings + icon panel UI +
  perspective/orthographic toggle + scene-dirty render-skip
- ✅ Undo/Redo (history.rs) + big rep-row UI revamp (drag/expand/style-icon/gear)
- ✅ M5 coloring schemes — `color.rs` `ColorMethod` {Element, Chain, ResID, ResName,
  Index, Beta, **SecStruct**} + `Colorizer` (per-method, with B-factor range / index
  gradient context / DSSP map). `geometry::build` colors each atom via the rep's `color`.
  Per-rep color dropdown next to the style dropdown, with drawn descriptive icons
  (`paint_color_icon`: CPK dots / categorical bars / rainbow / blue-white-red / SS ribbon).
- ✅ M6 **Cartoon** + secondary-structure coloring — `secstruct.rs` (`SsMap`: molar
  `Dssp` keyed by `resindex`, `SsClass` helix/sheet/coil, VMD `ss_color`); `geometry/
  cartoon.rs` (per-chain Catmull-Rom spline through Cα, carbonyl-derived ribbon frame with
  flip-consistency, Laplacian smoothing of helix/sheet Cα, elliptical cross-section morphing
  by SS class + sharp barbed β-arrowheads → indexed `MeshData`; see the cartoon.rs bullet
  above); `render/mesh.rs` + `shaders/mesh.wgsl`
  (Lambert-shaded `MeshVertex` pipeline, writes real depth, shares the offscreen buffer with
  the impostors). `RepKind::Cartoon` + `RepParams::Cartoon{coil_radius,ribbon_width,
  ribbon_thickness}`. **`RepParams` is now a per-style enum** (each variant carries only its
  own knobs — incl. `Vdw { scale }` (× VDW radius) and `Lines { width }` (px), both formerly
  unit variants); `geometry::build` dispatches on it (no more `kind` arg).
- ✅ MVP complete (M0–M6, all five representations).
- ✅ M7 **Trajectories** (native) — `trajectory.rs` (`Trajectory`/`LoadOptions`/`LoadMode`/
  `LoadMsg`) + `data/traj_loader.rs` (native, cfg-gated) + per-molecule Load dialog (`egui::Modal`
  + `rfd`) + VMD-style playback bar/slider + sync/async loading. See the trajectory note under
  *molar integration*. Verified on a multi-MODEL 2lao trajectory (atoms move per frame, slider/
  frame-field/play work).
- ✅ **molar made wasm-friendly + a pluggable byte source** (changes in the molar repo, not just
  molar_vis):
  - `FileFormatError` is now **`pub`** (+ `FileIoError::kind()`/`path()`), so callers match
    `FileFormatError::Eof` directly. **EOF unified**: pdb/gro/xyz now return the top-level
    `FileFormatError::Eof` (was each handler's own `Eof`), matching xtc/trr/dcd — also fixed a
    latent spurious-corruption warning on multi-MODEL PDB via `IoStateIterator`.
  - `molar_gromacs` (tpr/cpt, libloading) is **target-gated** to non-wasm; tpr/cpt handlers +
    dispatch arms + error variants `#[cfg(not(wasm))]`. `cargo build … --target
    wasm32-unknown-unknown` now **compiles** for both molar and molar_vis_core (xtc/trr/dcd/gro/
    pdb/xyz survive; tpr/cpt dropped). Remaining wasm *runtime* items (Instant→web-time shim,
    threads→worker, rayon pool) belong to the browser milestone.
  - **`DynSource`** (boxed `Read + Seek + Send`) + **`FileHandler::from_reader(ext, src)`**: every
    pure-Rust handler gained `from_source(DynSource)` (stores `BufReader<DynSource>` /
    `XTCReader<DynSource>`); `open(path)` now wraps a `File` into a `DynSource`. Lets molar read
    any format from a non-file source (in-memory buffer, browser Blob) with the unchanged sync API.
  - **XTC generic seek**: molly's seek path is `File`-bound only because of its internal `Buffer`
    optimization; the seek logic itself needs just `Read + Seek`. Ported faithfully **into molar's
    xtc handler** (`io/xtc_handler.rs`, `skip_positions`/`seek_next`/`skip_frames`/`seek_prev`/
    `skip_to_time`) using molly's **public** API (`XTCReader { pub file, pub step }`, `read_header`,
    `molly::reader::read_nbytes`, `molly::padding`, `Header`) — **no molly change**. Round-trip
    test `io::tests::from_reader_matches_open` asserts `from_reader(Cursor)` == `open(path)` for
    xtc & trr incl. forward/backward seek.
  - **`SelBoundParts` + `System::bind_with_state` / `state()` / `topology()`**: bind a `Sel` to a
    **disjoint** `(&Topology, &State)` (read-only) — used so trajectory frames render by reference
    (zero-copy). `SelBoundParts` impls the element providers directly (no `SystemProvider`), so it
    gets `iter_particle`/`Measure`/`Analysis` via the blankets but can't derive sub-selections (the
    viewer doesn't need that). Test `system::tests::bind_with_state_reads_external_coords`.
- ✅ **Zoom-to-selection / zoom-to-molecule** (`Camera::focus_bbox`) + **periodic-box wireframe**
  toggle (`geometry::box_wireframe`, per-molecule `box_gpu`).
- ✅ M8 **Browser app (single-threaded wasm)** — the viewer runs in the browser through eframe's
  `WebRunner` (wgpu, with a **WebGL2 fallback**), built/bundled with `trunk` and **deployed to
  GitHub Pages**. **Decision: single-threaded** (no SharedArrayBuffer/COOP-COEP/nightly — hostable on
  any static server). Pieces:
  - **molar wasm runtime** (committed + pushed to molar at rev *ea33c5f*; molar_vis now pins a later
    rev — *6ac04e8*, which also carries the selection-grammar word-boundary fix):
    `web_time::Instant` for the clock (std panics on wasm) + a `src/par.rs` serial-iterator shim so
    molar's rayon calls run single-threaded on wasm (rayon is now native-only); `IoStateIterator`
    reads serially on wasm.
  - **`crates/molar_vis_web`** — a `bin` whose wasm `main` calls `molar_vis_core::run_web`
    (`launch.rs`, `#[cfg(wasm)]`: `WebRunner::start` on the `<canvas id="molar_vis_canvas">`; panic +
    Info-level `console_log` hooks; surfaces a startup failure into the page `#loading`). `index.html`
    + trunk; native `main` is a stub. Build/serve: `cd crates/molar_vis_web && trunk serve`.
    `.cargo/config.toml` sets `getrandom_backend="wasm_js"` (wasm only); wgpu gets the `webgl` feature
    on wasm. The web build opens to a bundled molecule (`App::load_demo`, `include_bytes!` 2lao).
  - **WebGL2 fallback** (`render.rs`): WebGL2 lacks `INDEPENDENT_BLEND`, so the OIT pipelines (accum
    additive + reveal multiplicative) can't be created. `SceneRenderer::new` checks the adapter's
    downlevel flags → `oit_enabled`; when false it skips the OIT/composite passes and draws
    transparent reps with plain alpha blending in the opaque pass (`draw_reps` takes an explicit
    pipeline index). The theme is **pinned to Dark** (`ctx.set_theme(ThemePreference::Dark)`), else
    eframe follows the browser's light `prefers-color-scheme` and the UI comes up white.
  - **Browser file open** — the wasm picker is a shared `pick_file(accept, ctx, deliver)` helper
    (web-sys `<input type=file>` → `Blob::array_buffer` → bytes → `deliver`). `App::open_structure`
    forks native rfd vs `pick_file` (→ `file_rx` → `data::load_from_bytes`, molar `from_reader` over a
    `Cursor`). `add_loaded` is the shared "add molecule + frame camera" tail.
  - **Browser trajectory streaming** — the Load-trajectory button forks native (the dialog) vs
    `pick_file` tagged with the molecule (→ `traj_rx`). On wasm there are no threads, so instead of
    the native reader thread, `data::traj_wasm::TrajStream` keeps a `FileHandler` over the in-memory
    `Cursor` and `App::poll_wasm_loaders` reads a **batch of frames per `ui()`** (`next_batch`),
    streaming them into the `Trajectory` (same `seed_frame0`/`push_frame`/playback path as native);
    repaints continue until the stream is drained. No range/stride dialog on wasm yet (loads all).
  - **Deploy**: `.github/workflows/pages.yml` builds `trunk build --release --public-url /<repo>/`
    and publishes to Pages (auto-enables via `actions/configure-pages`). Demo:
    **https://yesint.github.io/molar_vis/**.
  - **Still TODO:** the **WebGPU** path (vs the WebGL2 fallback) wants its own live check; true
    random-access disk streaming (a Web Worker + `FileReaderSync` over a `Blob`) is unneeded for the
    in-memory approach but would help huge trajectories.
- ✅ M9 **Materials** — `material.rs` `Material` (11 VMD presets: Opaque/Transparent/Glass/
  Translucent/Ghost/Glossy/Diffuse/Metal + the **AO trio** AoChalky/AoShiny/AoEdgy; each
  `params()` → ambient/diffuse/specular/shininess/opacity/**outline**) + per-rep `material` (in
  `EditState`) + a **material dropdown** in row 2 (next to color, `material_picker`/
  `paint_material_icon`). The **AO materials** are VMD's ambient-occlusion-oriented presets
  (high diffuse, AoChalky matte / AoShiny with a highlight / AoEdgy matte + outline); they keep a
  small ambient so they're not pitch-black until real AO lands (SSAO assessed feasible; see the
  roadmap). **Outline** (VMD silhouette darkening) is packed as the **top bit of the shininess
  byte** (shininess uses the low 7 bits) — no vertex-layout change; the lit shaders' `apply_outline`
  darkens grazing-angle fragments (same Fresnel term as the selection-glow rim, subtractive).
  - **Transparency (Weighted-Blended OIT)**: `geometry::build` folds the material opacity into each
    element's color alpha; all shaders output it. **Each geometry has two pipelines** `[opaque, oit]`:
    `[0]` writes a single alpha-blended color target + depth (`fs_main`); `[1]` is the OIT pipeline —
    depth-test on, **depth-write off**, output to two targets via `fs_oit`. `render_scene` is **three
    passes** (skipped past pass 1 when nothing transparent is visible): (1) opaque → color+depth; (2)
    transparent → the **WBOIT** `accum` (RGBA16F, additive: Σ premultiplied color·weight) + `reveal`
    (R16F, multiplicative `dst*(1-α)`) targets, depth-tested against the opaque depth; (3) a fullscreen
    `oit_composite.wgsl` resolve blends `accum.rgb/accum.a` over the opaque color with `(SrcAlpha,
    1-SrcAlpha)` and `1-reveal` (McGuire & Bavoil). **Order-independent — no sort.** The OIT weight
    (`oit_weight` in each shader) biases strongly toward the camera using **linear eye-space depth
    normalized across the molecule's own front→back extent** (`camera.depth_range`, from
    `Camera::eye_depth_range`): the molecule occupies a razor-thin, non-linear slice of *window* depth,
    so naive NDC-depth weighting saturates and the resolve degenerates to a washed-out flat average of
    all layers — linear eye-space depth lets near layers dominate. Dense transparent VDW is still an
    inherently busy translucent blob (~30 overlapping crisp layers); single/few-layer cases (surface,
    cartoon) are clean. Impostor `fs_oit` still writes analytic `frag_depth` so OIT depth-tests against
    opaque geometry.
  - **Lighting**: `Material::pack_lighting()` packs the four coeffs into a `u32`
    (`ambient | diffuse<<8 | specular<<16 | shininess<<24`); `geometry::build` stamps it onto every
    sphere/cylinder/mesh-vertex's new `mat: u32` field (lines carry opacity only — unlit). The lit
    shaders (`sphere/cylinder/mesh.wgsl`) take `mat` (flat-interpolated), `unpack_mat` it, and run a
    shared **Blinn-Phong** `shade_material`: `base*(amb + dif*N·L) + spec*pow(N·H, 2+shin*128)`,
    white highlight, headlight `L=(0.3,0.4,1)`, view dir to eye (origin perspective / +z ortho).
    The cartoon mesh flips its normal to face the eye first (two-sided open ribbons). **`mesh.wgsl`
    additionally adds a dim opposite-front fill `(-0.5,-0.3,0.6)` gated by `(1−N·L)²`** so the flat
    ribbon's thin **lateral rims** (normals ⊥ the key light → near-black) get lifted *only in
    shadow/terminator* — key-lit areas and the specular highlight are untouched, so the slick look
    is preserved (sphere/cylinder are unchanged, single headlight only). Glossy=tight highlight,
    Diffuse=matte (specular 0), Metal=dark+broad highlight — all verified distinct.
  - ✅ **OIT** (was TODO): replaced the order-dependent two-phase blend with Weighted-Blended OIT
    (see *Transparency* above) — multi-layer transparency is now order-independent.
- ✅ M12 **Molecular surface (SES)** — `RepKind::Surface` + `RepParams::Surface { probe, quality }`,
  built in `geometry/surface.rs` as the **solvent-excluded (rolling-probe) surface via a grid
  distance-field + Surface Nets** (the robust PyMOL/Chimera/EDTSurf "distance maps + carving"
  method; renders through the existing lit-mesh pipeline). Pipeline: rasterize the SAS solid
  (voxel within `vdW+probe` of an atom) → exact Felzenszwalb–Huttenlocher EDT to the nearest
  outside voxel = `dist(x, solvent)` → isosurface at `dist = probe` (= morphological closing of
  the vdW balls by the probe) via **Surface Nets** (dual marching-cubes: one vertex per
  straddling cell → watertight by construction, smooth, no 256-entry tables). Per-vertex normal
  = −∇field; color seeded from the nearest atom, then **Laplacian-smoothed along the mesh**
  (`laplacian_smooth`/`smooth_attr`: 1-ring averaging over triangle edges — topology-aware, so it
  blends *along* the surface and doesn't bleed across a crevice like a 3-D distance blend would).
  Hard nearest-atom Voronoi patches → smooth gradients; the gradient-sampled **normals get a light
  Laplacian pass too** (de-facets the per-cell nearest-node gradient, then renormalized). Iteration
  counts scale with grid resolution (∝(1/h)²) so the physical smoothing distance stays ~constant;
  uniform color (`Solid`) skips the color pass. `quality` 0–4 → spacing 0.14–0.035 nm, voxel count capped at
  32M (auto-coarsen + `log::warn`). A **light separable [1,2,1] blur of the distance field**
  before Surface Nets (`smoothing` passes, **default 0** — opt-in now that the Laplacian mesh
  pass smooths the normals) removes the binary-occupancy voxel staircase from the surface
  *shape* (geometric smoothing the mesh-Laplacian can't do). Per-rep
  settings (**Style** tab) sliders: **Probe radius / Quality / Smoothing** (`RepParams::Surface`).
  Verified watertight/smooth on 2lao (~1 s), the symmetric
  cube, and 375k atoms (~10 s, 1.4M tris). `MOLAR_VIS_DEBUG_REP=surface`,
  `MOLAR_VIS_DEBUG_SURF=1` logs grid stats. **Dead-ends (documented in memory):** analytic
  convex+toroidal+concave patches (powersasa `surface_mesh`/`ses_mesh`, kept as an exact
  SAS-area API) are MSMS-style crack-prone and were abandoned; Ball-Pivoting re-meshing worked
  visually but was too slow. The grid is the only reliably watertight, scalable approach.
- ✅ **UI revamp + installable** — no `Scene`/`Molecules` headers (molecules listed directly);
  view/selection controls (projection · depth-cue · axes · pick mode · selection mode) live in a
  **top view toolbar** (`draw_view_toolbar`, `Panel::top` above the viewport — was a floating
  `draw_scene_overlay` Area on the 3D image); per-rep **settings caret** (not a gear) opening
  a **tabbed** panel **[Style] / [Traj] / [Periodic]** (`SettingsTab`); selection errors shown
  under the field; VMD mouse nav extended (roll on Shift+LMB, dolly on Shift+RMB) and
  zoom-to-fit fills ~90%. Crate is **installable** from GitHub git-deps (no local paths/patch).
- ✅ M10 **Custom solid selection colors** — `ColorMethod::Solid([u8;4])` (`color.rs`; `DEFAULT_SOLID`
  orange, `Colorizer` returns it verbatim) + an egui color-picker submenu in the color dropdown
  (`color_picker`: a `Solid` row — drawn via `color_option`, which returns a `Response` + optional
  ⏵ — that opens an `egui::containers::menu::SubMenu` with a preset swatch grid (`SOLID_SWATCHES`/
  `swatch_button`) + a full `color_picker_color32`; the submenu is `CloseOnClickOutside` so dragging
  the picker doesn't dismiss it). Undoable for free — `RepState` already snapshots `rep.color` and
  history compares `ColorMethod` generically.
- ✅ M13 **Save / load visualization state** — a JSON "session" file capturing the loaded
  molecules (by **source path**, reloaded from disk — not embedded), the full per-rep document,
  per-molecule visibility/box/trajectory, and the global view (camera/projection/depth-cue/
  axes/pick+selection modes). `session.rs` (`Session`/`MolSession`/`ViewState`/`MoleculeSource`/
  `TrajLoad`) + a **`Session` toolbar menu** (New/Save/Load) + native
  `App::{new_session,save_session,load_session,apply_session}` + `MOLAR_VIS_DEBUG_{SAVE,LOAD}_SESSION`
  hooks. **Built for extensibility — the design point:**
  the per-rep document is serialized through the *same* `history::RepState` undo/redo uses, so a
  new undoable rep field is persisted automatically with no second site to touch; the only manual
  plumbing is the small `ViewState` ⇄ `App::{view_state,apply_view_state}` seam. All fields are
  `#[serde(default)]` → forward/back-compatible. The domain types themselves (`RepKind`,
  `RepParams`, `ColorMethod`, `Material`, `PeriodicParams`, `Camera`, …) derive serde directly
  (no mirror structs to drift); `SsAlgorithm` rides a `#[serde(remote)]` shim, `Camera` uses
  glam's `serde` feature. Loading replaces the scene (open-document semantics) and resets undo
  history. Verified: 4 unit round-trip/compat tests + a headless save→load→save round-trip that
  is **byte-identical** (incl. a replayed 20-frame trajectory restored to frame 2, SS-colored
  Cartoon over `protein`, and the camera). Native only (wasm has no filesystem to reload sources);
  `session.rs` stays WASM-safe for a future browser download/upload path.
- ✅ M14 **Selection-input improvements** — `suggest.rs`. (1) **Visual errors**: molar formats a
  parse error with a `^` caret line; `parse_sel_error` extracts the caret char-offset + the
  "Expected …" message, and `sel_text_edit`'s `TextEdit` layouter paints the text from that offset
  to the end **red** (caret-at-end → highlights the last char), so the error is shown *in place* in
  the field (plus the clean message below). (2) **Suggestions**: `SelHints` (distinct chains /
  resnames / names + resid/resindex/index ranges, computed once from topology, cached per molecule
  on `App::sel_hints`); while editing, `SelHints::hint_for` shows the values for the **last keyword**
  typed (`chains: A B C R`, `resid: 2..120`, …) faintly under the field, **truncated with `…`** to one
  line. Both stale-feedback cues clear the moment the text is edited (`clear_sel_feedback` on
  `resp.changed()`) and are recomputed on commit. 3 unit tests
  (`last_keyword`, error-caret parse, pass-through); verified headlessly via
  `MOLAR_VIS_DEBUG_EDIT_REP` + `MOLAR_VIS_DEBUG_SEL`.
- ✅ M15 **Save molecules / selections to file + delete trajectory frames + molecule menu** —
  three "File I/O & state" roadmap items. (1) **Save** (native): `save_displayed(mol, path, rep)`
  writes via molar's `FileHandler::create` + `write` (whole `System` when `rep=None`, else
  `system.bind(sel)` = just the selected atoms) at the **displayed** frame — the frame `State` is
  swapped into the System around the write (frames render by reference, not held in the System) and
  restored after; format from the path extension (pdb/gro/xyz/ent). `App::save_molecule` (from the
  molecule menu) + `App::save_rep_selection` (a `FLOPPY_DISK` button just left of the rep's trash).
  (2) **Delete trajectory frames**: `Trajectory::delete_range(from,to)` / `decimate(stride)` (pure
  data, WASM-safe, clamp `current`) driven by a **`DeleteFramesDialog`** modal (Range / Decimate
  via the shared `tab_bar` tabs, `draw_delete_frames_dialog`) opened from the menu; not undoable
  (trajectory is view state). Empty
  result reverts to the static structure. (3) **Per-molecule `LIST` menu** replaces the standalone
  trash/box buttons: Save molecule · Show-periodic-box checkbox · Delete frames · Delete molecule.
  2 trajectory unit tests; save path verified headlessly (`MOLAR_VIS_DEBUG_SAVE_MOL` → valid PDB,
  1911 atoms). Save is native-only (molar writes to the filesystem); the menu/dialog/frame-deletion
  are cross-platform.
- ✅ M16 **Bonds + cartoon over PBC (dashed half-bonds / faded ribbon)** — a "Rendering & visuals"
  roadmap item. (1) **PBC-aware bond guessing** (`data/bonds.rs`, `distance_search_single_pbc` +
  minimum-image scoring when the structure has a box) so cross-face covalent bonds in a *wrapped*
  structure are found at all. (2) **Minimum-image dashed half-bonds** (`geometry.rs`
  `half_bond_ends` via `PeriodicBox::closest_image`; box from the bound's `BoxProvider::get_box`,
  no call-site changes): a bond crossing a face is drawn as two **dashed** stubs running from each
  atom **to its partner's nearest image** (full bond toward the image, not beyond — reaches where
  the partner is in the next cell), crossing opposite faces; nothing crosses the box interior.
  Cylinders + lines. (3) **Cartoon**: runs split at a PBC jump (`is_pbc_jump`) so the ribbon never
  crosses the box; a jump end is **extended one residue past the face** (ghost control point at the
  partner's image), stays opaque up to the face (`is_inside`), then is **dashed** beyond it
  (per-ring opaque/transparent stripes, no fade; mesh material stamping now *multiplies* alpha so
  the transparent gaps survive). Test fixtures:
  `tests/pbc_pair.pdb` (2-atom wrapped bond) + `tests/2lao_pbc_broken.pdb` (2lao shifted by
  half a box in X and wrapped into a snug box, so the protein is split across the X face) — both
  committed. Verified: bond count unchanged from the whole protein (1855); no long lines/ribbons
  across the box; dashed stubs reach the partner image; the cartoon ribbon is dashed beyond the
  boundary.
- ✅ M17 **Depth-cue modes (VMD `cuemode`) + cursor-centered zoom** — two "Rendering & visuals"
  items. (1) **Depth-cue falloff curves**: `CueMode {Linear, Exp, Exp2}` on `DepthCue` (matching
  the OpenGL fog equations), passed in `cue.w`; `apply_fog` (all 4 lit shaders) computes normalized
  depth `t∈[0,1]` and selects linear / `1−e^(−k·t)` / `1−e^(−(k·t)²)` (k=3), **normalized to reach
  full fog at the far plane** so switching modes keeps far-fog = `strength` and only changes the
  ramp. Mode tabs added to the depth-cue popup (shared `tab_bar`), which is now
  `CloseOnClickOutside` so it stays open while adjusting. `MOLAR_VIS_DEBUG_CUEMODE=linear|exp|exp2`.
  (2) **Cursor-centered wheel zoom**: `Camera::zoom_scroll(scroll, ndc, aspect)` pans `target` so
  the world point under the cursor stays put (focal-plane half-height `distance·tan(fov/2)` for both
  projections). Unit test `zoom_is_centered_on_cursor` (point projects back to the same screen NDC,
  both projections).
- ✅ M18 **VMD AO materials + screen-space ambient occlusion** — (1) added VMD's AO-oriented
  material presets `AoChalky`/`AoShiny`/`AoEdgy` (11 materials now); `AoEdgy` needed VMD's
  silhouette **Outline**, so `MaterialParams` gained `outline`, packed as the **top bit of the
  shininess byte** (no vertex-layout change), and the lit shaders gained `apply_outline` (grazing-
  angle darkening, same Fresnel term as the glow rim). (2) **SSAO** (`render/ssao.rs` +
  `shaders/ssao.wgsl`): a fullscreen multiply-blend pass after the opaque pass, normal-free
  (neighbour-in-front obscurance, golden-angle spiral kernel), reading the now-sampleable depth
  target; `Camera::ao` settings + a top-toolbar AO popup; gated to full WebGPU (skipped on WebGL2).
  See the *Ambient occlusion (SSAO)* architecture note. Verified: WGSL compiles, crevices darken on
  VDW (impostors) and surface (mesh), no startup regression. 30 tests pass.
- ✅ M19 **Real-time cast shadows (shadow mapping)** — VMD has ray-traced shadows; this is the
  cheap real-time equivalent, done **deferred** so it costs one extra geometry pass and **no
  lit-shader changes / no new pipelines**. A shadow pass renders the opaque geometry from a
  directional key light into a `2048²` depth map (reusing the opaque pipelines via a light-space
  `CameraUniform` entry — impostors self-compute light-space depth); the SSAO pass then projects
  each pixel into light space (`shadow_matrix` in `SsaoUniform`) and PCF-samples the map, folding
  the shadow into its multiply blend (`ao × shadow`). `Camera::shadow` (`Shadow { enabled,
  strength }`, off, serialized) + the shared lighting popup (AO + shadows) + `MOLAR_VIS_DEBUG_SHADOW`.
  Gated to full WebGPU like SSAO. See the *Cast shadows* architecture note. Verified on VDW + surface,
  alone and combined with AO; 30 tests pass.
- ✅ M20 **View-settings menu revamp + background** — (1) the top toolbar is now **selection controls
  (left) + a right-aligned hamburger** opening a tabbed **Camera / Lighting / Scene** window
  (`ViewTab`, hosted in a `Window` so nested click-to-open dropdowns/color pickers behave; closed on
  click-outside via `Popup::is_any_open`) — all the projection/depth-cue/lighting/axes controls moved
  off the toolbar into it (`view_tab_*`), with the depth cue gaining a *None* option and
  `slider_with_edit` (slider + numeric edit) rows, the axes a monitor "screen" widget with a live
  mini-render. (2) **Background** (`Camera::background`): flat color **or** a vertical gradient (a
  fullscreen pass, `render/background.rs`); fog fades to the background color. Both serialized (ride
  `Camera`'s serde). See the *Background* note + the *Top view toolbar* UI section. **A reflective
  ground plane was attempted here and reverted** — a finite floor quad's near edge is pinned to the
  camera near-clip (`distance − scene_radius`), which recedes on zoom-out (a visible sharp edge); the
  correct model is an *infinite* plane (screen-space ray-plane intersection, no edges). To be redone.
- ✅ M21 **Program settings + persisted config** — a **settings dialog** (toolbar cogwheel
  after undo/redo) exposing every knob that used to be hardcoded at launch, persisted to a JSON
  file in the platform config dir (created with defaults on first launch). `settings.rs` (`Settings`
  + `AppearanceSettings`/`RenderingSettings`/`ViewDefaults`/`RepDefaults`/`BehaviorSettings`,
  `ThemeMode`; pure data + serde, WASM-safe, all `#[serde(default)]` — see the module bullet) +
  `directories` (native-only dep). The five tabs are **Appearance** (theme/font scale/accent),
  **Rendering** (SSAA / shadow-map res), **View** (projection / background / depth-cue / AO /
  shadows / fit — *new-scene defaults*, with **Apply to current view**), **Representations**
  (default style/color/material/selection/surface-quality), **Behavior** (mouse sensitivity /
  default pick+selection mode / trajectory fps+loop / bond-guessing thresholds). Wiring: the old
  constants became settings-fed parameters — `theme::apply(&AppearanceSettings)`,
  `SceneRenderer::new(&RenderingSettings)` + `reconfigure` (SSAA/`shadow_res` are now fields; the
  shadow PCF texel rides the SSAO uniform's `misc.z`), `Camera` gained a `fill` field +
  sensitivity-scaled `orbit`/`roll`, `data::load_with(&BondParams)`, `Scene::add(&RepDefaults)` /
  `Representation::from_defaults`, `Molecule` trajectory fps/loop seeded on load. App-global knobs
  (theme/AA) apply **live** on Save; new-document defaults are read when the next scene/molecule is
  created (never mutating the open doc — the View tab's "Apply to current view" is the explicit
  push). `MOLAR_VIS_DEBUG_SETTINGS=[tab]` opens it headlessly, `MOLAR_VIS_DEBUG_DEFAULTS=1` skips
  the config file. Existing `MOLAR_VIS_DEBUG_REP/SEL/COLOR/MATERIAL/PICK/SELMODE` still override the
  settings. Verified: 4 new unit tests (41 total), native+wasm build green, headless screenshots of
  every tab, and a load→apply round-trip (edited config → Light theme + VDW/Chain default rep).
- ✅ M22 **CG (Martini) cartoon — secondary structure + helix ribbon** (a "Coarse-grained"
  roadmap item; the *display* half — bond guessing for CG is still TODO, the cartoon needs **no
  bonds**: it groups per-residue `BB`/`SC1` beads directly). Two parts, both in `secstruct.rs` +
  `geometry/cartoon.rs` (see those module bullets): **(1) geometric SS** for a CG backbone
  (`assign_cg_ss`) — DSSP can't run without the N/CA/C/O backbone, so helix/sheet are classified
  from the BB trace's virtual bond angle θ + virtual dihedral τ (scale-invariant), with a β-pairing
  filter (no non-sequential partner BB nearby → not a strand) so it never invents strands that
  aren't there. **(2) Wrapping-ribbon helices** — a CG helix has no carbonyl frame and its BB beads
  spiral the axis at ~100°/residue, so it's drawn as a flat ribbon **wrapped on the helix cylinder
  surface**: collapse BB → smooth axis, ride the surface at the all-atom-matched radius with a
  uniform phase **anchored to the real backbone at both ends**, evaluate the interior as an
  **analytic helix** (no CR overshoot/overlap), join the coil with a **Hermite** that uses the true
  spiral tangent (no doubled end stub), and **taper** the width into the loop tube at each end.
  Also landed a general **flat-ribbon shading** in `emit` (constant broad-face normal → crisp flat
  tape instead of a domed lens), which improves **all-atom** cartoons too. β-sheets render as the
  SC1-oriented arrow ribbon. Iterated heavily against the user's visual validation (helix-orientation
  was the hard part — the dead-ends: solid cylinder, raw-radial screw, CR-spline overshoot, blobby
  ellipse shading, full-size sharp ends, off-backbone least-squares phase). Verified on
  `tests/2lao_cg.pdb` (α/β) + a Martini membrane bundle; 38 tests pass, native+wasm green.
- ✅ M23 **Draw mode — interactive molecule sketching + on-the-fly minimization** (the ROADMAP
  "drawing molecules + simple UFF" items). A togglable Draw mode (Molecule menu → Draw, mutually
  exclusive with the pick modes) with a vertical right-side tools palette (`draw_tools_panel`):
  Draw/Erase tools + CPK element chips + bond-order icons. The unified **Draw tool** infers the
  action from the gesture (click empty → atom, drag from atom → bond, click bond → cycle order,
  Erase → delete); `App::draw: Option<DrawSession>` + the edit helpers on `Molecule`
  (`add_atom`/`add_bond`/`cycle_bond_order`). Structure edits are undoable via
  `MolState.structure: Option<StructureSnapshot>` (captured only for `editable` molecules). A
  greenfield cleanup force field (`minimize.rs`: harmonic bond/angle + weak torsion + WCA-repulsive
  vdW, analytic gradients, FIRE integrator) relaxes the sketch debounced + via a Clean-up button;
  molar's `Vec<Bond>` carries `BondOrder`, and Double/Triple/Aromatic render as parallel/dashed
  screen-space strands. `MOLAR_VIS_DEBUG_DRAW=methane|ethane|water|benzene` builds + relaxes a preset
  headlessly. (Documented in detail in the `app.rs` / `minimize.rs` sections above.)
- ✅ M24 **In-app scripting console (Rhai)** — the first slice of the scripting roadmap (the data
  layer was already Python-scriptable via molar's published `pymolar` PyO3 bindings; PyO3 can't
  target `wasm32-unknown-unknown`, so the *portable* surface is a pure-Rust embedded language).
  **Decision: Rhai** (pure-Rust, WASM-proven, best API-binding ergonomics, sandboxable) driven from
  an **in-app console window** (works in the browser too); external-terminal / Python-driver
  transports deferred behind a transport-agnostic command core. **Fluent OO surface** (per user
  feedback — the first flat-function cut, `color("chain")`, was "lame"): `mol(i).rep(j).set_style(…)
  .set_color(…).select(…)`, `mol(i).add_rep("cartoon").set_color("ss")`, `mol(i).show()/hide()/
  frame(n)/play()/focus(sel)`. `script.rs` (+ `script/{command,console}.rs`): lightweight
  `MolHandle`/`RepHandle` (index + `RepRef{Index,Last}` + shared command queue) whose methods push a
  `Command`; `evaluate_script` (Rhai fns push commands, no scene access) + `apply_scene_command`
  (GPU-free, testable; same field-set + dirty-flag the GUI does → converges on `rebuild_dirty`, no
  new render branch) + `App::{run_script,execute_command,draw_console}`. Free fns: `mol(i)`,
  `load(path)` (native), `list()`, plus Rhai's `print`. One undo checkpoint per script run. Console
  is a resizable bottom panel toggled from the **View menu** (`[x] Console`); `MOLAR_VIS_DEBUG_SCRIPT` runs a script
  at startup for headless verification. See the `script.rs` module bullet. Verified: 5 unit tests
  (62 total), native+wasm green, and a headless screenshot of `mol(0).rep(0).set_style("vdw")
  .set_color("resid"); mol(0).add_rep("cartoon")` (rep 0 → rainbow VDW spheres + a new Cartoon rep,
  in the console + rep list). Deferred: property setters (`rep.style = …`) + indexing (`mol[0]`,
  declined by the user), camera/background scripting, autocompletion, multi-line editor, `.rhai`
  file open/save, external/Python transports.
- 🟡 M11 **Atom picking + lasso selection** — `pick.rs` (`PickMode {Off, Click, Lasso}`,
  `PickHit`, `cursor_ray`, `ray_sphere`, `effective_radius`, `pick(scene, view, proj, ndc) ->
  Option<PickHit>`): a **CPU ray-cast** of the cursor against every visible atom **at its displayed
  position** (smoothed + periodic-replicated, via `bind_with_state(sel, smoothed_or_frame)` ×
  `PeriodicParams::offsets`), returning the nearest hit — but reporting the atom's **real** stored
  coord (`frame.coords[id]`, central image, un-smoothed), per the user's hard requirement.
  Pick/glow radius = the rep's drawn sphere (VDW `vdw·scale`, BallAndStick `vdw·sphere_scale`) else
  the **small Ball-and-Stick sphere size** (`vdw·0.25` = `BALLSTICK_SPHERE_SCALE` —
  Licorice/Lines/Cartoon/Surface). Pick-mode **dropdown** in the top view toolbar (Off default →
  no per-hover cost). `PickHit` also carries the hit's `mol` + global atom `id`.
  **Hover-info respects the selection mode** (`App::effective_selection_mode`): in **Atoms** mode,
  `draw_pick_overlay` paints a **cyan glowing outline ring** at the hit's projected displayed
  position + a **framed** lower-left info box `name resname resid` / `x, y, z` (real coords, **nm**);
  in **Residues** mode the whole hovered residue (`expand_selection` of the hit) is staged as the
  molecule's steady hover highlight (`Molecule::hover` → `hover_gpu`, glowing in the current style
  like a pending selection **but not pulsing and with no accept/discard UI**; rendered in the glow
  pass via the steady camera entry 1) + a residue info box (`draw_residue_info_overlay`:
  `resname resid` / `residue · N atoms`). `Bound H` is meaningless for single-atom hover, so it
  falls back to Atoms and is hidden from the toolbar dropdown in Click (lasso-only). The hover
  set is recomputed as the cursor moves (`set_hover`/`clear_hover`, repaint on change to rebuild the
  glow next frame). `MOLAR_VIS_DEBUG_PICK=1` forces a viewport-center pick (headless verification —
  hover can't be simulated on this Wayland box); pair with `MOLAR_VIS_DEBUG_SELMODE=residues`.
  - **GPU pick id-buffer (native hover):** the per-frame hover ray-cast is O(visible atoms), so on
    native the hover hit comes from an **async GPU id-buffer** instead. Each molecule's `pick_gpu` is
    one id-stamped sphere impostor per *pickable* atom — exactly the atoms CPU `pick` ray-casts, built
    by `build_pick` (eligible per `atom_in_rep`, at the displayed position + `effective_radius`),
    id = `[mol+1, rep<<21 | atom]`. They're drawn (`fs_pick` in `sphere.wgsl`) into a 1× **`Rg32Uint`**
    target + depth (front-most wins, analytic frag_depth). **Async, two methods:** `request_pick`
    renders the buffer + `copy_texture_to_buffer` the cursor texel + `map_async` (no stall);
    `poll_pick` (called every frame — also when *not* hovering, to free the readback) drives a
    non-blocking `device.poll(Poll)` and, when the map callback fires, decodes the texel →
    `(mol, rep, atom)`. The result lags 1–2 frames and is cached in `App::hover_pick`;
    `pick::hit_for_atom` rebuilds the `PickHit` from it each frame (O(1), no per-atom scan). A new
    pick is requested **only when the cursor moves or the view changes** (`last_pick_px`), so a
    stationary hover stays idle (0 GPU). `pick_gpu` rebuilds on geometry/coords change or a structural
    change (baked `mol+1` would go stale). **Periodic images are baked into `pick_gpu`** (a sphere per
    atom per drawn image, shifted by the lattice offset, same id), so the single-camera pick pass
    covers every image like CPU `pick`. **Native only** — gated `#[cfg(not(wasm))]`: WebGPU can't
    block on a readback and WebGL2 may not render integer targets, so **wasm keeps the CPU `pick`**.
    Validated headlessly under `MOLAR_VIS_DEBUG_PICK` (logs `gpu == cpu`): matches CPU on
    VDW/cartoon/ball-stick and with periodic images on.
  - **Lasso select** (`lasso_select`): in `PickMode::Lasso`, an LMB drag in `draw_viewport`
    accumulates `App::lasso_path` (pixel coords; **Alt+LMB orbits** instead — rotate the view without
    leaving Lasso mode; RMB/MMB/wheel still navigate), drawn as a cyan polyline; on release
    `finish_lasso` maps the path → clip-space NDC polygon and calls `lasso_select`, which projects
    every **style-eligible, displayed** atom (any periodic image inside the polygon counts) and
    groups hits per molecule (`LassoSelection { mol, atoms }`, deduped/sorted). A **screen-bbox
    pre-reject** (the polygon's NDC bounding box) drops atoms outside the lasso's rect in a 4-compare
    before the O(vertices) **even-odd** `point_in_polygon`, keeping the one-shot gesture cheap at
    scale (lasso stays CPU — it must select *occluded* atoms too, which a front-most GPU id-buffer
    can't; the GPU id-buffer is hover-only). The hits become each molecule's selection text via
    `pick::index_selection_string(atoms)` — a compact molar `index lo:hi …` string (consecutive runs
    → inclusive ranges; 0-based global atom index).
  - **Selection mode** (`SelectionMode`, toolbar dropdown next to the pick selector;
    `pick::expand_selection`): each gesture's raw hits are expanded per molecule **before** the set
    op — `Atoms` (exact), `Residues` (any hit residue selected whole — grown by walking outward by
    atom index while `resindex` holds, O(residue size), no full-system scan), or
    `Bound H` (hit **heavy** atoms + the H bonded to them via the guessed `bonds`; a hit H whose heavy
    atom isn't itself selected is dropped). Also drives **hover-info** (Atoms → ring + atom; Residues
    → steady whole-residue glow + residue box; `Bound H` is lasso-only and hidden in Click).
    `MOLAR_VIS_DEBUG_SELMODE=residues|boundh` sets it headlessly. Tested:
    `expand_residues_selects_whole_residue`, `expand_bound_h` (synthetic methane).
  - **Lasso set ops** (release modifier; `LassoOp` in `app.rs`): plain drag **replaces** the active
    selection, **Shift**+drag **adds** (unions), **Ctrl/⌘**+drag **subtracts** — merged per molecule
    in `finish_lasso` via a `BTreeSet` over the existing pending atoms (empty result → clears it). In
    Lasso mode an LMB drag draws the polygon unless **Alt** is held (then it orbits).
  - **Active (pending) selection — two-step commit** (`scene::PendingSelection`,
    `Molecule::pending`): a lasso does **not** make a rep directly. It stages a *pending* selection
    that's **view state, not undoable, excluded from `EditState`**, shown two ways: (1) a **GPU glow
    highlight in the current style** — `rebuild_dirty`'s `build_glow` builds, per visible rep,
    `(rep.sel ∩ pending)` in *that rep's own style/params* (Cartoon → ribbon, VDW → spheres, …),
    merged into the molecule's `glow_gpu` (`GeometryData::append`). **Cartoon glow reuses the parent
    ribbon's exact geometry**: the cartoon builder tags every vertex with its source `resindex`
    (`MeshData::vert_res`) and the last-built ribbon mesh is cached on the rep (`cartoon_cache`);
    `cartoon_submesh` then extracts just the chosen residues' triangles (kept when ≥2 of a triangle's
    3 verts are in the residue set — a clean cut at residue boundaries) and re-indexes them. Because
    it's the *same* vertices as the parent, the glow is coincident → passes the `≤` depth test cleanly
    (**no z-fight, no inflation**) and a **single residue** still yields its ribbon segment (a 1-residue
    spline is degenerate, which is why re-splining a subset failed). **Surface glow** still re-builds a
    subset isosurface (no residue tags) that diverges from the parent, so it's inflated into a thin
    shell (`inflate_mesh`, `GLOW_INFLATE`=0.025 nm outward along normals) to test above it; impostor
    glows coincide exactly and aren't offset. A final additive **glow pass**
    (`render_scene` pass 4, pipeline index `GLOW=2`) draws it with the shaders' `fs_glow` — an
    intense cyan **Fresnel rim** (bright at grazing angles + a strong body tint), **pulsing**: the
    camera uniform's `params.w` carries an animated multiplier (`0.70 + 0.30·sin(t·3.2)`, computed in
    `draw_viewport`) and while any selection is pending the viewport `request_repaint()`s + force-
    re-renders each frame so it breathes (idle = 0 GPU otherwise). Depth-tested `≤` against the scene
    depth (so occluded atoms don't glow), no depth-write. So the *selected geometry itself glows in
    its current style* — **not** a 2-D overlay. `glow_dirty` rebuilds it when the pending set or
    coords change, **or when any rep's geometry is rebuilt** (so the glow follows a live style/
    selection change); central image only. (2) a **minimal panel block** under the reps
    (`draw_reps_for`): a non-editable italic "selection" label + **green ✓ accept** + **🗑 discard**,
    no style/color/material row. **Accept** commits it as a normal, fully-editable **Ball-and-Stick**
    rep over the same `index …` text (this push *is* the undoable step — "add representation");
    **discard** drops it. `MOLAR_VIS_DEBUG_PENDING=<sel>` stages one headlessly.
  - **Style-specific eligibility** (shared by hover + lasso via `atom_in_rep(kind, name)`): a
    Cartoon rep is hit only on its **backbone** atoms (`cartoon_atom`: N/CA/C/O + terminal
    OT1/OT2/OXT — what the ribbon is built from), never side chains; every other style is hit on
    all selected atoms (Lines included, via its isolated-atom **crosses**). Tested:
    `lasso_full_screen_selects_all_for_vdw`, `lasso_cartoon_selects_only_backbone`.
  - **TODO:** more pick modes in the dropdown. Picking/lasso is O(visible atoms × images) — fine for
    small/medium systems; a spatial grid / GPU id-buffer is the optimization for huge ones.

## Roadmap

Forward-looking feature list (deleting traj frames, save/load state, app settings, more depth-cue
methods, background color, material editor, labels/measurement, Python bindings, embedded command
language, geometric primitives, raytracing, movies, whole-residue pick mode, CG/Martini bonds+SS,
plugins, selection-input improvements, drug-discovery goodies (PLIP interactions, SDF reading),
dashed PBC half-bonds, and visual structure editing) lives in **[ROADMAP.md](ROADMAP.md)** — in no
particular order. Move items into *Milestone status* above as they ship.
