//! A dynamic block reference's attribute field follows a grip dragged with the mouse on the
//! canvas (#1321): the DOOR reference's "W=36" attribute, read from the Width parameter, shows
//! "W=30" once Width's arrow is moved 6 units left, as AutoCAD updates attribute fields after a
//! grip edit. One undo step puts both back.

use cadcraft_engine::Session;
use cadcraft_engine::doc::dynblock::{self, GripPart};
use cadcraft_engine::doc::{EntityKind, Handle};
use cadcraft_engine::geom::Vec2;
use cadcraft_ui_egui::{CadApp, Services};
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

const REF: Handle = Handle(0x80);

/// DXF text from the fixture's lines of `code value`.
fn fixture() -> String {
    include_str!("../../io/tests/data/dynblock_door_field.txt")
        .lines()
        .map(|l| l.trim_end_matches('\r').trim_start())
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (code, value) = l.split_once(' ').unwrap_or((l, ""));
            format!("{code}\n{value}\n")
        })
        .collect()
}

fn harness() -> Harness<'static, CadApp> {
    // The app's file hooks: drawings are read by cadcraft-io.
    cadcraft_engine::cmd::file::set_io(cadcraft_engine::cmd::file::IoHooks {
        read: |b, name| cadcraft_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadcraft_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
    });
    let mut app = CadApp::new(Session::new(), Services::default());
    app.canvas.zoom_pending = false;
    app.open_bytes("door.dxf", fixture().as_bytes());
    // 60 frames a second, so two clicks fall within the double-click delay.
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    // After the zoom an opened drawing gets: the door and its attribute.
    h.state_mut().canvas.zoom_pending = false;
    h.state_mut().session.execute("view.set", &json!({"center": [118, 20], "height": 80})).unwrap();
    h.run_steps(3);
    h
}

fn screen(h: &Harness<'_, CadApp>, p: Vec2) -> Pos2 {
    h.state().canvas.xf.unwrap().to_screen(p)
}

fn button(h: &mut Harness<'_, CadApp>, p: Pos2, pressed: bool) {
    h.event(Event::PointerMoved(p));
    h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    h.run_steps(1);
}

/// Press at `a`, move to `b` in steps, let go at `b`.
fn drag(h: &mut Harness<'_, CadApp>, a: Pos2, b: Pos2) {
    h.event(Event::PointerMoved(a));
    h.run_steps(2);
    button(h, a, true);
    for i in 1..=8 {
        h.event(Event::PointerMoved(a + (b - a) * (i as f32 / 8.0)));
        h.run_steps(1);
    }
    button(h, b, false);
    h.run_steps(3);
}

fn click(h: &mut Harness<'_, CadApp>, p: Pos2) {
    button(h, p, true);
    button(h, p, false);
    h.run_steps(2);
}

fn width_text(h: &Harness<'_, CadApp>) -> String {
    match &h.state().session.doc().unwrap().entity(REF).unwrap().kind {
        EntityKind::Insert(i) => i.attribs[0].text.value.clone(),
        k => panic!("{k:?}"),
    }
}

#[test]
fn clicking_the_width_grip_and_a_new_point_updates_the_attribute_field() {
    let mut h = harness();
    assert_eq!(width_text(&h), "W=36");
    h.state_mut().session.set_selection(vec![REF]);
    h.run_steps(3);
    let g = dynblock::grips(h.state().session.doc().unwrap(), REF).into_iter().find(|g| g.node == 2 && g.part == GripPart::End).unwrap();
    let (from, to) = (screen(&h, g.at), screen(&h, g.at - Vec2::new(6.0, 0.0)));
    h.event(Event::PointerMoved(from));
    h.run_steps(2);
    // A click makes the grip hot (** STRETCH **), the mouse moves, a click places it.
    click(&mut h, from);
    assert_eq!(h.state().session.running.as_ref().map(|r| r.id.clone()).as_deref(), Some("grip"));
    for i in 1..=6 {
        h.event(Event::PointerMoved(from + (to - from) * (i as f32 / 6.0)));
        h.run_steps(1);
    }
    click(&mut h, to);
    if let Some(dir) = std::env::var_os("CADCRAFT_FIELD_SHOTS") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        h.render().unwrap().save(dir.join("dynblock-field-grip.png")).unwrap();
    }
    assert_eq!(width_text(&h), "W=30", "log: {:?}", h.state().session.log);
    h.state_mut().run("undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(width_text(&h), "W=36");
}

#[test]
fn a_quick_click_on_the_grip_after_selecting_is_a_grip_edit_not_a_double_click() {
    let mut h = harness();
    // A click on the reference (its attribute text) selects it; a click on its Width grip right
    // after (within the double-click time, elsewhere) makes the grip hot, as in AutoCAD.
    let at = screen(&h, Vec2::new(101.0, 50.5));
    h.event(Event::PointerMoved(at));
    h.run_steps(2);
    button(&mut h, at, true);
    button(&mut h, at, false);
    assert_eq!(h.state().session.selection(), vec![REF]);
    let g = dynblock::grips(h.state().session.doc().unwrap(), REF).into_iter().find(|g| g.node == 2 && g.part == GripPart::End).unwrap();
    let (from, to) = (screen(&h, g.at), screen(&h, g.at - Vec2::new(6.0, 0.0)));
    button(&mut h, from, true);
    button(&mut h, from, false);
    assert!(h.state().ui.dialog.is_none(), "no double-click action: {:?}", h.state().ui.dialog);
    assert_eq!(h.state().session.running.as_ref().map(|r| r.id.clone()).as_deref(), Some("grip"));
    for i in 1..=6 {
        h.event(Event::PointerMoved(from + (to - from) * (i as f32 / 6.0)));
        h.run_steps(1);
    }
    click(&mut h, to);
    assert_eq!(width_text(&h), "W=30", "log: {:?}", h.state().session.log);
}

#[test]
fn update_fields_from_the_command_line() {
    let mut h = harness();
    // A field that was saved with a stale value shows the current one after UPDATEFIELD.
    let d = h.state_mut().session.doc_mut().unwrap();
    d.modify_entity(REF, |e| {
        if let EntityKind::Insert(i) = &mut e.kind {
            i.attribs[0].text.value = "W=?".into();
            if let Some(f) = &mut i.attribs[0].field {
                f.children[0].value = Default::default();
            }
        }
    })
    .unwrap();
    h.state_mut().cmdline("_updatefield all\n");
    h.run_steps(3);
    // Accept the selection if the prompt is still up.
    if h.state().session.running.is_some() {
        h.state_mut().cmdline("\n");
        h.run_steps(3);
    }
    assert_eq!(width_text(&h), "W=36", "log: {:?}", h.state().session.log);
}

#[test]
fn pressing_and_dragging_a_grip_edits_it_too() {
    let mut h = harness();
    h.state_mut().session.set_selection(vec![REF]);
    h.run_steps(3);
    let g = dynblock::grips(h.state().session.doc().unwrap(), REF).into_iter().find(|g| g.node == 2 && g.part == GripPart::End).unwrap();
    let (from, to) = (screen(&h, g.at), screen(&h, g.at - Vec2::new(6.0, 0.0)));
    drag(&mut h, from, to);
    assert_eq!(width_text(&h), "W=30", "log: {:?}", h.state().session.log);
    assert!(h.state().session.running.is_none(), "the drag finished the edit");

    // An ordinary grip: a line's end dragged to a new point.
    let line = h.state_mut().session.execute("line", &json!({"points": [[100, 30], [120, 30]]})).unwrap();
    let line = Handle::parse_hex(line["handles"][0].as_str().unwrap()).unwrap();
    h.state_mut().session.set_selection(vec![line]);
    h.run_steps(3);
    let (from, to) = (screen(&h, Vec2::new(120.0, 30.0)), screen(&h, Vec2::new(125.0, 35.0)));
    drag(&mut h, from, to);
    let EntityKind::Line(l) = &h.state().session.doc().unwrap().entity(line).unwrap().kind else { panic!() };
    assert!((l.b.x - 125.0).abs() < 0.5 && (l.b.y - 35.0).abs() < 0.5, "{l:?} log: {:?}", h.state().session.log);
}

/// Two clicks in quick succession at `p`.
fn double_click(h: &mut Harness<'_, CadApp>, p: Pos2) {
    h.event(Event::PointerMoved(p));
    h.run_steps(2);
    for _ in 0..2 {
        h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.run_steps(1);
        h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(2);
}

fn running(h: &Harness<'_, CadApp>) -> Option<String> {
    h.state().session.running.as_ref().map(|r| r.id.clone())
}

#[test]
fn double_clicking_a_block_reference_edits_its_attributes() {
    let mut h = harness();
    // The door reference's attribute text: the Enhanced Attribute Editor, as AutoCAD opens on an
    // attributed (dynamic) block, named by the dynamic block, the attribute clicked current.
    let at = screen(&h, Vec2::new(101.0, 50.5));
    double_click(&mut h, at);
    assert_eq!(h.state().ui.dialog.as_deref(), Some("eattedit"), "log: {:?}", h.state().session.log);
    assert!(running(&h).is_none());
    let st = &h.state().eattedit;
    assert_eq!(st.block, "Door");
    assert_eq!(st.rows.get(st.current).map(|r| r.tag.as_str()), Some("WIDTH"));
}
