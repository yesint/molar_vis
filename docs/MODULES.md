# molar_vis — Workspace & modules

> Reference doc for [molar_vis](../CLAUDE.md). Split out of the master `CLAUDE.md` for on-demand reading — see it for the project overview, build quick-start, and the full docs index.

## Workspace & modules

`crates/molar_vis_core` (library, WASM-safe, all logic) + `crates/molar_vis` (native bin:
argv + logging) + `crates/molar_vis_web` (wasm bin, the trunk demo) + `crates/molar_vis_py` (native
PyO3 module — M26, see below) + `crates/molar_vis_js` (wasm-bindgen JS API — M27, the browser face
of the viewer, a cdylib built with wasm-pack; content gated to wasm32 so a native build compiles it
empty). **Modern module layout** (`<module>.rs` + `<module>/`, no `mod.rs`).

- `lib.rs` — module decls, `run`/`App` re-exports. Also re-exports the seam the native Python
  module needs: `App`, `AppJob`, `MolData`, `SharedSource`, `EvalError`.
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
- `app.rs` + `app/` — the `eframe::App`. **Split into a thin root + `app/` submodules** (M25,
  was a single 7276-line file): the root (`app.rs`, ~690 lines) holds the `App` struct + small
  private enums (`ViewTab`/`SettingsPage`/`Corner`/`LassoOp`), the `impl eframe::App for App { ui }`
  loop, `rebuild_dirty()` + render-skip logic, `defuse_broken_ime`, the `mod`/`use` wiring, and the
  IME tests. Everything else moved into `app/` (the `impl App` methods read `App`'s **private fields
  directly** — descendant modules see an ancestor's privates; the cross-module helpers/methods/types
  are `pub(super)`, the only non-mechanical change of the split):
  - `app/init.rs` — `App::new` + `debug_draw_preset` (the `MOLAR_VIS_DEBUG_*` hooks fire here).
  - `app/viewport.rs` — `draw_viewport` + hover/lasso/pending-selection methods.
  - `app/panels.rs` — left panel, menu bar, molecule list, top view toolbar, view-settings window.
  - `app/rep_panel.rs` — rep rows: selection field, rep params, Traj/Periodic tabs, traj bar.
  - `app/settings_dialog.rs` — the program-settings dialog (per-tab pages, apply, axes widget).
  - `app/pickers.rs` — style/color/material pickers + their icon/preview painters.
  - `app/widgets.rs` — shared egui helpers (`tab_bar`, `slider_with_edit`, `picker_button`, …).
  - `app/overlay.rs` — viewport overlays (pick/residue info, modifier hint, axes gizmo, glow ring).
  - `app/build.rs` — free-fn geometry builders (`build_glow`/`build_hover_detail`/`build_pick`/…).
  - `app/loaders.rs` — load/delete-frames/rename dialogs, loaders, `pick_file` (cfg-heavy IO).
  - `app/session_io.rs` — save molecule/selection/session, view-state seam, new/reset doc, demo.
  - `app/console.rs` — scripting-console UI + command-execution glue.
  - `app/draw.rs` + `app/draw_input.rs` — Draw-mode types + palette UI / input-gesture engine.
  - `app/dihedral.rs` — the **DihedralRotate tool** of edit (Draw) mode (alongside Draw/Erase; state
    in `DihedralState` on the active `DrawSession`, tool button in `draw_tools_panel`): click a
    **rotatable** bond (non-ring, non-terminal — found by cutting the bond and BFS-splitting the graph
    into the two sides) to set it as the rotation **axis**, then drag a **handle** drawn on each
    neighbouring bond (side-tinted) to twist that side of the molecule about the axis. The drag angle
    is the cursor's ray projected into the rotation plane (⟂ the axis); the per-frame delta drives
    `Molecule::rotate_fragment` (rigid rotation of one side's atoms about the bond line, editing the
    displayed coords in place — trajectory frame or owned System). Plain-LMB elsewhere orbits (only a
    handle drag suppresses orbit); a shared pymolar/JS molecule is skipped (can't mutate). Dispatched
    from `draw_input`. A twist is recorded as a `StructEdit::Coords` step (only the rotated atoms'
    before/after positions), so it's **undoable on any molecule** (see the `history.rs` bullet).
    `MOLAR_VIS_DEBUG_DIHEDRAL[=<mol>]` (+ `_ROTATE=<deg>`) exercises it headlessly.
- **`egui-stylesheet`** (**published crate**, `crates.io/crates/egui-stylesheet`, repo
  `github.com/yesint/egui-stylesheet`; local checkout at `../egui-stylesheet`, deliberately *not* a
  workspace member) — builds an `egui::Style` from a TOML **style sheet**: a `parent` egui preset
  (`dark`/`light`) plus only the fields that change. Extracted from this project once it stopped
  being molecule-specific; it has its own git repo, tests (6 + a doc test) and README (which
  documents the format), and is publishable as-is. How it works: serialize the parent `Visuals` to
  JSON, deep-merge the sheet's `[visuals]` table, deserialize back — so **every** egui field is
  settable by its own name with no per-field dispatcher to maintain, and fields egui *adds* keep
  their new defaults instead of being frozen. JSON (not TOML) is the merge medium because `Visuals`
  has `Option` fields and TOML cannot represent `None`. Sheet sections: `[palette]` (named colours,
  referenced as `"$name"`), `[visuals]` (egui field paths verbatim, dotted or as sub-tables),
  `[text]` (sugar: `normal`/`dim` → the five per-state `fg_stroke`s + `weak_text_color`),
  `[metrics]` (the `Style` knobs outside `Visuals`), `[extras]` (colours egui has **no field** for;
  the host app names them). Values are `#rgb`, `#rrggbb`, `#rrggbbaa` (**premultiplied**, as
  `Color32` requires), `"$name"`, or a raw `[r,g,b,a]`. A **misspelled field path is an error**, not
  a silent no-op (serde ignores unknown fields, so every override is checked against the parent's
  shape first) — the one failure mode a hand-edited file can't be allowed to have. `apply`
  **assigns** the parent + overrides rather than merging into the live visuals, so it's idempotent.
  Also exports `set_text_colors` and `contrasting_text`. It requires egui's **`serde`** feature,
  which it enables itself. **Prior art was searched**: the file-based crates (`egui-theme`,
  `egui-thematic`, `egui-stylist`) store a *full* `Style` dump, are editor-centric, and lag egui by
  1–15 versions; the palette crates (`catppuccin-egui`, `egui-themes`, `egui-aesthetix`) define
  themes in code. None does parent + partial overrides from a file.
- `themes/dark.toml`, `themes/light.toml` — **the app's actual theming, as data**: `include_str!`d
  into the binary (so no runtime file IO, and a `LazyLock` parses each once — tens of µs), applied by
  `style_sheet`. Everything that used to be a `const` in `theme.rs` lives here, with the reasoning
  in comments: the near-black dark panel with text fields *above* it and a **light** (white-bloom)
  shadow — a black shadow has nothing to darken against a dark panel or the dark viewport; and the
  light palette read off the **Purogrey** KDE scheme's own `[Colors:*]` groups (mid-grey 198 window,
  *black* text, bright 226/235 for indicators and fields, dark selection plate with near-white ink,
  a title bar one shade *below* the window body, and **no resting outline** — that's the 1 px
  hover-resize trap, see *Conventions*).
- `theme.rs` — the thin app layer over the above: which sheet is which, the **fonts** (not part of
  `Style`), the user's accent from `AppearanceSettings`, and `[extras]` lookup. `apply(ctx,
  &AppearanceSettings)` sets both themes' styles from the sheets, then `set_theme`s the chosen
  `ThemeMode` (Dark/Light/System) — so `System` follows the host either way. The **accent** is a
  *setting*, so it overrides the sheet: on dark it becomes `selection.bg_fill`, paired with an ink
  colour *measured* against it (`contrasting_text`), since a user-chosen colour can't have its
  pairing written down. **`Extras`** carries the colours egui models no field for — `ok` (the accept
  ✓) and the two selection-glow colours — cached per theme in `ctx.data_mut()` and read via
  `theme::ok_color(ui)` / `theme::glow_color(ctx, background)`; everything egui *does* model goes
  through the style instead (an error message reads `ui.visuals().error_fg_color`). The glow colour
  is picked **once** here and travels to the shaders in the camera uniform
  (`SceneRenderer::set_glow_color`), so the 3-D glow and the egui-drawn cues (hover ring, lasso,
  rubber band) cannot drift apart. **Bold text needs its own font** (`BOLD_FAMILY` /
  `theme::bold(size)` → `FontFamily::Name("bold")`, used via `widgets::bold_name`): egui bundles
  **no bold face** — only `Ubuntu-Light` — and `RichText::strong()` merely swaps in a brighter
  colour. `install_fonts` embeds **Ubuntu Regular** (at the head of the proportional family; egui's
  own default is Ubuntu-*Light*, too thin at this type scale) and **Ubuntu Bold**, i.e. the base
  font's *own* bold sibling — a bold face from another family (DejaVu, or the desktop's UI font)
  reads as a second **typeface** rather than as emphasis, which is the trap here. Both are subset to
  Latin-1 + ~30 typographic/scientific characters: **~18 kB** each against the stock 324 kB
  (`assets/subset-fonts.sh`, needs `pip install fonttools` — a build-time step; only the `.ttf`
  ships). UFL 1.0, the same licence as the Ubuntu-Light egui already bundles
  (`assets/Ubuntu-UFL.txt`). The families list the proportional fonts after them, so an emoji or a
  glyph the subset lacks falls back to the regular face instead of a missing-glyph box. Embedded
  rather than looked up, so it needs no availability check and behaves identically on native and
  wasm — an unbound `FontFamily::Name` would make egui panic. (Rejected: **faux bold** by
  double-drawing the glyphs — a hack; and the **system UI font** — on Linux the desktop's choice is
  not in fontconfig's generic alias, so honouring it meant either per-DE config parsing or an
  XDG-portal D-Bus call, far too much machinery for one font weight.) The viewport background follows
  the theme (`Background::for_theme`, applied by `App::follow_theme_background`, which replaces only
  the *preset* backgrounds so a custom one survives a theme switch).
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
- `color.rs` — CPK element colors → packed RGBA8 (`u32`); `ColorMethod`, `ColorSpec`, `Colorizer`.
  **`ColorMethod::Charge`** (M31) paints per-atom charge on a **diverging** red–white–blue ramp
  (`charge_ramp`: negative red, positive blue, white at zero — the chemistry convention, and the
  opposite direction from `beta_ramp`, which spans an arbitrary range rather than diverging about a
  meaningful zero), normalized by the largest **magnitude in the selection**, so the sign and the
  relative extremes read at any absolute scale (partial charges run to ~±0.8 e, formal ones ±1..2).
  *Which* charge it paints is an **option of the one scheme, not a second scheme**: `ChargeKind`
  {`Partial` (molar's always-present `charge` column), `Formal` (the optional `formal_charge` one —
  absent on most structures, which then paint uniformly white rather than failing)} lives on the
  representation (`Representation::charge_kind`) and is edited in the rep settings' **[Color]** tab.
  **`ColorSpec { method, charge_kind }`** bundles a scheme with its options so `Colorizer::new` and
  `geometry::build` take one value instead of growing a parameter per option; `Representation::color_spec()`
  produces it, and `From<ColorMethod>` covers the internal builders (glow, hover lens) that hard-code
  a method and never paint charges.
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
  come from the selected atoms; each bond is **one two-tone capsule** (`cylinders`: `p0→p1`,
  `color`=atom-a / `color1`=atom-b, split at the midpoint in the shader) — the cylinder impostor
  ray-casts a capped capsule (see the *Impostors* note), so Licorice draws atom balls only for
  **bondless** atoms (`spheres_where`/`bonded_mask`). Computes a
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
- `moldata.rs` — **`MolData`**, a molecule's topology+coordinates backend (M26): `Owned(System)`
  for the standalone app / wasm / drawing editor, or `Shared(Box<dyn SharedSource>)` for a molecule
  rendered **by reference** from an external owner (a pymolar `System`, via `molar_vis_py`). Kept as
  a directly-borrowable `Molecule.data` field (not behind `Molecule` methods) so rebuild loops can
  split-borrow it alongside `&mut reps`. Methods: `topology`/`state` (borrow), `bind`/`bind_with_state`
  (→ `SelBoundParts`, via molar's `Sel::bind_to` for the shared case), `select_all`, `evaluate`,
  `is_shared`, `system`/`system_mut` (owned-only escape hatches — save + the drawing editor), and the
  owned-only mutators (`set_state`/`append_atom`/`remove`/`select_all_bound_mut`, `unimplemented!`
  on the shared arm). `SharedSource` (pyo3-free trait: `topology`/`state`/`evaluate`) is implemented
  only in `molar_vis_py`. `bind` returns `SelBoundParts` for both backends; the only `SelBound`-needing
  path (file save → `SaveTopologyState`) is owned-only and routes through `system()`.
- `scene.rs` — `Scene { molecules, selected_mol, trash }`, `Molecule` (a `data: MolData` backend —
  owned `System` or shared external source, [[moldata.rs]] — + guessed `bonds` + bbox + `reps`;
  `data` is the single source of per-atom data, read by reference). `Molecule::new` (owned, from a
  `RawMolecule`) and `Molecule::new_shared` (from a `SharedSource` — guesses bonds/bbox from its
  topology/state) share a private `from_parts`; `Scene::add`/`Scene::add_shared` assign the `MolId`.
  `Representation` (kind / params / `sel_text` (editable buffer) / `expr: SelectionExpr`
  (compiled) / `sel: Sel` (evaluated) / `periodic: PeriodicParams` (image counts + Self/Box,
  in `EditState`) / visible / dirty flags / `RepGpu`), `evaluate()`
  (text → `SelectionExpr` → `Sel`). `Molecule` also owns a `trajectory: Trajectory` and the
  `seed_frame0`/`append_frames`/`push_frame`/`apply_current_frame` methods (see *molar integration*),
  plus a `source: MoleculeSource` (`File(path)`/`Bytes{name}`) and `traj_loads: Vec<TrajLoad>`
  (the trajectory files loaded into it, in order) — both for session save/load (see `session.rs`).
  **One molecule type** — every owned molecule is editable/undoable (there's no `editable` flag;
  a *shared* pymolar/JS molecule just can't be mutated in place). `Molecule::structure_snapshot`
  returns an `Arc<StructureSnapshot>` (atoms+coords+bonds) rebuilt **only when `structure_version`
  bumps** (bumped by every structural mutator — `add_atom`/`add_bond`/`remove_*`/`cycle_bond_order`/
  `set_atom_element`/`rotate_fragment`/`set_coords`), so it's O(1) at idle; `rotate_fragment`
  (dihedral) + `set_coords` (undo) edit the **displayed** coords in place (trajectory frame or owned
  System).
- `history.rs` — **undo/redo on one unified timeline** (M30). A single strict-LIFO stack of `Step`s,
  each either `Doc(EditState)` — the reps/visibility/groups **document**, snapshot-diffed at settle
  via `maybe_record` against the rolling `committed` baseline (as before) — or `Struct(MolId,
  StructEdit)` — a **self-contained structural delta**, recorded eagerly at gesture end via
  `record_struct`. **`StructEdit::Coords { atoms, before, after, frame }`** stores only the *moved*
  atoms' positions (dihedral twist, minimizer/cleanup) plus the coordinate store the delta targets —
  `frame: Some(i)` = trajectory frame *i*, `None` = the owned System — captured at edit time (via
  `Molecule::coord_edit_target`) so undo hits the **same** store even after the displayed frame or
  trajectory changes; a dihedral twist is undoable whether or not a trajectory is loaded. **`StructEdit::Topology {
  before, after: Arc<StructureSnapshot> }`** carries full before/after snapshots for atom/bond
  add/remove (molar re-indexes on removal, so per-atom topology deltas aren't feasible; rare + small,
  Arc-shared). **`StructEdit::Charges { atoms, before, after }`** (M31) is the espaloma assignment —
  shaped like `Coords` but with **no frame target**, since charges live in the topology rather than
  per trajectory frame (applied by `Molecule::set_charges`, which marks every rep `geom_dirty` because
  colors are baked into the geometry). A `Step::Struct` holds a **`Vec<(MolId, StructEdit)>`**, so one
  gesture that edits several molecules is one undo step (`History::record_structs`; undo replays it in
  reverse) — a group-wide charge assignment needs that, and `record_struct` is the one-element case. Structure is **not** in the document — `EditState`/`MolState` hold only reps+visibility;
  a molecule's *existence* (add/delete) rides the document but its live structure is preserved across
  add/delete undo by reusing the `Molecule` (with its `System`) from the scene/`trash`. `undo(&mut
  Scene)`/`redo` apply one step in place (Doc → swap+`apply` committed; Struct → `StructEdit::apply`
  writing coords / rebuilding via `reconcile_structure`) and return the label; there is **no
  jump-to-committed shortcut** (each delta must be replayed in order). `RepState`/`StructureSnapshot`
  live here (see the *save/load* design in `session.rs`). Recording sites: `app/draw_input.rs`
  `record_topology` wraps Draw/Erase + `toggle_hydrogens`; `drive_minimize` records the relax's
  `Coords`; `app/dihedral.rs` records the twist's `Coords` at drag end.
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
- `script.rs` (+ `script/command.rs`) — **the command layer, always compiled**: `Command` (the
  vocabulary of scene mutations) + `apply_scene_command` + the `parse_*` helpers. The app's own menu
  actions and the Python/JS hosts drive the viewer through it, so it does **not** ride the `scripting`
  feature; only the Rhai front end on top of it does (`script/engine.rs` + `script/console.rs`,
  below). `RepRef`/`Command` carry `#[allow(dead_code)]` because *which* variants have an in-crate
  constructor depends on which front ends are compiled in.
- `script/engine.rs` + `script/console.rs` — **in-app Rhai scripting console** (M24), behind the
  non-default **`scripting`** feature (M31). A
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
- `data.rs` + `data/loader.rs` (`RawMolecule`: System + resolved bonds + bbox; positions/
  radii are transient, used only for bond guessing) + `data/bonds.rs` + `data/traj_loader.rs`
  (**native-only**, `#[cfg(not(wasm))]`: `read_frames_sync`/`spawn_async`).
- `data/bonds.rs` — **where a molecule's connectivity comes from**. `bonds::guess` is the
  distance part (molar's grid search + a VDW-fraction filter; molar has no coordinate-based bond
  perception of its own, so this stays here). **`bonds::resolve` owns the policy** (M31), keyed off
  molar's own `BondStorage::has_orders()` signal:
  - **orders present** (an SDF/MOL bond block) → the file's table is taken **verbatim**. Distance
    guessing could only lose the orders, and those orders are what aromatic-ring perception and
    espaloma charges need — a Kekulé structure is not recoverable from distances.
  - **bonds but no orders** (a PDB's `CONECT`) → **unioned** with the distance guess. CONECT records
    the *exceptions* and may or may not be complete: 2 records in `2lao.pdb`, a complete 32716 in the
    Martini `cg.pdb` — so neither source is trusted alone. Union also means no structure can lose a
    bond it used to have (2lao still resolves to the same 1855), and it is what finally gives
    **coarse-grained** structures real bonds: CG bead spacing (~0.32 nm) exceeds `search_cutoff`, so
    `2lao_cg.pdb` went 290 guessed → 858 resolved and CG licorice draws a connected network.
  - **no bonds** (GRO/XYZ) → the guess is all there is.
  `bond_storage`/`bond_vec` bridge the viewer's flat `Vec<Bond>` and molar's columnar `BondStorage`
  (see `Molecule::sync_bonds_to_topology`).
- `render.rs` — `SceneRenderer`: offscreen color + `Depth32Float` targets (Strategy A) **plus
  Weighted-Blended OIT `accum` (RGBA16F) + `reveal` (R16F) targets** (in `Targets`, with an
  `oit_bind_group` for the resolve), **dynamic-offset** camera UBO (bind group 0; an array of
  `CameraUniform` at `CAMERA_STRIDE`=256 — entry 0 is the base camera, one extra per **periodic
  image** = base view × `Mat4::from_translation(i·a+j·b+k·c)`, grown/`make_camera_bind_group`'d as
  needed), sphere/cylinder/line/**mesh** pipelines (each `[opaque, oit, glow]` — index `GLOW=2`
  is the selection glow — blended **over** the composited color, depth-test `≤`, no depth-write) + a fullscreen **`composite_pipeline`**
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
  interleaves correctly with the impostors. **Render to file** (the "Save image" feature): the pass
  sequence is split out of `render_scene` (which owns the target (re)allocation + egui-texture
  update) into a private `render_core` that records into `self.targets`; `capture_begin` swaps a
  **temporary** target in for `self.targets`, runs `render_core` at an arbitrary `out×ssaa`
  resolution (window-independent), `copy_texture_to_buffer`s the color into a mappable buffer
  (256-byte row align), swaps the live target back (wgpu keeps the temp alive until the submitted
  copy completes), and returns a `CaptureReadback`. `CaptureReadback::read` de-pads rows, swizzles
  BGRA→RGBA, and downsamples `out×ssaa → out` (`image::imageops`) → an `RgbaImage`. Native drives the
  map with `device.poll(wait)` then reads; wasm polls `is_ready` each frame (the browser drives the
  map). The color target carries `COPY_SRC`. UI/IO lives in `app/export.rs` (see the Render menu).
- `render/raytrace.rs` + `render/shaders/raytrace.wgsl` — **GPU ray tracer** (Tachyon / PyMOL-`ray`
  quality: ray-traced ambient occlusion + shadows + Blinn-Phong, all rep types). **WebGPU/native
  only** (needs compute + storage buffers; gated on `DownlevelFlags::COMPUTE_SHADERS` → `Option<Raytracer>`
  on `SceneRenderer`, `None` on WebGL2). **CPU side**: `RtScene::gather(scene, RtView, dashed_pbc)` re-runs `geometry::build` per
  visible rep (same displayed frame/smoothing as `rebuild_dirty`) into flat GPU-friendly primitive
  arrays — analytic **spheres** + **cylinders** and shared-vertex **triangles** (cartoon/surface) —
  then a hand-rolled **binned-SAH BVH** over all of them (32-byte SoA nodes, `count==0`⇒interior with
  contiguous children, leaves carry **type-tagged** `(type<<30)|index` prim refs). **GPU side**: a
  **compute** pass (`cs_trace`) reads the prims + BVH from storage buffers and, per pixel, accumulates
  `samples` paths (camera ray via `inv(proj·view)` unproject — persp+ortho; explicit-stack BVH
  traversal w/ robust slab test; analytic ray-sphere/ray-**capsule** lifted from the impostor shaders +
  Möller–Trumbore; per hit: Blinn-Phong shading × a cast shadow × AO; sub-pixel jitter = AA; PCG RNG)
  into a linear `Rgba32Float` target, then a fullscreen **`fs_resolve`** tonemaps (clamp) into the
  sRGB scene color target (GPU auto-encodes — shade linear, no manual gamma). Reuses `Camera::ao`/
  `shadow`/`background`/**`depth_cue`** so the trace matches the controls — the fog is the shared
  rasterizer model verbatim (the same `near/far/strength/mode` from `cue_uniform`, the same three
  falloff curves, applied last to the primary hit's finished colour), since a fogged view traced
  unfogged comes back flat; its axial eye-space distance needs no extra uniform because the view
  axis comes from unprojecting the frustum centre (`view_axis`), correct for both projections.
  Materials via the shared `unpack_mat`.
  **Matching the rasterizer's geometry is a standing requirement, and four things had drifted** (all
  fixed; the trace is now checked side-by-side against a `_SAVE_IMAGE` of the same view): a bond is a
  **two-tone capsule**, so the tracer's cylinder needs *both* the hemispherical end caps
  (Licorice/Ball-and-Stick emit an atom sphere only for **bondless** atoms, so a capless cylinder
  left every bond an open tube with no atom ends) and the **midpoint colour split** (`m.z` = the p1
  half; tracing only `m.x` painted every bond its first atom's colour, so a C–F bond came out all
  grey); a **multi-order** bond's parallel strands are offset by the rasterizer's *vertex shader*
  from the camera, so `strand_offset` bakes the same shift in at gather time (hence `RtView`, and why
  `rt_scene_dirty` is set on a camera change too); and **lines** — the Lines rep, interaction dashes,
  the periodic box — are screen-space quads a ray knows nothing about, so `line_capsule` converts
  each segment's *pixel* width to a world radius at the traced camera (`RtView::world_per_px`, exact
  for both projections) with a **flat-ends flag** (`FLAG_FLAT_ENDS` in `m.w`) that suppresses the
  caps, since rounded ends lengthen each dash enough to close a dashed line's gaps. Lines get an
  ambient-only material so they trace as the unlit constant colour the rasterizer draws. Interaction
  dashes come from *two* molecules, so they don't come out of `geometry::build` at all — the gather
  calls the same `app::build::build_interactions` (now `pub(crate)`) the second `rebuild_dirty` pass
  uses.
  **The shading deliberately mirrors the rasterizer's `shade_material` so the trace matches the
  realtime view** (the user's reference, esp. with AO/shadows OFF — a fixed inflated ambient made the
  trace ~55 % too bright): Blinn-Phong `base·(mat.x + mat.y·N·L) + spec` lit by the **same view-space
  headlight** (`head_dir = inv_view·(0.3,0.4,1)`) using each **material's own** ambient/diffuse
  coefficients (`unpack_mat`), + VMD outline (top shininess bit, like the raster `apply_outline`).
  Shadows and AO are **deferred whole-color multiplies** (`color × shadow_vis × ao_vis`), exactly as
  the raster's SSAO/shadow pass does — so AO-off + shadow-off == the raster shading. The shadow is a
  **cone-jittered** ray toward a *separate* **world-space key light** (`inv_view·SHADOW_LIGHT_DIR_VIEW`
  — decoupled from the shading headlight, again like the raster, whose shadow map uses the key light;
  back faces shadow without a ray; per-sample cone = `shadow.softness × MAX_SHADOW_CONE` (0.45 rad ≈
  26°) → softness 0 razor-hard, 1 broad/diffuse).
  **AO is tuned to read as strongly as the realtime SSAO pass** (the user's reference) — three things
  matter, all calibrated by rendering VDW/cartoon both ways and matching molecule-region brightness:
  (1) **scene-relative occlusion distance** (`rt_uniform`: `scene_radius × ao.radius`, clamped
  0.3–6 nm), *not* the raw atom-scale nm radius — molecular cavities/folds (cartoon ribbons, surface
  dimples) are far larger than the ~0.4 nm atom-contact scale, so a fixed small radius lets every
  hemisphere ray escape and AO becomes invisible (was a bug — AO did nothing on cartoon/mesh).
  (2) **whole-color multiply** (above): AO multiplies the *entire* shaded color, like the SSAO pass —
  occluding only the ambient term left the key light un-occluded and read far too light.
  (3) **contrast-boosted occlusion fraction**:
  cos-weighted hemisphere AO is physically "correct" but light (most surface points see only ~10–20 %
  occlusion), so the per-sample fraction over `AO_RAYS` rays is raised to `pow(frac, AO_CONTRAST=0.55)`
  before scaling by strength — turning that modest occlusion into the strong edge/contact darkening
  SSAO shows. (`AO_RAYS≥3` is needed for the per-sample fraction to be non-binary so the curve
  applies.) Result: ray-traced AoEdgy VDW now matches the SSAO render's brightness, and cartoon AO is
  clearly visible.
  Drives the raytraced "Save image" **and the R-key viewport still**, both **frame-pumped** so the UI
  stays responsive. The accumulator is **ping-pong `Rgba32Float`** holding a **running average**, and
  the trace is a **resumable tiled stepper** (`trace_begin` + `trace_step`): it sweeps the image in
  `TRACE_TILE`×`TRACE_TILE` (256²) blocks, one block × one sample-chunk per GPU submit, doing
  `RT_STEP_SUBMITS` (4) submits per UI frame and resolving the latest *complete* chunk into the target —
  so the image refines progressively over frames while the window stays interactive (a "Ray tracing…" /
  "Saving…" overlay shows meanwhile). Tiling is **mandatory on big scenes**: a single whole-image
  dispatch of all samples hangs the GPU watchdog and **loses the device** (the reported crash). The
  per-submit chunk is bounded by *BVH-ray traversals*, **counting the rays cast per sample** — AO
  (`AO_RAYS`) + a shadow ray + GI bounces are incoherent traversals that dominate cost (an AO+shadow
  submit does ~6× a primary-only one), so the chunk shrinks accordingly. A tile origin rides `accum.zw`
  (shader pixel = origin + local id); the read accumulator always holds a complete chunk, so resolving
  mid-sweep is seam-free. `render_tiled` is now just a blocking begin+step-to-completion wrapper (the
  headless debug hook). The sample target is **lighting-dependent** (`Camera::rt_sample_target`): the
  image converges fast when there's little stochastic noise (measured — sub-pixel AA only settles by
  ~16–24 samples, AO + soft shadows ~48–64, GI is path-traced → many more), so it traces only 12 /
  24 / 48 respectively (a few refinement passes) instead of a fixed large count — past convergence, more samples just burn time
  without changing the image.
  **Viewport ray tracing is the explicit R key (PyMOL-`ray` style — no automatic trace-on-idle):**
  pressing **R** (not while a text field has focus, not in draw mode) frame-pumps `rt_still_*` into a
  **dedicated 1× texture** (`rt_color`/`rt_egui`, painted via `rt_texture_id` once the first chunk lands)
  and **holds the still until the camera/scene/size changes**, then drops to the realtime raster. **R
  honors the lighting incl. GI** (GI strength from `Camera::gi`). **Deferred start** (`rt_warm`/`RtKind`
  on `App`): a press/menu sets `rt_warm`, the controller paints the **"Ray tracing…/Saving…" overlay one
  frame**, then runs the (possibly blocking) scene gather + trace begin — so the overlay appears
  *immediately* instead of after the gather. **Works with an active selection**: a pending/hover
  selection glow is **suppressed while a still is warming/running/held** (`glow_pulse = 0`, its pulse no
  longer forces a redraw) — the glow isn't part of the trace (the gather ignores it) and the still shows
  no glow; it returns when the still is dropped. No continuous repaint when idle → **idle = 0 GPU** (the
  old auto-idle trace + its per-frame `request_repaint` + the 30k-atom size gate are all gone). The
  **Save image** path is an `RtJob::Save` (also deferred via `rt_warm`) driven by `App::service_rt_save`
  (native): pump `save_step` into an offscreen COPY_SRC target, then `save_finish` → async readback
  (`PollType::Poll` each frame) → write the PNG — no UI freeze; the live viewport (and its glow) stays
  interactive meanwhile. (WebGL2 wasm has no compute → ray tracer absent → Save falls back to the
  rasterized capture.) 4 BVH unit tests.
  **Global illumination (tier 2, `Camera::gi` = a 0..1 *strength*):** when `gi > 0` the trace
  path-traces (`shade_gi` in `raytrace.wgsl`) instead of tier-1 direct shading — per hit: direct key
  light (soft-shadowed) + `GI_BOUNCES` (3) cosine-weighted diffuse bounces, Russian-roulette terminated,
  gathering a uniform **sky dome** (`GI_SKY`, decoupled from the visible background so a dark backdrop
  still lights the molecule) on a ray miss — so cavities self-shadow (true AO) and colour bleeds between
  surfaces. GI **blends with tier-1 by the strength** — `mix(tier1, full_gi, gi)` per sample — so a tiny strength
  barely changes the look and it ramps **continuously** up to full GI (switching shading models at
  strength→0 made even 0.01 jolt the whole scene). The **strength rides `U.bg.w`** (0 = tier-1); the
  resolve likewise blends its tonemap `mix(clamp, ACES, gi)` (clamp = tier-1 raster match, ACES = GI's
  HDR shoulder), so the tonemap has no jump at 0 either.
  The surface decode + tier-1/GI shading are factored into shared shader fns
  (`surface_at`/`shadow_at`/`shade_tier1`/`shade_gi`). GI applies to **both** the Save-image render
  **and the R-key still** (both read `Camera::gi`); the Lighting-tab **Global illumination slider** +
  `MOLAR_VIS_DEBUG_GI=<strength>` drive it. Default **0 (off)** — GI is the heaviest trace (more
  iterations), so it's opt-in via the slider.
  **Transparency (stochastic):** the primary ray walks through surfaces, accepting each with
  probability = its **opacity** (the colour's alpha byte, `unpack_opacity`), else passing through to
  what's behind; averaged over the accumulated samples this is correct, order-independent alpha — so
  transparent materials (Glass/Ghost/Transparent…) show through instead of reading as solid (they were
  opaque in the trace before). Opaque surfaces (opacity 1) are always accepted at the first hit, so
  they're unchanged. Shadow/AO/GI-bounce rays still treat transparent geometry as opaque (a minor v1
  approximation — transparent things cast a full shadow).
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
  O(N). Also `neighbors_within(center, r, |id|)` — a **point** neighbor query (cell skirt clamped to
  grid bounds so a degenerate single-cell grid stays cheap), used by [[interactions.rs]] for
  contact detection. Pure logic, WASM-safe; 5 unit tests.
- `docking.rs` + `app/docking_dialog.rs` — **loading a docking result** (M32): a receptor plus
  the ligand poses docked into it, via **Molecule ▸ Load docking data…** (native only — it reads
  several files from disk). `docking.rs` is the pure half: **`docking_mode`** (how the receptor's
  frames line up with the poses — 1 frame = `Rigid`, one per pose = `Flexible`, anything else is
  an error rather than a guess) and **`sync_action`** (which side of a flexible pair to drive).
  `app/docking_dialog.rs` is the dialog + the load:
  - **Ligands**: one multi-record SDF *or* one file per pose (multi-select) → the poses become
    the members of one [`MolGroup`].
  - **Protein**: the **first file supplies the topology**, and the receptor's conformations are
    its own coordinates plus the frames of every file after it — *except* when those files
    already hold one conformation per pose, where the structure is the reference conformation
    rather than a pose. Both readings are real and they differ by exactly that one frame:
    **31 files for 31 poses** means the first file is pose 0's receptor (dropping it reported
    "30 frames but 31 ligands" and refused a perfectly good docking result — the bug this rule
    replaced), while **a structure + a 26-frame ensemble trajectory for 26 poses** means 26, not
    27, or every pose/receptor pairing is off by one. Nothing in the file list separates them (a
    "trajectory" may be a multi-model PDB; 30 one-model files are a fine ensemble) — but the
    **pose count** does, since only one reading can match it, so **`structure_frame_counts`**
    takes the one that does and otherwise counts the frame (the natural reading, what a lone
    file needs, and what makes the rejection message quote the honest total). With nothing after
    it the single file *is* the ensemble (a 1-model PDB → rigid, a 26-model one → flexible).
    Everything is read + validated **before** the scene is touched, so a rejected combination
    leaves the document untouched.
  - The pose group gets an **`Interactions` shared rep pointed at the receptor's rep** — exactly
    the state the partner picker would leave behind, so it round-trips undo/sessions for free via
    `RepState.partner`. The group's existing Licorice shared rep is kept (an Interactions rep
    draws only contact lines, so alone it would hide the poses).
  - **The view is set up for looking at a pose**, not for a fresh structure: every rep is scoped
    to `not apolh` (`HEAVY_ATOMS` — docked structures are fully protonated and the C–H hydrogens
    are pure haze; the *polar* ones are kept, being what H-bonds are made of and what the
    detector's explicit-H geometry test needs), the receptor gets a **cartoon coloured by
    secondary structure** on top of its lines (at `DOCKING_LINE_WIDTH` = 3 px, since the receptor
    is a backdrop read *through*), and the camera frames the **shown pose plus a 0.6 nm contact
    shell** (`POSE_VIEW_MARGIN`, sized to the interaction cutoffs, so every residue a contact
    line can reach is on screen — a bare ligand bbox fits so tightly the site vanishes).
  - **Flexible pairs step together** (`sync_docking_frames`, run per frame after the panels *and*
    the viewport): moving to another pose shows the conformation it was docked into, and scrubbing
    — or *playing* — the receptor trajectory shows the matching pose. Rather than hooking every
    control that can move either side, it compares both against the pair recorded last frame
    (`MolGroup::docking_sync`, transient) and propagates whichever moved, so playback works for
    free and no control can be missed. Applies to **any** group whose Interactions partner has one
    frame per member, however it was set up — not just what the dialog built.
- `analysis.rs` + `app/align_dialog.rs` — **superposition + RMSD** (M33), the first entry under the
  new **Analysis** menu. `analysis.rs` is the pure half (WASM-safe, touches only [`Scene`]): one
  `Request` (two `Side`s = molecule + selection + frame, plus `all_frames` / `common_subset` /
  `move_whole`) drives `rmsd` (measure) and `align` (fit, then measure). The maths is molar's
  (`fit_transform` / `rmsd`, `get_matching_atoms_by_name`); what lives here is everything around it:
  - **Pairing** (`pair_atoms`) — atom for atom by default, which needs equal counts and *says so*
    with the advice that fixes it; with **Common subset** molar aligns the two **atom-name
    sequences** and only matched atoms are compared. That recovers from a few missing atoms (an
    unresolved side chain, a different protonation) — it is **not** a structural matcher: names
    repeat down a chain, so a *systematic* difference (every `CA` gone from one side) lets it pair
    one residue with the next and the RMSD is then meaningless. Verified: it silently returns
    0.101 nm for two *identical* structures paired that way, hence the module-doc warning and the
    option being off in a request built by hand.
  - **A resolved `Plan`** (selections compiled, frames chosen, atoms paired) so both entry points
    are cheap per frame and every failure mode sits in one place — including a pairing under 3
    atoms, where a least-squares fit isn't determined.
  - **Two-phase apply**: every frame's fitted coordinates are computed **read-only first**, then
    written. The target may live in the *same* molecule (fitting a trajectory onto one of its own
    frames), so nothing may be written while positions are still being read — and a failure leaves
    the scene untouched. Phase 1 returns finished coordinates rather than a transform, which also
    keeps molar's nalgebra isometry type out of every signature (nalgebra is not a direct
    dependency; the whole molar boundary goes through molar's own aliases).
  - **Undo**: `align` returns one `StructEdit::Coords` per moved frame for the caller to record as
    **one** step (`History::record_structs`) — a 27-frame trajectory fit is one Ctrl+Z.
  `app/align_dialog.rs` is the window (see the *Menu bar* section for the semantics of each
  control). It is deliberately **not** a modal: each side can be filled by *clicking a
  representation* in the tree or the 3-D view, which a modal's backdrop would swallow, and the
  window grows when the readout appears, which a centred `Modal` answers by jumping its top edge
  (the same reason the settings window is a `Window`). A stale readout is dropped the moment any
  input changes, like a rep's selection feedback. Hooks: `MOLAR_VIS_DEBUG_ALIGN=<spec>` (runs it
  through the real button path) + `_ALIGN_DIALOG=[1|pick]`.
- `charges.rs` — **espaloma partial-charge assignment** on a selection (M31), via `molar_ff`'s
  `ApplyCharges`. **Native only** (`tract` + a ~600 kB bundled ONNX has no business in the wasm
  bundle, and the browser has no way to obtain charges anyway; charge *coloring* works everywhere).
  `compute_espaloma(mol, sel)` predicts, writes the charges into the molecule, and returns a
  `ChargeEdit { atoms, before, after }` so the caller can record it as one undo step. The model needs
  **chemistry, not coordinates**: explicit Kekulé orders and a *bond-complete* scope.
  - **The selection picks which molecules, not which atoms.** Charges are equilibrated over a whole
    connected graph, so `molar_ff` rejects a selection that cuts a bond — and ordinary *viewing*
    selections cut bonds constantly (`not apolh`, which the docking loader sets, severs every C–H).
    So the selection is first widened with **`Molecule::connected_closure`** to the complete
    molecules it touches. Without that, charging a docking result's poses failed outright. The
    hidden atoms still get their charges; they simply aren't painted.
  - The remaining likely failure is translated into advice rather than passed through raw: no bond
    orders → "load the molecule from an SDF/MOL, or draw it in the structure editor"
    (distance-guessed PDB/GRO connectivity is order-less, and *aromatized* bonds are rejected too —
    which is why `ensure_interaction_rings` uses the non-mutating `aromatic_rings`).
- `interactions.rs` — **non-covalent interaction detection** (M29; the `Interactions` rep style):
  pure, WASM-safe, PLIP-derived. `detect(a, b, params)` takes two `InteractionSet`s (heavy `AtomInfo`
  atoms + aromatic `RingInfo` + `ChargeGroup` cations/anions) and returns line segments for the six
  types. **Atom-level** (H-bond / hydrophobic / halogen) pairing is **grid-based** (`spatial::AtomGrid`
  over the larger set, query the smaller) — never O(N·M); **group-level** (salt bridge / π-stacking /
  π-cation) is O(n²) over the small ring/charge lists. **H-bond**: N/O/S donor/acceptor; explicit-H →
  D–A `< 4.1 Å` **and** D–H···A `> 100°`, else heavy-atom D–A `≤ 3.5 Å`. **Hydrophobic**: C with only
  C/H neighbours, `< 4.0 Å`, one per residue pair. **Salt bridge**: opposite-charge centroids `< 5.5 Å`.
  **π-stacking**: ring centroids `< 5.5 Å`, parallel (within angle tol + offset) or T-shaped.
  **π-cation**: ring centroid ↔ cation `< 6.0 Å`, offset from ring axis bounded. **Halogen**: C–X
  (Cl/Br/I)···(N/O/S) `< 4.0 Å`, C–X···A angle `> 140°`. All thresholds are user-editable via
  `InteractionSettings` (rides `RepParams`) → `DetectParams`. The builder
  (`app::build::{gather_set,build_interactions}`) gathers the sets from **two molecules'** displayed
  frames + bonds + cached aromatic rings + charged-group heuristics and emits dashed `LineVertex`s
  (`geometry::interaction_lines`, per-type colors); 9 unit tests incl. a grid-vs-brute-force check.
- **Hover detail lens** (QoL, `app.rs` + `scene.rs` `HoverDetail`): **off by default**, gated behind the
  `BehaviorSettings::hover_detail_lens` toggle (Settings ▸ Behavior → *Hover detail lens over
  cartoon/surface*); when off the trigger block in `draw_viewport` is skipped and any stale lens is
  cleared. When on, in Hover mode the **front-facing
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

