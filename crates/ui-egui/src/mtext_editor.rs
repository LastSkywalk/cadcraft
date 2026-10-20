//! The in-place MTEXT editor: while MTEXT asks for its text, a text box opens on the canvas at
//! the text's location and width with a formatting toolbar (font, height, bold, italic,
//! underline, overline, colour, alignment, lists, tabs) and a ruler for the paragraph's indents
//! and tab stops. The text and its formats live in the engine's
//! [`cadcraft_engine::mtext_edit::Formatted`]; this file only draws and forwards edits.

use std::sync::Arc;

use cadcraft_engine::mtext_edit::{CharFmt, Flag, Formatted, ListStyle, MTextColor, ParaAlign, TabKind};
use egui::text::{CCursor, CCursorRange, LayoutJob};
use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Shape, Stroke, TextFormat, pos2, vec2};

use crate::CadApp;
use crate::theme::Tokens;

mod measure;

/// Smallest and largest size of the text in the box, in points: the box stays readable at any zoom.
const MIN_PX: f32 = 13.0;
const MAX_PX: f32 = 36.0;
/// Families the font list offers besides the one set in the text.
const FAMILIES: [&str; 6] = ["Arial", "Times New Roman", "Courier New", "Verdana", "Georgia", "Calibri"];
/// The standard colours (ACI 1..=7) of the colour menu.
const SWATCHES: [u16; 7] = [1, 2, 3, 4, 5, 6, 7];
/// Narrowest the editor gets (the toolbar then takes two rows) and its width for text with no
/// width of its own (a dimension's, a multileader's without one).
const MIN_WIDTH: f32 = 330.0;
const DEFAULT_WIDTH: f32 = 420.0;
/// Size of a toolbar button.
const TOOL: f32 = 24.0;
/// Ruler snap, in text heights.
const SNAP: f64 = 0.25;

#[derive(Clone)]
struct State {
    fmt: Formatted,
    /// Format of the next typed character when nothing is selected.
    typing: CharFmt,
    /// Caret the `typing` format was set for.
    typing_at: usize,
    /// Height of the toolbar and ruler above the text box (last frame).
    top: f32,
    /// Selection (characters) of the last frame, for toolbar actions of this one.
    sel: (usize, usize),
    /// A caret position to set on the text box in the next frame (after a toolbar insertion).
    caret: Option<usize>,
    /// The text box has had the keyboard focus once (it takes it when it opens).
    focused: bool,
    /// A dimension's measured value (plain text), when its text is edited.
    measured: Option<String>,
    /// Where the measured value is in the text (characters), shown gray; `None`: the text is
    /// fixed.
    value: Option<(usize, usize)>,
    /// The text `value` was last placed in.
    seen: String,
}

fn state_id() -> egui::Id {
    egui::Id::new("mtext_editor_state")
}

fn edit_id() -> egui::Id {
    egui::Id::new("mtext_editor_text")
}

/// The ACI index or true colour as a screen colour.
fn color32(c: MTextColor) -> Color32 {
    let rgb = match c {
        MTextColor::Aci(v) => cadcraft_color::aci_rgb(u8::try_from(v).unwrap_or(7)),
        MTextColor::Rgb(v) => cadcraft_color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8),
        MTextColor::ByLayer | MTextColor::ByBlock => cadcraft_color::aci_rgb(7),
    };
    Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

/// Show the editor while MTEXT collects its text; clears its state when it isn't running.
pub fn show(app: &mut CadApp, ctx: &egui::Context) {
    let mtext = app.session.editing_mtext();
    let asking = app.session.current_prompt().is_some_and(|p| p.accept.text && !p.accept.point);
    let (Some(mt), true) = (mtext, asking) else {
        ctx.data_mut(|d| d.remove::<State>(state_id()));
        return;
    };
    let t = Tokens::get();
    let (rect, scale) = app.canvas.xf.map_or((ctx.content_rect(), 20.0), |x| (x.rect, x.scale.max(1e-6)));
    let mut st = ctx.data_mut(|d| d.get_temp::<State>(state_id())).unwrap_or_else(|| {
        // TEXTEDIT opens the editor on an object's text; MTEXT on an empty box.
        let mut fmt = if mt.contents.is_empty() { Formatted::default() } else { Formatted::from_contents(&mt.contents) };
        if let Some(f) = app.session.doc().ok().and_then(|d| d.text_style(&mt.style).map(|s| s.font.clone())) {
            // A family name, not a font file (`txt.shx`, `arial.ttf`).
            if !f.is_empty() && !f.contains('.') {
                fmt.base_font = f;
            }
        }
        // A dimension: its measurement where the text has `<>`.
        let measured = app.session.editing_measurement().map(|m| Formatted::from_contents(&m).text().to_string());
        let mut value = None;
        if let Some(m) = &measured
            && let Some(b) = fmt.text().find("<>")
        {
            let at = fmt.text()[..b].chars().count();
            let typing = fmt.fmt_at(at);
            let text = measure::splice(fmt.text(), (at, at + 2), m);
            fmt.set_text(&text, &typing);
            value = Some((at, at + m.chars().count()));
        }
        let caret = Some(fmt.len());
        let seen = fmt.text().to_string();
        State { fmt, typing: CharFmt::default(), typing_at: 0, top: 70.0, sel: (0, 0), caret, focused: false, measured, value, seen }
    });
    let base_h = if mt.height > 0.0 { mt.height } else { 0.2 };
    let px = ((base_h * scale) as f32).clamp(MIN_PX, MAX_PX);
    // The text's own width when it has one, never narrower than the toolbar's two rows nor
    // wider than most of the canvas; the toolbar and buttons wrap within it.
    let widest = (rect.width() * 0.9).max(MIN_WIDTH);
    let width = if mt.width > 0.0 { ((mt.width * scale) as f32).clamp(MIN_WIDTH, widest) } else { DEFAULT_WIDTH.min(widest) };
    // The box's top-left corner on screen (rotation is ignored while editing).
    let code = usize::from(mt.attach.clamp(1, 9) - 1);
    let (col, row) = ((code % 3) as f64, (code / 3) as f64);
    let ww = f64::from(width) / scale;
    let corner = cadcraft_geom::Vec2::new(mt.insert.x - col * ww / 2.0, mt.insert.y + row * base_h * 2.0);
    let at = app.canvas.xf.map_or(rect.center(), |x| x.to_screen(corner));
    let max_x = (rect.right() - width - 8.0).max(rect.left());
    let pos = pos2(at.x.clamp(rect.left(), max_x), at.y.clamp(rect.top() + st.top, (rect.bottom() - 120.0).max(rect.top() + st.top)));

    let mut done = Done::No;
    let top_start = pos.y - st.top;
    egui::Area::new(egui::Id::new("mtext_editor_area")).order(egui::Order::Foreground).fixed_pos(pos2(pos.x, top_start)).show(ctx, |ui| {
        egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.accent)).inner_margin(egui::Margin::same(0)).show(ui, |ui| {
            ui.set_width(width);
            let y0 = ui.cursor().top();
            toolbar(ui, &mut st, base_h, width);
            follow_value(&mut st);
            ruler(ui, &mut st, px, width, base_h);
            st.top = ui.cursor().top() - y0;
            text_box(ui, &mut st, base_h, px, width, &t);
            follow_value(&mut st);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(4.0);
                let finish = if ui.ctx().os().is_mac() { crate::tl!("⌘↩ to finish") } else { crate::tl!("Ctrl+Enter to finish") };
                if ui.button(crate::tl!("OK")).on_hover_text(finish).clicked() {
                    done = Done::Ok;
                }
                if ui.button(crate::tl!("Cancel")).clicked() {
                    done = Done::Cancel;
                }
                measurement_buttons(ui, &mut st);
            });
        });
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        done = Done::Cancel;
    } else if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
        done = Done::Ok;
    }
    match done {
        Done::No => {
            ctx.data_mut(|d| d.insert_temp(state_id(), st));
        }
        Done::Ok => {
            ctx.data_mut(|d| d.remove::<State>(state_id()));
            // The editor holds the whole contents: one input, then Enter ends MTEXT's text entry.
            // TEXTEDIT takes the text as it is, even emptied.
            let editing = app.session.running.as_ref().is_some_and(|r| r.id == "textedit");
            // A dimension's measured value goes back as `<>`.
            if let Some((a, b)) = st.value {
                let typing = st.fmt.fmt_at(a);
                let text = measure::splice(st.fmt.text(), (a, b), "<>");
                st.fmt.set_text(&text, &typing);
            }
            if !st.fmt.is_empty() || editing {
                let _ = app.session.input(cadcraft_engine::Input::Text(st.fmt.to_contents()));
            }
            if app.session.running.as_ref().is_some_and(|r| r.id == "mtext") {
                let _ = app.session.input(cadcraft_engine::Input::Enter);
            }
        }
        Done::Cancel => {
            ctx.data_mut(|d| d.remove::<State>(state_id()));
            app.session.cancel();
        }
    }
}

/// Keep the measured value's place after an edit (one that reaches into it removes it).
fn follow_value(st: &mut State) {
    if st.fmt.text() == st.seen {
        return;
    }
    if let Some(v) = st.value {
        let (text, value, caret) = measure::follow(&st.seen, st.fmt.text(), v);
        if text != st.fmt.text() {
            let typing = st.typing.clone();
            st.fmt.set_text(&text, &typing);
        }
        st.value = value;
        if caret.is_some() {
            st.caret = caret;
        }
    }
    st.seen = st.fmt.text().to_string();
}

/// A dimension's buttons: back to the measured value alone, or the measured value put in at the
/// caret (when the text is fixed).
fn measurement_buttons(ui: &mut egui::Ui, st: &mut State) {
    let Some(m) = st.measured.clone() else { return };
    ui.add_space(GAP);
    let reset = ui
        .button(crate::tl!("Reset to Measurement"))
        .on_hover_text(crate::tl!("The text becomes the measured value alone, which follows the dimension when it is stretched"));
    if reset.clicked() {
        st.fmt.set_text(&m, &CharFmt::default());
        st.value = Some((0, m.chars().count()));
        st.seen = st.fmt.text().to_string();
        st.caret = Some(m.chars().count());
        st.focused = false;
    }
    let insert = ui
        .add_enabled(st.value.is_none(), egui::Button::new(crate::tl!("Insert Measurement")))
        .on_hover_text(crate::tl!("Puts the measured value in at the caret: it follows the dimension, the text around it stays"));
    if insert.clicked() {
        let at = st.sel.0.min(st.sel.1).min(st.fmt.len());
        let typing = st.typing.clone();
        let text = measure::splice(st.fmt.text(), (at, at), &m);
        st.fmt.set_text(&text, &typing);
        st.value = Some((at, at + m.chars().count()));
        st.seen = st.fmt.text().to_string();
        st.caret = Some(at + m.chars().count());
        st.focused = false;
    }
}

#[derive(PartialEq)]
enum Done {
    No,
    Ok,
    Cancel,
}

/// Change the selected characters' format, or the next typed ones' when nothing is selected.
fn change(st: &mut State, f: impl Fn(&mut CharFmt)) {
    let (a, b) = st.sel;
    if a == b {
        f(&mut st.typing);
    } else {
        st.fmt.edit(a.min(b)..a.max(b), f);
    }
}

fn current(st: &State) -> CharFmt {
    let (a, b) = st.sel;
    if a == b { st.typing.clone() } else { st.fmt.fmt_at(a.min(b)) }
}

fn toggle(st: &mut State, flag: Flag) {
    let (a, b) = st.sel;
    if a == b {
        let on = !flag_of(&st.typing, flag);
        change(st, |c| set_flag(c, flag, on));
    } else {
        st.fmt.toggle(a.min(b)..a.max(b), flag);
    }
}

fn flag_of(c: &CharFmt, f: Flag) -> bool {
    match f {
        Flag::Bold => c.bold,
        Flag::Italic => c.italic,
        Flag::Underline => c.underline,
        Flag::Overline => c.overline,
    }
}

fn set_flag(c: &mut CharFmt, f: Flag, on: bool) {
    match f {
        Flag::Bold => c.bold = on,
        Flag::Italic => c.italic = on,
        Flag::Underline => c.underline = on,
        Flag::Overline => c.overline = on,
    }
}

fn toolbar(ui: &mut egui::Ui, st: &mut State, base_h: f64, width: f32) {
    let cur = current(st);
    let range = st.sel.0.min(st.sel.1)..st.sel.0.max(st.sel.1);
    let mut refocus = false;
    ui.horizontal_wrapped(|ui| {
        ui.add_space(4.0);
        // Font
        let shown = cur.font.clone().unwrap_or_else(|| st.fmt.base_font.clone());
        egui::ComboBox::from_id_salt("mtext_font").width(120.0).selected_text(shown.clone()).show_ui(ui, |ui| {
            for fam in FAMILIES.iter().map(|s| s.to_string()).chain(cur.font.clone().filter(|f| !FAMILIES.contains(&f.as_str()))) {
                if ui.selectable_label(shown == fam, &fam).clicked() {
                    change(st, |c| c.font = Some(fam.clone()));
                    refocus = true;
                }
            }
        });
        // Height
        let mut h = cur.height.unwrap_or(base_h);
        if ui
            .add(egui::DragValue::new(&mut h).speed(base_h * 0.02).range(base_h * 0.05..=base_h * 50.0).max_decimals(4))
            .on_hover_text(crate::tl!("Text height"))
            .changed()
        {
            let none = (h - base_h).abs() < 1e-9;
            change(st, |c| c.height = (!none).then_some(h));
        }
        ui.add_space(GAP);
        ui.horizontal(|ui| {
            for (flag, label, tip) in [
                (Flag::Bold, RichText::new("B").strong(), crate::tl!("Bold")),
                (Flag::Italic, RichText::new("I").italics(), crate::tl!("Italic")),
                (Flag::Underline, RichText::new("U").underline(), crate::tl!("Underline")),
            ] {
                if tool_text(ui, label, flag_of(&cur, flag)).on_hover_text(tip).clicked() {
                    toggle(st, flag);
                    refocus = true;
                }
            }
            if tool(ui, Glyph::Overline, flag_of(&cur, Flag::Overline)).on_hover_text(crate::tl!("Overline")).clicked() {
                toggle(st, Flag::Overline);
                refocus = true;
            }
            // Colour: an A underlined in the text's colour.
            let swatch = cur.color.map_or(ui.visuals().text_color(), color32);
            ui.menu_button(RichText::new("A").color(swatch).strong().underline(), |ui| {
                if ui.button(crate::tl!("ByLayer")).clicked() {
                    change(st, |c| c.color = None);
                    refocus = true;
                    ui.close();
                }
                ui.horizontal(|ui| {
                    for v in SWATCHES {
                        let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                        ui.painter().rect_filled(r, 2.0, color32(MTextColor::Aci(v)));
                        if resp.clicked() {
                            change(st, |c| c.color = Some(MTextColor::Aci(v)));
                            refocus = true;
                            ui.close();
                        }
                    }
                });
                let mut rgb = cur.color.map_or([255, 255, 255], |c| {
                    let k = color32(c);
                    [k.r(), k.g(), k.b()]
                });
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    let v = (u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2]);
                    change(st, |c| c.color = Some(MTextColor::Rgb(v)));
                }
            })
            .response
            .on_hover_text(crate::tl!("Text color"));
        });
        // Character formats on one row, paragraphs and symbols on the next, unless both fit.
        if width < ONE_ROW_WIDTH {
            ui.end_row();
            ui.add_space(4.0);
        } else {
            ui.add_space(GAP);
        }
        // Paragraph alignment
        let para = st.fmt.para(st.fmt.paragraph_of(range.start));
        ui.horizontal(|ui| {
            for (al, glyph, tip) in [
                (ParaAlign::Left, Glyph::AlignLeft, crate::tl!("Left")),
                (ParaAlign::Center, Glyph::AlignCenter, crate::tl!("Center")),
                (ParaAlign::Right, Glyph::AlignRight, crate::tl!("Right")),
                (ParaAlign::Justify, Glyph::Justify, crate::tl!("Justify")),
            ] {
                let on = para.align == al;
                if tool(ui, glyph, on).on_hover_text(tip).clicked() {
                    st.fmt.edit_paras(range.clone(), |p| p.align = if on { ParaAlign::Default } else { al });
                    refocus = true;
                }
            }
        });
        ui.add_space(GAP);
        // Lists and special characters
        ui.horizontal(|ui| {
            if tool(ui, Glyph::Bullets, false).on_hover_text(crate::tl!("Bulleted list")).clicked() {
                st.fmt.make_list(range.clone(), ListStyle::Bullet, 3.0 * base_h);
                refocus = true;
            }
            if tool(ui, Glyph::Numbers, false).on_hover_text(crate::tl!("Numbered list")).clicked() {
                st.fmt.make_list(range.clone(), ListStyle::Number, 3.0 * base_h);
                refocus = true;
            }
            if tool(ui, Glyph::Tab, false).on_hover_text(crate::tl!("Insert tab")).clicked() {
                insert(st, "\t");
                refocus = true;
            }
            for (label, tip, code) in [("°", crate::tl!("Degree"), "°"), ("±", crate::tl!("Plus/minus"), "±")] {
                if tool_text(ui, RichText::new(label), false).on_hover_text(tip).clicked() {
                    insert(st, code);
                    refocus = true;
                }
            }
            if tool(ui, Glyph::Diameter, false).on_hover_text(crate::tl!("Diameter")).clicked() {
                insert(st, "⌀");
                refocus = true;
            }
        });
    });
    if refocus {
        ui.ctx().memory_mut(|m| m.request_focus(edit_id()));
    }
}

/// Space between toolbar groups.
const GAP: f32 = 6.0;
/// Narrowest editor whose toolbar is one row.
const ONE_ROW_WIDTH: f32 = 660.0;

/// A toolbar picture drawn in code: the interface font has no glyph for some of these on every
/// system (they showed as empty boxes on Windows).
#[derive(Clone, Copy)]
enum Glyph {
    Overline,
    AlignLeft,
    AlignCenter,
    AlignRight,
    Justify,
    Bullets,
    Numbers,
    Tab,
    Diameter,
}

/// A toolbar button's frame: a square, highlighted when `on` or hovered.
fn tool_frame(ui: &mut egui::Ui, on: bool) -> (egui::Response, Rect, Stroke) {
    let resp = ui.allocate_response(vec2(TOOL, TOOL), Sense::click());
    let v = ui.style().interact_selectable(&resp, on);
    if on || resp.hovered() {
        ui.painter().rect(resp.rect, v.corner_radius, v.weak_bg_fill, v.bg_stroke, egui::StrokeKind::Inside);
    }
    let r = resp.rect;
    (resp, r, Stroke::new(1.4, v.fg_stroke.color))
}

/// A toolbar button showing `label`.
fn tool_text(ui: &mut egui::Ui, label: RichText, on: bool) -> egui::Response {
    let (resp, r, stroke) = tool_frame(ui, on);
    let galley =
        egui::WidgetText::from(label.color(stroke.color)).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button);
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, stroke.color);
    resp
}

/// A toolbar button showing `glyph`.
fn tool(ui: &mut egui::Ui, glyph: Glyph, on: bool) -> egui::Response {
    let (resp, r, stroke) = tool_frame(ui, on);
    let p = ui.painter();
    // A 14-point square in the middle: x, y from its top-left corner.
    let o = r.center() - vec2(7.0, 7.0);
    let at = |x: f32, y: f32| o + vec2(x, y);
    let line = |a: (f32, f32), b: (f32, f32)| p.line_segment([at(a.0, a.1), at(b.0, b.1)], stroke);
    // Four text lines, each (start, end) along x.
    let rows = |spans: [(f32, f32); 4]| {
        for (i, (a, b)) in spans.iter().enumerate() {
            let y = 1.5 + i as f32 * 3.7;
            line((*a, y), (*b, y));
        }
    };
    match glyph {
        Glyph::Overline => {
            line((2.0, 1.0), (12.0, 1.0));
            p.circle_stroke(at(7.0, 8.5), 4.5, stroke);
        }
        Glyph::AlignLeft => rows([(0.0, 14.0), (0.0, 9.0), (0.0, 14.0), (0.0, 7.0)]),
        Glyph::AlignCenter => rows([(0.0, 14.0), (2.5, 11.5), (0.0, 14.0), (3.5, 10.5)]),
        Glyph::AlignRight => rows([(0.0, 14.0), (5.0, 14.0), (0.0, 14.0), (7.0, 14.0)]),
        Glyph::Justify => rows([(0.0, 14.0); 4]),
        Glyph::Bullets | Glyph::Numbers => {
            for i in 0..3 {
                let y = 2.0 + i as f32 * 5.0;
                line((5.0, y), (14.0, y));
                if matches!(glyph, Glyph::Bullets) {
                    p.circle_filled(at(1.5, y), 1.4, stroke.color);
                } else {
                    // Short strokes standing for 1, 2, 3.
                    line((1.5, y - 1.8), (1.5, y + 1.8));
                }
            }
        }
        Glyph::Tab => {
            line((0.0, 7.0), (11.0, 7.0));
            line((7.0, 3.5), (11.0, 7.0));
            line((7.0, 10.5), (11.0, 7.0));
            line((13.5, 2.0), (13.5, 12.0));
        }
        Glyph::Diameter => {
            p.circle_stroke(at(7.0, 7.0), 5.0, stroke);
            line((1.0, 13.0), (13.0, 1.0));
        }
    }
    resp
}

/// Insert `s` at the caret (over the selection).
fn insert(st: &mut State, s: &str) {
    let (a, b) = (st.sel.0.min(st.sel.1), st.sel.0.max(st.sel.1));
    let chars: Vec<char> = st.fmt.text().chars().collect();
    let new: String = chars.iter().take(a).chain(s.chars().collect::<Vec<_>>().iter()).chain(chars.iter().skip(b)).collect();
    if st.fmt.set_text(&new, &st.typing.clone()) {
        let at = a + s.chars().count();
        st.sel = (at, at);
        st.typing_at = at;
        st.caret = Some(at);
    }
}

/// The ruler: `px` points per text height (`base_h` drawing units); paragraph indents and tab
/// stops are in drawing units, as `\p` codes are.
fn ruler(ui: &mut egui::Ui, st: &mut State, px: f32, width: f32, base_h: f64) {
    let t = Tokens::get();
    let range = st.sel.0.min(st.sel.1)..st.sel.0.max(st.sel.1);
    let p = st.fmt.para(st.fmt.paragraph_of(range.start));
    let (rect, bg) = ui.allocate_exact_size(vec2(width, 20.0), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, t.control);
    let tick = |u: f64| rect.left() + (u as f32) * px;
    let x_of = |v: f64| tick(v / base_h);
    let units = (f64::from(width) / f64::from(px)).floor() as i32;
    for u in 0..=units.clamp(0, 400) {
        let x = tick(f64::from(u));
        let major = u % 5 == 0;
        painter.line_segment([pos2(x, rect.bottom() - if major { 7.0 } else { 3.0 }), pos2(x, rect.bottom())], Stroke::new(1.0, t.text_faint));
        if major && u > 0 {
            painter.text(pos2(x + 2.0, rect.top() + 1.0), Align2::LEFT_TOP, u.to_string(), FontId::proportional(8.0), t.text_faint);
        }
    }
    let to_units = |x: f32| ((f64::from(x - rect.left()) / f64::from(px) / SNAP).round() * SNAP).clamp(0.0, 400.0) * base_h;
    // Tab stops: an L at the stop; a right-click removes one.
    for (pos, _kind) in &p.tabs {
        let x = x_of(*pos);
        painter.add(Shape::line(
            vec![pos2(x, rect.bottom() - 9.0), pos2(x, rect.bottom() - 2.0), pos2(x + 4.0, rect.bottom() - 2.0)],
            Stroke::new(1.5, t.accent),
        ));
    }
    let hover = bg.hover_pos();
    // Indent markers: the first line's above, the left indent's below.
    let first_abs = p.left + p.first;
    let mut drag = None;
    for (k, (u, up)) in [(first_abs, false), (p.left, true)].into_iter().enumerate() {
        let x = x_of(u);
        let y = if up { rect.bottom() - 1.0 } else { rect.top() + 1.0 };
        let dir = if up { -1.0 } else { 1.0 };
        let hit = Rect::from_center_size(pos2(x, y + dir * 4.0), vec2(10.0, 10.0));
        let r = ui.interact(hit, ui.id().with(("mtext_indent", k)), Sense::drag());
        let col = if r.hovered() || r.dragged() { t.accent } else { t.icon };
        painter.add(Shape::convex_polygon(vec![pos2(x - 4.0, y), pos2(x + 4.0, y), pos2(x, y + dir * 8.0)], col, Stroke::NONE));
        if r.dragged()
            && let Some(m) = r.interact_pointer_pos()
        {
            drag = Some((k, to_units(m.x)));
        }
    }
    if let Some((k, u)) = drag {
        if k == 0 {
            st.fmt.set_indents(range.clone(), u - p.left, p.left);
        } else {
            st.fmt.set_indents(range.clone(), p.first, u);
        }
    } else if bg.clicked()
        && let Some(m) = hover
    {
        st.fmt.add_tab(range.clone(), to_units(m.x), TabKind::Left);
    } else if bg.secondary_clicked()
        && let Some(m) = hover
    {
        st.fmt.remove_tab(range, to_units(m.x), 0.6 * base_h);
    }
    bg.on_hover_text(crate::tl!("Click the ruler to add a tab stop, right-click to remove one"));
}

fn text_box(ui: &mut egui::Ui, st: &mut State, base_h: f64, px: f32, width: f32, t: &Tokens) {
    let id = edit_id();
    if let Some(at) = st.caret.take()
        && let Some(mut s) = egui::TextEdit::load_state(ui.ctx(), id)
    {
        s.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(at))));
        s.store(ui.ctx(), id);
    }
    let fmt = st.fmt.clone();
    let value = st.value;
    let default_color = t.text;
    // The measured value's background (AutoCAD shows it as a gray field).
    let value_bg = Color32::from_rgba_unmultiplied(255, 255, 255, 46);
    let mut layouter = move |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| -> Arc<egui::Galley> {
        let mut job = LayoutJob::default();
        job.wrap.max_width = wrap;
        let s = text.as_str();
        let mut run = String::new();
        let mut cur: Option<(CharFmt, bool)> = None;
        let flush = |job: &mut LayoutJob, run: &mut String, (f, in_value): &(CharFmt, bool)| {
            if run.is_empty() {
                return;
            }
            let size = f.height.map_or(px, |h| (px * (h / base_h) as f32).clamp(6.0, 120.0));
            let color = f.color.map_or(default_color, color32);
            job.append(
                run,
                0.0,
                TextFormat {
                    font_id: FontId::proportional(size),
                    color,
                    italics: f.italic,
                    underline: if f.underline { Stroke::new(1.0, color) } else { Stroke::NONE },
                    // The editor has one weight of each font: bold is shown wider.
                    extra_letter_spacing: if f.bold { size * 0.06 } else { 0.0 },
                    background: if *in_value { value_bg } else { Color32::TRANSPARENT },
                    ..TextFormat::default()
                },
            );
            run.clear();
        };
        for (i, c) in s.chars().enumerate() {
            let f = (fmt.fmt_at(i), value.is_some_and(|(a, b)| i >= a && i < b));
            if cur.as_ref() != Some(&f) {
                if let Some(p) = &cur {
                    flush(&mut job, &mut run, p);
                }
                cur = Some(f);
            }
            run.push(c);
        }
        if let Some(p) = &cur {
            flush(&mut job, &mut run, p);
        }
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut buf = st.fmt.text().to_string();
    let out = egui::TextEdit::multiline(&mut buf)
        .id(id)
        .desired_width(width)
        .desired_rows(5)
        .margin(egui::Margin::symmetric(4, 4))
        .lock_focus(true)
        // Opaque: the object being edited doesn't show through.
        .background_color(t.field.to_opaque())
        .char_limit(cadcraft_engine::mtext_edit::MAX_CHARS)
        .layouter(&mut layouter)
        .show(ui);
    if !out.response.has_focus() && ((buf.is_empty() && st.fmt.is_empty()) || !st.focused) {
        out.response.request_focus();
    }
    st.focused = true;
    if out.response.changed() {
        let typing = st.typing.clone();
        st.fmt.set_text(&buf, &typing);
    }
    if let Some(r) = out.cursor_range {
        let (a, b) = (r.primary.index.0, r.secondary.index.0);
        st.sel = (a, b);
        // The caret doesn't stop inside the measured value.
        if a == b
            && let Some(v) = st.value
            && measure::outside(a, v) != a
        {
            st.caret = Some(measure::outside(a, v));
        }
        if a == b {
            if out.response.changed() {
                st.typing_at = a;
            } else if a != st.typing_at {
                // The caret moved: typing continues the formatting of the character before it.
                st.typing = st.fmt.fmt_at(a.saturating_sub(1));
                st.typing_at = a;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadcraft_engine::Session;

    use crate::Services;

    #[test]
    fn editor_opens_only_for_mtext_text_entry_and_finishes_with_formatted_contents() {
        let ctx = egui::Context::default();
        let mut app = CadApp::new(Session::new(), Services::default());
        let frame = |app: &mut CadApp| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))), ..Default::default() };
            for _ in 0..2 {
                let mut out = ctx.run_ui(input.clone(), |ui| show(app, ui.ctx()));
                out.textures_delta.clear();
            }
        };
        frame(&mut app);
        assert!(ctx.data_mut(|d| d.get_temp::<State>(state_id())).is_none());
        app.start("mtext");
        assert!(app.session.editing_mtext().is_none(), "no box yet");
        app.session.input(cadcraft_engine::Input::Point(cadcraft_geom::Vec2::new(1.0, 1.0))).unwrap();
        app.session.input(cadcraft_engine::Input::Point(cadcraft_geom::Vec2::new(6.0, 0.0))).unwrap();
        let m = app.session.editing_mtext().expect("box known");
        assert!((m.width - 5.0).abs() < 1e-9, "{}", m.width);
        frame(&mut app);
        let mut st = ctx.data_mut(|d| d.get_temp::<State>(state_id())).expect("editor open");
        st.fmt.set_text("hello", &CharFmt::default());
        st.fmt.toggle(0..5, Flag::Bold);
        let contents = st.fmt.to_contents();
        app.session.input(cadcraft_engine::Input::Text(contents.clone())).unwrap();
        app.session.input(cadcraft_engine::Input::Enter).unwrap();
        let d = app.session.doc().unwrap();
        let found = d.model.iter().any(|e| matches!(&e.kind, cadcraft_engine::doc::EntityKind::MText(m) if m.contents == contents));
        assert!(found, "{contents}");
    }
}
