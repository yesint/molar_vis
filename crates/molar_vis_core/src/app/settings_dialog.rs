//! Program-settings dialog: per-tab pages, apply, axes widget.
use super::*;
use super::widgets::*;


// --- Program-settings dialog: one function per tab (see `App::draw_settings_dialog`). ---

/// Black/white text chosen using sRGB relative luminance for maximum contrast.
fn swatch_ink(rgb: [u8; 4]) -> egui::Color32 {
    let linear = |c: u8| {
        let v = c as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2]);
    if luminance > 0.179 {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    }
}

fn color_popup(response: &egui::Response, rgba: &mut [u8; 4]) -> bool {
    let mut color = egui::Color32::from_rgb(rgba[0], rgba[1], rgba[2]);
    let mut changed = false;
    egui::Popup::menu(response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(COLOR_PICKER_WIDTH);
            if color_picker_contents(ui, &mut color, None) {
                *rgba = color.to_array();
                changed = true;
            }
        });
    changed
}

fn swatch_button(ui: &mut egui::Ui, rgba: [u8; 4], size: egui::Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    ui.painter().rect_filled(
        rect,
        3.0,
        egui::Color32::from_rgb(rgba[0], rgba[1], rgba[2]),
    );
    if response.hovered() || response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.5_f32, swatch_ink(rgba)),
            egui::StrokeKind::Inside,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn color_cell(ui: &mut egui::Ui, label: &str, rgba: &mut [u8; 4], size: egui::Vec2) -> bool {
    let response = swatch_button(ui, *rgba, size);
    ui.painter().text(
        response.rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(14.0),
        swatch_ink(*rgba),
    );
    color_popup(&response, rgba)
}

fn ss_icon(ui: &mut egui::Ui, index: usize, color: &mut [u8; 4]) {
    let response = swatch_button(ui, *color, egui::vec2(44.0, 30.0));
    let rect = response.rect.shrink(6.0);
    let painter = ui.painter();
    let stroke = egui::Stroke::new(3.0_f32, swatch_ink(*color));
    let point = |x: f32, y: f32| rect.min + egui::vec2(x * rect.width(), y * rect.height());
    if index < 3 {
        let points = (0..=32)
            .map(|i| {
                let t = i as f32 / 32.0;
                point(t, 0.5 + 0.3 * (t * std::f32::consts::TAU * 2.0).sin())
            })
            .collect();
        painter.add(egui::Shape::line(points, stroke));
    } else if index < 5 {
        painter.arrow(
            point(0.05, 0.5),
            egui::vec2(rect.width() * 0.85, 0.0),
            stroke,
        );
    } else {
        painter.add(egui::Shape::line(
            vec![
                point(0.0, 0.8),
                point(0.2, 0.3),
                point(0.45, 0.6),
                point(0.7, 0.2),
                point(1.0, 0.5),
            ],
            stroke,
        ));
    }
    color_popup(&response, color);
}

fn gradient_editor(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    labels: [&str; 3],
    gradient: &mut crate::color::Gradient,
) {
    ui.push_id(id, |ui| {
        if !title.is_empty() {
            ui.label(bold_name(ui, title));
        }
        let width = 400.0_f32.min(ui.available_width());
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 36.0), egui::Sense::hover());
        let mut handles = Vec::new();
        for i in 0..3 {
            let position = match i {
                0 => 0.0,
                1 => gradient.midpoint(),
                _ => 1.0,
            };
            let center = egui::pos2(
                rect.left() + 9.0 + (rect.width() - 18.0) * position,
                rect.center().y,
            );
            let response = ui.interact(
                egui::Rect::from_center_size(center, egui::vec2(18.0, rect.height())),
                ui.id().with(i),
                if i == 1 {
                    egui::Sense::click_and_drag()
                } else {
                    egui::Sense::click()
                },
            );
            if i == 1 && response.dragged() {
                if let Some(pos) = response.interact_pointer_pos() {
                    gradient.midpoint =
                        ((pos.x - rect.left() - 9.0) / (rect.width() - 18.0)).clamp(0.01, 0.99);
                }
            }
            handles.push(response);
        }
        for i in 0..128 {
            let c = gradient.sample(i as f32 / 127.0);
            let left = rect.left() + rect.width() * i as f32 / 128.0;
            let right = rect.left() + rect.width() * (i + 1) as f32 / 128.0;
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(left, rect.top()),
                    egui::pos2(right, rect.bottom()),
                ),
                0.0,
                egui::Color32::from_rgb(c[0], c[1], c[2]),
            );
        }
        for (i, response) in handles.into_iter().enumerate() {
            let position = match i {
                0 => 0.0,
                1 => gradient.midpoint(),
                _ => 1.0,
            };
            let center = egui::pos2(
                rect.left() + 9.0 + (rect.width() - 18.0) * position,
                rect.center().y,
            );
            let c = gradient.stops[i];
            let ink = swatch_ink(c);
            let handle = egui::Rect::from_center_size(center, egui::vec2(14.0, rect.height()));
            ui.painter()
                .rect_filled(handle, 0.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
            ui.painter().rect_stroke(
                handle,
                0.0,
                egui::Stroke::new(2.0_f32, ink),
                egui::StrokeKind::Inside,
            );
            if i == 1 {
                ui.painter().line_segment(
                    [center - egui::vec2(0.0, 5.0), center + egui::vec2(0.0, 5.0)],
                    egui::Stroke::new(1.0_f32, ink),
                );
            }
            let response = response
                .on_hover_text(if i == 1 {
                    format!(
                        "{} ({:.0}%). Drag to move; click to choose color.",
                        labels[i],
                        gradient.midpoint() * 100.0
                    )
                } else {
                    format!("{}: click to choose color", labels[i])
                })
                .on_hover_cursor(if i == 1 {
                    egui::CursorIcon::ResizeHorizontal
                } else {
                    egui::CursorIcon::PointingHand
                });
            color_popup(&response, &mut gradient.stops[i]);
        }
        ui.horizontal(|ui| {
            ui.label(labels[0]);
            ui.allocate_ui_with_layout(
                egui::vec2((width - 18.0).max(0.0), 18.0),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.label(labels[2]);
                },
            );
        });
    });
}

fn settings_page_colors(ui: &mut egui::Ui, settings: &mut Settings) {
    let colors = &mut settings.colors;
    color_preset_selector(ui, colors);
    ui.add_space(10.0);
    ui.label(bold_name(ui, "Elements"));
    ui.horizontal_wrapped(|ui| {
        for (number, symbol) in [
            (1, "H"),
            (6, "C"),
            (7, "N"),
            (8, "O"),
            (15, "P"),
            (16, "S"),
            (9, "F"),
            (17, "Cl"),
            (35, "Br"),
            (53, "I"),
            (5, "B"),
            (14, "Si"),
        ] {
            ui.push_id(number, |ui| {
                let mut rgba = colors.element(number);
                if color_cell(ui, symbol, &mut rgba, egui::vec2(34.0, 30.0)) {
                    colors.elements.insert(number, rgba);
                }
            });
        }
    });
    let window_id = ui.make_persistent_id("other_elements_window");
    let mut open = ui
        .ctx()
        .data_mut(|data| data.get_temp::<bool>(window_id).unwrap_or(false));
    if ui.button("Other elements...").clicked() {
        open = true;
    }
    egui::Window::new("Periodic table")
        .id(window_id)
        .open(&mut open)
        .resizable(false)
        .show(ui.ctx(), |ui| periodic_table(ui, colors));
    ui.ctx().data_mut(|data| data.insert_temp(window_id, open));
    ui.add_space(10.0);
    ui.label(bold_name(ui, "Secondary structure"));
    const LABELS: [&str; 9] = [
        "α helix",
        "3₁₀ helix",
        "π helix",
        "β sheet",
        "β bridge",
        "Turn",
        "Bend",
        "Polyproline",
        "Coil",
    ];
    egui::Grid::new("secondary_colors")
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            for (index, label) in LABELS.iter().enumerate() {
                ui.push_id(index, |ui| {
                    ss_icon(ui, index, &mut colors.secondary[index]);
                    ui.label(*label);
                });
                if index % 3 == 2 {
                    ui.end_row();
                }
            }
        });
    ui.add_space(10.0);
    ui.label(bold_name(ui, "Continuous palette"));
    gradient_editor(
        ui,
        "continuous_palette",
        "",
        ["0", "Midpoint", "1"],
        &mut colors.continuous,
    );
    ui.add_space(10.0);
    categorical_editor(ui, &mut colors.categorical);
    ui.add_space(8.0);
    if ui.button("Reset colors").clicked() {
        *colors = Default::default();
    }
}

fn color_preset_selector(ui: &mut egui::Ui, colors: &mut crate::color::ColorPreferences) {
    use crate::ColorPreset;
    let current = colors.preset();
    let mut selected = current;
    ui.horizontal(|ui| {
        ui.label(bold_name(ui, "Preset"));
        fixed_combo(ui, "color_preset", ColorPreset::ALL.iter().map(|preset| preset.name()).chain(["Custom"]))
            .selected_text(current.map_or("Custom", ColorPreset::name))
            .show_ui(ui, |ui| {
                for preset in ColorPreset::ALL {
                    ui.selectable_value(&mut selected, Some(preset), preset.name());
                }
            })
            .response
            .on_hover_text("Apply atom, secondary-structure and continuous colors");
    });
    if selected != current {
        if let Some(preset) = selected {
            preset.apply(colors);
        }
    }
}

fn categorical_editor(ui: &mut egui::Ui, palette: &mut Vec<[u8; 4]>) {
    ui.label(bold_name(ui, "Discrete palette"));
    let removable = palette.len() > 1;
    let mut remove = None;
    egui::Grid::new("categorical_palette_colors")
        .num_columns(16)
        .min_col_width(0.0)
        .spacing([4.0, 6.0])
        .show(ui, |ui| {
            for (index, color) in palette.iter_mut().enumerate() {
                ui.push_id(index, |ui| {
                    color_cell(ui, &(index + 1).to_string(), color, egui::vec2(34.0, 30.0));
                    if ui.add_enabled(removable, egui::Button::new("×").small()).on_hover_text("Remove color").clicked() {
                        remove = Some(index);
                    }
                });
                if index % 8 == 7 { ui.end_row(); }
            }
        });
    if let Some(index) = remove {
        palette.remove(index);
    }
    if ui.button("Add color").clicked() {
        palette.push(crate::color::categorical(palette.len()));
    }
}

fn periodic_table(ui: &mut egui::Ui, colors: &mut crate::color::ColorPreferences) {
    const SYMBOLS: [&str; 118] = [
        "H", "He", "Li", "Be", "B", "C", "N", "O", "F", "Ne", "Na", "Mg", "Al", "Si", "P", "S",
        "Cl", "Ar", "K", "Ca", "Sc", "Ti", "V", "Cr", "Mn", "Fe", "Co", "Ni", "Cu", "Zn", "Ga",
        "Ge", "As", "Se", "Br", "Kr", "Rb", "Sr", "Y", "Zr", "Nb", "Mo", "Tc", "Ru", "Rh", "Pd",
        "Ag", "Cd", "In", "Sn", "Sb", "Te", "I", "Xe", "Cs", "Ba", "La", "Ce", "Pr", "Nd", "Pm",
        "Sm", "Eu", "Gd", "Tb", "Dy", "Ho", "Er", "Tm", "Yb", "Lu", "Hf", "Ta", "W", "Re", "Os",
        "Ir", "Pt", "Au", "Hg", "Tl", "Pb", "Bi", "Po", "At", "Rn", "Fr", "Ra", "Ac", "Th", "Pa",
        "U", "Np", "Pu", "Am", "Cm", "Bk", "Cf", "Es", "Fm", "Md", "No", "Lr", "Rf", "Db", "Sg",
        "Bh", "Hs", "Mt", "Ds", "Rg", "Cn", "Nh", "Fl", "Mc", "Lv", "Ts", "Og",
    ];
    const PERIODS: [[u8; 18]; 9] = [
        [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2],
        [3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 6, 7, 8, 9, 10],
        [11, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 13, 14, 15, 16, 17, 18],
        [
            19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36,
        ],
        [
            37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54,
        ],
        [
            55, 56, 0, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86,
        ],
        [
            87, 88, 0, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118,
        ],
        [
            0, 0, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 0,
        ],
        [
            0, 0, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 0,
        ],
    ];
    egui::Grid::new("element_colors")
        .min_col_width(0.0)
        .max_col_width(34.0)
        .spacing([2.0, 2.0])
        .show(ui, |ui| {
            for period in PERIODS {
                for number in period {
                    ui.push_id(number, |ui| {
                        if number == 0 {
                            ui.allocate_exact_size(egui::vec2(34.0, 30.0), egui::Sense::hover());
                        } else {
                            let mut rgba = colors.element(number);
                            if color_cell(
                                ui,
                                SYMBOLS[number as usize - 1],
                                &mut rgba,
                                egui::vec2(34.0, 30.0),
                            ) {
                                colors.elements.insert(number, rgba);
                            }
                        }
                    });
                }
                ui.end_row();
            }
        });
}

/// Appearance tab: theme mode, UI font scale, accent color.
pub(super) fn settings_page_appearance(ui: &mut egui::Ui, s: &mut Settings) {
    let a = &mut s.appearance;
    egui::Grid::new("set_appearance")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Theme");
            fixed_combo(ui, "set_theme", ThemeMode::ALL.iter().map(|mode| mode.label()))
                .selected_text(a.theme.label())
                .show_ui(ui, |ui| {
                    for t in ThemeMode::ALL {
                        ui.selectable_value(&mut a.theme, t, t.label());
                    }
                });
            ui.end_row();

            ui.label("UI font scale");
            slider_with_edit(ui, &mut a.font_scale, 0.7..=1.6, true);
            ui.end_row();

            let accent_enabled = match a.theme {
                ThemeMode::Dark => true,
                ThemeMode::Light => false,
                ThemeMode::System => ui.ctx().system_theme().unwrap_or(ui.ctx().theme()) == egui::Theme::Dark,
            };
            ui.add_enabled(accent_enabled, egui::Label::new("Accent color"));
            ui.add_enabled_ui(accent_enabled, |ui| {
                color_submenu(ui, "set_accent", &mut a.accent);
            });
            ui.end_row();
        });
    ui.add_space(4.0);
}

/// Rendering tab: anti-aliasing (SSAA), shadow-map resolution, and periodic bonds.
pub(super) fn settings_page_rendering(ui: &mut egui::Ui, s: &mut Settings) {
    let r = &mut s.rendering;
    let ssaa_label = |n: u32| match n {
        1 => "Off (1×)",
        2 => "2× (default)",
        3 => "3×",
        4 => "4×",
        _ => "?",
    };
    egui::Grid::new("set_rendering")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Anti-aliasing").on_hover_text("Supersampling smooths the image at a cost proportional to this factor squared. Applies immediately.");
            fixed_combo(ui, "set_ssaa", [1, 2, 3, 4].into_iter().map(ssaa_label))
                .selected_text(ssaa_label(r.ssaa))
                .show_ui(ui, |ui| {
                    for n in [1u32, 2, 3, 4] {
                        ui.selectable_value(&mut r.ssaa, n, ssaa_label(n));
                    }
                });
            ui.end_row();

            ui.label("Camera transition animation [ms]")
                .on_hover_text("Duration of programmed camera moves, including zoom, focus and unobstructed view. Zero applies immediately. Mouse navigation stays direct.");
            ui.add(egui::DragValue::new(&mut r.camera_transition_ms).range(0..=u32::MAX).speed(25));
            ui.end_row();

            ui.label("Shadow-map resolution").on_hover_text("Higher resolutions improve cast-shadow detail. Used when cast shadows are enabled; applies immediately.");
            fixed_combo(ui, "set_shadow_res", ["1024²", "2048²", "4096²"].into_iter())
                .selected_text(format!("{}²", r.shadow_res))
                .show_ui(ui, |ui| {
                    for n in [1024u32, 2048, 4096] {
                        ui.selectable_value(&mut r.shadow_res, n, format!("{n}²"));
                    }
                });
            ui.end_row();
        });
    ui.add_space(4.0);
    ui.checkbox(&mut s.behavior.dashed_pbc_bonds, "Dashed wrap-around bonds")
        .on_hover_text(
            "Draw bonds that span a box face as dashed minimum-image half-bonds (and split \
             cartoon ribbons at the boundary). Off draws them as plain solid bonds. Applies to \
             the current scene.",
        );
}

/// Shared view-style controls for current-session edits and saved defaults.
pub(super) fn settings_page_view(
    ui: &mut egui::Ui,
    v: &mut crate::settings::ViewDefaults,
    page: &mut ViewPage,
    ray_supported: bool,
    scene_tex: Option<egui::TextureId>,
) {
    tab_bar(
        ui,
        page,
        &[
            (ViewPage::Camera, "Camera"),
            (ViewPage::Lighting, "Lighting"),
            (ViewPage::Scene, "Scene"),
        ],
    );
    ui.add_space(8.0);
    match page {
        ViewPage::Camera => {
            ui.label(bold_name(ui, "Projection and framing"));
            let proj_label = |p: Projection| match p {
                Projection::Perspective => "Perspective",
                Projection::Orthographic => "Orthographic",
            };
            egui::Grid::new("set_view")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Projection");
                    fixed_combo(ui, "set_proj", [Projection::Orthographic, Projection::Perspective].into_iter().map(proj_label))
                        .selected_text(proj_label(v.projection))
                        .show_ui(ui, |ui| {
                            for p in [Projection::Orthographic, Projection::Perspective] {
                                ui.selectable_value(&mut v.projection, p, proj_label(p));
                            }
                        });
                    ui.end_row();

                    ui.label("Frame fill").on_hover_text(
                    "Fraction of the viewport occupied after framing: 0.9 means 90%. Applies on the next zoom-to or Unobstructed View action; does not change the current zoom.",
                );
                    slider_with_edit(ui, &mut v.fill, 0.5..=1.0, true);
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.checkbox(&mut v.depth_cue.enabled, bold_name(ui, "Depth cue (fog)"));
            let cue_on = v.depth_cue.enabled;
            egui::Grid::new("set_cue")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.add_enabled(cue_on, egui::Label::new("Falloff"));
                    ui.add_enabled_ui(cue_on, |ui| {
                        fixed_combo(ui, "set_cue_mode", CueMode::ALL.iter().map(|mode| mode.label()))
                            .selected_text(v.depth_cue.mode.label())
                            .show_ui(ui, |ui| {
                                for m in CueMode::ALL {
                                    ui.selectable_value(&mut v.depth_cue.mode, m, m.label());
                                }
                            });
                    });
                    ui.end_row();
                    ui.add_enabled(cue_on, egui::Label::new("Strength"));
                    slider_with_edit(ui, &mut v.depth_cue.strength, 0.0..=1.0, cue_on);
                    ui.end_row();
                    ui.add_enabled(cue_on, egui::Label::new("Start"));
                    slider_with_edit(ui, &mut v.depth_cue.start, 0.0..=1.0, cue_on);
                    ui.end_row();
                });
        }
        ViewPage::Lighting => {
            ui.checkbox(&mut v.ao.enabled, bold_name(ui, "Ambient occlusion"));
            let ao_on = v.ao.enabled;
            egui::Grid::new("set_ao")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.add_enabled(ao_on, egui::Label::new("Strength"));
                    slider_with_edit(ui, &mut v.ao.strength, 0.0..=1.0, ao_on);
                    ui.end_row();
                    ui.add_enabled(ao_on, egui::Label::new("Radius (nm)"));
                    slider_with_edit(ui, &mut v.ao.radius, 0.05..=1.5, ao_on);
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.checkbox(&mut v.shadow.enabled, bold_name(ui, "Cast shadows"));
            let sh_on = v.shadow.enabled;
            egui::Grid::new("set_shadow")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.add_enabled(sh_on, egui::Label::new("Strength"));
                    slider_with_edit(ui, &mut v.shadow.strength, 0.0..=1.0, sh_on);
                    ui.end_row();
                });

            ui.add_enabled_ui(sh_on, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Softness");
                    slider_with_edit(
                        ui,
                        &mut v.shadow.softness,
                        crate::camera::Shadow::MIN_SOFTNESS..=1.0,
                        true,
                    );
                });
            });
            ui.add_space(8.0);
            ui.label(bold_name(ui, "Ray tracing")).on_hover_text(if ray_supported { "Press R in the viewport to ray-trace the view." } else { "Ray tracing is unavailable on this device." });
            ui.add_enabled_ui(ray_supported, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Global illumination");
                    slider_with_edit(ui, &mut v.gi, 0.0..=1.0, true);
                });
            });

        }
        ViewPage::Scene => {
            ui.horizontal(|ui| {
                ui.label(bold_name(ui, "Background"));
                ui.radio_value(&mut v.background.kind, BgKind::Solid, "Solid");
                ui.radio_value(&mut v.background.kind, BgKind::Gradient, "Gradient");
            });
            ui.horizontal(|ui| match v.background.kind {
                BgKind::Solid => {
                    ui.label("Color");
                    color_submenu(ui, "set_bg", &mut v.background.color);
                }
                BgKind::Gradient => {
                    ui.label("Top");
                    color_submenu(ui, "set_bg_top", &mut v.background.top);
                    ui.label("Bottom");
                    color_submenu(ui, "set_bg_bot", &mut v.background.bottom);
                }
            });

            ui.add_space(8.0);
            ui.label(bold_name(ui, "Orientation axes"));
            draw_axes_widget(ui, &mut v.axes_on, &mut v.axes_corner, scene_tex);
        }
    }
}

/// Representation preferences, applied to loaded and newly created representations.
pub(super) fn settings_page_reps(ui: &mut egui::Ui, s: &mut Settings) {
    let r = &mut s.reps;
    egui::Grid::new("set_reps")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Style");
            fixed_combo(ui, "set_rep_kind", RepKind::ALL.iter().map(|kind| kind.label()))
                .selected_text(r.kind.label())
                .show_ui(ui, |ui| {
                    for k in RepKind::ALL {
                        ui.selectable_value(&mut r.kind, k, k.label());
                    }
                });
            ui.end_row();

            ui.label("Color");
            fixed_combo(ui, "set_rep_color", ColorMethod::ALL.iter().map(|method| method.label()))
                .selected_text(r.color.label())
                .show_ui(ui, |ui| {
                    for c in ColorMethod::ALL {
                        ui.selectable_value(&mut r.color, c, c.label());
                    }
                });
            ui.end_row();

            ui.label("Material");
            let defaults = r.clone();
            super::pickers::material_selector(ui, &mut r.material, &defaults);
            ui.end_row();

            ui.label("Selection");
            ui.add(egui::TextEdit::singleline(&mut r.selection).desired_width(220.0));
            ui.end_row();
        });
    ui.add_space(4.0);
}

/// Behavior tab: mouse sensitivity, default pick/selection modes, trajectory
/// playback, and bond-guessing thresholds.
pub(super) fn settings_page_behavior(ui: &mut egui::Ui, s: &mut Settings) {
    let b = &mut s.behavior;
    ui.label(bold_name(ui, "Mouse"));
    egui::Grid::new("set_behavior_mouse")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Orbit sensitivity");
            slider_with_edit(ui, &mut b.orbit_sensitivity, 0.2..=3.0, true);
            ui.end_row();
            ui.label("Roll sensitivity");
            slider_with_edit(ui, &mut b.roll_sensitivity, 0.2..=3.0, true);
            ui.end_row();
        });

    ui.add_space(8.0);
    ui.label(bold_name(ui, "Picking"));
    egui::Grid::new("set_behavior_pick")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Pick mode");
            fixed_combo(ui, "set_pick_mode", [PickMode::Off, PickMode::Info, PickMode::Click, PickMode::Lasso].into_iter().map(|mode| mode.label()))
                .selected_text(b.pick_mode.label())
                .show_ui(ui, |ui| {
                    for m in [PickMode::Off, PickMode::Info, PickMode::Click, PickMode::Lasso] {
                        ui.selectable_value(&mut b.pick_mode, m, m.label());
                    }
                });
            ui.end_row();
            ui.label("Selection scope");
            fixed_combo(ui, "set_sel_mode", [SelectionMode::Atoms, SelectionMode::Residues, SelectionMode::BoundH].into_iter().map(|mode| mode.label()))
                .selected_text(b.selection_mode.label())
                .show_ui(ui, |ui| {
                    for m in [
                        SelectionMode::Atoms,
                        SelectionMode::Residues,
                        SelectionMode::BoundH,
                    ] {
                        ui.selectable_value(&mut b.selection_mode, m, m.label());
                    }
                });
            ui.end_row();
        });
    ui.add_space(8.0);
    ui.label(bold_name(ui, "Trajectory playback"));
    egui::Grid::new("set_behavior_traj")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Trajectory FPS");
            slider_with_edit(ui, &mut b.traj_fps, 1.0..=60.0, true);
            ui.end_row();
            ui.label("Loop playback");
            let mut looping = b.loop_mode == LoopMode::Loop;
            if ui.checkbox(&mut looping, "").changed() {
                b.loop_mode = if looping { LoopMode::Loop } else { LoopMode::Once };
            }
            ui.end_row();
        });

    ui.add_space(8.0);
    ui.label(bold_name(ui, "Bond detection"));
    egui::Grid::new("set_behavior_bonds")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("VDW factor");
            slider_with_edit(ui, &mut b.bond_factor, 0.3..=1.0, true);
            ui.end_row();
            ui.label("Search cutoff (nm)");
            slider_with_edit(ui, &mut b.bond_search_cutoff, 0.1..=0.5, true);
            ui.end_row();
            ui.label("Min distance (nm)");
            slider_with_edit(ui, &mut b.bond_min_dist, 0.0..=0.1, true);
            ui.end_row();
        });
    ui.checkbox(&mut b.bond_search_periodic, "Periodic search (bonds across box faces)")
        .on_hover_text(
            "Minimum-image bond search: also finds covalent bonds that cross a box face in a \
             wrapped structure. Off (default) is much faster for large structures.",
        );

    ui.add_space(8.0);
    ui.label(bold_name(ui, "Misc"));
    ui.checkbox(&mut b.hover_detail_lens, "Hover detail lens over cartoon/surface")
        .on_hover_text(
            "In pick/hover mode, reveal a faded ball-and-stick of the atoms under the cursor \
             over a Cartoon or Surface rep (hints where the atoms are). Off by default.",
        );
    ui.checkbox(&mut b.rep_zoom_unobstructed, "Rep and group magnifiers do an unobstructed view")
        .on_hover_text(
            "Representation and molecule-group magnifiers rotate and scale the camera to show \
             the target with the least obstruction (default). A group targets the shown member's \
             visible representations. Off → zoom to fit at the current orientation.",
        );
    ui.checkbox(&mut b.group_switch_zoom, "Zoom when switching molecules in mol. group")
        .on_hover_text(
            "Re-center the camera on the shown molecule when switching group members, \
             keeping the current zoom level. Off by default.",
        );
}

/// The orientation-axes "screen" widget: a monitor-like rectangle showing a mini
/// downsampled render of the scene, an on/off checkbox in its center, and a corner
/// radio **outside** each of the four corners (where the gizmo is anchored):
/// ```text
///   (o)          (o)
///      +--------+
///      |  [v]   |
///      +--------+
///   (o)          (o)
/// ```
pub(super) fn draw_axes_widget(
    ui: &mut egui::Ui,
    on: &mut bool,
    corner: &mut Corner,
    scene_tex: Option<egui::TextureId>,
) {
    let radio = 18.0;
    let margin = 22.0;
    let screen = egui::vec2(128.0, 82.0);
    let total = egui::vec2(screen.x + 2.0 * margin, screen.y + 2.0 * margin);
    let (rect, _) = ui.allocate_exact_size(total, egui::Sense::hover());
    let screen_rect = egui::Rect::from_center_size(rect.center(), screen);

    // The "screen": a mini downsampled render of the scene (last frame), or a dark
    // fill if no texture is available yet.
    let painter = ui.painter();
    painter.rect_filled(screen_rect, 4.0, ui.visuals().extreme_bg_color);
    if let Some(tex) = scene_tex {
        painter.image(
            tex,
            screen_rect.shrink(2.0),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
    painter.rect_stroke(
        screen_rect,
        4.0,
        egui::Stroke::new(1.0_f32, ui.visuals().widgets.inactive.fg_stroke.color),
        egui::StrokeKind::Inside,
    );

    // Corner radios at the widget's four outer corners (outside the screen rect).
    let h = radio * 0.5;
    let spots = [
        (Corner::TopLeft, egui::pos2(rect.left() + h, rect.top() + h)),
        (Corner::TopRight, egui::pos2(rect.right() - h, rect.top() + h)),
        (Corner::BottomLeft, egui::pos2(rect.left() + h, rect.bottom() - h)),
        (Corner::BottomRight, egui::pos2(rect.right() - h, rect.bottom() - h)),
    ];
    for (c, pos) in spots {
        let r = egui::Rect::from_center_size(pos, egui::vec2(radio, radio));
        if ui.put(r, egui::RadioButton::new(*corner == c, "")).clicked() {
            *corner = c;
        }
    }

    // On/off checkbox in the center of the screen, on a translucent backing so it
    // stays legible over the mini render.
    let cb = egui::Rect::from_center_size(screen_rect.center(), egui::vec2(26.0, 24.0));
    ui.painter()
        .rect_filled(cb.expand(3.0), 5.0, egui::Color32::from_black_alpha(170));
    ui.put(cb, egui::Checkbox::new(on, ""))
        .on_hover_text("Show orientation axes");
}
const SETTINGS_CONTENT_WIDTH: f32 = 640.0;
const SETTINGS_NAV_WIDTH: f32 = 156.0;
const SETTINGS_PAGES: [(SettingsPage, &str); 9] = [
    (SettingsPage::Appearance, "Appearance"),
    (SettingsPage::Rendering, "Rendering"),
    (SettingsPage::View, "View"),
    (SettingsPage::Representations, "Representations"),
    (SettingsPage::Styles, "Styles"),
    (SettingsPage::Materials, "Materials"),
    (SettingsPage::Interactions, "Interactions"),
    (SettingsPage::Colors, "Colors"),
    (SettingsPage::Behavior, "Behavior"),
];

fn settings_layout(
    ui: &mut egui::Ui,
    popup: bool,
    page: &mut SettingsPage,
    content: impl FnOnce(&mut egui::Ui, SettingsPage),
) {
    if popup {
        ui.set_width(300.0);
        egui::ScrollArea::vertical().max_height(380.0).auto_shrink([false, true])
            .show(ui, |ui| content(ui, *page));
        return;
    }
    ui.set_width(SETTINGS_NAV_WIDTH + SETTINGS_CONTENT_WIDTH + 24.0);
    let body_height = (ui.ctx().content_rect().height() - 160.0).clamp(120.0, 440.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.vertical(|ui| {
            ui.set_width(SETTINGS_NAV_WIDTH);
            egui::ScrollArea::vertical().id_salt("settings_navigation")
                .max_height(body_height).auto_shrink([false, false]).show(ui, |ui| {
                    for (candidate, label) in SETTINGS_PAGES {
                        if ui.add_sized(
                            [SETTINGS_NAV_WIDTH, dropdown_height(ui)],
                            egui::Button::selectable(*page == candidate, "").left_text(label),
                        ).clicked() {
                            *page = candidate;
                        }
                    }
                });
        });
        let (divider, _) = ui.allocate_exact_size(egui::vec2(1.0, body_height), egui::Sense::hover());
        ui.painter().line_segment([divider.center_top(), divider.center_bottom()],
            ui.visuals().widgets.noninteractive.bg_stroke);
        ui.vertical(|ui| {
            ui.set_width(SETTINGS_CONTENT_WIDTH);
            egui::ScrollArea::vertical().id_salt(("settings_content", *page as u8))
                .max_height(body_height).auto_shrink([false, false])
                .show(ui, |ui| content(ui, *page));
        });
    });
}

impl App {

    /// Effective new-rep defaults: the settings' `reps`, with the kind overridden by
    /// the `MOLAR_VIS_DEBUG_REP` env hook (headless verification). Recomputed when
    /// settings change.
    pub(super) fn effective_rep_defaults(settings: &Settings) -> RepDefaults {
        let mut d = settings.reps.clone();
        d.colors = std::sync::Arc::new(settings.colors.clone());
        if let Some(kind) = std::env::var("MOLAR_VIS_DEBUG_REP")
            .ok()
            .and_then(|s| RepKind::from_name(&s))
        {
            d.kind = kind;
        }
        d
    }

    fn current_settings(&self) -> Settings {
        let mut current = self.settings.clone();
        current.view = crate::settings::ViewDefaults::from_camera(&self.camera);
        current.behavior.pick_mode = self.pick_mode;
        current.behavior.selection_mode = self.selection_mode;
        current
    }

    pub(super) fn open_settings(&mut self, tab: SettingsPage) {
        if let Some(dialog) = &mut self.settings_dialog {
            dialog.tab = tab;
            dialog.popup = false;
            return;
        }
        let current = self.current_settings();
        self.settings_dialog = Some(SettingsDialog::capture(current, &self.scene, tab));
    }

    /// Apply runtime settings without writing startup defaults.
    fn apply_live_settings(
        &mut self,
        next: Settings,
        ctx: &egui::Context,
        frame: &mut eframe::Frame,
        redetect: bool,
        force: bool,
    ) -> Result<(), String> {
        let previous = self.settings.clone();
        let detected = if redetect
            && (force || previous.behavior.bond_params() != next.behavior.bond_params())
        {
            Some(
                self.scene
                    .molecules
                    .iter()
                    .map(|m| m.detect_bonds(&next.behavior.bond_params()))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        } else {
            None
        };
        if let Some(detected) = detected {
            for (mol, bonds) in self.scene.molecules.iter_mut().zip(detected) {
                mol.set_detected_bonds(bonds);
            }
        }
        let reps_changed = force || previous.reps != next.reps;
        let preferences = std::sync::Arc::new(next.colors.clone());
        if previous.appearance != next.appearance {
            crate::theme::apply(ctx, &next.appearance);
        }
        if previous.rendering.ssaa != next.rendering.ssaa || previous.rendering.shadow_res != next.rendering.shadow_res {
            if let Some(rs) = frame.wgpu_render_state() {
                self.renderer.reconfigure(rs, &next.rendering);
            }
        }
        next.view.seed_camera(&mut self.camera);
        self.pick_mode = next.behavior.pick_mode;
        self.selection_mode = next.behavior.selection_mode;
        for mol in &mut self.scene.molecules {
            if force || previous.behavior.traj_fps != next.behavior.traj_fps {
                mol.trajectory.speed_fps = next.behavior.traj_fps;
            }
            if force || previous.behavior.loop_mode != next.behavior.loop_mode {
                mol.trajectory.loop_mode = next.behavior.loop_mode;
            }
            for rep in &mut mol.reps {
                if rep.color_preferences != preferences {
                    rep.color_preferences = preferences.clone();
                    rep.geom_dirty = true;
                    mol.hover_detail_dirty = true;
                }
                if force || previous.reps.kind != next.reps.kind {
                    rep.kind = next.reps.kind;
                    rep.params = next.reps.style_params(rep.kind);
                }
                if force || previous.reps.color != next.reps.color {
                    rep.color = next.reps.color;
                }
                if force || previous.reps.material != next.reps.material {
                    rep.material = next.reps.material_for(next.reps.material);
                }
                if force || previous.reps.style_params(rep.kind) != next.reps.style_params(rep.kind) {
                    rep.params = next.reps.style_params(rep.kind);
                }
                if previous.reps.materials != next.reps.materials {
                    rep.material = next.reps.material_for(rep.material.preset());
                }
                if force || previous.reps.ss_algo != next.reps.ss_algo { rep.ss_algo = next.reps.ss_algo; }
                if force || previous.reps.selection != next.reps.selection {
                    rep.sel_text = next.reps.selection.clone();
                    rep.expr = None;
                    rep.sel_dirty = true;
                }
                if reps_changed
                    || previous.behavior.dashed_pbc_bonds != next.behavior.dashed_pbc_bonds
                {
                    rep.geom_dirty = true;
                }
            }
        }
        set_color_picker_palette(ctx, next.colors.discrete_palette());
        if let RepParams::Interactions { settings } = next.reps.style_params(RepKind::Interactions) {
            self.scene.interaction_settings = settings;
        }
        self.settings = next;
        self.rep_defaults = Self::effective_rep_defaults(&self.settings);
        self.last_render_camera = None;
        self.view_dirty = true;
        ctx.request_repaint();
        Ok(())
    }

    pub(super) fn draw_settings_dialog(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let Some(mut dialog) = self.settings_dialog.take() else {
            return;
        };
        let mut current = self.current_settings();
        set_color_picker_palette(ctx, current.colors.discrete_palette());
        let before = current.clone();
        let popup_was_open = dialog.child_popup_open || egui::Popup::is_any_open(ctx);
        let mut close = false;
        let mut factory = false;
        let mut revert = false;
        let mut save = false;
        let screen = ctx.content_rect();
        let popup = dialog.popup;
        let mut window_open = true;
        let mut window = egui::Window::new(if popup {
            "view_settings".to_string()
        } else {
            format!("{}  Settings", icon::GEAR_SIX)
        })
        .id(egui::Id::new(if popup {
            "view_settings"
        } else {
            "settings_window"
        }))
        .collapsible(false)
        .resizable(false);
        if popup {
            window = window
                .title_bar(false)
                .movable(false)
                .pivot(egui::Align2::RIGHT_TOP)
                .fixed_pos(dialog.anchor.right_bottom() + egui::vec2(0.0, 4.0));
        } else {
            window = window
                .open(&mut window_open)
                .movable(true)
                .pivot(egui::Align2::CENTER_TOP)
                .default_pos(egui::pos2(screen.center().x, screen.top() + 48.0));
        }
        if !popup && dialog.preview.is_none() && matches!(dialog.tab, SettingsPage::Styles | SettingsPage::Materials) {
            if let Some(rs) = frame.wgpu_render_state() {
                dialog.preview = Some(SettingsPreview::new(rs.clone()));
            }
        }
        if let Some(preview) = dialog.preview.as_mut() {
            let preferences = std::sync::Arc::new(current.colors.clone());
            for scene in [&mut preview.scene, &mut preview.protein] {
                for mol in &mut scene.molecules {
                    for rep in &mut mol.reps {
                        if rep.color_preferences != preferences {
                            rep.color_preferences = preferences.clone();
                            preview.last = None;
                        }
                    }
                }
            }
        }
        let inner = window.show(ctx, |ui| {
            settings_layout(ui, popup, &mut dialog.tab, |ui, page| match page {
                    SettingsPage::Appearance => settings_page_appearance(ui, &mut current),
                    SettingsPage::Rendering => settings_page_rendering(ui, &mut current),
                    SettingsPage::View => settings_page_view(
                        ui,
                        &mut current.view,
                        &mut dialog.view_page,
                        self.renderer.raytrace_supported(),
                        Some(self.renderer.texture_id()),
                    ),
                    SettingsPage::Representations => settings_page_reps(ui, &mut current),
                    SettingsPage::Styles => settings_page_styles(ui, &mut current, &mut dialog.style_page, dialog.preview.as_mut()),
                    SettingsPage::Materials => settings_page_materials(ui, &mut current, &mut dialog.material_page, dialog.preview.as_mut()),
                    SettingsPage::Interactions => settings_page_interactions(ui, &mut current),
                    SettingsPage::Colors => settings_page_colors(ui, &mut current),
                    SettingsPage::Behavior => settings_page_behavior(ui, &mut current),
            });
            ui.add_space(8.0);
            if let Some(error) = &dialog.save_error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.horizontal(|ui| {
                if ui
                    .button("Default")
                    .on_hover_text("Apply factory defaults to all settings")
                    .clicked()
                {
                    current = Settings::default();
                    factory = true;
                }
                if ui
                    .button("Revert")
                    .on_hover_text("Restore all settings from when this dialog opened")
                    .clicked()
                {
                    current = dialog.original.clone();
                    revert = true;
                }
                if ui
                    .button("Save")
                    .on_hover_text("Save all current settings as defaults for new sessions")
                    .clicked()
                {
                    save = true;
                }
            });

        });
        close |= !window_open;
        if popup {
            if let Some(inner) = inner {
                let hit_rect = dialog.last_rect.unwrap_or(inner.response.rect);
                let child_open = egui::Popup::is_any_open(ctx);
                if !child_open && !popup_was_open && ctx.input(|i| i.pointer.any_click()) {
                    if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                        close |= !hit_rect.contains(pos) && !dialog.anchor.contains(pos);
                    }
                }
                dialog.last_rect = Some(inner.response.rect);
                dialog.child_popup_open = child_open;
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape))
            && !popup_was_open
            && !egui::Popup::is_any_open(ctx)
        {
            close = true;
        }
        if factory || current != before {
            dialog.reps_changed |= factory || current.reps != before.reps;
            dialog.playback_changed |= factory
                || current.behavior.traj_fps != before.behavior.traj_fps
                || current.behavior.loop_mode != before.behavior.loop_mode;
            dialog.bonds_changed |=
                factory || current.behavior.bond_params() != before.behavior.bond_params();
            match self.apply_live_settings(current.clone(), ctx, frame, !revert, factory) {
                Ok(()) => dialog.save_error = None,
                Err(error) => {
                    dialog.save_error = Some(error.clone());
                    self.status = error;
                }
            }
        }
        if revert {
            dialog.restore_scene(&mut self.scene);
            self.last_render_camera = None;
            ctx.request_repaint();
        }
        if save {
            match self.current_settings().save() {
                Ok(()) => {
                    dialog.save_error = None;
                    self.status = "Defaults saved for new sessions".to_string();
                }
                Err(e) => {
                    let error = format!("Could not save defaults: {e}");
                    dialog.save_error = Some(error.clone());
                    self.status = error;
                }
            }

        }
        if !popup {
            self.last_settings_page = dialog.tab;
        }
        if !close {
            self.settings_dialog = Some(dialog);
        }
    }
}

impl SettingsDialog {
    fn capture(current: Settings, scene: &Scene, tab: SettingsPage) -> Self {
        let style_page = current.reps.kind;
        let material_page = current.reps.material.preset();
        Self {
            original: current,
            original_reps: scene
                .molecules
                .iter()
                .map(|m| {
                    (
                        m.id,
                        m.reps
                            .iter()
                            .map(crate::history::RepState::capture)
                            .collect(),
                        m.trajectory.speed_fps,
                        m.trajectory.loop_mode,
                    )
                })
                .collect(),
            original_bonds: scene
                .molecules
                .iter()
                .map(|m| (m.id, m.bonds.clone(), m.bond_guess_source.clone()))
                .collect(),
            bonds_changed: false,
            reps_changed: false,
            playback_changed: false,
            tab,
            view_page: ViewPage::default(),
            style_page,
            material_page,
            preview: None,
            popup: false,
            anchor: egui::Rect::NOTHING,
            last_rect: None,
            child_popup_open: false,
            save_error: None,
        }
    }

    fn restore_scene(&self, scene: &mut Scene) {
        if self.bonds_changed {
            for (id, bonds, source) in &self.original_bonds {
                if let Some(mol) = scene.molecules.iter_mut().find(|m| m.id == *id) {
                    mol.set_detected_bonds(bonds.clone());
                    mol.bond_guess_source = source.clone();
                }
            }
        }
        for (id, reps, fps, looping) in &self.original_reps {
            if let Some(mol) = scene.molecules.iter_mut().find(|m| m.id == *id) {
                if self.reps_changed {
                    for (rep, original) in mol.reps.iter_mut().zip(reps) {
                        rep.kind = original.kind;
                        rep.params = original.params;
                        rep.color = original.color;
                        rep.material = original.material;
                        rep.ss_algo = original.ss_algo;
                        rep.sel_text = original.sel_text.clone();
                        rep.expr = None;
                        rep.sel_dirty = true;
                        rep.geom_dirty = true;
                    }
                }
                if self.playback_changed {
                    mol.trajectory.speed_fps = *fps;
                    mol.trajectory.loop_mode = *looping;
                }
            }
        }
    }
}

#[cfg(test)]
mod live_settings_tests {
    use super::*;

    fn scene() -> Scene {
        let mut scene = Scene::default();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/2lao.pdb");
        scene.add(
            crate::data::load(&path).unwrap(),
            &crate::settings::RepDefaults::default(),
        );
        scene
    }

    #[test]
    fn revert_restores_distinct_rep_and_playback_values() {
        let mut scene = scene();
        let mol = &mut scene.molecules[0];
        mol.reps[0].kind = RepKind::Cartoon;
        mol.reps[0].color = ColorMethod::Chain;
        mol.reps.push(Representation::new(RepKind::Vdw));
        mol.trajectory.speed_fps = 37.0;
        mol.trajectory.loop_mode = LoopMode::Once;
        let original: Vec<_> = mol
            .reps
            .iter()
            .map(crate::history::RepState::capture)
            .collect();
        let mut dialog =
            SettingsDialog::capture(Settings::default(), &scene, SettingsPage::Appearance);
        dialog.reps_changed = true;
        dialog.playback_changed = true;
        for rep in &mut scene.molecules[0].reps {
            rep.kind = RepKind::Lines;
            rep.color = ColorMethod::Element;
        }
        scene.molecules[0].trajectory.speed_fps = 15.0;
        scene.molecules[0].trajectory.loop_mode = LoopMode::Loop;
        dialog.restore_scene(&mut scene);
        let mol = &scene.molecules[0];
        let restored: Vec<_> = mol
            .reps
            .iter()
            .map(crate::history::RepState::capture)
            .collect();
        assert!(original == restored);
        assert_eq!(mol.trajectory.speed_fps, 37.0);
        assert_eq!(mol.trajectory.loop_mode, LoopMode::Once);
    }

    #[test]
    fn live_bond_detection_preserves_recorded_chemical_orders() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/ligands20.sdf");
        let raw = crate::data::load_records(&path, &crate::data::BondParams::default())
            .unwrap()
            .remove(0);
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let mol = &scene.molecules[0];
        let params = crate::data::BondParams {
            factor: 0.3,
            search_cutoff: 0.1,
            ..Default::default()
        };
        let bonds = mol.detect_bonds(&params).unwrap();
        assert_eq!(bonds.len(), mol.bonds.len());
        for (original, detected) in mol.bonds.iter().zip(bonds) {
            assert_eq!(
                (original.i1, original.i2, original.order),
                (detected.i1, detected.i2, detected.order)
            );
        }
    }

    #[test]
    fn bond_settings_can_tighten_and_loosen_without_losing_source_bonds() {
        let mut scene = scene();
        let mol = &mut scene.molecules[0];
        let original = mol.bonds.clone();
        let source = mol.bond_guess_source.clone();
        let tight = crate::data::BondParams {
            factor: 0.3,
            ..Default::default()
        };
        let bonds = mol.detect_bonds(&tight).unwrap();
        assert!(bonds.len() < original.len());
        for bond in &source {
            assert!(bonds
                .iter()
                .any(|b| (b.i1 == bond.i1 && b.i2 == bond.i2)
                    || (b.i1 == bond.i2 && b.i2 == bond.i1)));
        }
        mol.set_detected_bonds(bonds);
        let relaxed = mol
            .detect_bonds(&crate::data::BondParams::default())
            .unwrap();
        assert_eq!(relaxed.len(), original.len());
        let mut dialog =
            SettingsDialog::capture(Settings::default(), &scene, SettingsPage::Behavior);
        dialog.bonds_changed = true;
        scene.molecules[0].set_detected_bonds(Vec::new());
        dialog.restore_scene(&mut scene);
        assert_eq!(
            scene.molecules[0].bonds.len(),
            scene.molecules[0].detect_bonds(&tight).unwrap().len()
        );
    }
}

/// Geometry defaults for one style; choosing a style here does not switch the scene.
pub(super) fn settings_page_styles(
    ui: &mut egui::Ui,
    s: &mut Settings,
    selected: &mut RepKind,
    preview: Option<&mut SettingsPreview>,
) {
    if *selected == RepKind::Interactions {
        *selected = RepKind::Lines;
    }
    ui.horizontal(|ui| {
        ui.label("Style").on_hover_text("Edit matching representations. Save keeps these options for new representations and sessions.");
        fixed_combo(ui, "settings_style", RepKind::ALL.iter().filter(|&&kind| kind != RepKind::Interactions).map(|kind| kind.label())).selected_text(selected.label()).show_ui(ui, |ui| {
            for kind in RepKind::ALL {
                if kind != RepKind::Interactions { ui.selectable_value(selected, kind, kind.label()); }
            }
        });
    });
    ui.add_space(8.0);
    let mut params = s.reps.style_params(*selected);
    let before = params;
    let controls_width = (ui.available_width() - PREVIEW_SIZE - 16.0).max(0.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        ui.vertical(|ui| {
            ui.set_max_width(controls_width);
            ui.spacing_mut().slider_width = 100.0;
            super::rep_panel::draw_style_options(ui, &mut params);
            if *selected == RepKind::Cartoon {
                ui.horizontal(|ui| {
                    ui.label("SS algorithm").on_hover_text(
                        "Algorithm used to assign the helix, sheet, and coil geometry.",
                    );
                    fixed_combo(ui, "settings_ss", ["DSSP", "DSSP (gmx)", "dss (PyMOL)"].into_iter())
                        .selected_text(match s.reps.ss_algo {
                            SsAlgorithm::Dssp => "DSSP",
                            SsAlgorithm::DsspGmx => "DSSP (gmx)",
                            SsAlgorithm::Dss => "dss (PyMOL)",
                        })
                        .show_ui(ui, |ui| {
                            for (algorithm, label) in [
                                (SsAlgorithm::Dssp, "DSSP"),
                                (SsAlgorithm::DsspGmx, "DSSP (gmx)"),
                                (SsAlgorithm::Dss, "dss (PyMOL)"),
                            ] {
                                ui.selectable_value(&mut s.reps.ss_algo, algorithm, label);
                            }
                        });
                });
            }
            if ui
                .button("Reset style")
                .on_hover_text("Restore this style's factory options")
                .clicked()
            {
                params = RepParams::for_kind(*selected);
                if *selected == RepKind::Cartoon {
                    s.reps.ss_algo = SsAlgorithm::default();
                }
            }
        });
        if let Some(preview) = preview {
            preview.show(
                ui,
                params,
                s.reps.material_for(s.reps.material),
                s.reps.ss_algo,
            );
        } else {
            preview_rectangle(ui);
        }
    });
    if params != before {
        s.reps.styles.retain(|p| p.kind() != *selected);
        s.reps.styles.push(params);
    }
}

pub(super) fn settings_page_materials(
    ui: &mut egui::Ui,
    s: &mut Settings,
    selected: &mut Material,
    preview: Option<&mut SettingsPreview>,
) {
    ui.horizontal(|ui| {
        ui.label("Material").on_hover_text(
            "Edit matching representations. Save keeps the shader options for new sessions.",
        );
        super::pickers::material_selector(ui, selected, &s.reps);
    });
    ui.add_space(8.0);
    let mut options = s.reps.material_for(*selected).options();
    let before = options;
    let mut reset = false;
    let controls_width = (ui.available_width() - PREVIEW_SIZE - 16.0).max(0.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        ui.vertical(|ui| {
            ui.set_max_width(controls_width);
            egui::Grid::new("material_options")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Opacity")
                        .on_hover_text("0 is invisible; 1 is fully opaque.");
                    slider_with_edit(ui, &mut options.opacity, 0.0..=1.0, true);
                    ui.end_row();
                    if *selected == Material::Velvet {
                        ui.label("Edge softness")
                            .on_hover_text("Strength of the soft, tinted lighting near the silhouette.");
                        slider_with_edit(ui, &mut options.hairiness, 0.0..=1.0, true);
                        ui.end_row();
                    }
                    if *selected == Material::FlatOutline {
                        ui.label("Contour width");
                        slider_with_edit(ui, &mut options.outline_width, 0.05..=1.0, true);
                        ui.end_row();
                        ui.label("Contour strength");
                        slider_with_edit(ui, &mut options.outline, 0.0..=1.0, true);
                        ui.end_row();
                    } else {
                        ui.label("Ambient")
                            .on_hover_text("Unlit fill contribution.");
                        slider_with_edit(ui, &mut options.ambient, 0.0..=1.0, true);
                        ui.end_row();
                        ui.label("Diffuse")
                            .on_hover_text("Strength of the light falling on the surface.");
                        slider_with_edit(ui, &mut options.diffuse, 0.0..=1.0, true);
                        ui.end_row();
                        ui.label(if *selected == Material::Velvet { "Sheen" } else { "Specular" })
                            .on_hover_text("Strength of surface highlights.");
                        slider_with_edit(ui, &mut options.specular, 0.0..=1.0, true);
                        ui.end_row();
                        ui.label(if matches!(*selected, Material::MolecularNodes | Material::Velvet) {
                            "Roughness"
                        } else {
                            "Shininess"
                        });
                        slider_with_edit(ui, &mut options.shininess, 0.0..=1.0, true);
                        ui.end_row();
                        if *selected == Material::AoEdgy {
                            ui.label("Outline");
                            let mut outline = options.outline > 0.5;
                            if ui.checkbox(&mut outline, "").changed() {
                                options.outline = if outline { 0.7 } else { 0.0 };
                            }
                            ui.end_row();
                        }
                    }
                });
            reset = ui
                .button("Reset material")
                .on_hover_text("Restore this material's factory shader options")
                .clicked();
        });
        ui.vertical(|ui| {
            let material = if reset {
                *selected
            } else {
                selected.with_options(options)
            };
            if let Some(preview) = preview {
                preview.show(ui, RepParams::Vdw { scale: 1.0 }, material, s.reps.ss_algo);
            } else {
                preview_rectangle(ui);
            }
        });
    });
    if reset {
        s.reps.materials.retain(|m| m.preset() != *selected);
    } else if options != before {
        s.reps.materials.retain(|m| m.preset() != *selected);
        s.reps.materials.push(selected.with_options(options));
    }
}

fn settings_interaction_options(
    ui: &mut egui::Ui,
    s: &mut crate::interactions::InteractionSettings,
) {
    fn section(
        ui: &mut egui::Ui,
        title: &str,
        enabled: &mut bool,
        color: &mut [u8; 4],
        fields: &mut [(&str, &mut f32, std::ops::RangeInclusive<f32>)],
    ) {
        ui.add_space(8.0);
        ui.push_id(title, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(enabled, bold_name(ui, title)).on_hover_text("Enable this interaction type");
                let swatch = swatch_button(ui, *color, egui::vec2(32.0, dropdown_height(ui)));
                color_popup(&swatch, color);
            });
            egui::Grid::new("parameters").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                for (label, value, range) in fields {
                    ui.add_enabled(*enabled, egui::Label::new(*label));
                    slider_with_edit(ui, value, range.clone(), *enabled);
                    ui.end_row();
                }
            });
        });
    }
    ui.checkbox(&mut s.glowing_path, "Glowing path")
        .on_hover_text("Render all enabled interaction types as glowing helices in their configured colors, brightest at the bond center.");
    ui.add_enabled(s.glowing_path, egui::Checkbox::new(&mut s.animate_glowing_path, "Animate glowing paths"))
        .on_hover_text("Rotate helices around their axes once every 2 seconds, keeping six turns per bond.");
    egui::Grid::new("interaction_rendering").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
        ui.label(if s.glowing_path { "Tube width" } else { "Line width (px)" })
            .on_hover_text("Controls the core and halo thickness of glowing tubes; 3 is the default. Dashed-line width is in pixels.");
        slider_with_edit(ui, &mut s.line_width, 1.0..=20.0, true);
        ui.end_row();
    });
    section(ui, "H-bonds", &mut s.hbonds, &mut s.colors[0], &mut [
        ("Donor–acceptor distance (nm)", &mut s.hbond_dist, 0.25..=0.5),
        ("Distance with H (nm)", &mut s.hbond_dist_h, 0.25..=0.5),
        ("Minimum D–H···A angle (deg)", &mut s.hbond_angle, 90.0..=180.0),
    ]);
    section(ui, "Hydrophobic", &mut s.hydrophobic, &mut s.colors[1], &mut [
        ("Maximum C···C distance (nm)", &mut s.hydrophobic_dist, 0.3..=0.55),
    ]);
    section(ui, "Salt bridges", &mut s.salt_bridges, &mut s.colors[2], &mut [
        ("Charge-centre distance (nm)", &mut s.salt_bridge_dist, 0.3..=0.7),
    ]);
    section(ui, "π stacking", &mut s.pi_stacking, &mut s.colors[3], &mut [
        ("Ring-centre distance (nm)", &mut s.pi_stacking_dist, 0.3..=0.8),
        ("Plane angle tolerance (deg)", &mut s.pi_stacking_angle, 0.0..=90.0),
        ("Maximum offset (nm)", &mut s.pi_stacking_offset, 0.0..=0.5),
    ]);
    section(ui, "π cation", &mut s.pi_cation, &mut s.colors[4], &mut [
        ("Ring–cation distance (nm)", &mut s.pi_cation_dist, 0.3..=0.8),
        ("Offset from ring axis (nm)", &mut s.pi_cation_offset, 0.0..=0.5),
    ]);
    section(ui, "Halogen bonds", &mut s.halogen, &mut s.colors[5], &mut [
        ("X···acceptor distance (nm)", &mut s.halogen_dist, 0.25..=0.5),
        ("Minimum C–X···A angle (deg)", &mut s.halogen_angle, 90.0..=180.0),
    ]);
}

fn settings_page_interactions(ui: &mut egui::Ui, s: &mut Settings) {
    let mut params = s.reps.style_params(RepKind::Interactions);
    let before = params;
    if let RepParams::Interactions { settings } = &mut params {
        settings_interaction_options(ui, settings);
    }
    if ui.button("Reset interactions").on_hover_text("Restore factory interaction defaults").clicked() {
        params = RepParams::for_kind(RepKind::Interactions);
    }
    if params != before {
        s.reps.styles.retain(|params| params.kind() != RepKind::Interactions);
        s.reps.styles.push(params);
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;

    #[test]
    fn settings_pages_form_a_vertical_list_and_open_interactions() {
        for theme in [crate::settings::ThemeMode::Dark, crate::settings::ThemeMode::Light] {
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx, &crate::settings::AppearanceSettings { theme, ..Default::default() });
            let mut page = SettingsPage::Appearance;
            let frame = |page: &mut SettingsPage, events| {
                ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 700.0))),
                    events, ..Default::default()
                }, |ui| {
                    settings_layout(ui, false, page, |ui, selected| {
                        assert!(ui.max_rect().left() > SETTINGS_NAV_WIDTH);
                        ui.label(format!("Content: {selected:?}"));
                    });
                    assert!(ui.min_rect().height() <= 440.5,
                        "settings body must not grow beyond its scrollable viewport");
                })
            };
            let output = frame(&mut page, vec![]);
            let entries: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if SETTINGS_PAGES.iter().any(|(_, label)| text.galley.text() == *label) =>
                    Some((text.galley.text().to_string(), text.pos, text.galley.size())),
                _ => None,
            }).collect();
            assert_eq!(entries.len(), SETTINGS_PAGES.len());
            for pair in entries.windows(2) {
                assert!(pair[1].1.y > pair[0].1.y);
                assert!((pair[1].1.x - pair[0].1.x).abs() < 0.1);
            }
            let (_, pos, size) = entries.iter().find(|(label, _, _)| label == "Interactions").unwrap();
            let pos = *pos + *size * 0.5;
            for pressed in [true, false] {
                frame(&mut page, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                    pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
                }]);
            }
            assert_eq!(page, SettingsPage::Interactions);
        }
    }

    #[test]
    fn interaction_page_edits_are_saved_as_new_representation_defaults() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut settings = Settings::default();
        let frame = |settings: &mut Settings, events| {
            ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 1000.0))),
                events, ..Default::default()
            }, |ui| settings_page_interactions(ui, settings))
        };
        let output = frame(&mut settings, vec![]);
        let label = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "H-bonds" =>
                Some(egui::Rect::from_min_size(text.pos, text.galley.size())),
            _ => None,
        }).unwrap();
        let checkbox = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) if rect.rect.right() < label.left()
                && (rect.rect.center().y - label.center().y).abs() < 1.0
                && (rect.rect.width() - rect.rect.height()).abs() < 0.1 => Some(rect.rect.center()),
            _ => None,
        }).unwrap();
        for pressed in [true, false] {
            frame(&mut settings, vec![egui::Event::PointerMoved(checkbox), egui::Event::PointerButton {
                pos: checkbox, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            }]);
        }
        let restored: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        let rep = Representation::from_defaults(&RepDefaults { kind: RepKind::Interactions, ..restored.reps });
        let RepParams::Interactions { settings } = rep.params else { panic!("interaction defaults") };
        assert!(!settings.hbonds);
    }
}

const PREVIEW_SIZE: f32 = 232.0;

fn preview_rectangle(ui: &mut egui::Ui) -> egui::Response {
    ui.allocate_exact_size(egui::Vec2::splat(PREVIEW_SIZE), egui::Sense::drag())
        .1
}

/// Neutral glycine, including explicit hydrogens and its carbonyl double bond.
fn small_molecule_preview_scene() -> Scene {
    use super::draw::Element;
    use crate::minimize::BondOrder;
    use glam::vec3;
    let atoms = [
        (Element::N, vec3(-0.145, 0.03, 0.0)),
        (Element::C, vec3(0.0, 0.0, 0.0)),
        (Element::C, vec3(0.13, 0.065, 0.0)),
        (Element::O, vec3(0.15, 0.185, 0.0)),
        (Element::O, vec3(0.225, -0.025, 0.0)),
        (Element::H, vec3(-0.19, 0.015, 0.085)),
        (Element::H, vec3(-0.185, 0.045, -0.087)),
        (Element::H, vec3(0.005, -0.055, 0.093)),
        (Element::H, vec3(-0.025, -0.067, -0.08)),
        (Element::H, vec3(0.31, 0.015, 0.0)),
    ];
    let raw = data::RawMolecule::single_atom("glycine", atoms[0].0.make_atom(), atoms[0].1)
        .expect("valid preview molecule");
    let mut scene = Scene::default();
    scene.add(raw, &crate::settings::RepDefaults::default());
    let mol = &mut scene.molecules[0];
    for &(element, pos) in &atoms[1..] {
        mol.add_atom(&element.make_atom(), pos);
    }
    for (a, b) in [
        (0, 1),
        (1, 2),
        (2, 4),
        (0, 5),
        (0, 6),
        (1, 7),
        (1, 8),
        (4, 9),
    ] {
        mol.add_bond(a, b, BondOrder::Single);
    }
    mol.add_bond(2, 3, BondOrder::Double);
    mol.refresh_bbox();
    mol.reps[0].color = ColorMethod::Element;
    mol.reps[0].sel = Some(mol.data.select_all());
    scene
}

/// Bounds include actual sphere/cap radii, multiple-bond offsets and mesh vertices.
fn preview_bounds(geom: &geometry::GeometryData) -> Vec<(glam::Vec3, f32)> {
    use glam::Vec3;
    let mut bounds = Vec::new();
    bounds.extend(
        geom.spheres
            .iter()
            .map(|s| (Vec3::from_array(s.center), s.radius)),
    );
    for c in &geom.cylinders {
        let a = Vec3::from_array(c.p0);
        let b = Vec3::from_array(c.p1);
        let r0 = c.profile[0].hypot(c.profile[1]);
        let r1 = (a.distance(b) - c.profile[2]).hypot(c.profile[3]);
        let flare = if c.profile[1] > 0.0 {
            r0.max(r1) + c.smoothing * r0.min(r1) / 3.0
        } else {
            0.0
        };
        let radius = c.radius.max(flare) + (c.offset[0] * c.offset[1]).abs();
        bounds.extend([(a, radius), (b, radius)]);
    }
    bounds.extend(
        geom.mesh
            .vertices
            .iter()
            .map(|v| (Vec3::from_array(v.pos), 0.0)),
    );
    bounds.extend(geom.lines.iter().map(|v| (Vec3::from_array(v.pos), 0.0)));
    bounds
}

fn fit_preview_camera(
    bounds: &[(glam::Vec3, f32)],
    orientation: glam::Quat,
) -> crate::camera::Camera {
    use glam::Vec3;
    let (mut min, mut max) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for &(center, radius) in bounds {
        min = min.min(center - Vec3::splat(radius));
        max = max.max(center + Vec3::splat(radius));
    }
    if bounds.is_empty() {
        min = Vec3::splat(-0.3);
        max = -min;
    }
    let mut camera = crate::camera::Camera::frame_bbox(min, max, 0.9);
    camera.orientation = orientation;
    let mut radius = 0.01_f32;
    for &(center, bead_radius) in bounds {
        radius = radius.max(center.distance(camera.target) + bead_radius);
    }
    // Fit a rotation-invariant envelope so dragging never changes zoom or clips caps.
    camera.scene_radius = radius;
    camera.distance = radius / (0.9 * (camera.fov_y * 0.5).tan());
    camera.background.color = [1.0; 4];
    camera
}

/// A separate offscreen viewport keeps Settings previews on the production shaders.
pub(super) struct SettingsPreview {
    rs: eframe::egui_wgpu::RenderState,
    renderer: SceneRenderer,
    scene: Scene,
    protein: Scene,
    last: Option<(RepParams, Material, SsAlgorithm)>,
    camera: crate::camera::Camera,
    bounds: Vec<(glam::Vec3, f32)>,
    render_size: [u32; 2],
}

impl SettingsPreview {
    fn new(rs: eframe::egui_wgpu::RenderState) -> Self {
        let scene = small_molecule_preview_scene();
        let renderer = SceneRenderer::new(
            &rs,
            &crate::settings::RenderingSettings {
                ssaa: 2,
                shadow_res: 256,
                ..Default::default()
            },
        );
        let raw = data::load_from_bytes(
            "cartoon-preview.pdb",
            include_bytes!("../../assets/cartoon-preview.pdb").to_vec(),
            &crate::data::BondParams::default(),
        )
        .expect("valid bundled crambin protein");
        let mut protein = Scene::default();
        protein.add(raw, &crate::settings::RepDefaults::default());
        protein.molecules[0].reps[0].color = ColorMethod::SecStruct;
        protein.molecules[0].reps[0].sel = Some(protein.molecules[0].data.select_all());
        let mut camera = crate::camera::Camera::default();
        camera.orientation = glam::Quat::from_rotation_x(0.45) * glam::Quat::from_rotation_y(-0.3);
        Self {
            rs,
            renderer,
            scene,
            protein,
            last: None,
            camera,
            bounds: Vec::new(),
            render_size: [0; 2],
        }
    }

    fn show(
        &mut self,
        ui: &mut egui::Ui,
        params: RepParams,
        material: Material,
        ss_algo: SsAlgorithm,
    ) -> egui::Response {
        let response = preview_rectangle(ui)
            .on_hover_text("Drag to rotate")
            .on_hover_cursor(egui::CursorIcon::Grab);
        let rotating = response.dragged_by(egui::PointerButton::Primary);
        if rotating {
            let delta = response.drag_delta();
            self.camera.orbit(delta.x, delta.y, 1.0);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let size = (PREVIEW_SIZE * ui.ctx().pixels_per_point())
            .round()
            .max(1.0) as u32;
        let render_size = [size; 2];
        let key = (params, material, ss_algo);
        let changed = self.last != Some(key);
        let geometry_changed = self
            .last
            .is_none_or(|(old_params, _, old_ss)| old_params != params || old_ss != ss_algo);
        if changed {
            let scene = if matches!(params.kind(), RepKind::Cartoon | RepKind::Surface) {
                &mut self.protein
            } else {
                &mut self.scene
            };
            let mol = &mut scene.molecules[0];
            mol.reps[0].color = if params.kind() == RepKind::Cartoon {
                ColorMethod::SecStruct
            } else {
                ColorMethod::Element
            };
            let geom = {
                let rep = &mol.reps[0];
                let bound = mol
                    .data
                    .bind_with_state(rep.sel.as_ref().unwrap(), mol.render_state());
                let ss =
                    geometry::needs_ss(&params, rep.color).then(|| SsMap::compute(&bound, ss_algo));
                geometry::build(
                    &bound,
                    mol.n_atoms,
                    &mol.bonds,
                    &params,
                    rep.color_spec(),
                    material,
                    ss.as_ref(),
                    false,
                )
            };
            self.bounds = preview_bounds(&geom);
            let rep = &mut mol.reps[0];
            rep.kind = params.kind();
            rep.params = params;
            rep.material = material;
            rep.gpu = self.renderer.upload(&self.rs, &geom);
            self.last = Some(key);
        }
        if geometry_changed {
            self.camera = fit_preview_camera(&self.bounds, self.camera.orientation);
        }
        if changed || rotating || self.render_size != render_size {
            let camera = &self.camera;
            let scene = if matches!(params.kind(), RepKind::Cartoon | RepKind::Surface) {
                &self.protein
            } else {
                &self.scene
            };
            self.renderer.render_scene(
                &self.rs,
                render_size,
                camera.view(),
                camera.proj(1.0),
                false,
                camera.cue_uniform(),
                camera.ao_uniform(),
                camera.shadow_uniform(),
                camera.background,
                camera.eye_depth_range(),
                0.0,
                scene,
            );
            self.render_size = render_size;
        }
        ui.painter().image(
            self.renderer.texture_id(),
            response.rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        response
    }
}

impl Drop for SettingsPreview {
    fn drop(&mut self) {
        self.rs
            .renderer
            .write()
            .free_texture(&self.renderer.texture_id());
    }
}

#[cfg(test)]
mod material_preview_tests {
    use super::*;

    #[test]
    fn small_molecule_contains_all_four_elements_and_fits_after_rotation() {
        let scene = small_molecule_preview_scene();
        let mol = &scene.molecules[0];
        let bound = mol
            .data
            .bind_with_state(mol.reps[0].sel.as_ref().unwrap(), mol.render_state());
        let elements: std::collections::BTreeSet<_> =
            bound.iter_atoms().map(|a| a.get_atomic_number()).collect();
        assert_eq!(elements, [1, 6, 7, 8].into_iter().collect());
        for kind in [
            RepKind::Vdw,
            RepKind::Licorice,
            RepKind::BallAndStick,
            RepKind::Lines,
            RepKind::Surface,
        ] {
            let geom = geometry::build(
                &bound,
                mol.n_atoms,
                &mol.bonds,
                &RepParams::for_kind(kind),
                mol.reps[0].color_spec(),
                Material::Opaque,
                None,
                false,
            );
            let bounds = preview_bounds(&geom);
            assert!(!bounds.is_empty(), "empty {kind:?} preview");
            let initial = fit_preview_camera(&bounds, glam::Quat::IDENTITY);
            for angle in [0.0, 0.8, 1.6, 2.4] {
                let camera = fit_preview_camera(&bounds, glam::Quat::from_rotation_y(angle));
                assert_eq!(
                    camera.distance, initial.distance,
                    "rotation changes preview zoom"
                );
                let half = camera.distance * (camera.fov_y * 0.5).tan();
                let mut extent = 0.0_f32;
                for &(center, radius) in &bounds {
                    let p = camera.orientation.conjugate() * (center - camera.target);
                    extent = extent.max(p.x.abs().max(p.y.abs()) + radius);
                    assert!(
                        p.x.abs() + radius < half && p.y.abs() + radius < half,
                        "{kind:?} clipped"
                    );
                }
                assert!(
                    angle != 0.0 || extent / half > 0.7,
                    "{kind:?} preview does not fill the image"
                );
            }
        }
    }

    #[test]
    #[ignore = "requires GPU; verifies stable preview layout and mouse rotation with actual rendering"]
    fn preview_layout_stays_fixed_and_mouse_drag_changes_render() {
        let rs = crate::render::test_gpu();
        let mut preview = SettingsPreview::new(rs);
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut settings = Settings::default();
        let mut selected = RepKind::Vdw;
        let mut elapsed_frames = 0;
        for height in [480.0, 600.0, 900.0] {
            let mut stable = None;
            for frame in 0..24 {
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(960.0, height),
                        )),
                        time: Some(elapsed_frames as f64 / 60.0),
                        ..Default::default()
                    },
                    |ui| {
                        let ctx = ui.ctx();
                        let window =
                            egui::Window::new("Settings")
                                .resizable(false)
                                .show(ctx, |ui| {
                                    ui.set_width(640.0);
                                    egui::ScrollArea::vertical()
                                        .max_height(440.0)
                                        .auto_shrink([false, true])
                                        .show(ui, |ui| {
                                            settings_page_styles(
                                                ui,
                                                &mut settings,
                                                &mut selected,
                                                Some(&mut preview),
                                            );
                                        });
                                });
                        let Some(window) = window else {
                            return;
                        };
                        if frame >= 8 {
                            let size = window.response.rect.size();
                            if let Some(old) = stable {
                                assert_eq!(size, old, "VDW preview layout oscillates");
                            }
                            stable = Some(size);
                        }
                    },
                );
                elapsed_frames += 1;
            }
            assert!(stable.is_some(), "Settings window never became visible");
        }
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            events,
            ..Default::default()
        };
        let params = RepParams::Vdw { scale: 1.0 };
        let mut rect = egui::Rect::NOTHING;
        for _ in 0..3 {
            let _ = ctx.run_ui(input(vec![]), |ui| {
                rect = preview
                    .show(ui, params, Material::Opaque, SsAlgorithm::default())
                    .rect;
            });
        }
        let capture = |preview: &mut SettingsPreview| {
            let camera = &preview.camera;
            let cap = preview.renderer.capture_begin(
                &preview.rs,
                232,
                232,
                camera.view(),
                camera.proj(1.0),
                false,
                camera.cue_uniform(),
                camera.ao_uniform(),
                camera.shadow_uniform(),
                camera.background,
                camera.eye_depth_range(),
                &preview.scene,
            );
            preview
                .rs
                .device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
            cap.read()
        };
        let before = capture(&mut preview);
        let orientation = preview.camera.orientation;
        let distance = preview.camera.distance;
        let target = preview.camera.target;
        let start = rect.center();
        let _ = ctx.run_ui(
            input(vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ui| {
                preview.show(ui, params, Material::Opaque, SsAlgorithm::default());
            },
        );
        let finish = start + egui::vec2(65.0, 30.0);
        let _ = ctx.run_ui(input(vec![egui::Event::PointerMoved(finish)]), |ui| {
            let response = preview.show(ui, params, Material::Opaque, SsAlgorithm::default());
            assert!(response.dragged_by(egui::PointerButton::Primary));
            assert_eq!(response.rect.size(), egui::Vec2::splat(PREVIEW_SIZE));
        });
        assert_ne!(preview.camera.orientation, orientation);
        assert_eq!(preview.camera.distance, distance, "drag changed zoom");
        assert_eq!(
            preview.camera.target, target,
            "drag moved the preview centre"
        );
        let after = capture(&mut preview);
        let changed = before
            .pixels()
            .zip(after.pixels())
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed > 1000, "drag did not change the rendered molecule");
        before.save("/tmp/settings-glycine-preview.png").unwrap();
        after
            .save("/tmp/settings-glycine-preview-rotated.png")
            .unwrap();
        let rotated = preview.camera.orientation;
        let _ = ctx.run_ui(
            input(vec![egui::Event::PointerButton {
                pos: finish,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]),
            |ui| {
                preview.show(ui, params, Material::Transparent, SsAlgorithm::default());
            },
        );
        assert_eq!(
            preview.camera.orientation, rotated,
            "slider edits reset rotation"
        );
        assert_eq!(
            preview.camera.distance, distance,
            "material edits changed zoom"
        );
        let _ = ctx.run_ui(input(vec![]), |ui| {
            preview.show(
                ui,
                RepParams::for_kind(RepKind::Surface),
                Material::Opaque,
                SsAlgorithm::default(),
            );
        });
        assert_eq!(preview.protein.molecules[0].reps[0].kind, RepKind::Surface);
        assert!(preview.bounds.len() > 1000, "protein surface mesh is empty");
        let camera = &preview.camera;
        let cap = preview.renderer.capture_begin(
            &preview.rs,
            232,
            232,
            camera.view(),
            camera.proj(1.0),
            false,
            camera.cue_uniform(),
            camera.ao_uniform(),
            camera.shadow_uniform(),
            camera.background,
            camera.eye_depth_range(),
            &preview.protein,
        );
        preview
            .rs
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        cap.read()
            .save("/tmp/settings-protein-surface-preview.png")
            .unwrap();
    }

    #[test]
    fn bundled_protein_has_cartoon_geometry_for_each_ss_algorithm() {
        let raw = data::load_from_bytes(
            "cartoon-preview.pdb",
            include_bytes!("../../assets/cartoon-preview.pdb").to_vec(),
            &crate::data::BondParams::default(),
        )
        .unwrap();
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let mol = &scene.molecules[0];
        let sel = mol.data.select_all();
        let bound = mol.data.bind_with_state(&sel, mol.render_state());
        for algorithm in [SsAlgorithm::Dssp, SsAlgorithm::DsspGmx, SsAlgorithm::Dss] {
            let ss = SsMap::compute(&bound, algorithm);
            let geom = geometry::build(
                &bound,
                mol.n_atoms,
                &mol.bonds,
                &RepParams::for_kind(RepKind::Cartoon),
                mol.reps[0].color_spec(),
                Material::Opaque,
                Some(&ss),
                false,
            );
            assert!(
                geom.mesh.vertices.len() > 100,
                "empty protein preview for {algorithm:?}"
            );
        }
    }

    #[test]
    fn every_settings_style_and_material_page_fits_the_dialog() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut settings = Settings::default();
        for kind in RepKind::ALL {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let mut selected = kind;
                    settings_page_styles(ui, &mut settings, &mut selected, None);
                    assert!(
                        ui.min_rect().width() <= 640.5,
                        "style page overhangs: {kind:?}, width {}",
                        ui.min_rect().width()
                    );
                },
            );
        }
        for material in Material::ALL {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let mut selected = material;
                    settings_page_materials(ui, &mut settings, &mut selected, None);
                    assert!(
                        ui.min_rect().width() <= 640.5,
                        "material page overhangs: {material:?}"
                    );
                },
            );
        }
    }

    #[test]
    fn style_preview_sits_next_to_the_controls_without_an_empty_column() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut preview_left = 0.0;
        let output = ctx.run_ui(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 900.0))),
            ..Default::default()
        }, |ui| {
            settings_page_styles(ui, &mut Settings::default(), &mut RepKind::BallAndStick, None);
            preview_left = ui.min_rect().right() - PREVIEW_SIZE;
        });
        let controls_right = output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) => Some(rect.rect.right()),
            _ => None,
        }).fold(0.0_f32, f32::max);
        assert!(preview_left - controls_right <= 20.0,
            "preview gap is {} points", preview_left - controls_right);
    }
}

#[cfg(test)]
mod colors_ui_tests {
    use super::*;

    #[test]
    fn preset_dropdown_applies_colors_and_reports_custom_edits() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut colors = crate::ColorPreferences::default();
        colors.categorical = vec![[10, 20, 30, 255]];
        let palette = colors.categorical.clone();
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO, egui::vec2(680.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let text_position = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label =>
                    Some(text.pos + text.galley.size() * 0.5),
                _ => None,
            }).unwrap_or_else(|| panic!("missing label {label}"))
        };
        let click = |pos| [true, false].map(|pressed| input(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]));
        for preset in [crate::ColorPreset::Vmd, crate::ColorPreset::Molstar, crate::ColorPreset::Default] {
            let mut output = ctx.run_ui(input(vec![]), |ui| color_preset_selector(ui, &mut colors));
            for _ in 0..2 {
                output = ctx.run_ui(input(vec![]), |ui| color_preset_selector(ui, &mut colors));
            }
            let current = colors.preset().unwrap().name();
            for event in click(text_position(&output, current)) {
                let _ = ctx.run_ui(event, |ui| color_preset_selector(ui, &mut colors));
            }
            for _ in 0..3 {
                output = ctx.run_ui(input(vec![]), |ui| color_preset_selector(ui, &mut colors));
            }
            for event in click(text_position(&output, preset.name())) {
                let _ = ctx.run_ui(event, |ui| color_preset_selector(ui, &mut colors));
            }
            assert_eq!(colors.preset(), Some(preset));
            assert_eq!(colors.categorical, palette);
        }
        colors.continuous.midpoint = 0.3;
        let output = ctx.run_ui(input(vec![]), |ui| color_preset_selector(ui, &mut colors));
        text_position(&output, "Custom");
    }

    #[test]
    fn categorical_palette_fits_the_settings_window() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut palette = crate::color::ColorPreferences::default().categorical;
        for _ in 0..3 {
            let output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(680.0, 800.0))),
                ..Default::default()
            }, |ui| {
                ui.set_width(640.0);
                categorical_editor(ui, &mut palette);
                assert!(ui.min_rect().width() <= 640.0);
            });
            for index in 1..=palette.len() {
                assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == index.to_string())));
            }
        }
    }

    #[test]
    fn categorical_palette_controls_add_remove_and_open_color_picker() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut palette = crate::color::ColorPreferences::default().categorical;
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(680.0, 700.0),
            )),
            events,
            ..Default::default()
        };
        for (label, expected_len) in [("Add color", 19), ("×", 18)] {
            let mut center = None;
            for _ in 0..3 {
                let output = ctx.run_ui(input(vec![]), |ui| categorical_editor(ui, &mut palette));
                center = output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                });
            }
            let pos = center.expect("palette control drawn");
            for pressed in [true, false] {
                let _ = ctx.run_ui(
                    input(vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]),
                    |ui| categorical_editor(ui, &mut palette),
                );
            }
            assert_eq!(palette.len(), expected_len);
        }
        let output = ctx.run_ui(input(vec![]), |ui| categorical_editor(ui, &mut palette));
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.rect.size() == egui::vec2(34.0, 30.0) => {
                    Some(rect.rect.center())
                }
                _ => None,
            })
            .unwrap();
        for pressed in [true, false] {
            let _ = ctx.run_ui(
                input(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]),
                |ui| categorical_editor(ui, &mut palette),
            );
        }
        assert!(egui::Popup::is_any_open(&ctx));
    }

    #[test]
    fn dragging_gradient_midpoint_moves_the_color_stop() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut ramp = crate::color::ColorPreferences::default().continuous;
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(680.0, 500.0),
            )),
            events,
            ..Default::default()
        };
        let mut center = None;
        for _ in 0..3 {
            let output = ctx.run_ui(input(vec![]), |ui| {
                gradient_editor(
                    ui,
                    "test",
                    "Test gradient",
                    ["Low", "Midpoint", "High"],
                    &mut ramp,
                )
            });
            center = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Rect(handle)
                    if handle.rect.width() == 14.0
                        && handle.rect.height() == 36.0
                        && handle.fill == egui::Color32::WHITE =>
                {
                    Some(handle.rect.center())
                }
                _ => None,
            });
        }
        let start = center.expect("midpoint handle drawn on the bar");
        let end = start + egui::vec2(382.0 * 0.25, 0.0);
        for events in [
            vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            vec![egui::Event::PointerMoved(end)],
            vec![egui::Event::PointerButton {
                pos: end,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        ] {
            let _ = ctx.run_ui(input(events), |ui| {
                gradient_editor(
                    ui,
                    "test",
                    "Test gradient",
                    ["Low", "Midpoint", "High"],
                    &mut ramp,
                )
            });
        }
        assert!(
            (ramp.midpoint - 0.75).abs() < 0.001,
            "midpoint was {}",
            ramp.midpoint
        );
        assert_eq!(ramp.sample(ramp.midpoint), ramp.stops[1]);
        assert!(
            !egui::Popup::is_any_open(&ctx),
            "dragging must not open the color picker"
        );
    }

    #[test]
    fn ss_icon_opens_its_picker_and_uses_a_contrasting_glyph() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut color = [255, 255, 255, 255];
        let input = |events| egui::RawInput {
            events,
            ..Default::default()
        };
        let mut center = egui::Pos2::ZERO;
        for _ in 0..3 {
            let output = ctx.run_ui(input(vec![]), |ui| ss_icon(ui, 0, &mut color));
            center = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if rect.fill == egui::Color32::WHITE => {
                        Some(rect.rect.center())
                    }
                    _ => None,
                })
                .unwrap();
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Path(path) if path.stroke.color == egui::epaint::ColorMode::Solid(egui::Color32::BLACK))));
        }
        for pressed in [true, false] {
            let _ = ctx.run_ui(
                input(vec![
                    egui::Event::PointerMoved(center),
                    egui::Event::PointerButton {
                        pos: center,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]),
                |ui| ss_icon(ui, 0, &mut color),
            );
        }
        assert!(egui::Popup::is_any_open(&ctx));
    }

    #[test]
    fn other_elements_button_opens_the_full_periodic_table_window() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut settings = Settings::default();
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 850.0),
            )),
            events,
            ..Default::default()
        };
        let mut position = None;
        for _ in 0..3 {
            let output = ctx.run_ui(input(vec![]), |ui| {
                settings_page_colors(ui, &mut settings)
            });
            assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == "Xe")));
            position = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "Other elements..." => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            });
        }
        let pos = position.unwrap();
        for pressed in [true, false] {
            let _ = ctx.run_ui(
                input(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]),
                |ui| settings_page_colors(ui, &mut settings),
            );
        }
        let mut labels = Vec::new();
        for _ in 0..4 {
            let output = ctx.run_ui(input(vec![]), |ui| {
                settings_page_colors(ui, &mut settings)
            });
            labels = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.job.text.clone()),
                    _ => None,
                })
                .collect();
        }
        for label in ["Periodic table", "Xe", "Hg", "U", "Og"] {
            assert!(labels.iter().any(|text| text == label), "missing {label}");
        }
    }

    #[test]
    fn periodic_table_cells_have_equal_sizes_for_single_and_double_symbols() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut colors = crate::color::ColorPreferences::default();
        let mut output = None;
        for _ in 0..3 {
            output = Some(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 700.0),
                    )),
                    ..Default::default()
                },
                |ui| periodic_table(ui, &mut colors),
            ));
        }
        let output = output.unwrap();
        for number in [1, 2, 6, 17, 53, 54] {
            let color = colors.element(number);
            let fill = egui::Color32::from_rgb(color[0], color[1], color[2]);
            let rectangles: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if rect.fill == fill => Some(rect.rect.size()),
                    _ => None,
                })
                .collect();
            assert!(!rectangles.is_empty(), "missing element {number}");
            assert!(
                rectangles
                    .iter()
                    .all(|size| *size == egui::vec2(34.0, 30.0)),
                "element {number}: {rectangles:?}"
            );
        }
    }

    #[test]
    fn symbol_text_has_contrast_on_light_dark_and_saturated_cells() {
        assert_eq!(swatch_ink([255; 4]), egui::Color32::BLACK);
        assert_eq!(swatch_ink([0, 0, 0, 255]), egui::Color32::WHITE);
        assert_eq!(swatch_ink([0, 0, 255, 255]), egui::Color32::WHITE);
        assert_eq!(swatch_ink([255, 220, 48, 255]), egui::Color32::BLACK);
    }

    #[test]
    fn new_representations_and_duplicates_use_the_active_palette() {
        let mut settings = Settings::default();
        settings.colors.elements.insert(6, [20, 100, 200, 255]);
        let defaults = App::effective_rep_defaults(&settings);
        let rep = crate::scene::Representation::from_defaults(&defaults);
        assert_eq!(*rep.color_spec().preferences, settings.colors);
        assert_eq!(*rep.duplicate().color_spec().preferences, settings.colors);
    }

    #[test]
    fn clicking_an_element_opens_its_color_picker() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut settings = Settings::default();
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(680.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let mut center = None;
        for _ in 0..3 {
            let output = ctx.run_ui(input(vec![]), |ui| {
                settings_page_colors(ui, &mut settings)
            });
            center = output.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == "H" => {
                    Some(t.pos + t.galley.size() * 0.5)
                }
                _ => None,
            });
        }
        let pos = center.expect("hydrogen cell drawn");
        for pressed in [true, false] {
            let _ = ctx.run_ui(
                input(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]),
                |ui| settings_page_colors(ui, &mut settings),
            );
        }
        assert!(egui::Popup::is_any_open(&ctx));
        assert!(
            settings.colors.elements.is_empty(),
            "opening a picker must not create an override"
        );
    }

    #[test]
    fn colors_page_draws_all_editors_without_changing_preferences() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut settings = Settings::default();
        let before = settings.colors.clone();
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(680.0, 800.0),
                )),
                ..Default::default()
            },
            |ui| settings_page_colors(ui, &mut settings),
        );
        assert_eq!(settings.colors, before);
        let text: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.job.text.as_str()),
                _ => None,
            })
            .collect();
        for label in [
            "H",
            "C",
            "N",
            "O",
            "I",
            "Other elements...",
            "α helix",
            "β sheet",
            "Coil",
            "0",
            "1",
            "Discrete palette",
            "Add color",
        ] {
            assert!(text.contains(&label), "missing {label}");
        }
    }
}
