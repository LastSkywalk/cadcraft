//! Attribute fields through commands (#1321): a dynamic block grip edit and a property change
//! show in the attribute that reads the reference, one undo step puts both back, UPDATEFIELD and
//! REGEN evaluate fields again, and a value typed over a field's makes it plain text.

use cadcraft_engine::Session;
use cadcraft_engine::doc::{EntityKind, Handle};
use serde_json::json;

const REF: Handle = Handle(0x80);

fn session() -> Session {
    let src: String = include_str!("../../io/tests/data/dynblock_door_field.txt")
        .lines()
        .map(|l| l.trim_end_matches('\r').trim_start())
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (code, value) = l.split_once(' ').unwrap_or((l, ""));
            format!("{code}\n{value}\n")
        })
        .collect();
    let d = cadcraft_io::read_dxf(src.as_bytes()).unwrap();
    let mut s = Session::empty();
    s.open_drawing(d, "door", None);
    s
}

fn attrib(s: &Session) -> (String, bool) {
    match &s.doc().unwrap().entity(REF).unwrap().kind {
        EntityKind::Insert(i) => (i.attribs[0].text.value.clone(), i.attribs[0].field.is_some()),
        k => panic!("{k:?}"),
    }
}

#[test]
fn a_grip_edit_and_its_undo_show_in_the_attribute() {
    let mut s = session();
    let g = s.execute("blocks.reference.grips", &json!({ "handle": REF.hex() })).unwrap();
    let end = g.as_array().unwrap().iter().find(|g| g["node"] == 2 && g["part"] == "end").cloned().unwrap();
    let (x, y) = (end["at"][0].as_f64().unwrap(), end["at"][1].as_f64().unwrap());
    s.execute("blocks.reference.grip", &json!({ "handle": REF.hex(), "node": 2, "part": "end", "to": [x - 6.0, y] })).unwrap();
    assert_eq!(attrib(&s).0, "W=30");
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(attrib(&s).0, "W=36");
    s.execute("blocks.reference.set", &json!({ "handle": REF.hex(), "name": "Width", "value": "32" })).unwrap();
    assert_eq!(attrib(&s).0, "W=32");
}

#[test]
fn updatefield_and_regen_evaluate_fields_again() {
    let mut s = session();
    let stale = |s: &mut Session| {
        s.doc_mut()
            .unwrap()
            .modify_entity(REF, |e| {
                if let EntityKind::Insert(i) = &mut e.kind {
                    i.attribs[0].text.value = "W=?".into();
                }
            })
            .unwrap();
    };
    stale(&mut s);
    let r = s.execute("updatefield", &json!({ "handles": [REF.hex()] })).unwrap();
    assert_eq!(r["updated"], 1);
    assert_eq!(attrib(&s).0, "W=36");
    stale(&mut s);
    s.execute("updatefield", &json!({})).unwrap();
    assert_eq!(attrib(&s).0, "W=36", "every object when none is given");
    stale(&mut s);
    s.execute("regen", &json!({})).unwrap();
    assert_eq!(attrib(&s).0, "W=36");
}

#[test]
fn a_typed_value_replaces_the_field() {
    let mut s = session();
    s.execute("attedit", &json!({ "handle": REF.hex(), "values": { "width": "W=40" } })).unwrap();
    assert_eq!(attrib(&s), ("W=40".into(), false));
    // The reference changing no longer changes the typed value.
    s.execute("blocks.reference.set", &json!({ "handle": REF.hex(), "name": "Width", "value": "30" })).unwrap();
    assert_eq!(attrib(&s).0, "W=40");
}
