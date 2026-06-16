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
cargo run -p molar_vis -- tests/2lao.pdb [more files...]   # each file = one molecule
cargo test -p molar_vis_core
cargo build -p molar_vis_core --target wasm32-unknown-unknown   # WASM-readiness check (now green)
```

- Test assets in `tests/`: `2lao.pdb` (1911 atoms), `large_375k.gro` (375,548 atoms,
  generated — **not in git**; regenerate per `tests/README.md` with `gmx genconf`).
- Dev machine is **Wayland**; screenshot a running window with `spectacle -b -n -a -o out.png`
  (**`-a` = active window — use this**; `-f` full-screen captures blank on this compositor).
- Headless verification env hooks (native only): `MOLAR_VIS_DEBUG_REP=vdw|licorice|ballstick|lines|cartoon|surface`
  (+ `MOLAR_VIS_DEBUG_SURF=1` logs surface grid stats),
  `MOLAR_VIS_DEBUG_SEL="<selection>"`,
  `MOLAR_VIS_DEBUG_COLOR=element|chain|resid|resname|index|beta|secstruct`,
  `MOLAR_VIS_DEBUG_ALLCOLORS=1` (one rep per color scheme, cycling styles — shows every icon),
  `MOLAR_VIS_DEBUG_ORBIT=<deg>`, `MOLAR_VIS_DEBUG_ORTHO=1`,
  `MOLAR_VIS_DEBUG_TRAJ=<path>` (load a trajectory into mol 0, bypassing the dialog) +
  `MOLAR_VIS_DEBUG_FRAME=<n>` (display frame n) + `MOLAR_VIS_DEBUG_TRAJ_FROM/TO/STRIDE=<n>`
  (load range/stride) + `MOLAR_VIS_DEBUG_TRAJ_PLAY=1` (auto-play, exercises the incremental
  update path) + `MOLAR_VIS_DEBUG_BOX=1` (show mol 0's periodic box) +
  `MOLAR_VIS_DEBUG_PBC="px,py,pz"` (set mol 0 first rep's +a/+b/+c periodic image counts + box;
  exercises periodic-image rendering — 2lao has a CRYST1 box) +
  `MOLAR_VIS_DEBUG_SMOOTH=<window>` (set mol 0 first rep's trajectory smoothing window; pair with
  `MOLAR_VIS_DEBUG_TRAJ`) +
  `MOLAR_VIS_DEBUG_PICK=1` (force hover-info pick mode + pick at the viewport center each frame, so
  the glow/info overlay can be screenshot headlessly) +
  `MOLAR_VIS_DEBUG_SELMODE=residues|boundh` (set the lasso selection-expansion mode; default Atoms) +
  `MOLAR_VIS_DEBUG_PENDING=<selection>` (stage that selection on **every** molecule as an
  active/pending selection — exercises the lasso glow highlight + per-molecule accept/discard UI,
  incl. the multi-molecule case, without a mouse drag) +
  `MOLAR_VIS_DEBUG_AXES=1` (show the VMD-style orientation-axes gizmo) +
  `MOLAR_VIS_DEBUG_MATERIAL=<name>` (set mol 0's first rep material, e.g. Transparent) +
  `MOLAR_VIS_DEBUG_FOCUS=<selection>` (zoom the camera to fit that selection — exercises
  zoom-to-selection). Generate a quick test trajectory with the Python snippet that wrote
  `tests/2lao_traj.pdb` (multi-MODEL, **not in git**).

## Tech stack (working versions)

eframe / egui / egui-wgpu **0.34.3**, wgpu **29.0.3**, egui-phosphor **0.12** (icon font),
glam **0.32** (GPU/camera math), nalgebra **0.34** (molar boundary), bytemuck **1.25**,
molar **1.4** (**git dep** `git = "https://github.com/yesint/molar.git"`,
`default-features=false` → `Float=f32`; pulls `powersasa` transitively from git).
GROMACS 2026.1 available as `gmx`.

**Installable** — molar and powersasa come from GitHub (no sibling checkouts, no
`[patch]`). `Cargo.lock` pins the resolved git revisions. To develop molar/powersasa
locally, temporarily add a `[patch."…powersasa-llm.git"] powersasa = { path = "…" }`
and/or point `molar` at a local path — but don't commit those.

## Workspace & modules

`crates/molar_vis_core` (library, WASM-safe, all logic) + `crates/molar_vis` (native bin:
argv + logging). **Modern module layout** (`<module>.rs` + `<module>/`, no `mod.rs`).

- `lib.rs` — module decls, `run`/`App` re-exports.
- `launch.rs` — `AppLaunch`, eframe bootstrap (`Renderer::Wgpu`).
- `app.rs` — `eframe::App`; owns `SceneRenderer`, `Camera`, `Scene`; left panel
  (Scene/Molecules/Representations/Controls) + central viewport; `rebuild_dirty()`
  and the render-skip logic. Holds the `MOLAR_VIS_DEBUG_*` hooks.
- `theme.rs` — installs the Phosphor icon font + a high-contrast dark style, larger fonts.
- `camera.rs` — quaternion arcball `Camera`. VMD mouse nav (in `app.rs::draw_viewport`):
  LMB orbit · **Shift+LMB `roll`** (screen-plane, about the view axis) · RMB (or MMB)
  `pan` · **Shift+RMB `zoom_drag`** (dolly along view Z) · wheel `zoom_scroll`. Perspective
  **and** orthographic projection. `frame_bbox`/`focus_bbox` use `fit_distance` (fit the
  bbox's **longest dimension to ~90%** of the viewport; bounding-sphere radius still drives
  near/far). `#[derive(PartialEq)]` drives render-skip.
- `color.rs` — CPK element colors → packed RGBA8 (`u32`); `ColorMethod`, `Colorizer`.
- `secstruct.rs` — `SsMap` (molar `Dssp` keyed by `resindex`), `SsClass` (helix/sheet/coil),
  VMD `ss_color`. Shared by the Cartoon rep and the SecStruct color scheme.
- `geometry.rs` — `RepKind`, `RepParams` (**per-style enum**), `GeometryData`/`MeshData`;
  `build(system, sel, bonds, params, color)` binds the `Sel` (`system.bind`), reads
  positions/atoms via `iter_particle` (nothing cached), and dispatches on `params`. Spheres
  come from the selected atoms; bonds are half-bond split, colored by each atom. Computes a
  `SsMap` once when the rep is Cartoon or colored by SecStruct.
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
  `normalize`→NaN→white on NVIDIA.)
- `scene.rs` — `Scene { molecules, selected_mol, trash }`, `Molecule` (molar `System` +
  guessed `bonds` + bbox + `reps`; the `System` is the single source of per-atom data),
  `Representation` (kind / params / `sel_text` (editable buffer) / `expr: SelectionExpr`
  (compiled) / `sel: Sel` (evaluated) / `periodic: PeriodicParams` (image counts + Self/Box,
  in `EditState`) / visible / dirty flags / `RepGpu`), `evaluate()`
  (text → `SelectionExpr` → `Sel`). `Molecule` also owns a `trajectory: Trajectory` and the
  `seed_frame0`/`append_frames`/`push_frame`/`apply_current_frame` methods (see *molar integration*).
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
- `pick.rs` — CPU ray-cast atom picking (`PickMode {Off, HoverInfo, Lasso}`, `PickHit` (carries
  the hit `mol` + atom `id`),
  `cursor_ray`, `ray_sphere`, `effective_radius`, `pick`) **and lasso selection** (`lasso_select`,
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
  (exact), `Residues` (any hit residue selected whole, by `resindex`), or `BoundH` (hit **heavy**
  atoms + the H bonded to them via the guessed `bonds`; a hit H whose heavy atom isn't selected is
  dropped). Applied to each lasso gesture's hits in `finish_lasso` *before* the set op, and to the
  hovered atom in `draw_viewport` (Residues → whole-residue highlight). `BoundH` is lasso-only
  (`App::effective_selection_mode` falls back to Atoms for hover).

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
- **Depth cueing (fog)** — linear fog fades all geometry toward the background (`BG` in
  `render.rs`, also the clear color) by eye-space distance. The camera uniform carries
  `cue = [near, far, strength, _]` (eye-space, derived per frame by `Camera::cue_uniform`
  from `distance`/`scene_radius` + the scene-relative `DepthCue { enabled, start, strength }`
  on `Camera`) + `fog_color`. Every fragment shader applies the shared `apply_fog(color,
  eye_z)`; line/mesh pass eye-space `z` as a varying, the impostors use their ray hit. Lives
  in `Camera` so its `PartialEq` re-renders on change; controls in the Scene panel.
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
- Bonds aren't in GRO (partial in PDB); guessed at load (`distance_search_single` +
  `dist < 0.6*(vdw_i+vdw_j)`).
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

## UI layout

**Left panel** = toolbar + the molecule list directly (no `Scene`/`Molecules`
collapsing headers; global scene controls live in the top view toolbar, below).
Toolbar: **`Open`** button (`App::open_structure` — native `rfd` picker filtered to
topology+coords formats pdb/ent/gro/xyz/tpr; loads via `data::load`, `scene.add`s a new
molecule, frames the camera on the first one, undoable via the normal checkpoint) · then
undo/redo buttons, each with a `▼` dropdown for **cumulative** undo/redo (also Ctrl+Z /
Ctrl+Shift+Z / Ctrl+Y). Then one **molecule row** each: expand-caret + name + atom count +
**Load-trajectory** (`FOLDER_OPEN`, left of the name), right-justified **add-rep** ·
**periodic-box toggle** (`BOUNDING_BOX`) · **zoom-to-molecule** (`MAGNIFYING_GLASS_PLUS` →
`Camera::focus_bbox`) · eye · trash; a trajectory control bar + slider appears below when
>1 frame; reps listed (indented) when the molecule caret is open.

**Top view toolbar** (`draw_view_toolbar`, an `egui::Panel::top("view_toolbar")` *above*
the viewport — a real panel, **not** a floating `Area` over the 3D image; spans the central
area right of the left panel, added in `ui()` between the left panel and `draw_viewport`).
Two groups split by a `ui.separator()`:
**view** — a **projection-cycle** button (Perspective↔Orthographic, icon+tooltip change;
**orthographic is the default**), a **depth-cue** button (`GRADIENT` glyph, filled when the cue
is enabled) opening a `Popup::menu` cue panel (enabled + Strength/Start sliders — a popup, so the
toolbar stays fixed-height), and an **axes-gizmo dropdown** (`ARROWS_OUT_CARDINAL`: an *On*
checkbox + a 2×2 corner-radio grid `Corner {TopLeft,TopRight,BottomLeft,BottomRight}`, default
BottomRight — VMD-style orientation axes drawn onto the 3D image by `draw_axes_overlay`;
`MOLAR_VIS_DEBUG_AXES=1` enables it headlessly);
**selection** — a **pick-mode dropdown** (`Off` default / `Hover info` / `Lasso select` — see
`pick.rs` / M11; in `Lasso` an LMB drag accumulates `App::lasso_path` and **Alt+LMB orbits**
(rotate the view without leaving Lasso mode), the polygon is drawn as a cyan polyline, and on
release `finish_lasso` stages the enclosed atoms as each molecule's **active (pending) selection**
(`Molecule::pending`, *not* a rep yet) — a glowing highlight + minimal accept/discard UI;
**two-step**, so accepting is the only undoable part) and a **selection-mode dropdown**
(`Atoms`/`Residues`/`Bound H` — how the lasso expands its hits; `App::selection_mode`, see
`pick::expand_selection`). In Lasso mode the trailing **modifier hint** (rotate/add/subtract)
follows on the right. All buttons are the **same `overlay_button`
helper** — a fixed-height framed button whose glyph/label is **centered by its ink bounds**
(`Galley::mesh_bounds`), not the font line-box, so Phosphor glyphs with different metrics line
up vertically (`ui.button`/`selectable_label` center the line-box → ragged row); the
dropdowns/popups hang off `egui::Popup::menu(&resp)`. More scene controls will join it later.

Each rep is a **two-row block** (`ui.vertical`; the whole block is the reorder drop target
via `dnd_hover_payload`/`dnd_release_payload`):
- **Row 1**: **drag handle** (`DOTS_SIX_VERTICAL` in `dnd_drag_source(payload=index)`) ·
  **selection field** (fills width; focusing sets `editing_rep` and expands it to a
  full-width editor, collapsing on Enter/blur) · right-justified compact actions
  (`Layout::right_to_left` + `compact_actions`): **zoom-to-selection** (`MAGNIFYING_GLASS_PLUS`
  → `Camera::focus_bbox` on the rep's `sel` bbox) · eye · duplicate · trash. The rep's
  **selection error** (if any) is shown in red on the next line, aligned under the field.
- **Row 2** (a **settings caret** — `CARET_RIGHT`/`CARET_DOWN`, where the drag handle is in
  row 1 — toggles `params_open`; then) **style** dropdown · **color** dropdown · **material**
  dropdown (`material_picker`, shaded-sphere icon faded by opacity). The expanded settings
  panel (`draw_rep_params`) is **tabbed** — **[Style]** (per-style geometry params: VDW
  *Sphere scale*, Lines *Line width (px)*, Licorice/Ball-and-Stick radii, Cartoon ribbon
  dims, Surface probe/quality/smoothing + SS-algorithm + Defaults; every style now has at
  least one tunable so Defaults is always shown), **[Traj]** (`draw_traj_tab`: *Update every
  frame* = `rep.dynamic`; *Recompute SS every frame* = `ss_per_frame` for Cartoon/SecStruct;
  *Smooth window* = `rep.smooth_window` — odd (1=off, 3,5,7…; a half-width `DragValue` shown as the
  window via `custom_formatter`), trajectory smoothing; sets `coords_dirty`), **[Periodic]** (`draw_periodic_tab`, **only shown when the
  molecule has a box** — gated by `mol.system.state().pbox.is_some()`: *Self* / *Box* checkboxes
  + six `DragValue` spinboxes −x/+x/−y/+y/−z/+z giving the image counts along ±a,±b,±c; these
  are render-only so the tab returns a `view_dirty` bool instead of setting `geom_dirty`); tab in
  `rep.settings_tab: SettingsTab`. Style and color are **icon+text** buttons built by the shared
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
  - **molar wasm runtime** (committed + pushed to molar, rev *ea33c5f*; molar_vis pins that git rev):
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
- ✅ M9 **Materials** — `material.rs` `Material` (8 VMD presets: Opaque/Transparent/Glass/
  Translucent/Ghost/Glossy/Diffuse/Metal; each `params()` → ambient/diffuse/specular/shininess/
  opacity) + per-rep `material` (in `EditState`) + a **material dropdown** in row 2 (next to color,
  `material_picker`/`paint_material_icon`).
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
- 🟡 M11 **Atom picking + lasso selection** — `pick.rs` (`PickMode {Off, HoverInfo, Lasso}`,
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
  falls back to Atoms and is hidden from the toolbar dropdown in HoverInfo (lasso-only). The hover
  set is recomputed as the cursor moves (`set_hover`/`clear_hover`, repaint on change to rebuild the
  glow next frame). `MOLAR_VIS_DEBUG_PICK=1` forces a viewport-center pick (headless verification —
  hover can't be simulated on this Wayland box); pair with `MOLAR_VIS_DEBUG_SELMODE=residues`.
  - **Lasso select** (`lasso_select`): in `PickMode::Lasso`, an LMB drag in `draw_viewport`
    accumulates `App::lasso_path` (pixel coords; **Alt+LMB orbits** instead — rotate the view without
    leaving Lasso mode; RMB/MMB/wheel still navigate), drawn as a cyan polyline; on release
    `finish_lasso` maps the path → clip-space NDC polygon and calls `lasso_select`, which projects
    every **style-eligible, displayed** atom (any periodic image inside the polygon counts,
    **even-odd** `point_in_polygon`) and groups hits per molecule (`LassoSelection { mol, atoms }`,
    deduped/sorted). The hits become each molecule's selection text via
    `pick::index_selection_string(atoms)` — a compact molar `index lo:hi …` string (consecutive runs
    → inclusive ranges; 0-based global atom index).
  - **Selection mode** (`SelectionMode`, toolbar dropdown next to the pick selector;
    `pick::expand_selection`): each gesture's raw hits are expanded per molecule **before** the set
    op — `Atoms` (exact), `Residues` (any hit residue selected whole, grouped by `resindex`), or
    `Bound H` (hit **heavy** atoms + the H bonded to them via the guessed `bonds`; a hit H whose heavy
    atom isn't itself selected is dropped). Also drives **hover-info** (Atoms → ring + atom; Residues
    → steady whole-residue glow + residue box; `Bound H` is lasso-only and hidden in HoverInfo).
    `MOLAR_VIS_DEBUG_SELMODE=residues|boundh` sets it headlessly. Tested:
    `expand_residues_selects_whole_residue`, `expand_bound_h` (synthetic methane).
  - **Lasso set ops** (release modifier; `LassoOp` in `app.rs`): plain drag **replaces** the active
    selection, **Shift**+drag **adds** (unions), **Ctrl/⌘**+drag **subtracts** — merged per molecule
    in `finish_lasso` via a `BTreeSet` over the existing pending atoms (empty result → clears it). In
    Lasso mode an LMB drag draws the polygon unless **Alt** is held (then it orbits).
  - **Active (pending) selection — two-step commit** (`scene::PendingSelection`,
    `Molecule::pending`): a lasso does **not** make a rep directly. It stages a *pending* selection
    that's **view state, not undoable, excluded from `EditState`**, shown two ways: (1) a **GPU glow
    highlight in the current style** — `rebuild_dirty`'s `build_glow` rebuilds, per visible rep,
    `(rep.sel ∩ pending)` in *that rep's own style/params* (Cartoon → ribbon, VDW → spheres, …),
    merged into the molecule's `glow_gpu` (`GeometryData::append`). **Mesh-style glow (Cartoon/
    Surface) is inflated into a thin shell** (`inflate_mesh`, `GLOW_INFLATE`=0.025 nm outward along
    vertex normals): the glow mesh is re-splined over the *subset* of selected atoms, so it nearly
    but not exactly coincides with the parent's full mesh (SS-dependent smoothing/cleanup diverges at
    the subset ends) → two near-coplanar surfaces z-fight → patchy. The outward shell makes the glow
    test cleanly *above* the parent (its back faces still fail the `≤` depth test and stay hidden, so
    no double-blend); impostor glows coincide exactly and aren't offset. A final additive **glow pass**
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
