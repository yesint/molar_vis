# molar_vis — Milestone status

> Reference doc for [molar_vis](../CLAUDE.md). Split out of the master `CLAUDE.md` for on-demand reading — see it for the project overview, build quick-start, and the full docs index.

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
  (`add_atom`/`add_bond`/`cycle_bond_order`). Structure edits are undoable via the unified
  `history` timeline (`StructEdit::Topology`/`Coords`; M30 — was `MolState.structure` +
  the now-removed `editable` flag). A
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
- ✅ M25 **`app.rs` modularization** — the 7276-line `app.rs` (4× the next file) was split into a
  thin root (~690 lines: `App` struct + enums + `ui` loop + `rebuild_dirty` + IME tests) plus **14
  `app/` submodules** by concern (see the `app.rs` + `app/` module bullet). Pure no-behavior-change
  move: the `impl App` methods distribute across files reading `App`'s private fields directly
  (descendant-module privacy), with cross-module helpers/methods/moved-struct-fields bumped to
  `pub(super)` — the only non-mechanical edit. Also folded in 4 clippy cleanups exposed along the way
  (De Morgan in `rebuild_dirty`, `while let` in `poll_loaders`, two `is_none_or` in `draw_viewport`).
  Verified: native build 0 warnings, wasm build 0 errors (4 warnings, all pre-existing — confirmed
  byte-identical via git-stash; the split even *removed* the original's `SphereInstance` warning by
  gating it), 62 tests pass, app-module clippy clean, the save→load→save **session round-trip stays
  byte-identical**, `SAVE_MOL` writes 1911 atoms, and a screenshot shows the app rendering + the
  console-applied script working.
- ✅ M26 **Native Python module — drive the viewer from Python/Jupyter (zero-copy)** — the
  "molar_vis becomes a proper Python module" half of the dual-host scripting plan (see the
  [[scripting-dual-host-architecture]] memory). `import molar_vis as mv; s = mv.System('p.pdb');
  vis = mv.spawn(); mol = vis.add_mol(s); rep = mol.add_rep(s('protein'), style='cartoon',
  color='ss'); sel.translate([1,0,0])  # live; for r in vis.mols[0].reps: r.style='lines'`. The
  viewer renders **directly from the pymolar `System`** (no copy), on a **background thread** so the
  Python REPL stays responsive. Three layers:
  - **`crates/molar_vis_py`** (new cdylib `name = "molar_vis"`, pyo3 0.27 + maturin; deps
    molar_vis_core + molar_python (rlib) + molar): `PySystemSource { sys: Py<System>, top/st: *const`
    raw ptrs `}` impls `SharedSource` — `new(py, sys)` caches `r_top()`/`r_st()` pointers under the
    GIL, `topology()`/`state()` deref them (the pymolar `UnsafeCell`-under-GIL model; `unsafe impl
    Send`), `evaluate()` uses `(&SelectionExpr).into_sel_index(systempy, None)`. `spawn() ->
    Visualizer` runs `eframe::run_native` on a `std::thread` with winit `with_any_thread(true)`
    (Wayland+X11/Windows; macOS pending) + a `Sender<AppJob>` to the GUI thread. `Visualizer`/
    `MolHandle`/`RepHandle` pyclasses: `add_mol(Py<System>)`, `add_rep(sel=,style=,color=,material=)`,
    `mols`/`reps` getters, `rep.style/color/material` `#[setter]`s, `rep.select(sel)`. Append-only
    structure tracked in a shared `Arc<Mutex<Vec<usize>>>` (rep count per mol) — no query channel.
    `Visualizer` also has the full **view-controls** surface (mirrors the view-settings UI):
    `rotate`/`roll`/`pan`/`zoom`/`reset_view`, `projection`, `background`/`background_gradient`,
    `axes`, `depth_cue`, `ambient_occlusion`, `shadows` — each parses its string enum args
    (`Projection`/`CueMode`/`Corner`, re-exported from core) then sends a job to a `pub` `App`
    view method (`rotate_view`/`set_projection`/`set_background_*`/`show_axes`/`set_depth_cue`/…),
    which mutate `Camera`/`axes_*` (Camera `PartialEq` re-renders automatically). Camera grew
    angle/fraction/factor nav helpers (`rotate_deg`/`roll_deg`/`pan_fraction`/`zoom_by`).
    The `#[pymodule]` calls `molar_python::register_molar(m)` so `System`/`Sel`/… are re-exported with
    one consistent PyO3 type identity across the analysis + viewer APIs.
  - **`molar_vis_core` seam**: `MolData::Shared` + `SharedSource` ([[moldata.rs]]); `App` gained an
    external job channel — `pub type AppJob = Box<dyn FnOnce(&mut App) + Send>`, `jobs_rx` field,
    `set_jobs(rx)`, `run_external_jobs()` drained at the top of `ui()` (and while connected the
    viewport `request_repaint_after(16ms)` to poll, since egui only calls `ui` on input/repaint);
    `mark_shared_dirty()` re-marks a shared molecule `coords_dirty` so a Python-side `sel.translate()`
    (in-place coord edit) renders live (reused trajectory `coords_dirty` path, no DSSP) — but **only
    when its coordinates actually changed**, detected by polling a coordinate **version counter** (see
    below) and comparing to `Molecule.shared_coords_version`; a static shared molecule costs nothing
    (idle = 0 GPU preserved). `SharedSource::coords_version()` + `MolData::coords_version()` expose it.
    `pub` App methods the jobs call: `add_shared_molecule`, `add_rep_default`,
    `set_rep_{style,color,material,selection}` (selection via `pick::index_selection_string`).
  - **molar changes** (pushed to master, rev `ae3b3d8`): `Sel::bind_to(&top,&st) -> SelBoundParts`
    (the disjoint parts-bind the shared backend needs); `SystemPy`/`SelPy` `r_top`/`r_st`/`py_*`/
    `index` accessors made `pub`; molar_python now `crate-type=["cdylib","rlib"]` + re-exports
    `System`/`Sel`/`State`/`Topology` + a reusable `pub fn register_molar(m)`; fixed pre-existing
    molar_python `Bond`-type drift (`&[usize;2]`→`&Bond`); and (ae3b3d8) a **coords version counter**:
    `StatePy` carries an `AtomicU64` bumped (`Release`) by every in-place coord mutator
    (`Sel.translate`/`apply_transform`/`unwrap_simple`, the `coords` setter, `Particle.pos`/`x`/`y`/`z`),
    read via `coords_version_atomic()`; `PySystemSource` caches `*const AtomicU64` and loads it
    (`Acquire`) lock-free per frame (no GIL). ~free + unread for standalone pymolar. molar_vis pins
    molar @ `ae3b3d8`.
  - **Dep note**: molar wants nalgebra 0.35, numpy 0.27 (in molar_python) wants 0.34 → workspace
    pinned to nalgebra 0.34.2 (`cargo update -p nalgebra@0.35.0 --precise 0.34.2`). pyo3 0.27.2 builds
    for CPython 3.14 with `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`.
  - **Verified at runtime** (maturin + python3.14 + display on the dev box): `import molar_vis`, load
    2lao (1911 atoms), `s('protein')` 1822, spawn opens the real window on the bg thread (61 fps,
    REPL live), add_mol+add_rep renders the **SS-colored cartoon** (screenshot), `vis.mols[0].reps`
    enumerates reps, `rep.style='lines'` flips live, and `sel.translate()` of `resid 1:120` (904
    atoms) visibly deforms the cartoon live. Native build 0 warnings, 63 core tests pass, wasm 4
    pre-existing warnings. **Deferred**: macOS main-thread loop; GIL-discipline
    on the render-time raw-ptr reads (in-place edits are at worst a 1-frame glitch, never UB; only a
    Python-side `System.state` *reassignment* could dangle — documented limitation); two-way edits
    *from* the viewer back to pymolar; the web JS-API/anywidget half of the dual-host plan.
- ✅ M27 **Browser JavaScript API — drive the viewer from a web page (wasm-bindgen)** — the
  **web half** of the dual-host plan (the deferred M26 item). A surrounding page controls the
  running wasm viewer through a surface that mirrors `molar_vis_py` almost line-for-line:
  `import init, { start, System } from "./pkg/molar_vis.js"; await init(); const vis =
  start("molar_vis_canvas"); const sys = System.from_bytes("p.pdb", bytes); const mol =
  vis.add_mol(sys); const rep = mol.add_rep(sys.select("protein"), "cartoon", "ss"); rep.style =
  "lines"; vis.rotate(30,15); vis.projection("perspective");`. Three layers:
  - **`crates/molar_vis_js`** (new cdylib `name = "molar_vis"`, wasm-bindgen, built with **wasm-pack
    `--target web`**): the JS face mirroring `molar_vis_py` — `System`/`Sel`/`Visualizer`/`MolHandle`/
    `RepHandle` + `start()`, the same handle bookkeeping (`Rc<RefCell<Vec<usize>>>` rep counts, the
    single-threaded analog of `_py`'s `Arc<Mutex<…>>`), the same view-control surface. Content gated
    to `#![cfg(target_arch = "wasm32")]` so a native `cargo build` compiles it to an empty cdylib (only
    wasm-pack builds it for real). The handles push [`AppJob`] closures onto a channel drained in
    `App::ui` (the M26 seam reused verbatim — no `ui()` change); `start()` boots `eframe::WebRunner`
    via `spawn_local` with `app.set_jobs(rx)` (no demo auto-load), returns the `Visualizer`
    synchronously (commands buffer in the channel until the App drains them — same pattern as
    `_py::spawn`). One viewer per page (a `thread_local` guard).
  - **The data model** — unlike `_py` (which shares pymolar memory via raw pointers under the GIL),
    the browser **owns** its data: a JS `System` holds an `Rc<System>`, shared into the scene by
    reference via a **`WebSystemSource`** (a `SharedSource` impl, [[moldata.rs]]) with **plain safe
    borrows** (`self.system.topology()`/`.state()` tied to `&self` through the `Rc`) — no raw
    pointers, no `unsafe`, no GIL. `evaluate` calls the core `evaluate(&System, text)` directly.
    `System.from_bytes` parses via molar `FileHandler::from_reader` (bonds/bbox guessed at add time
    in `Molecule::new_shared`); `select` → `Sel` (a frozen `Vec<usize>` from `Sel::get_index_slice`).
    v1 coordinates are **static after load** (`coords_version` constant); live JS coord edits deferred.
  - **`molar_vis_core` seam** (minimal): the **`AppJob` alias is cfg-split** — `Box<dyn FnOnce(&mut
    App) + Send>` on native, **`Box<dyn FnOnce(&mut App)>` (no `Send`) on wasm**, since the browser is
    single-threaded (the channel never crosses a thread) and a job captures the non-`Send` `Rc<System>`.
    The three view-enum parsers (`parse_projection`/`parse_corner`/`parse_cue_mode`) were **hoisted**
    from `_py` into `script/command.rs` (return `Result<_, String>`; each binding maps to its own error
    type) and re-exported alongside `scene::evaluate`. No new render branch.
  - **Demo + CI (dogfood)** — the GitHub Pages demo (`crates/molar_vis_js/web/index.html`) now drives
    the viewer through the **public JS API** (fetch `2lao.pdb` → `System.from_bytes` → `add_mol` →
    `add_rep`), so the surface can't silently rot; `.github/workflows/pages.yml` builds with wasm-pack
    and assembles `dist/` (index.html + `pkg/` + `2lao.pdb`). `crates/molar_vis_web` (trunk bin) stays
    for `trunk serve` local dev but is no longer the published artifact.
  - **Verified**: native build green + 63 core tests pass; `molar_vis_py` still green (parser hoist);
    `cargo build -p molar_vis_core --target wasm32-unknown-unknown` green; `wasm-pack build` emits
    `pkg/molar_vis.js` with all six named exports (+ correct `.d.ts`); and a **headless chromium** run
    of the demo executed the full API path end-to-end — `total=1911 protein=1822 mols=1 reps=2` (the
    1822 protein count matches the M26 runtime verification). Only the rendered pixels need a human
    glance (same `add_shared_molecule`/`rebuild_dirty` path already verified for `_py` + the app).
    **Deferred**: the anywidget/Jupyter wrapper (the same `pkg/` + a thin `_esm`); multi-viewer per
    page; live JS-driven coordinate edits (needs interior mutability + a `SharedSource::state` change);
    camelCase method aliases (snake_case kept canonical for pymolar parity).
- ✅ M28 **Molecular groups (multi-molecule SDF)** — a "drug-discovery goodies / SDF reading"
  roadmap item. molar's SDF handler returns a **fresh `(Topology, State)` per `$$$$` record**
  (each a distinct molecule with its own atoms+bonds); `data::load_records` (+ wasm bytes variant,
  `data/loader.rs`) loops `FileHandler::read()` to EOF to load them all. A multi-record SDF/MOL (≥2
  records) becomes a **`MolGroup`** (`scene.rs`): the members are ordinary `Molecule`s in the flat
  `scene.molecules` tagged with `Molecule.group: Option<GroupId>`, plus a side `Scene.groups` layer
  — so the whole render/rebuild/pick/raytrace pipeline (which iterates flatly and gates on
  `mol.visible`) is **unchanged**. **Only one member is shown at a time** (`MolGroup.current`,
  enforced by `apply_visibility`: shown member visible ∧ group eye, others hidden). **Shared reps**
  (apply to every member) are *not* a separate field — the live, editable shared `Representation`s
  are the **first `Molecule.n_shared` reps of the shown member** (the single source of truth, so the
  renderer needs no group-awareness); `switch_group_member` strips the prefix off the old member and
  re-materializes it onto the new one (re-evaluated against its topology). UI (`app/panels.rs`
  `draw_group_entry` + `app/rep_panel.rs` `draw_group_bar`): a panel entry with a **STACK** icon +
  file name + expander, a **trajectory-style cycle bar (first/prev/slider/next/last, no play/fps)**
  to choose the shown member (**partial focus** — cycling or clicking a member name shows it and
  **centers** the camera on it by panning the target only, keeping the current zoom; the header
  magnifier is a **full zoom-to-fit**; the slider tooltip is anchored **under the knob** showing
  "N/M `<name>`"), an **Edit** button (opens the *shown* member in the drawing editor), and a group
  `LIST` menu (**Save group…** — writes all members to one multi-record file via `save_group_to`;
  **Delete group**). **Two independent expanders**: the header caret shows the **shared reps** + a
  nested **"Molecules (N)"** sub-expander (its own `members_expanded` fold), which lists each member
  (real name from the SDF title line, a **clickable label** that partial-focuses it; the **shown
  member's row is highlighted** with an accent bar; a per-member `LIST` menu → **Delete molecule**)
  collapsible to its **own** reps (`draw_reps_for` gained a `start,end,is_shared` sub-range). The whole left-panel list
  scrolls (a single panel-level `ScrollArea`, non-floating scrollbar; fps in a bottom sub-panel).
  Per-member delete preserves the shared reps (`Scene::remove_grouped_molecule` re-materializes them
  onto the new shown member). New-group default
  shared rep = **Licorice** (small organics). **Undo**: `history::GroupState` in `EditState` captures
  shared reps + membership + group visibility (member-own reps captured for free as the suffix);
  member-switch is **view state, not undoable** (grouped members' visibility pinned constant in
  capture so cycling never lands on the stack). **Sessions**: `session::GroupSession`/`MemberSession`
  (`Session.groups`, all `#[serde(default)]`); capture excludes group members from `molecules`;
  `apply_session` re-opens the SDF and rebuilds members by record index — group session round-trips.
  Test fixture **`tests/ligands20.sdf`** (20 ChEMBL drugs via `obabel --gen3d`, sizes
  metformin→atorvastatin). Headless hooks `MOLAR_VIS_DEBUG_SDF=<path>` (+ `_GROUP_MEMBER=<n>` /
  `_GROUP_EXPAND=1`). Verified: 68 tests (incl. a group session round-trip), native+wasm+py green,
  clippy clean; headless renders of member 0 (aspirin) / member 7 (diazepam, camera re-fit) / a
  reloaded session (member 5, dopamine) + a panel screenshot (group row, cycle bar, member list).
- ✅ M29 **Protein–ligand interactions (`Interactions` rep)** — a "drug-discovery goodies / PLIP
  interactions" roadmap item: a new rep **style** that draws the six non-covalent interaction types —
  **H-bonds, hydrophobic, salt bridges, π-stacking, π-cation, halogen bonds** — as Discovery-Studio-style
  dashed lines (green / grey / orange / purple / magenta / teal) between its own selection and a chosen
  **partner** rep in *this or another molecule* (the first rep that references atoms outside its own
  molecule). Pieces: **detection** ([[interactions.rs]], pure/WASM-safe, PLIP-derived — atom-level types
  grid-based, *not* N×M; group types over the small ring/charge lists; per-type user-editable cutoffs +
  angles/offsets via `InteractionSettings`→`DetectParams`; auto H-vs-heavy-atom H-bond fallback for
  structures without hydrogens; see that module bullet); **gather** (`app::build::gather_set` builds an
  `InteractionSet` per rep = heavy atoms (+attached H / hydrophobic flag / halogen antecedent), aromatic
  **rings** within the selection (centroid+normal; ring atom sets from `Molecule::ensure_interaction_rings`,
  a cached molar ring-perception on a topology clone — no bond side-effects), and **charged groups**
  (`charged_groups`: Arg/Lys/His +, Asp/Glu/C-term −, ligand carboxylate/guanidinium/phosphate by
  connectivity — heuristic, no formal charges)); **cross-molecule build** runs in a **second
  `rebuild_dirty` pass** (reads two molecules, so it's outside the `&mut`-iterator loop; rebuilds when its
  own flags or *either* endpoint molecule changed, tracked by `mol_changed`; both molecules' ring caches
  are populated mutably first); **partner reference** `Representation.partner: Option<(MoleculeSource,
  usize)>` — serializable + reload-stable, so it round-trips undo/redo **and** sessions for free via
  `history::RepState.partner` (no MolId↔source seam); **auto-update on partner change** — the second
  pass also rebuilds on `view_dirty` (visibility / molecule add-remove / group member switch), and
  `partner_index` is **group-following**: a partner pointing at a [`MolGroup`] member resolves to the
  group's *currently-shown* member, so sliding the group's member slider moves the interactions to the
  newly-shown ligand (partner label + detection stay in sync); **UI** (`app/rep_panel.rs`) — switching a rep **to**
  Interactions **clones the old rep** (its previous style, kept visible, re-inserted just above) so the
  molecule's look isn't lost (an Interactions rep only draws contact lines); the style is auto-expanded.
  Color/material pickers are hidden (type picks the color); instead the style row carries the **Partner**
  controls inline (clickable "Mol N: Rep M" focus link + **⊕ Choose…**), and the expanded params are a
  single rep-level **Line width** slider (applies to all types) + a **Settings…** button opening the
  **tabbed dialog** (`draw_interactions_dialog`, one tab per type, each with that type's full parameter
  set — H-bonds has D–A distance, D–A-with-H, min angle — + a Reset-all footer; line width is *not* in
  the dialog); **partner-pick mode** (`App::rep_pick`, now shared with the alignment dialog's ⌖
  picker via [`RepPick`] — M33; `app/viewport.rs`) — [Choose…] enters a
  mode where hovering a rep's geometry highlights the whole rep (finger cursor, via `pick::PickHit.rep`)
  and a click assigns it, **and** clicking a rep's **panel row** also assigns it, Esc / empty-click
  cancels, one undo checkpoint (`assign_partner`). Interaction lines **are** ray-traced (as thin
  flat-ended capsules, like every other line — see the `render/raytrace.rs` bullet). Verified: 80 tests (grid-vs-brute-force, water-dimer ±H,
  hydrophobic dedup, halogen angle, salt bridge, π-stacking offset, π-cation, group-following partner),
  native+wasm+py green,
  clippy clean; headless renders of a 2lao interface split (H-bond/hydrophobic/salt-bridge dashes) + the
  tabbed dialog (H-bonds & π-stack tabs), a **byte-identical** session round-trip preserving the partner,
  and a live panel screenshot. Hooks `MOLAR_VIS_DEBUG_INTERACTIONS=1` + `_INTERACTIONS_DIALOG=[type]`.
  Deferred: distance labels; water bridges / metal complexes; weak C–H donors; ligand aromaticity relies
  on molar perception (PDB ligands without bond orders may miss rings); scripting access.
- ✅ M30 **Dihedral rotation (edit-mode tool) + one molecule type + delta undo** — three coupled
  changes. **(1) Dihedral rotation** (`app/dihedral.rs`): click a **rotatable** bond (non-ring,
  non-terminal — found by cutting the bond and BFS-splitting the graph) to set it as the rotation
  **axis**, then drag a side-tinted **handle** on a neighbouring bond to twist that half of the
  molecule about the bond (drag angle = the cursor ray projected into the plane ⟂ the axis; the
  per-frame delta drives `Molecule::rotate_fragment`, a rigid rotation of one side's atoms). It's the
  third **tool** of edit (Draw) mode (`DrawTool::DihedralRotate`, state in `DihedralState` on
  `DrawSession`, palette button in `draw_tools_panel`, dispatched from `draw_input`) — *not* a
  separate mode; plain-LMB elsewhere orbits, only a handle drag suppresses orbit. **(2) One molecule
  type**: removed the `Molecule.editable` flag — every owned molecule is editable/undoable; the "Edit"
  button just enters edit mode; the session-save warning keys off a non-`File` source instead. **(3)
  Delta undo** (`history.rs`): a unified `Doc | Struct` timeline — coordinate moves (dihedral,
  minimizer) record a `StructEdit::Coords` delta of **only the moved atoms**; topology edits record a
  `StructEdit::Topology` before/after `Arc<StructureSnapshot>`; per-settle document capture no longer
  touches per-atom data (structure left the document; add/delete-molecule preserves structure via the
  trash). Idle capture is O(1) (`structure_snapshot` rebuilds only on a `structure_version` bump).
  So a dihedral twist (or draw/erase/cleanup) is now **undoable on any molecule** — no "Edit"-to-enable
  step. Verified: native+wasm+py green, **84 tests** (Coords/Topology round-trips, Doc–Struct
  interleave, identity-shared snapshot), clippy clean; headless save-image confirms the twist applies +
  a windowed screenshot of the three-tool palette with the amber axis / side-tinted handles.
  **Follow-ups shipped:** the flaky "?" unspecified-bond overlay was dropped from edit mode; a
  molecular **group** header gained an **Edit** button (opens the *shown* member in the editor); and
  **session save persists edited structures** — a structurally-edited File-source molecule
  (`structure_version > 0`) is written next to the session as `<stem>.edited.<ext>` and the session
  is pointed at that copy, so the edits reload (original untouched; verified: edited file written +
  session round-trip reloads the edited coords). Deferred: persisting edits of *grouped* members
  (they still reload from the SDF by index); edge-on-axis drag is ill-conditioned (freezes rather
  than jumps).
- ✅ M31 **molar 2.1 migration + charge coloring (espaloma) + optional scripting** — four coupled
  changes.
  **(1) molar 2.1** (SoA atom + columnar bond storage): every atom property read became an
  `AtomLike::get_*()` call on the `AtomRef`/`AtomRefMut` proxies (there is no `&Atom`/`&Bond` to
  borrow any more), our three `&Atom`-taking helpers now take the proxy, and the perception bridge
  goes through a prebuilt `BondAdjacency`. See the *molar 2.1 API* notes. Driven off rustc's own error
  list so only flagged expressions moved.
  **(2) Connectivity** (the "is our perception redundant?" audit): our distance-based bond guessing
  stays (molar has none), and our aromaticity was already molar's — but the loader was **discarding
  every bond the file gave us**, which silently meant aromatic-ring perception found *nothing* on any
  loaded molecule (a benzene of order-less bonds reads as sp3), disabling π-stacking/π-cation for
  exactly the ligands M29 was built for. `bonds::resolve` now owns the policy (SDF verbatim / CONECT
  unioned / guess), the resolved graph is **published into the topology** (`sync_bonds_to_topology`,
  via molar's new `System::set_bonds`) so `polh`/`apolh` and `molar_ff` see it, and
  `ensure_interaction_rings` uses molar's new **non-mutating `aromatic_rings`** instead of cloning the
  whole topology to run the destructive `perceive`. Side wins: CG bonds (`2lao_cg.pdb` 290 → 858) and
  a molar bug where every cysteine `SG` was typed as **seaborgium** (the PDB element column was
  ignored).
  **(3) Charge coloring** (`ColorMethod::Charge` + `ChargeKind` + `ColorSpec` + the **[Color]** tab +
  `charges.rs` + the native-only `molar_ff` dep): a diverging red–white–blue ramp normalized to the
  selection's extremes, with partial/formal as an **option of the one scheme**, and espaloma
  prediction on demand recorded as an undoable `StructEdit::Charges`. Also capped the multi-order bond
  strand radius at Ball-and-Stick's — with SDF orders now surviving, Licorice's double bonds were
  twice as thick *and* twice as splayed.
  **(4) `scripting` feature** (non-default): Rhai + the console UI moved behind it, split along the
  seam that already existed — the `Command`/`apply_scene_command` layer the app's menus and the
  Python/JS hosts use stays always-compiled in `script.rs`; everything Rhai-specific moved to
  `script/engine.rs`.
  Verified: **96 tests** with `--features scripting`, 92 without (16 new across the four changes),
  all five crates building for native + wasm32 in **both** configurations with no warnings, clippy
  clean, a byte-identical session round-trip, and headless renders of the charge scheme on aspirin
  (21 atoms, −0.683…+0.830 e, carbonyl O red / their C blue), the SS/B-factor/ResID schemes, the four
  rep styles, and the fixed Licorice double bonds (Ball-and-Stick byte-identical to before). molar
  pinned at rev `f161420` (2.1.0 + `implicit_hydrogens` restored, `aromatic_rings`,
  `System::set_bonds`, the PDB element-column fix). **Deferred**: name-based element guessing is still
  ambiguous for sources with no element column (a GRO's `SG` is still seaborgium — fixing it means
  teaching the guesser about remoteness codes); computed charges are not saved in sessions (molecules
  reload from disk); espaloma needs Kekulé orders, so PDB/GRO inputs can't be charged at all; and the
  `float_literal_f32_fallback` warnings from rustc 1.97 on egui literals are untouched (31 sites, all
  pre-existing and unrelated).
- ✅ M32 **Docking results — receptor + ligand poses in one load** — **Molecule ▸ Load docking
  data…** (`docking.rs` + `app/docking_dialog.rs`, native): a dialog with *Protein* `[Choose…]` /
  *Ligands* `[Choose…]` / `[Load]` `[Cancel]` that does in one step what was a fiddly manual
  sequence — open the receptor, append its ensemble, open the poses as a group, add an
  `Interactions` rep, aim it with the partner picker. Ligands are one multi-record SDF or one file
  per pose (multi-select) → a [`MolGroup`]; the protein's first file supplies the topology and its
  own coordinates + the frames of the files after it are the conformations, validated to be
  **1 frame (rigid docking) or one per pose (flexible)** before the scene is touched. The pose
  group gets an `Interactions` shared rep already pointed at the receptor, and a flexible pair
  **steps together in both directions** (pose ⇄ receptor frame, playback included) — see the
  module bullet for the reconcile design and for when the receptor's own structure frame counts
  as a pose (`structure_frame_counts`). Fixtures: `tests/jak2.pdb` (4844
  atoms) + `tests/jak2_inhs.sd` (26 ChEMBL inhibitors) + a generated 26-frame `tests/jak2_traj.pdb`
  (not in git; see `tests/README.md`). Verified: 13 unit tests over the three pure decisions
  (mode from frame counts, whether the structure file's own frame is a pose, which side to drive
  — first reconcile, either direction, playback,
  both-moved precedence, idle), 103 tests total (107 with `scripting`), all crates green for
  native + wasm32 in both feature configurations with no warnings, and headless offscreen checks
  of the dialog, the loaded result (pose in the site with green H-bond dashes, receptor at 26
  frames), both coupling directions at several indices, rigid mode staying decoupled, and the
  mismatch error. Hooks `MOLAR_VIS_DEBUG_DOCKING[_POSE|_FRAME|_DIALOG]`. **Deferred**: sessions
  don't yet record the pairing as *docking* (the group + the Interactions partner round-trip, so
  the coupling re-establishes itself on load, but a multi-file ligand selection has no single
  source to reload the group from — the members keep their own per-file sources, as for a
  browser-loaded group); no per-pose score display or sorting.
- ✅ M33 **Analysis menu — superposition + RMSD** — a new top-level **Analysis** menu whose first
  entry, **Align…**, opens a non-modal window that fits one selection onto another and reports the
  RMSD ([`analysis.rs`] + `app/align_dialog.rs`; see those bullets and the *Menu bar* section for
  every control's semantics). The maths is molar's (`fit_transform`/`rmsd`, and
  `get_matching_atoms_by_name` for the **Common subset** pairing); the work here is the pairing
  policy, the frame/atom bookkeeping, a **read-everything-then-write** apply (the target may be a
  frame of the very molecule being moved), and one **undoable** step per press however many frames
  it moved. Along the way the Interactions partner picker was generalized into one
  **`RepPick`** gesture (`Partner` | `Align(side)`, dispatched by `App::choose_rep`), which is what
  let the dialog drop the "Selection vs Existing rep" distinction its first sketch had: a rep is a
  molecule plus a selection, so the ⌖ picker just writes both into the row. Verified: 8 unit tests
  (identical copies → 0; a rigidly rotated+translated copy recovered to <1e-3 nm; the count-mismatch
  refusal and the name-paired path; a <3-atom fit refused; **undo restores the pre-align
  coordinates**; `Move whole molecule` off moving only the selection; both label shapes; a 20-member
  SDF group listed as **one** dropdown row with its members inside), 137
  tests total (141 with `scripting`), native + wasm32 +
  `molar_vis_py` green with no warnings, clippy clean, and headless checks: an identical-pair RMSD of
  0.000, the count-mismatch message, a 27-frame `same as source` trajectory fit
  (`0.036 nm — mean of 27 frames (min 0.000, max 0.061)`, min 0 being the reference frame fitting
  itself), before/after renders of a 40°-rotated + 1.5 nm-translated copy of 2lao coming back onto
  the original, two group members aligned by common subset (0.499 nm) and a member against
  itself (0.000), and window shots of the dialog fresh, with a readout, in rep-pick mode, and
  defaulting to a group's shown member. Hooks
  `MOLAR_VIS_DEBUG_ALIGN` / `_ALIGN_DIALOG`. **Deferred**: RMSD-vs-frame curves (the readout is a
  summary line, not a plot); mass-weighted RMSD (molar has `rmsd_mw`, unexposed); no scripting/JS/
  Python binding for `align` yet; and the picker's *click delivery* is the partner picker's own path,
  so it is covered by that feature's verification rather than re-checked headlessly (a click on a
  rep row can't be simulated).
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
    intense **Fresnel rim** (bright at grazing angles + a strong body tint) in a color chosen on the CPU
    from the **viewport backdrop** (`theme::glow_color`, from the theme's `[extras]`: a bright cyan on
    a dark background, a **gold** on a light one — a highlighter on paper) and carried to the shaders
    in the camera uniform, then blended **over** the scene. It used to be one bright cyan blended
    *additively*, which is invisible on a white background — adding to white cannot brighten it; the
    egui-drawn cues (hover ring, lasso polygon, draw-mode rubber band) read the *same* value, so they
    cannot drift from the 3-D glow. **Pulsing**: the
    camera uniform's `params.w` carries an animated multiplier (`0.70 + 0.30·sin(t·3.2)`, computed in
    `draw_viewport`) and while any selection is pending the viewport `request_repaint()`s + force-
    re-renders each frame so it breathes (idle = 0 GPU otherwise). Depth-tested `≤` against the scene
    depth (so occluded atoms don't glow), no depth-write. So the *selected geometry itself glows in
    its current style* — **not** a 2-D overlay. `glow_dirty` rebuilds it when the pending set or
    coords change, **or when any rep's geometry is rebuilt** (so the glow follows a live style/
    selection change); central image only. (2) a **minimal panel block** under the reps
    (`draw_reps_for` → the shared **`pending_stub`**): a marquee glyph + bold "selection" + the atom
    count + **green ✓ accept** + **🗑 discard**, no style/color/material row. The stub is **painted
    in the glow colour and pulses with the glow** — its plate, border and glyph take their alpha from
    `theme::glow_color` × **`App::glow_pulse`**, the *same* two values that drive the highlighted
    geometry (one formula, sampled by the panel and the viewport in the same frame, so they can't
    drift into reading as two unrelated cues). It used to be one more dim italic label among the
    reps and was easy to miss entirely, which made a captured selection look uncommittable. The glow
    colour follows the **viewport backdrop**, not the panel, so it is deliberately *not* used as
    label ink (a light cyan on the light theme's panel would be text at a fraction of its contrast);
    and a suppressed glow (pulse 0, while a ray-traced still is held) leaves the stub at full
    intensity rather than invisible — it is a control, not part of the render, so it only stops
    breathing. **Accept** commits it as a normal, fully-editable **Ball-and-Stick**
    rep over the same `index …` text (this push *is* the undoable step — "add representation");
    **discard** drops it. For a **[`MolGroup`]** member the stub belongs with **that member's own
    rows**, not with the group — the atoms it captured are the *shown member's*, and its siblings
    know nothing about them (accept likewise pushes onto the member, past its shared prefix). Those
    rows are the deepest thing in the panel, behind the group entry *and* the nested "Molecules"
    list *and* the member's rep fold, all closed on a fresh group — so **the tree comes to the
    selection**: `Scene::reveal_pending` opens all three folds and parks a one-shot
    **`Reveal::Pending(mol)`** in `Scene::reveal`, which the pass that draws the stub honours with
    `ui.scroll_to_rect` (the member's own-reps pass therefore runs even with *no* own reps, which a
    freshly loaded SDF member has). Set on a *new* capture only — merging more atoms into an
    existing one doesn't re-scroll, and a branch the user has since folded away stays folded.
    **Accept** does the same for the rep it created (`reveal_rep` → `Reveal::Rep(mol, j)`), which is
    otherwise appended out of sight at the end of a fold. Unconsumed requests are dropped after one
    panel pass (`draw_left_panel`), so one naming a row that has since been deleted can't fire late.
    `MOLAR_VIS_DEBUG_PENDING=<sel>` (+ `_ACCEPT_PENDING=1`) stages/commits one headlessly.
  - **Style-specific eligibility** (shared by hover + lasso via `atom_in_rep(kind, name)`): a
    Cartoon rep is hit only on its **backbone** atoms (`cartoon_atom`: N/CA/C/O + terminal
    OT1/OT2/OXT — what the ribbon is built from), never side chains; every other style is hit on
    all selected atoms (Lines included, via its isolated-atom **crosses**). Tested:
    `lasso_full_screen_selects_all_for_vdw`, `lasso_cartoon_selects_only_backbone`.
  - **TODO:** more pick modes in the dropdown. Picking/lasso is O(visible atoms × images) — fine for
    small/medium systems; a spatial grid / GPU id-buffer is the optimization for huge ones.

