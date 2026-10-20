//! The in-place editor of a single-line text: TEXTEDIT on a text or an attribute definition (a
//! double-click on one) opens a one-line box over the text with its text selected, as AutoCAD
//! does. Enter or a click elsewhere keeps the edit, Esc leaves the text as it was; either way
//! TEXTEDIT goes on to ask for the next object.

use egui::text::{CCursor, CCursorRange};
use egui::{FontId, Stroke, pos2};

use crate::CadApp;
use crate::theme::Tokens;

/// Smallest and largest size of the text in the box, in points.
const MIN_PX: f32 = 13.0;
const MAX_PX: f32 = 48.0;
/// Longest text the box takes (characters).
const MAX_CHARS: usize = 65_536;

#[derive(Clone)]
struct State {
    text: String,
    /// The box has opened: focused, its text selected.
    opened: bool,
}

fn state_id() -> egui::Id {
    egui::Id::new("text_inplace_state")
}

fn edit_id() -> egui::Id {
    egui::Id::new("text_inplace_edit")
}

/// Show the box while TEXTEDIT asks for a single-line text's new text; clears its state otherwise.
pub fn show(app: &mut CadApp, ctx: &egui::Context) {
    let line = app.session.editing_line();
    let asking = app.session.current_prompt().is_some_and(|p| p.accept.text && !p.accept.point);
    let (Some(t), true) = (line, asking) else {
        ctx.data_mut(|d| d.remove::<State>(state_id()));
        return;
    };
    let tk = Tokens::get();
    let (rect, scale) = app.canvas.xf.map_or((ctx.content_rect(), 20.0), |x| (x.rect, x.scale.max(1e-6)));
    let mut st = ctx.data_mut(|d| d.get_temp::<State>(state_id())).unwrap_or_else(|| State { text: t.value.clone(), opened: false });
    let px = ((t.height.max(1e-9) * scale) as f32).clamp(MIN_PX, MAX_PX);
    // The box sits on the text's baseline at its insertion point (rotation is ignored while editing).
    let at = app.canvas.xf.map_or(rect.center(), |x| x.to_screen(t.insert.xy()));
    let top = at.y - px * 1.35;
    let pos =
        pos2(at.x.clamp(rect.left(), (rect.right() - 160.0).max(rect.left())), top.clamp(rect.top(), (rect.bottom() - px * 2.0).max(rect.top())));
    let width = ctx.fonts_mut(|f| f.layout_no_wrap(st.text.clone(), FontId::proportional(px), tk.text).size().x) + px * 2.0;
    let mut keep = false;
    let mut drop = false;
    egui::Area::new(egui::Id::new("text_inplace_area")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::NONE.fill(tk.panel).stroke(Stroke::new(1.0, tk.accent)).show(ui, |ui| {
            let mut out = egui::TextEdit::singleline(&mut st.text)
                .id(edit_id())
                .font(FontId::proportional(px))
                .desired_width(width.clamp(120.0, rect.width().max(120.0)))
                .char_limit(MAX_CHARS)
                .margin(egui::Margin::symmetric(4, 2))
                .show(ui);
            if !st.opened {
                out.response.request_focus();
                let n = st.text.chars().count();
                out.state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(n))));
                out.state.store(ui.ctx(), out.response.id);
                st.opened = true;
            } else if out.response.lost_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    drop = true;
                } else {
                    // Enter, or a click elsewhere.
                    keep = true;
                }
            }
        });
    });
    if !keep && !drop && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        drop = true;
    }
    if keep || drop {
        ctx.data_mut(|d| d.remove::<State>(state_id()));
        let input = if keep && st.text != t.value { cadcraft_engine::Input::Text(st.text) } else { cadcraft_engine::Input::Enter };
        if let Err(e) = app.session.input(input) {
            app.session.echo(e.to_string());
        }
    } else {
        ctx.data_mut(|d| d.insert_temp(state_id(), st));
    }
}
