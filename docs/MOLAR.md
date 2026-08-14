# molar_vis — molar integration notes

> Reference doc for [molar_vis](../CLAUDE.md). Split out of the master `CLAUDE.md` for on-demand reading — see it for the project overview, build quick-start, and the full docs index.

## molar integration notes

### molar 2.1 API (the SoA migration, M31)

molar 2.0/2.1 flipped `Topology` to **struct-of-arrays** storage. The consequences that matter here:

- **There is no `&Atom` or `&Bond` to borrow.** `iter_atoms`/`get_atom` hand out **`AtomRef` /
  `AtomRefMut`** column proxies (a `Copy` two-word `{storage, index}` handle) and bonds come back as
  **`BondRef`**. So every property read is an `AtomLike::get_*()` call, not a field access, and a
  helper that used to take `&Atom` takes `impl AtomLike` (by value — it's `Copy`) or the concrete
  `AtomRef` where a closure signature needs naming. `Particle.atom` is an `AtomRef`.
  The owned `Atom` / `Bond` rows remain as the **construction** types (`AtomStorage::push(&Atom)`).
- **Interface vs storage widths**: `AtomLike::get_resid()` returns `isize` while the column (and
  `Atom.resid`, and `Atom::with_resid`) is `i32` — like `usize` at the bond-pair boundary. Mirror the
  *storage* width in our own types and narrow at the getter.
- **`Topology.bonds` is a `BondStorage`** (columnar: an always-present pair column + an *optional*
  order column, absent for connectivity-only sources — hence `has_orders()`, which
  `bonds::resolve` keys its policy off). It caches a **`BondAdjacency`**; every molar graph
  routine (`sssr_rings`, `aromatic_rings`, `implicit_hydrogens`, `perception::perceive`, all of
  `molar_ff`) takes a **prebuilt** one — build it from *whichever bond table the callee will read*
  (`BondAdjacency::build(n, top.bonds.iter_pairs())`), or the bond indices it hands out won't index
  that table.
- **Optional atom columns** (`type_name` / `type_id` / `formal_charge` / `flags`) → getters return
  `Option`; a `None` column costs nothing. `charge` (partial) and `formal_charge` (integer) are
  **separate** properties — that split is what the `Charge` color scheme's two options read.
- **`System::set_bonds`** (a molar addition for this migration) installs connectivity that did not
  come from the structure file. Bonds take no part in the topology/state size invariant, so they are
  the one part of a topology swappable on a live system; this is how our resolved bond graph reaches
  the `polh`/`apolh` selection keywords, `perceive`, and `molar_ff`. See
  `Molecule::sync_bonds_to_topology`.
- **Perception**: `perceive(&mut Topology)` annotates in place and is **destructive of Kekulé
  structure** (an aromatic ring's bonds all become `Aromatic`). Use the non-mutating
  **`aromatic_rings(mol, adj)`** / `sssr_rings(adj)` when the orders must survive — notably before
  charge assignment, which rejects `Aromatic` bonds outright. `implicit_hydrogens(mol, adj)` was
  restored in molar for the editor's hydrogen toggle.

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
  `resindex`, `resname`, `index`, `chain`, `within …`, and **`polh`** / **`apolh`** (polar /
  apolar hydrogens — H bonded to an electronegative N/O/F/S atom vs to a non-electronegative
  heavy atom like carbon, read from the topology **bond graph** — which the viewer publishes via
  `Molecule::sync_bonds_to_topology`, so these work on any loaded molecule; they match nothing only
  when the topology genuinely has no bonds. molar addition — see the molar-integration note).
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
- Connectivity is resolved **once at load** by `bonds::resolve` (see the `data/bonds.rs` bullet:
  an SDF/MOL table verbatim, a PDB `CONECT` unioned with the guess, GRO/XYZ guessed) and never
  recomputed on a frame change. The viewer keeps it as a flat `Vec<Bond>` on `Molecule` — indexable
  `Copy` rows for the geometry/pick hot paths, and the only bond store a *shared* pymolar/JS molecule
  has, since we don't own that topology — and **republishes it into the owned `System`'s topology**
  (`Molecule::sync_bonds_to_topology`, at construction and after every bond edit) so molar's
  bond-reading machinery sees the same graph the viewer draws: the `polh`/`apolh` keywords,
  `perceive`, and `molar_ff`'s typing/charges.
- The distance-guessing half is `distance_search_single` + `dist < 0.6*(vdw_i+vdw_j)`
  (`BondParams` = factor/cutoff/min_dist/**periodic**). **Periodic bond search is opt-in** (`BondParams.periodic`, the
  *Periodic search* setting — off by default): only then does `bonds::guess` use
  `distance_search_single_pbc` + minimum-image scoring to find covalent bonds across a box face in a
  wrapped structure. The PBC search is **much slower** (scans neighbouring cells), so the default
  non-periodic path keeps large-structure loads fast; a non-wrapped protein gets the same bonds
  either way (wrapping bonds are then rendered as dashed PBC half-bonds — see `geometry.rs`).
- Secondary structure for M6 cartoon: `molar::Dssp` (10-variant `SS` enum).

