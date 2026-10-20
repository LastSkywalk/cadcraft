//! A double-click on an object does what AutoCAD's default double-click action for it does
//! (#1321): text opens the in-place editor, a dimension the multiline editor with its
//! measurement, a block reference with attributes the Enhanced Attribute Editor, another block
//! reference the Edit Block Definition dialog, a polyline PEDIT, anything else the Properties
//! palette with the object selected.

use cadcraft_engine::doc::{EntityKind, Handle};
use cadcraft_engine::geom::Vec2;
use cadcraft_engine::{Input, Session};
use cadcraft_ui_egui::{CadApp, Services};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

fn harness(setup: impl FnOnce(&mut Session)) -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.canvas.zoom_pending = false;
    setup(&mut app.session);
    app.session.set_selection(Vec::new());
    // 60 frames a second, so two clicks fall within the double-click delay.
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.state_mut().canvas.zoom_pending = false;
    h.state_mut().session.execute("view.set", &json!({"center": [50, 50], "height": 100})).unwrap();
    h.run_steps(3);
    h
}

fn handle(v: Value) -> Handle {
    let h = v["handle"].as_str().or_else(|| v["handles"][0].as_str()).unwrap();
    Handle::parse_hex(h).unwrap()
}

fn screen(h: &Harness<'_, CadApp>, p: Vec2) -> Pos2 {
    h.state().canvas.xf.unwrap().to_screen(p)
}

/// Two clicks in quick succession at world point `p`.
fn double_click(h: &mut Harness<'_, CadApp>, p: Vec2) {
    let p = screen(h, p);
    h.event(Event::PointerMoved(p));
    h.run_steps(2);
    for _ in 0..2 {
        h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.run_steps(1);
        h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(3);
}

fn running(h: &Harness<'_, CadApp>) -> Option<String> {
    h.state().session.running.as_ref().map(|r| r.id.clone())
}

fn kind(h: &Harness<'_, CadApp>, e: Handle) -> EntityKind {
    h.state().session.doc().unwrap().entity(e).unwrap().kind.clone()
}

fn key(h: &mut Harness<'_, CadApp>, k: Key, modifiers: Modifiers) {
    h.event(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers });
    h.event(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers });
    h.run_steps(2);
}

#[test]
fn text_opens_a_one_line_box_over_it_with_its_text_selected() {
    let mut e = Handle(0);
    let mut h = harness(|s| e = handle(s.execute("text", &json!({"at": [40, 50], "text": "OLD", "height": 5})).unwrap()));
    double_click(&mut h, Vec2::new(44.0, 52.0));
    assert_eq!(running(&h).as_deref(), Some("textedit"), "log: {:?}", h.state().session.log);
    assert_eq!(h.state().session.editing_line().map(|t| t.value), Some("OLD".into()));
    // Typing replaces the selected text; Enter keeps it and TEXTEDIT asks for the next object.
    h.event(Event::Text("NEW".into()));
    h.run_steps(1);
    key(&mut h, Key::Enter, Modifiers::NONE);
    match kind(&h, e) {
        EntityKind::Text(t) => assert_eq!(t.value, "NEW"),
        k => panic!("{k:?}"),
    }
    assert!(h.state().session.editing_line().is_none());
    assert!(h.state().session.current_prompt().is_some_and(|p| p.message.contains("annotation")), "{:?}", h.state().session.current_prompt());
}

#[test]
fn escape_in_the_text_box_leaves_the_text_as_it_was() {
    let mut e = Handle(0);
    let mut h = harness(|s| e = handle(s.execute("text", &json!({"at": [40, 50], "text": "OLD", "height": 5})).unwrap()));
    double_click(&mut h, Vec2::new(44.0, 52.0));
    h.event(Event::Text("NEW".into()));
    h.run_steps(1);
    key(&mut h, Key::Escape, Modifiers::NONE);
    match kind(&h, e) {
        EntityKind::Text(t) => assert_eq!(t.value, "OLD"),
        k => panic!("{k:?}"),
    }
    assert!(h.state().session.editing_line().is_none());
}

#[test]
fn mtext_opens_the_multiline_editor_on_its_contents() {
    let mut h = harness(|s| {
        s.execute("mtext", &json!({"at": [30, 60], "text": "first\\Psecond", "height": 4, "width": 40})).unwrap();
    });
    double_click(&mut h, Vec2::new(36.0, 58.0));
    assert_eq!(running(&h).as_deref(), Some("textedit"), "log: {:?}", h.state().session.log);
    assert_eq!(h.state().session.editing_mtext().map(|m| m.contents), Some("first\\Psecond".into()));
    assert!(h.query_by_label("OK").is_some(), "the editor's OK button");
}

#[test]
fn a_dimension_shows_its_measurement_in_the_editor_and_keeps_it_measured() {
    let mut e = Handle(0);
    let mut h = harness(|s| e = handle(s.execute("dimlinear", &json!({"p1": [30, 40], "p2": [70, 40], "at": [50, 55]})).unwrap()));
    let pos = match kind(&h, e) {
        EntityKind::Dimension(d) => cadcraft_engine::render::dimension_in(h.state().session.doc().unwrap(), &d).text_pos,
        k => panic!("{k:?}"),
    };
    double_click(&mut h, pos);
    assert_eq!(running(&h).as_deref(), Some("textedit"), "log: {:?}", h.state().session.log);
    let shown = h.state().session.editing_mtext().map(|m| m.contents).unwrap_or_default();
    assert!(shown.starts_with("40"), "the measurement, not <>: {shown:?}");
    // Text added around the measurement keeps it measured: `<>` in the override.
    h.state_mut().session.input(Input::Text(format!("{shown} mm"))).unwrap();
    match kind(&h, e) {
        EntityKind::Dimension(d) => assert_eq!(d.text, "<> mm"),
        k => panic!("{k:?}"),
    }
}

#[test]
fn a_dimension_left_as_it_was_stays_measured() {
    let mut e = Handle(0);
    let mut h = harness(|s| e = handle(s.execute("dimlinear", &json!({"p1": [30, 40], "p2": [70, 40], "at": [50, 55]})).unwrap()));
    let pos = match kind(&h, e) {
        EntityKind::Dimension(d) => cadcraft_engine::render::dimension_in(h.state().session.doc().unwrap(), &d).text_pos,
        k => panic!("{k:?}"),
    };
    double_click(&mut h, pos);
    assert!(h.state().session.editing_mtext().is_some());
    key(&mut h, Key::Enter, Modifiers::COMMAND);
    match kind(&h, e) {
        EntityKind::Dimension(d) => assert_eq!(d.text, ""),
        k => panic!("{k:?}"),
    }
}

/// A block "TAG" with an attribute VALUE (default "A") and a square, and a reference to it at
/// (50, 50); returns the reference.
fn block_with_attribute(s: &mut Session) -> Handle {
    let sq = handle(s.execute("rectang", &json!({"p1": [0, 0], "p2": [20, 20]})).unwrap());
    let att = handle(s.execute("attdef", &json!({"tag": "VALUE", "prompt": "Value?", "default": "A", "at": [2, 8], "height": 4})).unwrap());
    s.execute("block", &json!({"name": "TAG", "base": [0, 0], "handles": [sq.hex(), att.hex()], "keep": "delete"})).unwrap();
    handle(s.execute("insert", &json!({"name": "TAG", "at": [40, 40], "attribs": {"VALUE": "A"}})).unwrap())
}

#[test]
fn a_block_with_attributes_opens_the_enhanced_attribute_editor() {
    let mut r = Handle(0);
    let mut h = harness(|s| r = block_with_attribute(s));
    double_click(&mut h, Vec2::new(43.0, 49.0));
    assert_eq!(h.state().ui.dialog.as_deref(), Some("eattedit"), "log: {:?}", h.state().session.log);
    assert!(running(&h).is_none());
    assert_eq!(h.state().eattedit.block, "TAG");
    assert_eq!(h.state().eattedit.rows.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(), ["VALUE"]);
    h.state_mut().eattedit.rows[0].value = "B".into();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state().ui.dialog.is_none());
    match kind(&h, r) {
        EntityKind::Insert(i) => assert_eq!(i.attribs[0].text.value, "B"),
        k => panic!("{k:?}"),
    }
}

#[test]
fn a_block_without_attributes_opens_edit_block_definition_on_it() {
    let mut h = harness(|s| {
        let sq = handle(s.execute("rectang", &json!({"p1": [0, 0], "p2": [20, 20]})).unwrap());
        s.execute("block", &json!({"name": "SQUARE", "base": [0, 0], "handles": [sq.hex()], "keep": "delete"})).unwrap();
        s.execute("insert", &json!({"name": "SQUARE", "at": [40, 40]})).unwrap();
    });
    double_click(&mut h, Vec2::new(40.0, 50.0));
    assert_eq!(h.state().ui.dialog.as_deref(), Some("bedit"), "log: {:?}", h.state().session.log);
    assert_eq!(h.state().bedit.name, "SQUARE");
    assert!(running(&h).is_none());
}

#[test]
fn a_polyline_starts_pedit_on_it() {
    let mut h = harness(|s| {
        s.execute("pline", &json!({"vertices": [[20, 20], [80, 20], [80, 70]]})).unwrap();
    });
    double_click(&mut h, Vec2::new(50.0, 20.0));
    assert_eq!(running(&h).as_deref(), Some("pedit"), "log: {:?}", h.state().session.log);
    assert!(h.state().session.current_prompt().is_some_and(|p| p.keywords.iter().any(|k| k == "Join")), "{:?}", h.state().session.current_prompt());
}

#[test]
fn a_line_is_selected_with_the_properties_palette_open() {
    let mut e = Handle(0);
    let mut h = harness(|s| e = handle(s.execute("line", &json!({"points": [[20, 30], [80, 30]]})).unwrap()));
    h.state_mut().ui.show_palettes = false;
    double_click(&mut h, Vec2::new(50.0, 30.0));
    assert!(running(&h).is_none());
    assert_eq!(h.state().session.selection(), vec![e]);
    assert!(h.state().ui.show_palettes, "the Properties palette opens");
}
