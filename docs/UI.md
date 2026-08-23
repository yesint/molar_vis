# molar_vis — UI layout

> Reference doc for [molar_vis](../CLAUDE.md). Split out of the master `CLAUDE.md` for on-demand reading — see it for the project overview, build quick-start, and the full docs index.

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
  `selectable_label`) · **Load docking data…** (native; the receptor + ligand-pose loader — see
  the `docking.rs` bullet) · **Load…** (`App::open_structure` — native `rfd` picker / wasm file picker
  filtered to topology+coords formats pdb/ent/gro/xyz/tpr; loads via `data::load`, `scene.add`s a new
  molecule, frames the camera on the first one, undoable via the normal checkpoint).
- **Session** — **New** (`App::new_session` — drop all molecules + reset camera/history to an empty
  document; **pure in-memory, so available on wasm too**) · **Save…** (`App::save_session`) ·
  **Load…** (`App::load_session`) — saving/loading the whole visualization state as a JSON session
  (see `session.rs`). **Save/Load are native-only** (they reload molecules from disk source paths);
  only **New** shows on wasm.
- **Render** — **Image…** opens a small **save dialog** (`App::image_dialog` / `draw_image_dialog` in
  `app/export.rs`): pick the **output size** (`Viewport (1×)` / `2×` / `4×`, each labelled with the
  resulting px) + **format** (PNG only for now), then **Save** → `App::export_request` → `export_image`, which on **native pops the `rfd` save dialog
  *first* (before rendering) and renders to the chosen path**; **wasm triggers a browser download** (Blob → object URL →
  `<a download>`). **On a compute-capable device (WebGPU/native) this is a full GPU ray trace**
  (ray-traced AO + shadows + Blinn-Phong, all rep types — see the `render/raytrace.rs` bullet),
  **frame-pumped with a "Saving…" overlay so the UI stays responsive** (no freeze); **WebGL2 falls
  back to a high-res capture of the rasterized view**. With the View-settings **Global illumination**
  slider > 0, the trace is **path-traced GI** (soft sky-dome ambient + indirect colour bleeding, ACES
  tonemap — see the GI bullet under `render/raytrace.rs`). Separately, pressing **R** in the viewport
  ray-traces the current view in place (PyMOL-`ray` style; honors AO/shadows + GI) and holds it until
  the camera moves; see `render/raytrace.rs`.
- **Edit** — **Undo** / **Redo** (single step, each labelled with the next action's
  `describe_change` and a `shortcut_text`; the old `▼` **cumulative** undo/redo dropdown is gone, but
  Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y still repeat — `History::undo_n`/`redo_n`/`undo_len`/`redo_len`
  remain as test-only/API machinery) · **Settings…** (`GEAR_SIX`) opening the program-settings window
  (`App::draw_settings_dialog`; see `settings.rs` / M21).
- **Analysis** — measurements on the loaded structures; pure computation on the scene, so it is in
  the browser build too. **Align…** (`ARROWS_IN`) opens the alignment window
  (`app/align_dialog.rs`, [`analysis.rs`]): **Source** and **Target** rows, each `[molecule ▾]
  [selection text] [⌖ pick a rep] [frame ⇕]`, then the checkboxes *All frames* · *Same as source* ·
  *Common subset* · *Move whole molecule* (**all off by default**), an **RMSD:** readout that
  appears once something has been computed, and `[Align] [RMSD] [Close]`. The decisions behind it:
  - **There is no "Selection" vs "Existing rep" distinction** (the first sketch had one): a rep *is*
    a molecule plus a selection, so the **⌖ picker** just writes those into the row — click a rep in
    the tree or in the 3-D view — and the text stays editable. It reuses the Interactions partner
    picker's whole gesture through **`RepPick`** (`Partner` | `Align(side)`) — one pick mechanism
    with a destination attached, dispatched by `App::choose_rep`, rather than one flag per feature.
  - **The molecule dropdown is group-aware and scrolls** (`mol_entries`/`chosen`/`group_entry`): a
    [`MolGroup`] is **one** expandable row (`⧉ ligands20.sdf — diazepam ▸`) whose submenu lists the
    members with the shown one marked as the panel marks it; clicking the row itself takes the
    **shown** member, so the common case is one click. Listing members flat is what broke this
    control — a 20-pose SDF buried every other molecule — and it also misrepresented a group, which
    the rest of the UI treats as one thing showing one member at a time. Both levels are wrapped in
    a `ScrollArea` (`LIST_MAX_H`), since a menu taller than the screen can't be used at all. A
    chosen member reads qualified by its group (`ligands20.sdf: aspirin`), and a freshly opened
    dialog defaults to the group's **shown** member (`default_mol`), never to a hidden sibling.
  - **The selection field has a fixed width and the window grows around it** (`SEL_FIELD_W`,
    `set_min_width(MIN_WIDTH)` rather than `set_width`): it used to take whatever the molecule
    dropdown left over, so choosing a group member — whose label carries the group's name — squeezed
    the longest thing typed in this dialog down to a few characters. Both dropdowns share one
    reserved width (the wider of the two current labels, `max_label_width`), which also keeps the
    two rows' columns lined up with each other.
  - **`Common subset` is off by default**: atom for atom is the honest comparison and it *reports*
    a count mismatch, whereas name-pairing guesses — something to switch on deliberately.
  - **`All frames` sits on the Source row**, because it belongs to what *moves*: it fits every frame
    of the source molecule onto whatever the target is (a whole trajectory onto a reference
    structure, or onto one of its own frames). Enabled only when that molecule has >1 frame.
  - **`Same as source`** makes the target the source's own molecule + selection at the frame in the
    **source's** frame box (that box is then the *reference*), which with `All frames` is the usual
    trajectory fit. With it off, the frame that moves is the molecule's **displayed** one — the
    source's box is spoken for, and comparing a frame with itself would do nothing.
  - **`Move whole molecule`** applies the fit to every atom of the source molecule; **off (the
    default)** moves only the atoms its selection matched. The transform comes from the selection
    either way. Off means a partial selection is moved *out of* its molecule — which is the user's
    explicit choice of default.
  - **RMSD** is reported after the fit for [Align] (so it is the residual, measured from the stored
    coordinates) and without moving anything for [RMSD]; over several frames it reads
    `mean of N frames (min …, max …)`, since one number for a fitted trajectory can't say whether
    it is uniformly close or has outlier frames.
- **View** — **`[x] Console`** (a `CHECK_SQUARE`/`SQUARE`-marked toggle of `console_open` — the Rhai
  scripting console bottom panel; opening it sets `console.focus_input` so the input grabs focus; see
  `script/engine.rs` / M24). This is the menu's only entry, so the **whole View menu is behind the
  `scripting` feature** (M31) and absent from a default build.

Then one **molecule row** each:
expand-caret + **name** (**bold** via `widgets::bold_name` — the embedded Ubuntu Bold; see
`theme.rs`; a group's *shown* member is additionally underlined) (the atom/frame counts are no
longer shown inline — they're a **hover tooltip** on the name: `N atoms / M frames`) + **Load-trajectory** (`FOLDER_OPEN`, left of the
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
    (enable + Strength + **Softness**; `Camera::shadow` — Softness rides `shadow_uniform`'s 4th slot
    and is used only by the ray tracer's soft penumbra) + a **Ray tracing** group — a "Press R to
    ray-trace the view" hint (the viewport still is the **R key**, PyMOL-`ray` style; greyed without a
    compute-capable device) + a **Global illumination** strength slider (`Camera::gi`, 0..1, 0 = off/default —
    path-traced GI applied to both the R-key still and Save image). The AO/shadow controls feed both
    the R-key still and Save image.
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
  **selection field** (fills the row's remaining width, bounded by `Sides::shrink_left`
  against the compact action group) · right-justified compact actions
  (`Layout::right_to_left` + `compact_actions`): **zoom-to-selection** (`MAGNIFYING_GLASS_PLUS`
  → `Camera::focus_bbox` on the rep's `sel` bbox) · eye · a **per-rep menu** (`LIST` hamburger)
  holding the less-frequent actions so the row stays uncluttered: **Edit (draw mode)** (`Edit`
  action → `open_rep_in_editor` — Draw scoped to *this* rep's selection; the item is highlighted
  when this rep is the active draw target), **Duplicate** (`COPY`), **Save selection…**
  (`FLOPPY_DISK` → `save_rep_selection`, native), **Delete** (`TRASH`). (Editing/draw is now
  per-representation — the molecule and group rows no longer carry a pencil.) The rep's
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
  are render-only so the tab returns a `view_dirty` bool instead of setting `geom_dirty`), and
  **[Color]** (`draw_color_tab`, **only shown for a color scheme that has options** — gated the same
  way [Periodic] is gated on a box, so today only `Charge`): *partial* / *formal* **radio buttons**
  (`rep.charge_kind`) with an **icon-only ⚡ button right after *Partial*** (tip *"Compute Espaloma
  charges on selection"*, native-only — it sits with that option because partial is the charge it
  assigns; formal charges are read from the structure, never computed). **Only failures are shown**,
  wrapped and red — a success shows in the colors, and `App::ui` drops the message on the next
  click/keypress/scroll so a stale error can't sit there reading as if it were still true. Because
  the panel only sees the rep, the button is reported back via **`RepParamsOutcome`** (the same
  deferred-action pattern as the Interactions partner/settings buttons) and
  `App::compute_rep_charges` runs it once the `&mut Molecule` borrow ends. For a **shared rep of a
  [`MolGroup`]** it charges **every member**, not just the shown one (a group is meant to be treated
  as one set, and only one member is visible at a time), recording all of them as a **single**
  `StructEdit::Charges` step via `History::record_structs` so one Ctrl+Z takes it all back. Tab in
  `rep.settings_tab: SettingsTab`. The tab bar uses the shared **`tab_bar(ui, &mut current, &[(T,
  label)…])`** helper — the **app-default tab style** (underline tabs: selected = bold + accent
  underline, others weak/clickable), reused by every tabbed UI (rep settings, the delete-frames
  dialog, …) so they stay consistent. Style and color are **icon+text** buttons built by the shared
  `picker_button(label, draw_icon)` helper (drawn glyph + label + caret, painted as a **button at
  rest** — fill/stroke/ink from the widget state, so a dropdown reads as clickable against a
  same-coloured panel and matches the buttons beside it → `egui::Popup::menu`
  of icon+label rows). `paint_style_icon` draws each `RepKind`; `paint_color_icon` (which takes the panel's
  `ink` colour, since the **molecular** colours it depicts are fixed but a pale one — carbon grey,
  the charge ramp's white centre — dissolves into a light panel without the ink-derived hairline it
  outlines every swatch with) draws each
  `ColorMethod` (Element = CPK dots, Chain = interlocking colored links, ResID =
  backbone-with-residues diagram, ResName = "ALA" on rainbow, Index = "123" colored digits,
  Beta = "B" on rainbow, **Solid = a filled swatch of the chosen color**). The `Solid` row is a
  **submenu** (`egui::containers::menu::SubMenu`, ⏵): hovering opens a panel with a preset
  swatch grid (`SOLID_SWATCHES`, `swatch_button`) + a full `color_picker_color32` (the submenu uses
  `CloseOnClickOutside` so dragging the picker doesn't dismiss it).

History labels via `describe_change` ("edit selection", "change coloring",
"reorder representations", …). FPS in the footer.

