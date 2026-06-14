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
  tube) with β-arrowheads; emits indexed `MeshData`. Mirrors VMD `draw_cartoon_ribbons`.
- `scene.rs` — `Scene { molecules, selected_mol, trash }`, `Molecule` (molar `System` +
  guessed `bonds` + bbox + `reps`; the `System` is the single source of per-atom data),
  `Representation` (kind / params / `sel_text` (editable buffer) / `expr: SelectionExpr`
  (compiled) / `sel: Sel` (evaluated) / visible / dirty flags / `RepGpu`), `evaluate()`
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
  `oit_bind_group` for the resolve), camera UBO (bind group 0), sphere/cylinder/line/**mesh**
  pipelines (each `[opaque, oit]`) + a fullscreen **`composite_pipeline`** (`oit_bgl`), `RepGpu`
  (per-rep buffers; mesh = vertex + u32 index buffer; buffers carry `COPY_DST`), `upload()` (recreate
  buffers), **`update()`** (in-place `write_buffer` when element counts match, for coords-only
  frame changes), `render_scene()` (3-pass: opaque → OIT → composite; `draw_reps` shared), `texture_id()`.
  Plus `render/{sphere,cylinder,line,mesh,camera_uniform}.rs` and `render/shaders/*.wgsl` (incl.
  `oit_composite.wgsl`; lit shaders carry `fs_main` + `fs_oit`). The cartoon mesh writes real depth
  and interleaves correctly with the impostors.

## Key architecture

- **Strategy A rendering** — the 3D scene is drawn into our *own* offscreen color +
  depth textures, then composited into egui as an `Image`. egui's render pass has no
  depth attachment; this gives full depth control for impostors.
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
  `iter_particle()` (`Particle { id, atom, pos }`). Empty/invalid selection → `Err`
  (shown in red), keeps prior geometry.
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
  reference via `bind_with_state(sel, &frames[current])`. Routing per rep: `dynamic` →
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
collapsing headers; global scene controls moved to the viewport overlay below).
Toolbar: **`Open`** button (`App::open_structure` — native `rfd` picker filtered to
topology+coords formats pdb/ent/gro/xyz/tpr; loads via `data::load`, `scene.add`s a new
molecule, frames the camera on the first one, undoable via the normal checkpoint) · then
undo/redo buttons, each with a `▼` dropdown for **cumulative** undo/redo (also Ctrl+Z /
Ctrl+Shift+Z / Ctrl+Y). Then one **molecule row** each: expand-caret + name + atom count +
**Load-trajectory** (`FOLDER_OPEN`, left of the name), right-justified **add-rep** ·
**periodic-box toggle** (`BOUNDING_BOX`) · **zoom-to-molecule** (`MAGNIFYING_GLASS_PLUS` →
`Camera::focus_bbox`) · eye · trash; a trajectory control bar + slider appears below when
>1 frame; reps listed (indented) when the molecule caret is open.

**Viewport overlay** (`draw_scene_overlay`, top-left `egui::Area` over the 3D image):
a single **projection-cycle** button (Perspective↔Orthographic, icon+tooltip change;
**orthographic is the default**) and a **depth-cue** button (`GRADIENT` glyph) that toggles
(`cue_panel_open`) an inline cue panel (enabled + Strength/Start sliders). More scene
controls will join it later.

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
  more per-frame options later), **[Periodic]** (periodic-image rendering — TBD); tab in
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
  by SS class with β-arrowheads → indexed `MeshData`); `render/mesh.rs` + `shaders/mesh.wgsl`
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
- ⏳ M8 **Browser streaming** (not yet) — `WasmBlobReader` (Read+Seek via worker-only
  `FileReaderSync` over a `Blob`), a wasm Web Worker loader (wasm-threads), `web_sys::File` picker,
  `eframe::WebRunner` entry + `index.html` served with COOP/COEP. The `from_reader` core above is
  the foundation; only the wasm runtime assembly remains.
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
    The cartoon mesh flips its normal to face the eye first (two-sided open ribbons). Glossy=tight
    highlight, Diffuse=matte (specular 0), Metal=dark+broad highlight — all verified distinct.
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
  before Surface Nets (`smoothing` passes, default 2) removes the binary-occupancy voxel
  staircase so both the surface and its gradient-derived normals come out smooth. Per-rep
  settings (**Style** tab) sliders: **Probe radius / Quality / Smoothing** (`RepParams::Surface`).
  Verified watertight/smooth on 2lao (~1 s), the symmetric
  cube, and 375k atoms (~10 s, 1.4M tris). `MOLAR_VIS_DEBUG_REP=surface`,
  `MOLAR_VIS_DEBUG_SURF=1` logs grid stats. **Dead-ends (documented in memory):** analytic
  convex+toroidal+concave patches (powersasa `surface_mesh`/`ses_mesh`, kept as an exact
  SAS-area API) are MSMS-style crack-prone and were abandoned; Ball-Pivoting re-meshing worked
  visually but was too slow. The grid is the only reliably watertight, scalable approach.
- ✅ **UI revamp + installable** — no `Scene`/`Molecules` headers (molecules listed directly);
  global scene controls (projection cycle + depth-cue) moved to a floating top-left
  **viewport overlay** (`draw_scene_overlay`); per-rep **settings caret** (not a gear) opening
  a **tabbed** panel **[Style] / [Traj] / [Periodic]** (`SettingsTab`); selection errors shown
  under the field; VMD mouse nav extended (roll on Shift+LMB, dolly on Shift+RMB) and
  zoom-to-fit fills ~90%. Crate is **installable** from GitHub git-deps (no local paths/patch).
- ✅ M10 **Custom solid selection colors** — `ColorMethod::Solid([u8;4])` (`color.rs`; `DEFAULT_SOLID`
  orange, `same_kind` for picker highlight, `Colorizer` returns it verbatim) + an egui color-picker
  submenu in the color dropdown (`color_picker`: a `Solid` row that opens an `egui` `SubMenu` with a
  preset swatch grid + a full `color_picker_color32`). Undoable for free — `RepState` already
  snapshots `rep.color` and history compares `ColorMethod` generically.
- ⏳ M11 **Atom picking + mouse lasso selection** — pick atoms (GPU id-buffer or CPU ray-cast vs
  impostors) and lasso-select (polygon over projected positions) → feed a selection; hooks into
  `draw_viewport` input.
