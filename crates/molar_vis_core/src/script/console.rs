//! The in-app scripting console: its scrollback/input state ([`ScriptConsole`]) and
//! the egui window that draws it ([`show`]). The window is pure UI — it returns the
//! submitted source line; `App::draw_console` feeds that to `App::run_script`.

/// One line of console scrollback.
pub struct ConsoleLine {
    pub kind: LineKind,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// The command the user submitted (echoed).
    Input,
    /// `print`/`list` output (or an informational message).
    Output,
    /// An evaluation or apply error.
    Error,
}

/// State of the scripting console (open/closed lives on `App` as `console_open`).
pub struct ScriptConsole {
    pub lines: Vec<ConsoleLine>,
    pub input: String,
    /// Submitted lines, for Up/Down recall.
    pub history: Vec<String>,
    /// Cursor into `history` while recalling (`None` = editing a fresh line).
    pub hist_cursor: Option<usize>,
    /// Set when the console is opened so [`show`] grabs the input field next frame.
    pub focus_input: bool,
}

impl Default for ScriptConsole {
    fn default() -> Self {
        Self {
            lines: vec![ConsoleLine {
                kind: LineKind::Output,
                text: "molar_vis console (Rhai). e.g. mol(0).rep(0).set_color(\"chain\"); list()"
                    .to_string(),
            }],
            input: String::new(),
            history: Vec::new(),
            hist_cursor: None,
            focus_input: false,
        }
    }
}

impl ScriptConsole {
    /// Replace the input with the previous history entry (Up).
    fn recall_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let i = match self.hist_cursor {
            Some(0) => 0,
            Some(i) => i - 1,
            None => self.history.len() - 1,
        };
        self.hist_cursor = Some(i);
        self.input = self.history[i].clone();
    }

    /// Step forward through history toward the fresh line (Down).
    fn recall_next(&mut self) {
        match self.hist_cursor {
            Some(i) if i + 1 < self.history.len() => {
                self.hist_cursor = Some(i + 1);
                self.input = self.history[i + 1].clone();
            }
            Some(_) => {
                self.hist_cursor = None;
                self.input.clear();
            }
            None => {}
        }
    }
}

/// Draw the console as a **resizable bottom panel** inside `ui` (so the viewport
/// fills the space above it). Returns `Some(source)` when the user submitted a line
/// this frame (Enter or the Run button). The input row is pinned to the bottom via a
/// nested panel so it stays visible and editable at any panel height.
pub fn show(ui: &mut egui::Ui, open: &mut bool, console: &mut ScriptConsole) -> Option<String> {
    let mut submitted = None;
    egui::Panel::bottom("script_console")
        .resizable(true)
        .default_size(190.0)
        .size_range(egui::Rangef::new(96.0, 500.0))
        .show_inside(ui, |ui| {
            // Header: title + close button (phosphor X, not a tofu glyph).
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Console").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(egui_phosphor::regular::X)
                        .on_hover_text("Close console")
                        .clicked()
                    {
                        *open = false;
                    }
                });
            });
            // Input row pinned to the bottom via a nested bottom panel (the structure
            // that keeps the outer panel at its set height — computing a scroll height
            // from `available_height` instead fed back and blew the panel up). The
            // field is `add_sized` to leave room for the Run (↵) button on its right —
            // a plain left→right row, no `right_to_left`/INFINITY (which broke sizing).
            egui::Panel::bottom("script_console_input").show_inside(ui, |ui| {
                ui.add_space(3.0);
                let mut submit = false;
                ui.horizontal(|ui| {
                    let btn_w = 30.0;
                    let row_h = ui.spacing().interact_size.y;
                    let field_w = (ui.available_width() - btn_w - ui.spacing().item_spacing.x).max(60.0);
                    let resp = ui.add_sized(
                        [field_w, row_h],
                        egui::TextEdit::singleline(&mut console.input)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("command…  e.g. mol(0).add_rep(\"cartoon\")"),
                    );
                    submit = ui
                        .add_sized([btn_w, row_h], egui::Button::new(egui_phosphor::regular::ARROW_ELBOW_DOWN_LEFT))
                        .on_hover_text("Run (Enter)")
                        .clicked();
                    // Grab focus the frame the console was opened from the View menu.
                    if console.focus_input {
                        resp.request_focus();
                        console.focus_input = false;
                    }
                    // ↑/↓ recall history while focused (singleline ignores them otherwise).
                    if resp.has_focus() {
                        let (up, down) = ui.input(|i| {
                            (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown))
                        });
                        if up {
                            console.recall_prev();
                        } else if down {
                            console.recall_next();
                        }
                    }
                    // Detect Enter BEFORE re-requesting focus (request_focus would re-grab
                    // focus this frame and mask the Enter-induced lost_focus()).
                    let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (entered || submit) && !console.input.trim().is_empty() {
                        let src = console.input.trim().to_string();
                        console.history.push(src.clone());
                        console.hist_cursor = None;
                        console.input.clear();
                        submitted = Some(src);
                    }
                    // Keep the field focused so the user can keep typing commands.
                    if entered || submit {
                        resp.request_focus();
                    }
                });
                ui.add_space(2.0);
            });

            // Scrollback fills the remaining space above the input (newest at bottom).
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &console.lines {
                        let (prefix, color) = match line.kind {
                            LineKind::Input => ("> ", egui::Color32::from_gray(150)),
                            LineKind::Output => ("", egui::Color32::from_gray(220)),
                            LineKind::Error => ("", egui::Color32::from_rgb(240, 120, 120)),
                        };
                        ui.label(
                            egui::RichText::new(format!("{prefix}{}", line.text))
                                .monospace()
                                .color(color),
                        );
                    }
                });
        });
    submitted
}
