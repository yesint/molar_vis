//! Shared egui widget helpers used across the app submodules.
use super::*;

/// Viewport pixel position → clip-space NDC (each component in `[-1, 1]`, y up). The single
/// source of the pixel→NDC mapping shared by picking, drawing, the lasso, and the dihedral
/// tool — change the viewport-to-NDC convention here, not in each call site.
pub(super) fn px_to_ndc(px: egui::Pos2, rect: egui::Rect) -> glam::Vec2 {
    glam::vec2(
        ((px.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0,
        1.0 - ((px.y - rect.top()) / rect.height().max(1.0)) * 2.0,
    )
}


/// A compact icon button: frameless at rest, with a background highlight on
/// hover, plus a tooltip. Implemented via `selectable_label` (always unselected)
/// because the theme overrides text color, so a frameless `Button` would show no
/// hover feedback, whereas `selectable_label` highlights its background.
pub(super) fn icon_button(ui: &mut egui::Ui, glyph: &str, hover: &str) -> egui::Response {
    ui.selectable_label(false, glyph).on_hover_text(hover)
}

/// An undo/redo menu item with its action and shortcut on top and a smaller
/// description beneath. Both lines belong to the same clickable row.
pub(super) fn history_menu_item(
    ui: &mut egui::Ui,
    enabled: bool,
    glyph: &str,
    action: &str,
    shortcut: &str,
    description: Option<&str>,
) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        let font = egui::TextStyle::Button.resolve(ui.style());
        let small = egui::TextStyle::Small.resolve(ui.style());
        let title = ui.painter().layout_no_wrap(format!("{glyph}  {action}"), font.clone(), egui::Color32::PLACEHOLDER);
        let shortcut = ui.painter().layout_no_wrap(shortcut.to_owned(), small, egui::Color32::PLACEHOLDER);
        let description = description.map(|text| egui::WidgetText::from(egui::RichText::new(text).small()).into_galley(
            ui, Some(egui::TextWrapMode::Truncate), 320.0, egui::TextStyle::Small,
        ));
        let pad = ui.spacing().button_padding;
        let indent = pad.x + font.size + ui.spacing().icon_spacing;
        let width = (title.size().x + shortcut.size().x + 24.0 + pad.x * 2.0)
            .max(description.as_ref().map_or(0.0, |text| indent + text.size().x + pad.x));
        let height = (title.size().y + 2.0 * pad.y
            + description.as_ref().map_or(0.0, |text| text.size().y + 3.0))
            .max(ui.spacing().interact_size.y);
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
        let visuals = ui.style().interact(&response);
        if response.hovered() || response.has_focus() {
            ui.painter().rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
        }
        let top = rect.top() + pad.y;
        ui.painter().galley(egui::pos2(rect.left() + pad.x, top), title.clone(), visuals.text_color());
        ui.painter().galley(egui::pos2(rect.right() - pad.x - shortcut.size().x,
            top + (title.size().y - shortcut.size().y) * 0.5), shortcut, ui.visuals().weak_text_color());
        if let Some(description) = description {
            ui.painter().galley(egui::pos2(rect.left() + indent, top + title.size().y + 3.0),
                description, ui.visuals().weak_text_color());
        }
        response
    }).inner
}

/// Horizontal gap between compact action icons. Also the gap to pass an
/// [`egui::containers::Sides`] whose sides are compact groups — `Sides` reads its default
/// gap from the *parent* `Ui`, which hasn't been tightened, so without this the seam
/// between the two sides would be wider than the seams within them.
pub(super) const COMPACT_SPACING: f32 = 2.0;

/// Paint `galley` centred in `rect` by its **ink** bounds rather than its font line-box.
///
/// Every container egui offers aligns text on the line-box, which includes ascender/descender
/// space the glyph may not use — so a Phosphor icon or a short label comes out visibly high or
/// low inside a fixed-height frame. Measuring `Galley::mesh_bounds` instead puts the drawn
/// pixels dead centre. There is no egui equivalent, hence this.
pub(super) fn paint_ink_centered(
    painter: &egui::Painter,
    rect: egui::Rect,
    galley: std::sync::Arc<egui::Galley>,
    color: egui::Color32,
) {
    let ink = galley.mesh_bounds;
    painter.galley(rect.center() - ink.center().to_vec2(), galley, color);
}

/// Tighten spacing for a group of action icons (call first in the group's `ui`).
pub(super) fn compact_actions(ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing.x = COMPACT_SPACING;
    ui.spacing_mut().button_padding = egui::vec2(3.0, 1.0);
}

/// Widest option label in the themed button font, for menu rows and shared columns.
pub(super) fn max_label_width<'a>(ui: &egui::Ui, labels: impl Iterator<Item = &'a str>) -> f32 {
    let txt = ui.visuals().text_color();
    let font = egui::TextStyle::Button.resolve(ui.style());
    labels
        .map(|l| {
            ui.painter()
                .layout_no_wrap(l.to_owned(), font.clone(), txt)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max)
}

/// The closed height of a standard egui ComboBox, including its themed padding.
pub(super) fn dropdown_height(ui: &egui::Ui) -> f32 {
    // Use laid-out text: its pixel rounding is the same as ComboBox's selected label.
    let text_height = ui.painter().layout_no_wrap(
        "Mg".to_owned(),
        egui::TextStyle::Button.resolve(ui.style()),
        ui.visuals().text_color(),
    ).size().y;
    (text_height.max(ui.spacing().icon_width)
        + 2.0 * ui.spacing().button_padding.y)
        .max(ui.spacing().interact_size.y)
}

fn picker_parts(ui: &egui::Ui) -> (f32, f32, f32, f32) {
    (20.0_f32.max(ui.spacing().icon_width), ui.spacing().icon_width,
        ui.spacing().button_padding.x, ui.spacing().icon_spacing)
}

pub(super) fn picker_width_range(ui: &egui::Ui, label_width: f32) -> (f32, f32) {
    let (icon, caret, pad, gap) = picker_parts(ui);
    let chrome = icon + caret + 2.0 * (pad + gap);
    let compact = chrome + label_width.min(egui::TextStyle::Button.resolve(ui.style()).size * 4.0);
    (compact.max(100.0), (chrome + label_width + 0.5).max(100.0))
}

/// A dropdown with an icon, selected label, and caret. Row layout can expand its
/// stable width through `combo_width` as more space becomes available.
/// Returns the click response (drive a `Popup` off it).
pub(super) fn picker_button(
    ui: &mut egui::Ui,
    label: &str,
    label_width: f32,
    draw_icon: impl FnOnce(&egui::Painter, egui::Rect),
) -> egui::Response {
    // `PLACEHOLDER` now, real ink once the state is known (see `overlay_button`).
    let (icon_w, caret_w, pad, gap) = picker_parts(ui);
    let (compact, full) = picker_width_range(ui, label_width);
    let w = ui.spacing().combo_width.max(compact).min(full);
    let label_width = w - icon_w - caret_w - 2.0 * (pad + gap);
    let galley = egui::WidgetText::from(label).into_galley(
        ui, Some(egui::TextWrapMode::Truncate), label_width, egui::TextStyle::Button,
    );
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, dropdown_height(ui)), egui::Sense::click());
    // Painted as a **button** at rest, not just on hover: it *is* a dropdown, and against a
    // panel of the same colour there was nothing to click on until the cursor found it. The
    // fills come from the widget state, so it matches every other button in either theme.
    let vis = ui.style().interact(&resp);
    ui.painter().rect_filled(rect, vis.corner_radius, vis.weak_bg_fill);
    if vis.bg_stroke.width > 0.0 {
        ui.painter()
            .rect_stroke(rect, vis.corner_radius, vis.bg_stroke, egui::StrokeKind::Inside);
    }
    let txt = vis.text_color();
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + pad, rect.center().y - 8.0),
        egui::vec2(icon_w, 16.0),
    );
    draw_icon(ui.painter(), icon_rect);
    ui.painter().galley(
        egui::pos2(icon_rect.right() + gap, rect.center().y - galley.size().y * 0.5),
        galley,
        txt,
    );
    let caret = egui::Rect::from_center_size(
        egui::pos2(rect.right() - pad - caret_w * 0.5, rect.center().y),
        egui::vec2(caret_w * 0.7, caret_w * 0.45),
    );
    ui.painter().add(egui::Shape::convex_polygon(
        vec![caret.left_top(), caret.right_top(), caret.center_bottom()],
        txt,
        egui::Stroke::NONE,
    ));
    resp.on_hover_text(label)
}

/// Reserve all option labels so selecting an item never resizes a standard dropdown.
pub(super) fn fixed_combo<'a>(
    ui: &egui::Ui,
    id: impl std::hash::Hash,
    labels: impl Iterator<Item = &'a str>,
) -> egui::ComboBox {
    let width = max_label_width(ui, labels) + ui.spacing().icon_width
        + ui.spacing().icon_spacing + 2.0 * ui.spacing().button_padding.x;
    egui::ComboBox::from_id_salt(id).width(width.max(ui.spacing().combo_width))
}

/// One control in the viewport overlay toolbar: a fixed-height framed button
/// whose content (an icon glyph, or `label + caret`) is centered by its **ink**
/// bounds (`Galley::mesh_bounds`), not the font line-box. `ui.button` /
/// `selectable_label` center the line-box, so Phosphor glyphs with different
/// metrics (the depth-cue lines sat low, the cube high) looked vertically
/// ragged; ink-centering lines them up. `active` paints the selection fill
/// (toggle / open state). Returns the response — drive a `Popup::menu` off it.
pub(super) fn overlay_button(ui: &mut egui::Ui, content: &str, active: bool) -> egui::Response {
    let h = dropdown_height(ui);
    const R: f32 = 4.0;
    let font = egui::TextStyle::Button.resolve(ui.style());
    // Laid out with `PLACEHOLDER`, which `Painter::galley` fills in from the colour passed
    // below — the ink colour isn't known yet (it depends on the button's own state, which
    // needs its `Response`), and an *active* button must take it from the selection rather
    // than the panel, or its glyph sits unreadably on the selection plate.
    let galley = ui
        .painter()
        .layout_no_wrap(content.to_owned(), font, egui::Color32::PLACEHOLDER);
    let w = (galley.size().x + 2.0 * ui.spacing().button_padding.x).max(h);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
    let vis = ui.style().interact_selectable(&resp, active);
    let fill = if active {
        ui.visuals().selection.bg_fill
    } else {
        vis.weak_bg_fill
    };
    ui.painter().rect_filled(rect, R, fill);
    if vis.bg_stroke.width > 0.0 {
        ui.painter()
            .rect_stroke(rect, R, vis.bg_stroke, egui::StrokeKind::Inside);
    }
    paint_ink_centered(ui.painter(), rect, galley, vis.text_color());
    resp
}

/// A tree node's name (molecule / group / member) in **bold**.
///
/// egui's `RichText::strong()` only swaps in a brighter colour — its bundled fonts have no bold
/// face — so this selects the embedded **Ubuntu Bold**, the base font's own bold sibling (see
/// [`crate::theme::bold`]), at the current body size so it lines up with the rest of the row.
pub(super) fn bold_name(ui: &egui::Ui, text: &str) -> egui::RichText {
    let size = egui::TextStyle::Body.resolve(ui.style()).size;
    egui::RichText::new(text).font(crate::theme::bold(size))
}

/// A molecule or representation index that toggles visibility. Both node types
/// use the single-line selection field height, independent of compact action groups.
pub(super) fn index_badge(
    ui: &mut egui::Ui,
    prefix: &str,
    index: usize,
    visible: bool,
) -> egui::Response {
    index_badge_label(ui, &format!("{prefix}{index}"), visible)
}

pub(super) fn index_badge_label(ui: &mut egui::Ui, label: &str, visible: bool) -> egui::Response {
    let height = node_row_height(ui);
    let galley = egui::WidgetText::from(bold_name(ui, label))
        .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button);
    // Allocate before styling: egui's Button frame margins vary with hover strokes.
    let size = egui::vec2(galley.size().x + 2.0 * ui.spacing().button_padding.x, height);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let visuals = ui.style().interact_selectable(&response, visible);
    ui.painter().rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
    ui.painter().rect_stroke(
        rect,
        visuals.corner_radius,
        egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    paint_ink_centered(ui.painter(), rect, galley, visuals.text_color());
    response.on_hover_text(if visible { "Hide" } else { "Show" })
}

fn node_row_height(ui: &egui::Ui) -> f32 {
    // TextEdit's single-line body text has two points of margin above and below.
    ui.painter().layout_no_wrap(
        "Mg".to_owned(),
        egui::TextStyle::Body.resolve(ui.style()),
        ui.visuals().text_color(),
    ).size().y + 4.0
}

/// Square molecule expander aligned with the visibility badge and selection row.
pub(super) fn molecule_expander(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let height = node_row_height(ui);
    let caret = if open { icon::CARET_DOWN } else { icon::CARET_RIGHT };
    let galley = egui::WidgetText::from(caret)
        .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button);
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(height), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
        ui.painter().rect_stroke(rect, visuals.corner_radius, visuals.bg_stroke, egui::StrokeKind::Inside);
    }
    paint_ink_centered(ui.painter(), rect, galley, visuals.text_color());
    response
}

/// Run `add_contents` in a left-to-right area that fills what is left of a row whose
/// right-aligned actions are already placed (call it last inside the row's
/// `right_to_left` layout), with the row's normal spacing back (the actions use
/// [`compact_actions`]). Put a [`truncated`] name in it, so a long name ends in "…"
/// instead of pushing the actions out of view.
pub(super) fn row_rest<R>(
    ui: &mut egui::Ui,
    spacing: &egui::style::Spacing,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        *ui.spacing_mut() = spacing.clone();
        add_contents(ui)
    })
    .inner
}

/// A label truncated with "…" to the available width minus `reserve` (the width any
/// widgets after it on the row need), so they stay visible.
pub(super) fn truncated(ui: &mut egui::Ui, label: egui::Label, reserve: f32) -> egui::Response {
    let w = (ui.available_width() - reserve).max(0.0);
    ui.scope(|ui| {
        ui.set_max_width(w);
        ui.add(label.truncate())
    })
    .inner
}

/// A plain text label vertically centered the same way [`overlay_button`] centers its glyph
/// (by ink bounds, at the toolbar button height) — so a label sitting next to `overlay_button`
/// dropdowns lines up with them instead of riding high/low.
pub(super) fn toolbar_label(ui: &mut egui::Ui, text: &str) {
    let h = dropdown_height(ui);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let col = ui.visuals().text_color();
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, col);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(galley.size().x, h), egui::Sense::hover());
    paint_ink_centered(ui.painter(), rect, galley, col);
}

// ---------------------------------------------------------------------------------------
// The modal shell.
// ---------------------------------------------------------------------------------------

/// A modal's state: whatever the dialog itself needs, plus the error slot the shell owns.
///
/// Held by [`App`] as `Option<ModalState<_>>`, so `Some` == open — the one invariant every
/// modal obeys. The error is here rather than in the inner state because it is the shell that
/// sets it (from a failed commit) and the shell that renders it, and because a modal that
/// *can't* fail then gets the same treatment for free if it ever grows a fallible commit.
pub(super) struct ModalState<S> {
    pub(super) inner: S,
    /// Why the last commit attempt failed, rendered in place above the button row. The body
    /// may clear it when the input that failed changes (e.g. a different file is chosen).
    pub(super) error: Option<String>,
}

impl<S> ModalState<S> {
    pub(super) fn new(inner: S) -> Self {
        Self { inner, error: None }
    }
}

/// The fixed presentation of one modal: its egui id, width, heading, and the label of its
/// affirmative button.
pub(super) struct ModalSpec<'a> {
    pub(super) id: &'a str,
    pub(super) width: f32,
    pub(super) heading: &'a str,
    /// Label of the affirmative button — "Load", "Delete", "Rename", "Save…".
    pub(super) commit: &'a str,
}

/// What a modal's body reports back for this frame.
pub(super) struct ModalBody {
    /// Whether the affirmative button is enabled (e.g. no file chosen yet → not yet).
    pub(super) can_commit: bool,
    /// Set by a body that decides the outcome itself — the rename field commits on Enter.
    /// Leave `None` to let the footer buttons decide.
    pub(super) action: Option<DialogAction>,
}

impl ModalBody {
    /// The common case: only the footer decides, and the commit button is enabled iff `ok`.
    pub(super) fn enabled(ok: bool) -> Self {
        Self { can_commit: ok, action: None }
    }
}

/// What a modal decided this frame.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DialogAction {
    /// Nothing decided — stay open.
    Keep,
    /// Dismissed: Cancel, Escape, or a click on the backdrop.
    Cancel,
    /// The affirmative button (or Enter) fired.
    Commit,
}

/// Draw one of the app's transient modals, and run its commit.
///
/// Owns everything the five modals had each hand-rolled: the take-from-`Option` (which is
/// what frees `&mut App` for the commit), the fixed width, the heading, the error line, the
/// separator and the commit/cancel row, `Modal::should_close()`, **Escape**, and the
/// fourth state the load and docking dialogs both open-coded — a commit that *fails* reopens
/// the dialog with the reason, so the input can be corrected without starting over.
///
/// Centralising `should_close()` and Escape is the behaviour fix here: the image dialog
/// discarded its `ModalResponse` (so it alone had no backdrop-click dismissal) and both it
/// and the settings window only *peeked* at Escape without consuming it, which made
/// dismissal order-dependent when more than one was open. Now every modal consumes Escape.
///
/// `slot` is a field projection (`|a| &mut a.load_dialog`), so the shell can put the state
/// back after the commit has had full `&mut App`. `body` gets `&App` for the read-only scene
/// facts a dialog displays ("Into <name> (N atoms)", the frame count, the viewport size).
pub(super) fn modal_shell<S>(
    app: &mut App,
    ctx: &egui::Context,
    slot: fn(&mut App) -> &mut Option<ModalState<S>>,
    spec: ModalSpec<'_>,
    body: impl FnOnce(&mut egui::Ui, &mut S, &mut Option<String>, &App) -> ModalBody,
    commit: impl FnOnce(&mut App, &mut S) -> Result<(), String>,
) {
    let Some(mut state) = slot(app).take() else {
        return;
    };
    let mut action = DialogAction::Keep;

    let modal = egui::Modal::new(egui::Id::new(spec.id)).show(ctx, |ui| {
        ui.set_width(spec.width);
        ui.heading(spec.heading);
        let ModalState { inner, error } = &mut state;
        let reported = body(ui, inner, error, app);
        if let Some(a) = reported.action {
            action = a;
        }
        if let Some(e) = error.as_deref() {
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(egui::RichText::new(e).color(ui.visuals().error_fg_color)).wrap(),
            );
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(reported.can_commit, egui::Button::new(spec.commit))
                .clicked()
            {
                action = DialogAction::Commit;
            }
            if ui.button("Cancel").clicked() {
                action = DialogAction::Cancel;
            }
        });
    });

    // A backdrop click or Escape dismisses. Escape is **consumed** (`key_pressed` on the
    // mutable input) so it dismisses exactly one modal, not every one that happens to be open.
    if modal.should_close() || ctx.input_mut(|i| i.key_pressed(egui::Key::Escape)) {
        action = DialogAction::Cancel;
    }

    *slot(app) = resolve_modal(action, state, |inner| commit(app, inner));
}

/// Decide a modal's fate from its `action`: the state to store back, or `None` to close.
///
/// Split out of [`modal_shell`] because it is the whole of the shell's *logic* — the four
/// states, including the commit that fails and reopens — and it is testable without an
/// [`App`] (which needs a live wgpu device). `commit` takes only the inner state so the
/// caller can close over `&mut App` here.
fn resolve_modal<S>(
    action: DialogAction,
    mut state: ModalState<S>,
    commit: impl FnOnce(&mut S) -> Result<(), String>,
) -> Option<ModalState<S>> {
    match action {
        DialogAction::Keep => Some(state),
        DialogAction::Cancel => None,
        DialogAction::Commit => match commit(&mut state.inner) {
            Ok(()) => None,
            // Reopen showing why, so the input can be corrected without starting over.
            Err(e) => {
                state.error = Some(e);
                Some(state)
            }
        },
    }
}

/// The app's standard **tab bar**: underline-style tabs (the selected tab is bold
/// with an accent underline; the others are weak, clickable text) instead of
/// disconnected toggle buttons. Sets `*current` to the clicked tab and returns
/// whether the selection changed. Use this for *all* tabbed UIs so they look the
/// same (rep settings, the delete-frames dialog, …).
pub(super) fn tab_bar<T: Copy + PartialEq>(ui: &mut egui::Ui, current: &mut T, tabs: &[(T, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        for &(tab, label) in tabs {
            let selected = *current == tab;
            let txt = if selected {
                egui::RichText::new(label).strong()
            } else {
                egui::RichText::new(label).color(ui.visuals().weak_text_color())
            };
            let resp = ui
                .add(egui::Label::new(txt).sense(egui::Sense::click()))
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() && !selected {
                *current = tab;
                changed = true;
            }
            if selected {
                let r = resp.rect;
                ui.painter().hline(
                    r.x_range(),
                    r.bottom() + 2.0,
                    egui::Stroke::new(2.0_f32, ui.visuals().selection.bg_fill),
                );
            }
        }
    });
    changed
}

/// A value slider with a numeric edit box beside it (the "[slider] [edit]"
/// pattern), both bound to the same value; `enabled` greys both out.
pub(super) fn slider_with_edit(
    ui: &mut egui::Ui,
    v: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    enabled: bool,
) {
    ui.horizontal(|ui| {
        ui.add_enabled(
            enabled,
            egui::Slider::new(v, range.clone()).show_value(false),
        );
        ui.add_enabled(
            enabled,
            egui::DragValue::new(v).speed(0.01).range(range).fixed_decimals(2),
        );
    });
}

pub(super) const COLOR_PICKER_WIDTH: f32 = 230.0;
pub(super) const COLOR_SWATCH_GAP: f32 = 6.0;
pub(super) const COLOR_SWATCH_SIZE: f32 = (COLOR_PICKER_WIDTH - 5.0 * COLOR_SWATCH_GAP) / 6.0;

pub(super) fn set_color_picker_palette(ctx: &egui::Context, palette: &[[u8; 4]]) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("color_picker_palette"), palette.to_vec()));
}

/// Shared popup contents for every color selector: the palette and the free picker.
pub(super) fn color_picker_contents(
    ui: &mut egui::Ui,
    color: &mut egui::Color32,
    palette: Option<&[[u8; 4]]>,
) -> bool {
    let before = *color;
    ui.set_width(COLOR_PICKER_WIDTH);
    let cached = ui.ctx().data(|data| data.get_temp::<Vec<[u8; 4]>>(egui::Id::new("color_picker_palette")))
        .unwrap_or_else(|| crate::color::ColorPreferences::default().discrete_palette().to_vec());
    let mut rgba = color.to_array();
    if solid_presets(ui, palette.unwrap_or(&cached), &mut rgba) {
        *color = egui::Color32::from_rgb(rgba[0], rgba[1], rgba[2]);
    }
    ui.separator();
    free_color_picker(ui, color);
    *color != before
}

/// A small clickable color swatch in the `Solid` submenu grid. Highlights on
/// hover and rings the currently-selected color.
pub(super) fn swatch_button(ui: &mut egui::Ui, c: [u8; 4], selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::Vec2::splat(COLOR_SWATCH_SIZE), egui::Sense::click());
    ui.painter()
        .rect_filled(rect, 2.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
    let stroke = if selected {
        egui::Stroke::new(2.0_f32, ui.visuals().selection.bg_fill)
    } else if resp.hovered() {
        egui::Stroke::new(1.5_f32, ui.visuals().widgets.hovered.fg_stroke.color)
    } else {
        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(80))
    };
    ui.painter()
        .rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
    resp.on_hover_text(format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]))
        .clicked()
}

pub(super) fn solid_presets(ui: &mut egui::Ui, palette: &[[u8; 4]], current: &mut [u8; 4]) -> bool {
    let mut changed = false;
    egui::Grid::new("solid_presets")
        .min_col_width(COLOR_SWATCH_SIZE)
        .min_row_height(COLOR_SWATCH_SIZE)
        .spacing(egui::Vec2::splat(COLOR_SWATCH_GAP))
        .show(ui, |ui| {
            for (index, &color) in palette.iter().enumerate() {
                if swatch_button(ui, color, color == *current) {
                    *current = color;
                    changed = true;
                }
                if (index + 1) % 6 == 0 {
                    ui.end_row();
                }
            }
        });
    changed
}

fn color_gradient_mesh(
    rect: egui::Rect,
    columns: u32,
    rows: u32,
    color_at: impl Fn(f32, f32) -> egui::Color32,
) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    for y in 0..=rows {
        for x in 0..=columns {
            let u = x as f32 / columns as f32;
            let v = y as f32 / rows as f32;
            mesh.colored_vertex(rect.min + rect.size() * egui::vec2(u, v), color_at(u, v));
            if x < columns && y < rows {
                let i = y * (columns + 1) + x;
                mesh.add_triangle(i, i + 1, i + columns + 1);
                mesh.add_triangle(i + 1, i + columns + 2, i + columns + 1);
            }
        }
    }
    mesh
}

/// KDE-style hue/saturation field and a vertical brightness slider, with RGB editing
/// and a preview. Remember hue/saturation while the chosen color is black/gray.
pub(super) fn free_color_picker(ui: &mut egui::Ui, color: &mut egui::Color32) -> bool {
    use egui::ecolor::HsvaGamma;
    let before = *color;
    let mut rgb = [color.r(), color.g(), color.b()];
    let mut rgb_changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (label, channel) in ["R", "G", "B"].into_iter().zip(rgb.iter_mut()) {
            ui.label(label);
            rgb_changed |= ui.add_sized(
                [45.0, dropdown_height(ui)],
                egui::DragValue::new(channel).range(0..=255).speed(1.0),
            ).changed();
        }
    });
    if rgb_changed {
        *color = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    }
    let (preview, _) = ui.allocate_exact_size(
        egui::vec2(COLOR_PICKER_WIDTH, 16.0), egui::Sense::hover(),
    );
    let (rect, mut response) = ui.allocate_exact_size(
        egui::vec2(COLOR_PICKER_WIDTH, 200.0), egui::Sense::click_and_drag(),
    );
    let cache_id = response.id.with("hsv");
    let mut hsv = ui.ctx().data(|data| data.get_temp::<(egui::Color32, HsvaGamma)>(cache_id))
        .filter(|(cached, _)| *cached == *color)
        .map(|(_, hsv)| hsv)
        .unwrap_or_else(|| HsvaGamma::from(*color));
    hsv.a = 1.0;
    let field = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right() - 28.0, rect.bottom()));
    let value_bar = egui::Rect::from_min_max(
        egui::pos2(field.right() + 8.0, rect.top()),
        egui::pos2(rect.right() - 6.0, rect.bottom()),
    );
    if let Some(pointer) = response.interact_pointer_pos() {
        let origin = ui.input(|input| input.pointer.press_origin()).unwrap_or(pointer);
        if origin.x > field.right() + 4.0 {
            hsv.v = (1.0 - (pointer.y - value_bar.top()) / value_bar.height()).clamp(0.0, 1.0);
        } else {
            hsv.h = (1.0 - (pointer.x - field.left()) / field.width()).clamp(0.0, 1.0);
            hsv.s = (1.0 - (pointer.y - field.top()) / field.height()).clamp(0.0, 1.0);
        }
        *color = hsv.into();
    }
    ui.ctx().data_mut(|data| data.insert_temp(cache_id, (*color, hsv)));
    ui.painter().rect_filled(preview, 0.0, *color);
    if ui.is_rect_visible(rect) {
        ui.painter().add(egui::Shape::mesh(color_gradient_mesh(field, 48, 24, |x, y| {
            HsvaGamma { h: 1.0 - x, s: 1.0 - y, ..hsv }.into()
        })));
        ui.painter().add(egui::Shape::mesh(color_gradient_mesh(value_bar, 1, 32, |_, y| {
            HsvaGamma { v: 1.0 - y, ..hsv }.into()
        })));
        for area in [field, value_bar] {
            ui.painter().rect_stroke(area, 0.0, ui.visuals().widgets.noninteractive.bg_stroke,
                egui::StrokeKind::Inside);
        }
        let center = field.min + field.size() * egui::vec2(1.0 - hsv.h, 1.0 - hsv.s);
        let painter = ui.painter().with_clip_rect(field.intersect(ui.clip_rect()));
        for (width, ink) in [(3.0_f32, egui::Color32::WHITE), (1.0_f32, egui::Color32::BLACK)] {
            for delta in [egui::vec2(6.0, 0.0), egui::vec2(0.0, 6.0)] {
                painter.line_segment([center - delta, center + delta], egui::Stroke::new(width, ink));
            }
        }
        let y = value_bar.bottom() - hsv.v * value_bar.height();
        ui.painter().add(egui::Shape::convex_polygon(
            vec![egui::pos2(value_bar.right(), y), egui::pos2(rect.right(), y - 4.0),
                egui::pos2(rect.right(), y + 4.0)],
            ui.visuals().text_color(), egui::Stroke::NONE,
        ));
    }
    if *color != before {
        response.mark_changed();
    }
    response.changed()
}

#[cfg(test)]
mod free_color_picker_tests {
    use super::*;

    #[test]
    fn index_badges_keep_rows_aligned_on_hover() {
        for theme in [crate::settings::ThemeMode::Light, crate::settings::ThemeMode::Dark] {
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx, &crate::settings::AppearanceSettings {
                theme, ..Default::default()
            });
            for visible in [false, true] {
                let frame = |pointer| {
                    let mut rects = Vec::new();
                    let _ = ctx.run_ui(egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO, egui::vec2(500.0, 300.0),
                        )),
                        events: vec![egui::Event::PointerMoved(pointer)],
                        ..Default::default()
                    }, |ui| {
                        for prefix in ["m", "r"] {
                            ui.horizontal(|ui| {
                                rects.push(index_badge(ui, prefix, 0, visible).rect);
                                let mut selection = "all".to_owned();
                                rects.push(super::super::rep_panel::sel_text_edit(
                                    ui, &mut selection, egui::Id::new(prefix), 150.0, None,
                                ).rect);
                            });
                        }
                        ui.horizontal(|ui| {
                            rects.push(molecule_expander(ui, visible).rect);
                            rects.push(ui.label("Molecule name").rect);
                        });
                    });
                    rects
                };
                let corner = egui::pos2(499.0, 299.0);
                frame(corner);
                let resting = frame(corner);
                assert_eq!(resting[0].height(), resting[2].height());
                assert_eq!(resting[0].height(), resting[1].height());
                assert_eq!(resting[2].height(), resting[3].height());
                assert_eq!(resting[0].height(), resting[4].height());
                for index in [0, 2, 4] {
                    frame(resting[index].center());
                    assert_eq!(resting, frame(resting[index].center()),
                        "{theme:?}, visible={visible}: hovering a badge shifts the row");
                }
            }
        }
    }

    #[test]
    fn menus_size_to_their_content_on_a_wide_screen() {
        for menu in ["style", "color", "edit"] {
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
            for _ in 0..3 {
                let _ = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0))),
                    ..Default::default()
                }, |ui| {
                    let header = ui.button("Menu");
                    egui::Popup::open_id(&ctx, egui::Popup::default_response_id(&header));
                    let popup = egui::Popup::menu(&header).show(|ui| {
                        let spacing = ui.spacing().item_spacing.y;
                        match menu {
                            "style" => for kind in RepKind::ALL { super::super::pickers::style_option(ui, kind, false); },
                            "color" => for method in ColorMethod::ALL { super::super::pickers::color_option(ui, method, false, false); },
                            _ => {
                                history_menu_item(ui, false, icon::ARROW_COUNTER_CLOCKWISE, "Undo", "Ctrl+Z", None);
                                history_menu_item(ui, false, icon::ARROW_CLOCKWISE, "Redo", "Ctrl+Shift+Z", None);
                                ui.separator();
                                let _ = ui.button("Settings…");
                            }
                        }
                        assert_eq!(ui.spacing().item_spacing.y, spacing, "normal list spacing must survive");
                    }).unwrap();
                    assert!(popup.response.rect.width() < 260.0,
                        "{menu} menu expanded to {}", popup.response.rect.width());
                });
            }
        }
    }

    #[test]
    fn shared_color_picker_displays_and_selects_the_active_palette() {
        let ctx = egui::Context::default();
        let mut palette = crate::color::ColorPreferences::default().discrete_palette().to_vec();
        palette[0] = [11, 122, 250, 255];
        set_color_picker_palette(&ctx, &palette);
        let mut color = egui::Color32::BLACK;
        let frame = |color: &mut egui::Color32, events| ctx.run_ui(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 700.0))),
            events, ..Default::default()
        }, |ui| { color_picker_contents(ui, color, None); });
        let output = frame(&mut color, vec![]);
        let swatches: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) if (rect.rect.width() - COLOR_SWATCH_SIZE).abs() < 0.1
                && (rect.rect.height() - COLOR_SWATCH_SIZE).abs() < 0.1 && rect.fill.a() > 0 =>
                Some((rect.rect.center(), rect.fill.to_array())),
            _ => None,
        }).collect();
        assert_eq!(swatches.iter().map(|(_, rgba)| *rgba).collect::<Vec<_>>(), palette);
        let pos = swatches[0].0;
        for pressed in [true, false] {
            frame(&mut color, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            }]);
        }
        assert_eq!(color.to_array(), palette[0]);
    }

    #[test]
    fn history_description_is_small_and_clicks_the_same_menu_item() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let frame = |clicked: &mut bool, events| ctx.run_ui(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
            events, ..Default::default()
        }, |ui| {
            *clicked |= history_menu_item(ui, true, icon::ARROW_COUNTER_CLOCKWISE, "Undo", "Ctrl+Z",
                Some("toggle representation")).clicked();
        });
        let mut clicked = false;
        let output = frame(&mut clicked, vec![]);
        let text = |label: &str| output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text().contains(label) =>
                Some(egui::Rect::from_min_size(text.pos, text.galley.size())),
            _ => None,
        }).unwrap();
        let title = text("Undo");
        let shortcut = text("Ctrl+Z");
        let description = text("toggle representation");
        assert!((title.center().y - shortcut.center().y).abs() < 0.1);
        assert!(description.top() > title.bottom());
        assert!(description.height() < title.height());
        let pos = description.center();
        for pressed in [true, false] {
            frame(&mut clicked, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            }]);
        }
        assert!(clicked);
    }

    #[test]
    fn color_field_and_brightness_keep_the_chosen_hue_through_black() {
        for theme in [crate::settings::ThemeMode::Light, crate::settings::ThemeMode::Dark] {
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx, &crate::settings::AppearanceSettings {
                theme, ..Default::default()
            });
            let mut color = egui::Color32::WHITE;
            let frame = |color: &mut egui::Color32, events| {
                ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 500.0))),
                    events, ..Default::default()
                }, |ui| {
                    free_color_picker(ui, color);
                    assert!((ui.min_rect().width() - COLOR_PICKER_WIDTH).abs() < 0.1);
                })
            };
            let output = frame(&mut color, vec![]);
            let gradients: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) => Some(mesh.calc_bounds()),
                _ => None,
            }).collect();
            assert_eq!(gradients.len(), 2);
            let (field, brightness) = (gradients[0], gradients[1]);
            assert!((brightness.right() + 6.0 - field.left() - COLOR_PICKER_WIDTH).abs() < 0.1);
            let click = |color: &mut egui::Color32, pos| {
                for pressed in [true, false] {
                    frame(color, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                        pos, button: egui::PointerButton::Primary, pressed,
                        modifiers: egui::Modifiers::NONE,
                    }]);
                }
            };
            click(&mut color, egui::pos2(field.center().x, field.top() + 0.01));
            assert!(color.r() < 80 && color.g() > 250 && color.b() > 250);
            click(&mut color, egui::pos2(brightness.center().x, brightness.bottom() - 0.001));
            assert_eq!(color, egui::Color32::BLACK);
            click(&mut color, egui::pos2(field.left() + field.width() * 2.0 / 3.0, field.top() + 0.01));
            assert_eq!(color, egui::Color32::BLACK);
            click(&mut color, egui::pos2(brightness.center().x, brightness.top() + 0.01));
            assert!(color.g() > 250 && color.r() < 80 && color.b() < 80);
        }
    }
}

/// A color "selector": a swatch button that opens (on click, downward) a popup
/// holding a full color picker. A nested `Popup::menu` so it stays within the parent
/// `CloseOnClickOutside` menu's hierarchy, and `CloseOnClickOutside` itself so
/// dragging the picker doesn't dismiss it. `c` is linear RGBA 0..1; the picker works
/// in sRGB `Color32`, converted through `egui::Rgba` so the swatch is WYSIWYG.
pub(super) fn color_submenu(ui: &mut egui::Ui, _id: &str, c: &mut [f32; 4]) {
    let mut col: egui::Color32 =
        egui::Rgba::from_rgba_unmultiplied(c[0], c[1], c[2], 1.0).into();
    let header = ui.add(egui::Button::new("").fill(col).min_size(egui::vec2(30.0, 16.0)));
    egui::Popup::menu(&header)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            // Fixed width so the picker doesn't resize as its contents change.
            ui.set_width(COLOR_PICKER_WIDTH);
            if color_picker_contents(ui, &mut col, None) {
                let lin = egui::Rgba::from(col);
                *c = [lin.r(), lin.g(), lin.b(), 1.0];
            }
        });
}


/// The modal shell's four states, tested without an [`App`] (which needs a live wgpu device):
/// [`resolve_modal`] is the whole of the shell's logic, and the state that matters most is the
/// fourth — a commit that fails must leave the dialog **open, with the reason**, so the input
/// can be corrected instead of the work being thrown away. Both the load-trajectory and
/// load-docking dialogs hand-rolled that before; now there is one implementation to check.
#[cfg(test)]
mod modal_shell_tests {
    use super::*;

    /// A stand-in dialog state: a value the commit reads, and a record of it having run.
    #[derive(Default)]
    struct Probe {
        value: u32,
        committed: bool,
    }

    #[test]
    fn keep_holds_the_state_and_does_not_commit() {
        let out = resolve_modal(DialogAction::Keep, ModalState::new(Probe { value: 7, ..Default::default() }), |p| {
            p.committed = true;
            Ok(())
        });
        let out = out.expect("Keep must stay open");
        assert_eq!(out.inner.value, 7, "the edit buffer must survive the frame");
        assert!(!out.inner.committed, "Keep must not run the commit");
        assert!(out.error.is_none());
    }

    #[test]
    fn cancel_closes_and_does_not_commit() {
        let mut ran = false;
        let out = resolve_modal(DialogAction::Cancel, ModalState::new(Probe::default()), |_| {
            ran = true;
            Ok(())
        });
        assert!(out.is_none(), "Cancel must close");
        assert!(!ran, "Cancel must not run the commit");
    }

    #[test]
    fn a_successful_commit_runs_and_closes() {
        let mut seen = 0;
        let out = resolve_modal(DialogAction::Commit, ModalState::new(Probe { value: 42, ..Default::default() }), |p| {
            seen = p.value;
            Ok(())
        });
        assert!(out.is_none(), "a successful commit must close the dialog");
        assert_eq!(seen, 42, "the commit must see the dialog's state");
    }

    /// The fourth state: reopen with the message, keeping everything the user had entered.
    #[test]
    fn a_failed_commit_reopens_with_the_error() {
        let out = resolve_modal(DialogAction::Commit, ModalState::new(Probe { value: 3, ..Default::default() }), |_| {
            Err("no such file".to_string())
        });
        let out = out.expect("a failed commit must leave the dialog open");
        assert_eq!(out.error.as_deref(), Some("no such file"));
        assert_eq!(out.inner.value, 3, "the entered state must be preserved for correcting");
    }

    /// A retry that succeeds must clear the previous failure's message along with the dialog —
    /// i.e. the error rides the state, so closing disposes of it.
    #[test]
    fn a_retry_that_succeeds_closes_despite_a_stale_error() {
        let mut state = ModalState::new(Probe { value: 1, ..Default::default() });
        state.error = Some("first attempt failed".into());
        let out = resolve_modal(DialogAction::Commit, state, |_| Ok(()));
        assert!(out.is_none(), "a successful retry must close, discarding the stale error");
    }
}
