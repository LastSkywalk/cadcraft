//! The Enhanced Attribute Editor: a double-click on a block reference with attributes (or on
//! one of its attributes) opens it, as in AutoCAD. It lists the reference's attributes (tag,
//! prompt, value) with the one clicked current; the value box edits the current one. Apply and
//! OK write the changed values with ATTEDIT (one undo step), Cancel leaves them. ATTEDIT or
//! EATTEDIT typed or from a menu with such a reference selected opens it too ([`route`]).

use cadcraft_engine::doc::{EntityKind, Handle};
use egui::{RichText, Sense, vec2};
use serde_json::{Map, Value, json};

use crate::CadApp;
use crate::theme::Tokens;

/// One attribute row.
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub tag: String,
    pub prompt: String,
    pub value: String,
    /// The value the drawing has.
    pub saved: String,
}

/// The dialog's state while it is open.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub insert: Option<Handle>,
    /// The block's name (a dynamic block's own name, not its anonymous variant).
    pub block: String,
    pub rows: Vec<Row>,
    pub current: usize,
    pub error: Option<String>,
}

/// Whether block reference `ins` has attributes to edit (constant ones have none).
pub fn has_editable(ins: &cadcraft_engine::doc::Insert) -> bool {
    ins.attribs.iter().any(|a| !a.constant)
}

/// ATTEDIT / EATTEDIT typed or from a menu, with one block reference that has attributes to edit
/// selected: the editor opens on it. Otherwise (and for JSON calls) the command runs as usual,
/// asking at the command line.
pub fn route(app: &mut CadApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if !matches!(id, "attedit" | "ate" | "eattedit") || !params.is_null() {
        return None;
    }
    let [h] = app.session.selection()[..] else { return None };
    open(app, h, None).then_some(Ok(Value::Null))
}

/// Open the editor on block reference `h`, with the attribute nearest `at` (world) current.
/// Returns false when `h` has no attribute to edit.
pub fn open(app: &mut CadApp, h: Handle, at: Option<cadcraft_geom::Vec2>) -> bool {
    let Ok(d) = app.session.doc() else { return false };
    let Some(EntityKind::Insert(ins)) = d.entity(h).map(|e| &e.kind) else { return false };
    if !has_editable(ins) {
        return false;
    }
    let editable: Vec<_> = ins.attribs.iter().filter(|a| !a.constant).collect();
    let rows: Vec<Row> = editable
        .iter()
        .map(|a| Row { tag: a.tag.clone(), prompt: a.prompt.clone(), value: a.text.value.clone(), saved: a.text.value.clone() })
        .collect();
    let current = at
        .and_then(|p| {
            editable
                .iter()
                .enumerate()
                .filter(|(_, a)| !a.invisible)
                .map(|(i, a)| (i, (a.text.insert.xy() - p).len()))
                .filter(|(_, dist)| dist.is_finite())
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        })
        .unwrap_or(0);
    let block = crate::bedit::definition_name(d, h, &ins.block);
    app.eattedit = State { insert: Some(h), block, rows, current, error: None };
    app.ui.dialog = Some("eattedit".into());
    true
}

/// Write the changed values; false (with the error shown) when ATTEDIT refused them.
fn apply(app: &mut CadApp) -> bool {
    let Some(h) = app.eattedit.insert else { return true };
    let changed: Map<String, Value> =
        app.eattedit.rows.iter().filter(|r| r.value != r.saved).map(|r| (r.tag.clone(), Value::String(r.value.clone()))).collect();
    if changed.is_empty() {
        return true;
    }
    match app.run("attedit", json!({ "handle": h.hex(), "values": changed })) {
        Ok(_) => {
            for r in &mut app.eattedit.rows {
                r.saved = r.value.clone();
            }
            app.eattedit.error = None;
            true
        }
        Err(e) => {
            app.eattedit.error = Some(e);
            false
        }
    }
}

/// The Enhanced Attribute Editor window.
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    // The reference went away (undo, another command): nothing left to edit.
    let alive = app.eattedit.insert.is_some_and(|h| app.session.doc().ok().and_then(|d| d.entity(h)).is_some());
    if !alive {
        *open = false;
        return;
    }
    let (mut ok, mut cancel, mut applied) = (false, false, false);
    let st = &mut app.eattedit;
    egui::Window::new(crate::tl!("Enhanced Attribute Editor"))
        .id(egui::Id::new("Enhanced Attribute Editor"))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            let tag = st.rows.get(st.current).map(|r| r.tag.clone()).unwrap_or_default();
            egui::Grid::new("eattedit-head").num_columns(2).spacing(vec2(8.0, 2.0)).show(ui, |ui| {
                ui.label(RichText::new(crate::tl!("Block:")).color(t.text_dim));
                ui.label(&st.block);
                ui.end_row();
                ui.label(RichText::new(crate::tl!("Tag:")).color(t.text_dim));
                ui.label(&tag);
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.label(RichText::new(crate::tl!("Attribute")).strong());
            egui::Frame::new().fill(t.field).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(4.0).inner_margin(4.0).show(ui, |ui| {
                ui.set_min_width(460.0);
                egui::ScrollArea::vertical().id_salt("eattedit-rows").max_height(220.0).show(ui, |ui| {
                    egui::Grid::new("eattedit-rows-grid").num_columns(3).striped(true).min_col_width(120.0).show(ui, |ui| {
                        ui.label(RichText::new(crate::tl!("Tag")).color(t.text_dim));
                        ui.label(RichText::new(crate::tl!("Prompt")).color(t.text_dim));
                        ui.label(RichText::new(crate::tl!("Value")).color(t.text_dim));
                        ui.end_row();
                        for (i, r) in st.rows.iter().enumerate() {
                            let on = i == st.current;
                            let a = ui.selectable_label(on, &r.tag);
                            let b = ui.add(egui::Label::new(&r.prompt).sense(Sense::click()));
                            let c = ui.add(egui::Label::new(&r.value).sense(Sense::click()));
                            if a.clicked() || b.clicked() || c.clicked() {
                                st.current = i;
                            }
                            ui.end_row();
                        }
                    });
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Value:"));
                if let Some(r) = st.rows.get_mut(st.current) {
                    let edit = ui.add(egui::TextEdit::singleline(&mut r.value).id(egui::Id::new("eattedit-value")).desired_width(400.0));
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        ok = true;
                    }
                }
            });
            if let Some(e) = &st.error {
                ui.colored_label(t.warn, e);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(crate::tl!("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button(crate::tl!("OK")).clicked() {
                        ok = true;
                    }
                    let dirty = st.rows.iter().any(|r| r.value != r.saved);
                    if ui.add_enabled(dirty, egui::Button::new(crate::tl!("Apply"))).clicked() {
                        applied = true;
                    }
                });
            });
        });
    if applied {
        apply(app);
    }
    if cancel || (ok && apply(app)) {
        *open = false;
    }
    if !*open {
        app.eattedit = State::default();
    }
}
