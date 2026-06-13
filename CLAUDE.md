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
- Headless verification env hooks (native only): `VMD_RS_DEBUG_REP=vdw|licorice|ballstick|lines|cartoon`,
  `VMD_RS_DEBUG_SEL="<selection>"`,
  `VMD_RS_DEBUG_COLOR=element|chain|resid|resname|index|beta|secstruct`,
  `VMD_RS_DEBUG_ALLCOLORS=1` (one rep per color scheme, cycling styles — shows every icon),
  `VMD_RS_DEBUG_ORBIT=<deg>`, `VMD_RS_DEBUG_ORTHO=1`.

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
  (text → `SelectionExpr` → `Sel`).
- `data.rs` + `data/loader.rs` (`RawMolecule`: System + guessed bonds + bbox; positions/
  radii are transient, used only for bond guessing) + `data/bonds.rs` (VDW-fraction filter).
- `render.rs` — `SceneRenderer`: offscreen color + `Depth32Float` targets (Strategy A),
  camera UBO (bind group 0), sphere/cylinder/line/**mesh** pipelines, `RepGpu` (per-rep
  buffers; mesh = vertex + u32 index buffer), `upload()`, `render_scene()`, `texture_id()`.
  Plus `render/{sphere,cylinder,line,mesh,camera_uniform}.rs` and `render/shaders/*.wgsl`.
  The cartoon mesh writes real depth and interleaves correctly with the impostors.

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
toggles, **orthographic is the default**; **depth-cue** on/off + Strength/Start sliders) →
`Molecules` (one row each: name + atom count,
right-justified eye/trash) → `Representations` ("Add" button, then rich rows). No
standalone controls section — params live in a per-rep gear popup.

Each rep is a **two-row block** (`ui.vertical`; the whole block is the reorder drop target
via `dnd_hover_payload`/`dnd_release_payload`):
- **Row 1**: **drag handle** (`DOTS_SIX_VERTICAL` in `dnd_drag_source(payload=index)`) ·
  **selection field** (fills width; focusing sets `editing_rep` and expands it to a
  full-width editor, collapsing on Enter/blur) · right-justified compact actions
  (`Layout::right_to_left` + `compact_actions`): eye · update-every-frame (`rep.dynamic`, ↻) ·
  duplicate · trash.
- **Row 2** (indented by the drag-handle width, so it aligns under the selection field):
  **style** dropdown · **color** dropdown · **gear** (`GEAR_SIX`, toggles the inline
  `draw_rep_params` expander). Style and color are **icon+text** buttons built by the shared
  `picker_button(label, draw_icon)` helper (drawn glyph + label + caret → `egui::Popup::menu`
  of icon+label rows). `paint_style_icon` draws each `RepKind`; `paint_color_icon` draws each
  `ColorMethod` (Element = CPK dots, Chain = interlocking colored links, ResID =
  backbone-with-residues diagram, ResName = "ALA" on rainbow, Index = "123" colored digits,
  Beta = "B" on rainbow).

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
  own knobs); `geometry::build` dispatches on it (no more `kind` arg).
- ✅ MVP complete (M0–M6, all five representations).
