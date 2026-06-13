# CLAUDE.md — vmd_rs

A modern, legacy-free molecular viewer modeled after VMD, in **pure Rust**,
targeting Linux/Windows/macOS/WebAssembly. It builds on **molar** (the user's Rust
molecular library: IO, selections, topology, DSSP) and renders on **eframe/egui +
wgpu** with hand-written WGSL GPU ray-cast impostors.

The user, Semen Yesylevsky, is the author of molar. The full approved plan lives at
`~/.claude/plans/we-are-going-to-rippling-wreath.md`; per-session memory at
`~/.claude/projects/-home-semen-work-Projects-vmd-rs/memory/`.

## Build / run / test

```sh
cargo build
cargo run -p vmd_rs -- tests/2lao.pdb [more files...]   # each file = one molecule
cargo test -p vmd_rs_core
cargo build -p vmd_rs_core --target wasm32-unknown-unknown   # WASM-readiness check
```

- Test assets in `tests/`: `2lao.pdb` (1911 atoms), `large_375k.gro` (375,548 atoms,
  generated — **not in git**; regenerate per `tests/README.md` with `gmx genconf`).
- Dev machine is **Wayland**; screenshot a running window with
  `spectacle -b -n -f -o out.png` (`-a` = active window).
- Headless verification env hooks (native only): `VMD_RS_DEBUG_REP=vdw|licorice|ballstick|lines`,
  `VMD_RS_DEBUG_SEL="<selection>"`, `VMD_RS_DEBUG_ORBIT=<deg>`, `VMD_RS_DEBUG_ORTHO=1`.

## Tech stack (working versions)

eframe / egui / egui-wgpu **0.34.3**, wgpu **29.0.3**, egui-phosphor **0.12** (icon font),
glam **0.32** (GPU/camera math), nalgebra **0.34** (molar boundary), bytemuck **1.25**,
molar **1.4** (local path dep `../molar/molar`, `default-features=false` → `Float=f32`).
GROMACS 2026.1 available as `gmx`.

## Workspace & modules

`crates/vmd_rs_core` (library, WASM-safe, all logic) + `crates/vmd_rs` (native bin:
argv + logging). **Modern module layout** (`<module>.rs` + `<module>/`, no `mod.rs`).

- `lib.rs` — module decls, `run`/`App` re-exports.
- `launch.rs` — `AppLaunch`, eframe bootstrap (`Renderer::Wgpu`).
- `app.rs` — `eframe::App`; owns `SceneRenderer`, `Camera`, `Scene`; left panel
  (Scene/Molecules/Representations/Controls) + central viewport; `rebuild_dirty()`
  and the render-skip logic. Holds the `VMD_RS_DEBUG_*` hooks.
- `theme.rs` — installs the Phosphor icon font + a high-contrast dark style, larger fonts.
- `camera.rs` — quaternion arcball `Camera` (orbit/pan/zoom), perspective **and**
  orthographic projection, `frame_bbox`. `#[derive(PartialEq)]` drives render-skip.
- `color.rs` — CPK element colors → packed RGBA8 (`u32`).
- `geometry.rs` — `RepKind`, `RepParams`, `GeometryData`; `build(system, sel, bonds, …)`
  binds the `Sel` (`system.bind`) and reads positions/atoms via `iter_particle` — nothing
  cached. Spheres come from the selected atoms; bonds are emitted where both endpoints are
  selected (half-bond split, colored by each atom).
- `scene.rs` — `Scene { molecules, selected_mol, trash }`, `Molecule` (molar `System` +
  guessed `bonds` + bbox + `reps`; the `System` is the single source of per-atom data),
  `Representation` (kind / params / `sel_text` (editable buffer) / `expr: SelectionExpr`
  (compiled) / `sel: Sel` (evaluated) / visible / dirty flags / `RepGpu`), `evaluate()`
  (text → `SelectionExpr` → `Sel`).
- `data.rs` + `data/loader.rs` (`RawMolecule`: System + guessed bonds + bbox; positions/
  radii are transient, used only for bond guessing) + `data/bonds.rs` (VDW-fraction filter).
- `render.rs` — `SceneRenderer`: offscreen color + `Depth32Float` targets (Strategy A),
  camera UBO (bind group 0), sphere/cylinder/line pipelines, `RepGpu` (per-rep buffers),
  `upload()`, `render_scene()`, `texture_id()`. Plus `render/{sphere,cylinder,line,
  camera_uniform}.rs` and `render/shaders/*.wgsl`.

## Key architecture

- **Strategy A rendering** — the 3D scene is drawn into our *own* offscreen color +
  depth textures, then composited into egui as an `Image`. egui's render pass has no
  depth attachment; this gives full depth control for impostors.
- **Impostors** — spheres & cylinders are GPU ray-cast in fragment shaders that write
  analytic `frag_depth`, so they occlude correctly against each other (and, later, the
  cartoon mesh). The camera uniform carries a perspective flag: perspective uses an
  eye-ray from the origin; **orthographic uses a parallel ray with origin on the camera
  plane (z=0)** so the near hit has t>0 (a past bug black-screened ortho). Lines are
  plain 1px GL lines. Half-bond coloring = two half-segments per bond, colored by each
  endpoint atom.
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
- Selection grammar incl.: `all`, `protein`, `backbone`, `water`, `name`, `resid`,
  `resindex`, `resname`, `index`, `chain`, `within …`.
- **Trajectory plan (future):** `System::set_state(&mut self, State) -> Result<State>` —
  plain `&mut`, **no interior mutability needed** (the App owns the `Scene` mutably). Per
  frame: read a `State` (`FileHandler::read_state`), `mol.system.set_state(frame)`, then
  re-evaluate each rep's stored `SelectionExpr` → fresh `Sel` (required for coordinate-
  dependent selections like `within …`) and rebuild geometry. `Sel`s stay valid across
  `set_state` as long as topology is unchanged.
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

## UI layout (left panel)

History toolbar (undo/redo buttons, each with a `▼` dropdown listing named actions for
**cumulative** undo/redo; also Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y) → `Scene` (projection icon
toggles; **orthographic is the default**) → `Molecules` (one row each: name + atom count,
right-justified eye/trash) → `Representations` ("Add" button, then rich rows). No
standalone controls section — params live in a per-rep gear popup.

Each rep row (custom layout, not a Grid): **drag handle** (`DOTS_SIX_VERTICAL` wrapped in
`dnd_drag_source(payload=index)`; rows are drop targets via `dnd_hover_payload`/
`dnd_release_payload`, reorder applied after the loop) · **selection field** (narrow;
focusing it sets `editing_rep` and re-renders the row as a single full-width editor,
collapsing on Enter/blur) · **drawn style-icon dropdown** (`paint_style_icon` draws each
`RepKind`; click → `egui::Popup::menu` with icon+label rows via `style_option`) ·
**right-justified compact action group** (`Layout::right_to_left` + `compact_actions`:
button_padding (3,1), item_spacing.x 2): gear (`GEAR_SIX`, opens `draw_rep_params` popup) ·
eye · update-every-frame (`rep.dynamic`, ↻) · duplicate · trash. History labels via
`describe_change` ("edit selection", "reorder representations", …). FPS in the footer.

## Milestone status

- ✅ M0 scaffold + offscreen triangle
- ✅ M1 molar load + VDW sphere impostors (analytic frag_depth)
- ✅ M2 arcball camera + VMD mouse nav
- ✅ M3 bonds → Licorice / Ball-and-Stick / Lines (cylinder impostors, half-bond lines)
- ✅ M4 multi-molecule / multi-rep scene + selection strings + icon panel UI +
  perspective/orthographic toggle + scene-dirty render-skip
- ✅ Undo/Redo (history.rs) + big rep-row UI revamp (drag/expand/style-icon/gear)
- ✅ M5 coloring schemes — `color.rs` `ColorMethod` {Element, Chain, ResID, ResName,
  Index, Beta} + `Colorizer` (per-method, with B-factor range / index gradient context);
  `geometry::build` colors each atom via the rep's `color`. Per-rep color dropdown in the
  table next to the style dropdown, with drawn descriptive icons (`paint_color_icon`: CPK
  dots / categorical bars / rainbow / blue-white-red). **Secondary-structure coloring is
  deferred to M6** (molar `Dssp::ss()` is an ordered backbone array with no resindex map;
  build that mapping with the cartoon work).
- ⏭ Next: **M6 cartoon** (spline + DSSP), which also unlocks secondary-structure coloring.
