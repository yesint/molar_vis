# REFACTOR.md — reducing `App` coupling

A working plan for keeping the GUI manageable **inside egui**, after evaluating (and rejecting) a
move to a retained-mode toolkit. Ordered by value/risk; each phase is independently landable and
independently useful. Line references are against `4e2958d`.

**Verdict on switching toolkits: no.** The app is four hosts (native bin, wasm, PyO3 module, JS API)
bootstrapping through eframe, and it owns its own wgpu device — `launch::early_z_wgpu_options`
requests `SHADER_EARLY_DEPTH_TEST`, and `render.rs` allocates its own color/depth/OIT/shadow/id
targets because egui's pass has no depth attachment. Slint pins its own wgpu version and is
GPL/commercial; Iced's MVU is incompatible with `AppJob = Box<dyn FnOnce(&mut App)>` driven from
Python and JS; Xilem is pre-1.0. The cost is months of re-solving embedding, and it would carry the
same 46-field god object into the new framework.

---

## What the audits corrected

Five things in the original sketch were wrong, and they change the plan's shape:

1. **Field grouping alone achieves nothing.** Descendant-module privacy is unchanged by clustering —
   `self.rt.job` is still reachable from all 18 `app/` modules. The goal ("signatures declare what
   they touch") requires **methods to move off `impl App`**, which is both cheaper and more
   effective. 35+ methods touch exactly one cross-cutting field and can move with zero borrow risk.
2. **A `Dialogs` cluster is anti-cohesive.** Its 10 fields are *never* co-used — each is an
   independent `Option` with one opener and one consumer. Bundling forces 6 modules to hold
   `&mut Dialogs` for one field each, and hides the real invariant.
3. **"One dispatch site" already exists** — `app.rs:1013-1021`, 9 lines, 7 of 8 dialogs. There is no
   dispatch problem. And `trait Dialog { fn show(&mut self, ui) -> Outcome }` is *incompatible* with
   3 of the 8: the load dialog needs `&self.scene` in its closure (`loaders.rs:378`), interactions
   needs `&mut self.scene` (`rep_panel.rs:1323`), view-settings holds `&mut self` wholesale
   (`panels.rs:170-174`).
4. **A generalized outcome type merges three unrelated concerns.** Of 31 deferred locals, only ~20
   are borrow conflicts; 5 are index invalidation (deleting a molecule shifts the loop's indices)
   and 6 are frame timing (`export_request` waits for a valid `wgpu_render_state`; `rt_warm` waits
   for the overlay to paint). One type would misdescribe two of the three.
5. **The layout proposal was ~5× oversized.** The real target is 3 edits in `rep_panel.rs`.
   `TableBuilder` has no target in this app (no scrolling retained-column table exists anywhere).
   `sizing_pass` has no target either — and *cannot* replace `widgets::max_label_width`, which
   measures the max over **all** dropdown options so the button never resizes with the selection,
   whereas a sizing pass measures only current content. The `available_height` feedback loop is
   already fixed (`script/console.rs:104-109`); 0 hits crate-wide.

**Honest ceiling.** Native default build is 46 fields (55 declared, minus wasm/native/feature
gating). All of this lands it around **30**, not 8. The remaining 8 are load-bearing: `scene` (260
access sites), `camera` (106), `view_dirty` (44), `settings` (37), `status` (37), `renderer` (31),
`history` (17), `rep_defaults` (12) — 70% of all state traffic, each touched by 7+ modules.

**And the deeper caveat:** none of this changes module *reach*. After clustering, `app/panels.rs`
still sees `self.rt.job`, because `RtState`'s fields must be `pub(super)`. The only mechanism that
genuinely restricts reach is moving a sub-struct's definition **out of `app` into a sibling module**
with private fields plus an accessor API. That is a larger, later question — noted here so the
smaller phases aren't mistaken for it.

---

## Phase 0 — `scripts/check.sh`  ·  ~30 min  ·  risk: none

**Not CI.** A cold CI matrix here is tens of minutes — 591 lock-file packages, 7 `tract-*` crates
(`tract-linalg` is among the slowest-compiling crates anywhere), 10 `wgpu`/`naga` crates, and 6
git-sourced deps that get no crates.io cache reuse. CI is deliberately release-tag-only (`89e550e`),
there is one committer, and the tests get run locally anyway.

What is missing is not automation-in-the-cloud but a **driver**: there are 71 `MOLAR_VIS_DEBUG_*`
hooks and not one script that invokes any of them. Every headless check in every milestone's
"Verified:" line was typed by hand.

So: one committed shell script, warm-cache, run by hand after each phase.

```sh
#!/bin/sh
set -e
# molar_vis_core is the only crate with tests.
# NB -p, not --workspace: molar_vis_py enables pyo3 `extension-module` unconditionally
# and is cdylib-only, so a test harness for it has no libpython to link.
cargo test -p molar_vis_core
cargo test -p molar_vis_core --features scripting
cargo build --target wasm32-unknown-unknown -p molar_vis_core

# Session save->load->save must stay byte-identical.
# _DEFAULTS=1 keeps the run reproducible and off the dev's saved config.
H="MOLAR_VIS_DEBUG_HIDDEN=1 MOLAR_VIS_DEBUG_EXIT=1 MOLAR_VIS_DEBUG_DEFAULTS=1"
env $H MOLAR_VIS_DEBUG_SAVE_SESSION=/tmp/a.json cargo run -qp molar_vis -- tests/2lao.pdb
env $H MOLAR_VIS_DEBUG_LOAD_SESSION=/tmp/a.json \
       MOLAR_VIS_DEBUG_SAVE_SESSION=/tmp/b.json cargo run -qp molar_vis -- tests/2lao.pdb
cmp /tmp/a.json /tmp/b.json

# Renders, for eyeballing.
for r in vdw licorice cartoon surface; do
  env $H MOLAR_VIS_DEBUG_REP=$r MOLAR_VIS_DEBUG_SAVE_IMAGE=/tmp/$r.png \
    cargo run -qp molar_vis -- tests/2lao.pdb
done
```

Record a baseline (the four PNGs + the session JSON) before Phase 1, so the pure-refactor phases
can be checked for pixel-identity rather than merely "still runs".

---

## Phase 1 — Move single-field methods off `impl App`  ·  ~750 LOC moved, ~40 changed  ·  risk: none

The highest-leverage change, and it touches **no field declarations**. Every method below touches
exactly one cross-cutting field, verified by per-function field attribution.

### 1A — 19 scene-only methods → `impl Scene`

```
app.rs:386            mark_shared_dirty
console.rs:79         scene_summary
dihedral.rs:447       dihedral_coords_of          :654  dihedral_preview_hover
docking_dialog.rs:334 style_receptor              :350  style_poses
docking_dialog.rs:398 add_docking_interactions    :426  sync_docking_frames
docking_dialog.rs:453 docking_receptor
draw_input.rs:69      atom_world                  :503  toggle_hydrogens
rep_panel.rs:254      draw_pending_block
session_io.rs:190     save_group_to
viewport.rs:734       set_hover                   :753  clear_hover
viewport.rs:766       set_hover_detail            :845  merge_into_pending
```
Plus 2 touching only `{scene, view_dirty}` (`docking_dialog.rs:475 set_receptor_frame`,
`draw_input.rs:638 flag_edit`) and 3 touching only `{scene, status}` (`session_io.rs:122/135/163`).

Callers become `self.scene.set_hover(…)`. **This removes 5 of the 12 whole-`self` calls inside
`draw_viewport`** (`viewport.rs:324, 386, 428, 570, 598, 603`), which is what makes that 677-line
function tractable without moving it.

### 1B — 5 camera-only helpers → `impl Camera`

```
dihedral.rs:486       dihedral_plane_angle
draw_input.rs:22      drawing_plane_point    :47  cursor_world_ray
draw_input.rs:54      world_to_pixel         :207 drag_dir
```
Keep the 10 `pub` Python/JS view setters (`app.rs:486-572`) on `App` — they are the public host API
and they set `view_dirty`.

### 1C — `rebuild_dirty` → free function

`app.rs:580-886` (306 lines, the hottest function in the app) touches only `renderer` + `scene` +
`settings` + `view_dirty`:

```rust
fn rebuild_dirty(scene: &mut Scene, renderer: &SceneRenderer,
                 settings: &Settings, view_dirty: bool, rs: &RenderState) -> bool
```

All four are verified disjoint at every call site. The Interactions second pass
(`app.rs:840-884`) already exists *because* it needs two molecules at once — that structure is
preserved verbatim.

**Verification:** `sh scripts/check.sh` — and since this phase is a pure move, the four renders must
come back **pixel-identical** to the Phase 0 baseline, not merely render.

---

## Phase 2 — One state struct per dialog  ·  ~90 LOC  ·  risk: low

Mechanical. Folds 5 loose satellite fields into their dialogs, so **11 App fields → 4** and every
dialog obeys the same "open == `Some`" invariant (which today it does not).

| from | to |
|---|---|
| `settings_draft: Option<Settings>` + `settings_tab: SettingsPage` | `Option<SettingsDialog { draft, tab }>` |
| `interactions_dialog: Option<(MolId, usize)>` + `interactions_tab: InteractionKind` | `Option<InteractionsDialog { mol, rep, tab }>` |
| `view_menu_open: bool` + `view_menu_rect: Option<Rect>` + `view_tab: ViewTab` | `Option<ViewMenu { tab, last_rect }>` |
| `rename_mol: Option<(MolId, String)>` | `Option<RenameDialog { mol, name }>` |

Sites: `app.rs:109-111/155/177/191/210-220`; `init.rs:534/557/567/849-850/870-872`; the four draw
fns' headers; `panels.rs:125-127/141-197/503-512`.

**Trap:** `ViewMenu.last_rect` is the close-on-click-outside geometry (`panels.rs:185-196`) and must
survive the frame in which the window redraws. Folding it into the same `Option` that carries "open"
is fine, but do not reset it on redraw — that reintroduces the bug the field exists to fix.

---

## Phase 3 — The `draw_traj_bar` fit test  ·  ~45 min  ·  risk: none

The first of Phase 6's tests, pulled forward because it is the regression test for the bug Phase 4
fixes — write it now, confirm it passes, then confirm it still passes after the `Sides` migration.

`draw_traj_bar` (`rep_panel.rs:573`) and `draw_group_bar` (`:712`) take only `&mut Ui` plus plain
data, so no wgpu device is involved. Assert their contents fit inside the row **in both themes at
several panel widths** — that is exactly what the hardcoded `reserve = 52.0` cannot guarantee.

Model it on `theme.rs:270 hover_does_not_resize_widgets`, which is already a working headless egui
layout test: `egui::Context::default()`, a synthetic `RawInput { screen_rect, events }`,
`ctx.run_ui(...)`, rect assertions, and the two-frame settle its comment at `:298-300` explains
("egui reads a widget's state from the previous frame's response").

---

## Phase 4 — The three `Sides` edits  ·  ~1–1.5 h  ·  risk: low

`egui::containers::Sides` (`sides.rs:45`, verified present in 0.34.3) — the right closure is already
`Layout::right_to_left(Align::Center)` (`sides.rs:199-205`), so widget order is preserved and
migration is near-mechanical. **Phase 3's test must exist first, and must pass before and after.**

1. **`rep_panel.rs:681-683` and `rep_panel.rs:727-730` → `Sides::shrink_left()`.** Byte-duplicated:
   ```rust
   let reserve = 52.0;   // room for the two trailing buttons + spacing
   ui.spacing_mut().slider_width = (ui.available_width() - reserve).max(40.0);
   ```
   `52.0` hardcodes the rendered width of two icon buttons **that have not been added yet**, and that
   width is theme data — both sheets set `button_padding = [8.0, 4.0]` and `item_spacing = [8.0, 7.0]`
   (`themes/dark.toml:45`, `themes/light.toml:91`), further overridden to `2.0` / `(3,1)` by
   `compact_actions` (`widgets.rs:25-26`). Change padding, spacing, or the Phosphor glyph metrics and
   the slider silently over/under-runs its row in two places. `shrink_left()` **measures** the right
   side (`sides.rs:177`) instead. This is a latent bug fix, not cosmetics.
2. **`rep_panel.rs:900` + `934-942` → `Sides::shrink_left()`.** The canonical pre-`Sides`
   workaround: `right_to_left` wrapping a `left_to_right` that reads `available_width()`. Removes a
   nesting level; the selection field becomes `desired_width(f32::INFINITY)` since its `max_rect` is
   now bounded. Check `mark_empty_selection` (`rep_panel.rs:10`) and the focused/unfocused branch at
   `:878` still align.
3. *(Optional, DRY not layout)* the three identical ink-centering blocks — `widgets.rs:128-130`,
   `widgets.rs:153-155`, `draw.rs:400-402` — into one `paint_ink_centered` helper.

**Leave alone:** the 6 plain `right_to_left` sites (`panels.rs:122/647/914/1065`,
`rep_panel.rs:264/1152`) — correct and measurement-free, and `Sides` defaults to
`height = interact_size.y` (40×18) while `overlay_button` is `const H = 26.0` (`widgets.rs:103`), so
migrating them needs an explicit `.height(26.0)` or every toolbar glyph shifts. Also leave: all of
`pickers.rs` and `overlay.rs` (deliberate pixel painting), the `mesh_bounds` ink centering (no egui
equivalent — every container aligns on the font line-box), `draw_axes_widget`'s bezel/blit, and
`panels.rs:1022`/`1104-1112`'s `Shape::Noop` reserve-and-backfill (egui's own `Frame` does exactly
this at `frame.rs:393`/`:489`).

---

## Phase 5 — `modal_shell` for the 4 real Modals  ·  net ≈ −80 LOC  ·  risk: medium

The four `Modal`s (load traj, delete frames, rename, docking) plus the near-miss image dialog do
share one shape: take-from-`Option` → `Modal::new(id)` + `set_width` → heading → body →
commit/cancel row → `should_close()` → put back or drop. Two already share `DialogAction`
(`loaders.rs:34-38`, `use` at `docking_dialog.rs:12`).

- Extract the shell into `widgets.rs`, owning the take/put-back, the button row, the width, and
  `should_close()`.
- Extend `DialogAction` with the **fourth state** that load and docking both hand-roll:
  commit-that-may-fail → reopen with `error` set (`loaders.rs:483-486`,
  `docking_dialog.rs:190-195`). Both already carry an `error: Option<String>` field
  (`loaders.rs:16`, `docking_dialog.rs:29`); the shell should own and render it.

**Bugs this fixes for free.** `export.rs:29` discards its `ModalResponse`, so the image dialog is the
only modal with **no backdrop close**, and its Escape is a non-consuming peek
(`export.rs:60-62`) — as is the settings window's (`settings_dialog.rs:486-488`). Consuming vs
peeking Escape across dialogs makes dismissal order-dependent today.

**Prerequisite.** `load_dialog`, `rename_mol` and `image_dialog` have **neither a unit test nor a
`MOLAR_VIS_DEBUG_*` hook**, and `app/` has 3 tests total. Add `_LOADDIALOG` / `_RENAME` /
`_IMAGEDIALOG` openers (trivial — mirror `init.rs:627`) before this lands, or accept manual
verification of three of the five.

**Explicitly out of scope:** the settings / interactions / view-settings `Window`s. Their divergence
is documented design (`settings_dialog.rs:413-420` — a Modal re-centers each frame so its top jumps
as tab height changes; `panels.rs:136-140/176-184`), two of them have no apply step to share, and
settings uniquely needs `&mut eframe::Frame`.

---

## Phase 6 — Widget-layer tests, no new dependencies  ·  ~2–4 h  ·  risk: none

**Phase 3 was the first of these, pulled forward** to gate the Phase 4 migration. These are the
rest.

**`egui_kittest` is not needed, and is unavailable offline anyway.** The pattern already exists in
this repo, twice: `theme.rs:270 hover_does_not_resize_widgets` is a working headless egui **layout
regression test** — `egui::Context::default()`, synthetic `RawInput { events: vec![PointerMoved] }`,
`ctx.run_ui`, rect assertions across both themes, with the two-frame settle documented at `:298-300`
("egui reads a widget's state from the previous frame's response"). `app.rs:1125 ime_workaround_tests`
drives a real `TextEdit` through synthetic IME events the same way.

The M25 split left a clean seam, and it falls on the testable side. These take only `&mut Ui` plus
plain data, and need no wgpu device:

`draw_rep_params` (`rep_panel.rs:94`) · `draw_color_tab` (`:405`) · `draw_periodic_tab` (`:465`) ·
`draw_traj_tab` (`:527`) · `draw_traj_bar` (`:573`) · `draw_group_bar` (`:712`) ·
`draw_axes_widget` (`settings_dialog.rs:342` — already takes `scene_tex: Option<TextureId>`, so
`None` works) · all of `widgets.rs` · all of `pickers.rs`. `Representation::new(kind)`
(`scene.rs:250`) constructs fine because `RepGpu` derives `Default` (`render.rs:388`).

Highest-value targets, chosen because they encode invariants nothing currently checks:

- **(a)** `draw_rep_params` shows/hides the Periodic and Color tabs correctly and falls back to
  `Style` when the active tab's condition disappears.
- **(b)** `picker_button` width is invariant across every `RepKind` / `ColorMethod` / `Material`
  label — the `max_label_width` contract (`widgets.rs:32-42`).
- **(c)** `draw_traj_bar` / `draw_group_bar` row fit — **done as Phase 3**, ahead of Phase 4.

**Do not frame this as replacing `MOLAR_VIS_DEBUG_SAVE_UI`.** The hook verifies *appearance* (theme
palettes, `Area` fade-in, glyph alignment, the 3D image composited under the panels), which rect
assertions cannot judge. Image snapshots could, at the cost of a GPU-bearing CI runner, golden PNGs,
and exactly the wall-clock fade-in flakiness the hook already fights (`export.rs:261-268` — hence
the 48-frame wait). Tests for invariants, screenshots for looks.

---

## Phase 7 — The two genuinely cohesive clusters  ·  ~150 LOC  ·  risk: low

Only these two survive co-usage analysis.

- **`RtState`** — `rt_scene_dirty`, `rt_warm`, `rt_warm_shown`, `rt_job`, `rt_still`,
  `pending_capture`, `export_request`, `image_dialog`, `debug_ui_frames`. 50 sites across
  `viewport.rs` (28), `export.rs` (19), `app.rs` (2), `panels.rs` (1). Co-usage is near-perfect:
  `viewport.rs:122-124` reads three together, `:179-188` writes three, `:195-227` touches four,
  `export.rs:104-110` writes five in six lines. Move `export.rs`'s five functions to `impl RtState`
  taking `(&mut SceneRenderer, &Camera, &Scene, &mut String)`; `draw_viewport` keeps `&mut self`.
- **`Console`** *(scripting)* — `console_open`, `console`, `script`. 13 sites; `console.rs:20`
  already passes the first two as a `&mut` pair into `script::console::show`.

Also worth doing here, both small and both fixing a real ownership smell:

- **`axes_on` + `axes_corner` → into `Camera`.** They are view state, serialized in `ViewState`
  (`session_io.rs:227-228/240-241`) and driven by the host API (`app.rs:547-556`). They belong with
  the camera, not with the menu widget that edits them. 8 sites.
- **`loaders` + `wasm_loaders` → into `Scene`.** They are `MolId`-keyed side tables mutated in
  lockstep with molecule removal at `panels.rs:768-770/782-784/795-797` and
  `draw_input.rs:569-571`. Separating them from `scene` would institutionalise the orphan-loader bug
  the current adjacency prevents; folding them in makes it one owner's invariant. 22 sites.

---

## Phase 8 — Per-function action enums  ·  ~260 LOC  ·  risk: medium-high  ·  do last

**Not** a crate-wide `PanelAction` — that would be a ~25-variant grab bag whose `match` arms live in
the same functions that already hold the `if let Some(x)` blocks, so no net reduction, and it loses
the per-payload typing (today each local's Rust type *is* its payload: `Option<(MolId, String)>`,
`Option<(usize, Representation)>`, `Option<(MoleculeSource, usize)>`).

Two per-function enums, and the win is **correctness**, not tidiness:

- **`draw_reps_for`**: `rep_panel.rs:832-850`'s 11 `Option`s → `Option<RepAction>`. The four
  index-mutating arms — `reorder` → `duplicate` → `clone_rep` → `delete`, applied at
  `rep_panel.rs:1172/1181/1193/1201` — become **provably one-per-frame** instead of relying on the
  unenforced "only one button is clicked" invariant, whose failure mode is wrong index arithmetic.
  Keep `view_dirty` a separate bool: it legitimately accumulates from many widgets (`:925`, `:1039`,
  `:1178`).
- **`draw_group_entry`**: `panels.rs:886-898`'s 8 bools + 3 `Option`s → `Option<GroupAction>`, and
  **fold the two `&mut Option<T>` out-params** (`panels.rs:851-854`) into the return:
  `-> GroupOutcome { view_dirty: bool, escalate: Option<GroupEscalation> }`. That removes the only
  out-param pair in the codebase and makes the signature honest about the index-invalidation
  constraint documented at `panels.rs:1210-1212`.

Precondition: the Phase 3 + Phase 6 tests exist, plus manual drag-reorder / duplicate / delete /
group-member-delete passes. The apply order at `rep_panel.rs:1172-1212` and `panels.rs:1137-1215` is
load-bearing and currently untested.

**Convention to state once** (~30 lines, a `PanelOutcome<A>` plus a doc rule). There are four shapes
in the codebase today — a returned flag struct (`RepParamsOutcome`, `rep_panel.rs:87`), a bare
`bool` meaning "view_dirty" (7 sites), a returned `Option<T>` payload (`pickers.rs:116`,
`rep_panel.rs:712`), and one `&mut Option<T>` out-param pair. The shape that fits:
`-> Outcome { view_dirty: bool, action: Option<Action> }` — `view_dirty` is additive, `action` is
exclusive. `RepParamsOutcome` is already 90% this.

**Leave the timing-driven App flags alone**: `export_request`, `rt_warm`/`rt_job`/`rt_still`,
`pending_capture`, `pending_undo_n`/`pending_redo_n`, `minimize_pending`. They are not "UI couldn't
act because of a borrow" — they are "the wgpu render state isn't valid yet" (`app.rs:1094-1099`),
"the overlay must paint one frame first" (`viewport.rs:195-223`), "wait for the pointer to settle"
(`draw_input.rs:658`). Folding them into a panel-outcome type would misdescribe them.

---

## Not doing, and why

- **A `Dialogs` mega-cluster** (10 fields) — anti-cohesive; never co-used; would force 6 modules to
  hold `&mut Dialogs` for one field each.
- **`trait Dialog`** — incompatible with 3 of the 8 (they need `&self.scene` / `&mut self.scene` /
  `&mut self` inside the closure). Making it fit needs a `DialogCx` partial-reborrow struct that
  costs more than the ~150 lines of scaffolding it replaces.
- **A single dispatch site** — already exists (`app.rs:1013-1021`). The 8th dialog
  (view-settings, `panels.rs:131`) *cannot* move there: it needs the hamburger's `Response::rect` as
  its anchor, produced inside the toolbar panel's closure.
- **Moving the 5 orchestrators off `impl App`** — `ui` (`app.rs:891`), `draw_viewport`
  (`viewport.rs:15`), `draw_menu_bar` (`panels.rs:385`), `draw_molecule_list` (`panels.rs:563`),
  `draw_reps_for` (`rep_panel.rs:776`). Each would need 7–12 parameters; `draw_viewport` would need
  13 callees re-plumbed. A 12-parameter signature declares "almost everything", which is what
  `&mut self` already says honestly. ~1,800 lines for a negative readability delta.
- **Grouping the 8 cross-cutting fields** — 534 sites, 70% of traffic. For `view_dirty` and `status`
  specifically, the right fix is *returning* the flag/message, which `panels.rs:10` and
  `rep_panel.rs:783` already do.
- **`TableBuilder` / `egui_extras`** — no target exists; there is no scrolling retained-column table
  in the app. The molecule list is a tree of rows with per-row semantics.
- **`sizing_pass()`** — no target either; and it cannot replace `max_label_width` (measures current
  content, not max-over-options).
- **`egui_taffy` / `egui_flex`** — unavailable offline, and after Phase 4 no site wants them.
- **`egui_kittest`** — not vendored, cannot be added here, and `theme.rs:270` proves it isn't needed
  for the 80% case.
- **A `UndoIntent` struct** — 7 sites. Collapse `pending_undo_n`/`pending_redo_n` into one field or
  have `draw_menu_bar` return the intent.
- **Test CI.** Heavy (see Phase 0) and pointless here: CI is release-tag-only by design
  (`89e550e`), and the sole committer runs the tests locally. `scripts/check.sh` is the answer
  instead.

---

## Separate items surfaced by the audits

Not part of this refactor; recorded so they aren't lost.

- **`view_menu_rect` (`app.rs:220`) is a real wart** but neither `Sides` nor `sizing_pass` touches
  it — it is a Window-used-as-Popup problem. `panels.rs:136-140` records that `Popup` +
  `CloseOnClickOutside` was rejected because nested dropdowns fought it, but `pickers.rs:429-433`
  and `widgets.rs:228-229` now use exactly that successfully for nested pickers. Worth one
  experiment.
- **Rename is not undoable.** `MolState` (`history.rs:177-182`) carries `id`/`visible`/`reps` only,
  so `mol.name` is outside the document (it *is* in `MolSession.name` for sessions). Deliberate or
  an oversight — worth a decision.
- **Doc-comment rot from the M25 split.** `widgets.rs:158-162` carries `draw_rep_params`'s doc
  stranded above `tab_bar`'s own; `widgets.rs:134-136` carries `toolbar_label`'s doc stranded above
  `bold_name`'s, leaving `toolbar_label` (`:147`) undocumented.
- **`egui::Grid` is itself previous-frame-lagged** (`grid.rs:110` fresh `curr_state`, `:227`
  `prev_col_width`) and `debug_assert`s against being nested in a `right_to_left` Ui (`grid.rs:100`).
  Worth knowing before adding Grids inside the rep rows.

---

## Order and totals — all done

Phases are numbered in execution order. Each landed as its own commit, verified against
`scripts/check.sh` (the four renders and the session round-trip came back **byte-identical**
after every phase).

| phase | what | risk | commit |
|---|---|---|---|
| 0 ✅ | `scripts/check.sh` + a recorded baseline | none | — |
| 1 ✅ | 22 single-field methods → `Scene`/`Camera`; `rebuild_dirty` → free fn | none | `b245f6f` |
| 2 ✅ | One state struct per dialog (8 fields → 4) | low | `f019454` |
| 3 ✅ | The traj/group bar fit tests (precede Phase 4) | none | `6cfad99` |
| 4 ✅ | Three `Sides` edits in `rep_panel.rs` | low | `d4c1c3f` |
| 5 ✅ | `modal_shell` + the 4th dialog state (+ 3 debug hooks first) | medium | `bedc157` |
| 6 ✅ | Remaining widget-layer `ctx.run_ui` tests | none | `884e57e` |
| 7 ✅ | `RtState`, `Console`, axes → `Camera`, loaders → `Scene` | low | `329d001` |
| 8 ✅ | `RepAction` / `GroupAction` enums + the outcome convention | med-high | `2b9d90e` |

Field count: **46 → 32** in the native default build (55 → 37 declared). Tests: 92/96 → 122/126.

### Where the plan was wrong, and what was done instead

* **Phase 1's attribution was optimistic for three methods.** Per-function field attribution
  (rather than eyeballing) shows `dihedral_preview_hover` reaches scene + camera + draw,
  `sync_docking_frames` reaches camera transitively via `switch_group_member_synced`, and
  `toggle_hydrogens` reaches draw via `after_draw_edit`. The first two stayed on `App`; the
  third was split, with the mutation on `Scene`. Two that mixed in `view_dirty`/`status` return
  the flag/message instead, per the plan's own note. `drawing_plane_point`/`drag_dir` reach
  `draw` too, so they moved to `Camera` with the plane depth as an explicit parameter.
* **Phase 2's count was 8 fields, not 11**, and the `ViewMenu` keeps `open` *inside* the struct
  rather than becoming an `Option`: it is a toolbar popover flipped open and shut constantly and
  its tab is sticky across that, so an `Option` would reset it to Camera on every reopen.
* **Phase 3's probe widths had to start at 336.** The trajectory bar's *first* row has an
  intrinsic floor (310.25 pt, 329.44 above 50 frames) that no reserve arithmetic can help — a
  separate, pre-existing left-panel minimum-width limit. Above it the old reserve left **2 px**
  of slack; after Phase 4 it is 0 by construction.
* **Phase 5 came out net +180 LOC, not −80.** The functional code did shrink; the shell's doc
  comments and its 5 unit tests more than made up the difference.
* **Phase 7's `loaders` → `Scene` needed more than a field move.** The plan's stated benefit
  ("one owner's invariant") does not follow from relocating the fields, because every removal
  site is in `app/`. So `Scene::trash_molecule` / `trash_grouped_molecule` now own the whole
  remove → drop-loaders → trash → clamp ritual and the four sites each collapse to one call.
  Also, `axes_on` in `Camera` exposed that `Camera::frame_bbox` builds a *whole* camera and
  would have switched the gizmo off on a new document — the four such sites now go through one
  `App::reframe_camera`.
* **Phase 8's precondition (manual GUI passes) could not be met.** Substituted by extracting the
  index arithmetic onto `Molecule` and unit-testing it *before* changing any caller. No generic
  `PanelOutcome<A>` type: the three outcomes' payloads differ, so the convention is written down
  once instead.
* **Not done:** moving `export.rs`'s functions to `impl RtState`. `draw_image_dialog` goes
  through `modal_shell` (which needs `&mut App`), and `export_image` reaches 8 of `App`'s fields
  — the plan's proposed 4-parameter signature does not survive contact, and a 6-parameter one
  would be worse than `&mut self`, which is the plan's own reasoning about the 5 orchestrators.
