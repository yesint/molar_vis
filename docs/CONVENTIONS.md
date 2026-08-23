# molar_vis — Conventions & gotchas

> Reference doc for [molar_vis](../CLAUDE.md). Split out of the master `CLAUDE.md` for on-demand reading — see it for the project overview, build quick-start, and the full docs index.

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
- **Never set `visuals.override_text_color`** (the theme used to, and it was a trap): it forces one
  colour on *all* widget text — including a **selected** widget's, whose colour egui otherwise takes
  from `visuals.selection.stroke`. A toggle painted with the selection plate then kept the panel's
  ink and came out **black-glyph-on-dark-plate** in the light theme. `theme::set_text_colors` sets
  the five per-state `fg_stroke`s instead. Both palettes give every *resting* state the same ink,
  so **frameless buttons still show no text hover feedback** — use `selectable_label` (frameless at
  rest, highlights its background on hover) or a framed widget for a clickable icon.
- **No hand-made colour track beside egui's.** A semantic colour that egui models is read from the
  style — an error message is `ui.visuals().error_fg_color`, not a helper. egui models only
  `warn_fg_color` / `error_fg_color`, so anything else (the accept ✓'s green, the selection-glow
  colours) is named in a sheet's `[extras]` and read via `theme::ok_color` / `theme::glow_color`.
  Never a literal in a widget: the pale green and salmon red that used to be hardcoded were picked
  against a near-black panel and were all but invisible on the light theme's mid-grey — the pending
  selection's **accept ✓** could not be seen at all. Note the glow follows the *viewport background*,
  not the UI theme (a cue over the render must contrast with the render), and reaches the shaders
  through the camera uniform so there is one decision, not a CPU/GPU pair. Colours drawn over the 3-D
  view — the axes gizmo, the dihedral overlay, the modifier-hint pill — keep their own literals; they
  sit on the render, not on a panel.
- A widget you paint yourself must take its ink from `Style::interact_selectable(&resp, active)`
  (`vis.text_color()`), **not** `ui.visuals().text_color()` — the latter is the panel's ink and is
  wrong the moment the widget paints a selection plate behind it (`widgets::overlay_button`,
  `draw::bond_order_icon`). Lay the galley out with `Color32::PLACEHOLDER` and pass the real colour
  to `Painter::galley`, since the state isn't known until the `Response` exists.
- **Drive focus-dependent layout from `ui.memory(|m| m.has_focus(id))`, never from a flag you keep
  in sync with `Response::gained_focus()`/`lost_focus()`.** The rep row swaps between a bounded field
  (with action buttons) and a full-width editor depending on whether the selection field is focused
  (`rep_panel::draw_reps_for`). Tracking that with a separate `editing_rep` flag toggled on the focus
  *events* drifts: a missed/duplicated event (or a re-render that overwrites the flag) leaves it
  cleared-but-focused (row stuck collapsed — re-clicks did nothing) or set-but-unfocused (stuck open).
  Reading the field's **actual** focus each frame can't drift. Keep a flag only as a **one-shot open
  request** (e.g. the debug hook): render the field, `Response::request_focus()` once, then let
  `has_focus` sustain the layout.
- Icons: `egui_phosphor::regular::{EYE, EYE_SLASH, TRASH, COPY, PLUS, PERSPECTIVE, CUBE}`;
  the font is installed in `theme::apply` via `egui_phosphor::add_to_fonts`.
- **Wayland IME workaround** (`defuse_broken_ime` at the top of `App::ui`, Linux-gated):
  recent Wayland compositors make winit stream `Ime(Disabled)` + deliver typed chars as
  `Ime(Commit(..))` with no `Enabled`/`Preedit`, which egui 0.34.3 mishandles so text
  fields accept only the **first** character (paste/backspace still work). We rewrite
  `Ime(Commit)`→`Text` and drop stray `Ime` events. No-op on X11; macOS/Windows untouched.
  See `mod ime_workaround_tests` and the [[wayland-ime-textinput-workaround]] memory.

