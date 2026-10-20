//! What a double-click on an object does, as AutoCAD's default double-click actions:
//!
//! - text, attribute definitions, multiline text, dimensions and multileaders with text: TEXTEDIT
//!   with the object picked, which opens the in-place editor on its text (a one-line box for
//!   single-line text, the multiline editor otherwise; a dimension shows its measurement);
//! - a block reference with attributes: the Enhanced Attribute Editor, with the attribute clicked
//!   current;
//! - another block reference (a dynamic one too): the Edit Block Definition dialog with its block
//!   chosen, which opens the Block Editor on OK;
//! - a polyline: PEDIT on it; a spline: SPLINEDIT on it; a hatch: the Hatch Edit dialog;
//! - anything else: the object selected and the Properties palette open.
//!
//! Viewports (double-click to work inside one) are handled before this, by the canvas.

use cadcraft_engine::Input;
use cadcraft_engine::doc::{Drawing, EntityKind, Handle};
use cadcraft_geom::Vec2;
use serde_json::Value;

use crate::CadApp;

/// What a double-click on an object does.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// TEXTEDIT on it (in place).
    EditText,
    /// The Enhanced Attribute Editor on a block reference.
    Attributes,
    /// The Edit Block Definition dialog, with this block chosen.
    EditBlock(String),
    Pedit,
    Splinedit,
    HatchEdit,
    Properties,
}

/// The block definition a double-click on reference `h` edits: a dynamic block's own name rather
/// than its anonymous variant; `None` for an external reference or an anonymous block of no
/// dynamic block (nothing the Block Editor offers).
fn block_to_edit(d: &Drawing, h: Handle, name: &str) -> Option<String> {
    if let Some(r) = cadcraft_engine::doc::dynblock::reference(d, h) {
        return Some(r.definition);
    }
    let b = d.block(name)?;
    (!b.anonymous && !b.name.starts_with('*') && b.xref_path.is_none()).then(|| b.name.clone())
}

/// The action for object `h` of drawing `d`.
pub fn action_for(d: &Drawing, h: Handle) -> Option<Action> {
    let e = d.entity(h)?;
    Some(match &e.kind {
        EntityKind::Text(_) | EntityKind::AttDef(_) | EntityKind::MText(_) | EntityKind::Dimension(_) => Action::EditText,
        EntityKind::MLeader(m) if m.text.is_some() => Action::EditText,
        EntityKind::Insert(i) if i.attribs.iter().any(|a| !a.constant) => Action::Attributes,
        EntityKind::Insert(i) => block_to_edit(d, h, &i.block).map_or(Action::Properties, Action::EditBlock),
        EntityKind::LwPolyline(_) => Action::Pedit,
        EntityKind::Spline(_) => Action::Splinedit,
        EntityKind::Hatch(_) => Action::HatchEdit,
        _ => Action::Properties,
    })
}

/// Start a command on the object at `p` (world), picked as if clicked at its prompt.
fn start_on(app: &mut CadApp, id: &str, p: Vec2) {
    app.session.set_selection(Vec::new());
    app.start(id);
    if let Err(e) = app.session.input(Input::Point(p)) {
        app.session.echo(e.to_string());
    }
}

/// Run the double-click action on the object at `p` (world); false when nothing is there.
pub fn at(app: &mut CadApp, p: Vec2) -> bool {
    let Some(h) = cadcraft_engine::cmd::curves::pick_at(&app.session, p) else { return false };
    let Some(action) = app.session.doc().ok().and_then(|d| action_for(d, h)) else { return false };
    run(app, h, action, p);
    true
}

/// Run `action` on object `h`, double-clicked at `p` (world).
pub fn run(app: &mut CadApp, h: Handle, action: Action, p: Vec2) {
    app.session.pending_window = None;
    app.canvas.hot_grips.clear();
    match action {
        Action::EditText => start_on(app, "textedit", p),
        Action::Pedit => start_on(app, "pedit", p),
        Action::Splinedit => start_on(app, "splinedit", p),
        Action::Attributes => {
            if !crate::eattedit::open(app, h, Some(p)) {
                app.session.set_selection(vec![h]);
                let _ = app.run("properties", Value::Null);
            }
        }
        Action::EditBlock(name) => {
            app.session.set_selection(Vec::new());
            crate::bedit::open_on(app, name);
        }
        Action::HatchEdit => {
            app.session.set_selection(vec![h]);
            crate::hatchdlg::open_edit(app);
        }
        Action::Properties => {
            app.session.set_selection(vec![h]);
            if let Err(e) = app.run("properties", Value::Null) {
                app.session.echo(e);
            }
        }
    }
}
