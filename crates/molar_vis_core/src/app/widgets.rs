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

/// Widest of `labels` in the picker-button font. A picker button reserves this so it
/// keeps a **constant width** as the selection changes — a wider label must not grow
/// the button and reflow/resize the whole panel. Measured once per picker per frame.
pub(super) fn max_label_width<'a>(ui: &egui::Ui, labels: impl Iterator<Item = &'a str>) -> f32 {
    let txt = ui.visuals().text_color();
    labels
        .map(|l| {
            ui.painter()
                .layout_no_wrap(l.to_owned(), egui::FontId::proportional(14.0), txt)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max)
}

/// A dropdown button showing a drawn icon + a text label + a caret. `label_w` is the
/// width reserved for the label (pass [`max_label_width`] of all options so the button
/// doesn't change size with the selection). `draw_icon` paints into the given rect;
/// returns the click response (drive a `Popup` off it).
pub(super) fn picker_button(
    ui: &mut egui::Ui,
    label: &str,
    label_w: f32,
    draw_icon: impl FnOnce(&egui::Painter, egui::Rect),
) -> egui::Response {
    // `PLACEHOLDER` now, real ink once the state is known (see `overlay_button`).
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(14.0),
        egui::Color32::PLACEHOLDER,
    );
    let (icon_w, caret_w, pad, gap) = (26.0_f32, 11.0_f32, 5.0_f32, 5.0_f32);
    // Reserve the widest option's label width (fixed button size); the current label is
    // drawn left-aligned within it.
    let w = pad + icon_w + gap + label_w.max(galley.size().x) + gap + caret_w + pad;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 20.0), egui::Sense::click());
    // Painted as a **button** at rest, not just on hover: it *is* a dropdown, and against a
    // panel of the same colour there was nothing to click on until the cursor found it. The
    // fills come from the widget state, so it matches every other button in either theme.
    let vis = ui.style().interact(&resp);
    ui.painter().rect_filled(rect, 3.0, vis.weak_bg_fill);
    if vis.bg_stroke.width > 0.0 {
        ui.painter()
            .rect_stroke(rect, 3.0, vis.bg_stroke, egui::StrokeKind::Inside);
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
    ui.painter().text(
        egui::pos2(rect.right() - pad - caret_w * 0.5, rect.center().y),
        egui::Align2::CENTER_CENTER,
        icon::CARET_DOWN,
        egui::FontId::proportional(10.0),
        txt,
    );
    resp
}

/// One control in the viewport overlay toolbar: a fixed-height framed button
/// whose content (an icon glyph, or `label + caret`) is centered by its **ink**
/// bounds (`Galley::mesh_bounds`), not the font line-box. `ui.button` /
/// `selectable_label` center the line-box, so Phosphor glyphs with different
/// metrics (the depth-cue lines sat low, the cube high) looked vertically
/// ragged; ink-centering lines them up. `active` paints the selection fill
/// (toggle / open state). Returns the response — drive a `Popup::menu` off it.
pub(super) fn overlay_button(ui: &mut egui::Ui, content: &str, active: bool) -> egui::Response {
    const H: f32 = 26.0;
    const R: f32 = 4.0;
    let font = egui::TextStyle::Button.resolve(ui.style());
    // Laid out with `PLACEHOLDER`, which `Painter::galley` fills in from the colour passed
    // below — the ink colour isn't known yet (it depends on the button's own state, which
    // needs its `Response`), and an *active* button must take it from the selection rather
    // than the panel, or its glyph sits unreadably on the selection plate.
    let galley = ui
        .painter()
        .layout_no_wrap(content.to_owned(), font, egui::Color32::PLACEHOLDER);
    let w = (galley.size().x + 14.0).max(H);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, H), egui::Sense::click());
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

/// A plain text label vertically centered the same way [`overlay_button`] centers its glyph
/// (by ink bounds, at the toolbar button height) — so a label sitting next to `overlay_button`
/// dropdowns lines up with them instead of riding high/low.
pub(super) fn toolbar_label(ui: &mut egui::Ui, text: &str) {
    const H: f32 = 26.0;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let col = ui.visuals().text_color();
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, col);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(galley.size().x, H), egui::Sense::hover());
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
            ui.set_min_width(230.0);
            ui.set_max_width(230.0);
            if egui::color_picker::color_picker_color32(
                ui,
                &mut col,
                egui::color_picker::Alpha::Opaque,
            ) {
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
