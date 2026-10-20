//! Fields of block attributes (#1321): an attribute whose value AutoCAD computes from its dynamic
//! block reference — a length or a count, say — keeps its field through a read and a save (DXF
//! and DWG), and shows the new value when the reference changes. The drawing
//! is the synthetic DOOR block (with the grips of the engine's grip tests) and a reference whose
//! attribute reads "W=<Width>".

use cadcraft_doc::dynblock::{self, GripPart};
use cadcraft_doc::field::{FieldObject, FieldValue};
use cadcraft_doc::{Drawing, EntityKind, Handle};
use cadcraft_geom::Vec2;

/// DXF text from lines of `code value`.
fn dxf(src: &str) -> String {
    src.lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let l = l.trim_start();
            let (code, value) = l.split_once(' ').unwrap_or((l, ""));
            format!("{code}\n{value}\n")
        })
        .collect()
}

fn fixture() -> String {
    dxf(include_str!("data/dynblock_door_field.txt"))
}

const REF: Handle = Handle(0x80);

fn attrib(d: &Drawing, h: Handle) -> cadcraft_doc::Attrib {
    match &d.entity(h).unwrap().kind {
        EntityKind::Insert(i) => i.attribs[0].clone(),
        k => panic!("{k:?}"),
    }
}

#[test]
fn an_attribute_field_is_read_with_its_reference_as_its_object() {
    let d = cadcraft_io::read_dxf(fixture().as_bytes()).unwrap();
    let a = attrib(&d, REF);
    assert_eq!(a.text.value, "W=36");
    let f = a.field.expect("the attribute's field");
    assert_eq!((f.evaluator.as_str(), f.code.as_str()), ("_text", "W=%<\\_FldIdx 0>%"));
    let c = &f.children[0];
    assert_eq!(c.objects, [FieldObject::Host], "the reference the attribute is on");
    assert_eq!(c.value, FieldValue::Double { value: 36.0, text: "36".into() });
    assert_eq!(c.format(), Some("%lu2%pr0"));
    // Nothing changed: evaluating it again gives what AutoCAD saved.
    let mut d2 = d.clone();
    assert!(!cadcraft_doc::field::update_insert(&mut d2, REF));
}

#[test]
fn a_grip_drag_shows_the_new_width_in_the_attribute() {
    let mut d = cadcraft_io::read_dxf(fixture().as_bytes()).unwrap();
    let g = dynblock::grips(&d, REF).into_iter().find(|g| g.node == 2 && g.part == GripPart::End).unwrap();
    dynblock::drag_grip(&mut d, REF, 2, GripPart::End, g.at - Vec2::new(6.0, 0.0)).unwrap();
    assert_eq!(attrib(&d, REF).text.value, "W=30");
    // Width snaps to its values: 30, 32 or 36.
    dynblock::set_property(&mut d, REF, "Width", "33").unwrap();
    assert_eq!(attrib(&d, REF).text.value, "W=32");
}

/// The drawing saved as DXF in its own version (AutoCAD 2018: fields are written from 2004).
fn save(d: &Drawing) -> String {
    String::from_utf8(cadcraft_io::write(d, "door.dxf").unwrap()).unwrap()
}

/// The field's objects in a written file: FIELD records and what the attribute's dictionary
/// chain leads to.
fn written_fields(text: &str) -> (usize, Vec<String>) {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let pairs: Vec<(&str, &str)> = lines
        .chunks(2)
        .filter_map(|c| match c {
            [a, b] => Some((*a, *b)),
            _ => None,
        })
        .collect();
    let fields = pairs.iter().filter(|(c, v)| *c == "0" && *v == "FIELD").count();
    let codes =
        pairs.iter().filter(|(c, _)| *c == "2").map(|(_, v)| v.to_string()).filter(|v| v.contains("_FldIdx") || v.contains("AcObjProp")).collect();
    (fields, codes)
}

#[test]
fn the_field_survives_a_dxf_save() {
    let mut d = cadcraft_io::read_dxf(fixture().as_bytes()).unwrap();
    dynblock::set_property(&mut d, REF, "Width", "30").unwrap();
    let text = save(&d);
    let (n, codes) = written_fields(&text);
    assert_eq!(n, 2, "the text field and its child");
    assert!(codes.iter().any(|c| c.contains("Parameter(2).UpdatedDistance")), "{codes:?}");
    assert!(text.contains("ACAD_FIELD") && text.contains("AcDbField") && text.contains("FIELDLIST"));
    let back = cadcraft_io::read_dxf(text.as_bytes()).unwrap();
    // The reference was renumbered or kept; find it by its attribute.
    let r = back.model.iter().find(|e| matches!(&e.kind, EntityKind::Insert(i) if i.attribs.iter().any(|a| a.tag == "WIDTH"))).unwrap().handle;
    let a = attrib(&back, r);
    assert_eq!(a.text.value, "W=30");
    let f = a.field.expect("the field after a save");
    assert_eq!(f.children[0].objects, [FieldObject::Host]);
    assert_eq!(f.children[0].value.text(), "30");
    // It still reads the reference: a change after reading the saved file shows too.
    let mut back = back;
    dynblock::set_property(&mut back, r, "Width", "32").unwrap();
    assert_eq!(attrib(&back, r).text.value, "W=32");
}

#[test]
fn the_field_list_lists_the_written_fields() {
    let d = cadcraft_io::read_dxf(fixture().as_bytes()).unwrap();
    let text = save(&d);
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let at = lines.windows(2).position(|w| w == ["0", "FIELDLIST"]).expect("a field list");
    let body: Vec<&str> = lines[at + 2..].chunks(2).take_while(|c| c[0] != "0").flat_map(|c| c.iter().copied()).collect();
    let listed: Vec<&str> = body.chunks(2).skip_while(|c| c != &["100", "AcDbIdSet"]).filter(|c| c[0] == "330").map(|c| c[1]).collect();
    let count = body.chunks(2).find(|c| c[0] == "90").map(|c| c[1]);
    assert_eq!(listed.len(), 2, "{body:?}");
    assert_eq!(count, Some("2"));
    assert!(!listed.contains(&"0"), "no dangling entries");
}

#[test]
fn the_field_survives_a_dwg_save() {
    let mut d = cadcraft_io::read_dxf(fixture().as_bytes()).unwrap();
    dynblock::set_property(&mut d, REF, "Width", "30").unwrap();
    let bytes = cadcraft_io::write(&d, "door.dwg").unwrap();
    let back = cadcraft_io::read(&bytes, "door.dwg").unwrap();
    let r = back.model.iter().find(|e| matches!(&e.kind, EntityKind::Insert(i) if i.attribs.iter().any(|a| a.tag == "WIDTH"))).unwrap().handle;
    let a = attrib(&back, r);
    assert_eq!(a.text.value, "W=30");
    let f = a.field.expect("the field after a DWG save");
    assert_eq!(f.children[0].objects, [FieldObject::Host]);
}

#[test]
fn a_copy_reads_itself() {
    let mut d = cadcraft_io::read_dxf(fixture().as_bytes()).unwrap();
    let e = (**d.entity(REF).unwrap()).clone();
    let copy = d.add(&cadcraft_doc::Space::Model, e.common.clone(), e.kind.clone()).unwrap();
    // The copy refers to the dynamic block like the original (no values of its own yet).
    let g = dynblock::grips(&d, copy).into_iter().find(|g| g.node == 2 && g.part == GripPart::End).unwrap();
    dynblock::drag_grip(&mut d, copy, 2, GripPart::End, g.at - Vec2::new(4.0, 0.0)).unwrap();
    assert_eq!(attrib(&d, copy).text.value, "W=32");
    assert_eq!(attrib(&d, REF).text.value, "W=36", "the original keeps its own width");
}

#[test]
fn hostile_field_trees_do_not_panic() {
    let src = fixture();
    let cases = [
        // A child that is the field itself, and a child that doesn't exist.
        src.replace("360\n93\n97\n0\n", "360\n92\n360\n93\n360\nFFFF\n97\n0\n"),
        // An object index past the objects, a huge child count, a code with no end.
        src.replace("Object(%<\\_ObjIdx 0>%)", "Object(%<\\_ObjIdx 99>%)").replace("90\n1\n360\n93", "90\n2147483647\n360\n93"),
        src.replace("W=%<\\_FldIdx 0>%", "W=%<\\_FldIdx 0"),
        // The dictionary chain points at itself.
        src.replace("3\nTEXT\n360\n92", "3\nTEXT\n360\n91"),
    ];
    for c in cases {
        let Ok(mut d) = cadcraft_io::read_dxf(c.as_bytes()) else { continue };
        cadcraft_doc::field::update_all(&mut d);
        let _ = dynblock::set_property(&mut d, REF, "Width", "30");
        let _ = cadcraft_io::write(&d, "door.dxf");
    }
}

/// The fields of a field tree: the root and every child, at any depth.
fn nodes(f: &cadcraft_doc::field::Field) -> usize {
    1 + f.children.iter().map(nodes).sum::<usize>()
}

#[test]
fn a_field_tree_that_lists_its_children_over_and_over_reads_quickly() {
    let src = fixture();
    let many = |h: &str| format!("360\n{h}\n").repeat(256);
    // The root lists itself 256 times; its child lists itself too.
    let cyclic = src
        .replace("90\n1\n360\n93\n97\n0\n", &format!("90\n257\n{}360\n93\n97\n0\n", many("92")))
        .replace("90\n0\n97\n1\n331\n80\n", &format!("90\n256\n{}97\n1\n331\n80\n", many("93")));
    // No cycle, but each of 16 levels lists the next 256 times: 256^15 paths through the tree.
    let mut chain = String::new();
    for k in 0..16u32 {
        let next = if k < 15 { many(&format!("{:X}", 0xA00 + k + 1)) } else { String::new() };
        chain.push_str(&format!(
            "0\nFIELD\n5\n{:X}\n330\n92\n100\nAcDbField\n1\n_text\n2\nX\n90\n{}\n{next}97\n0\n7\nACFD_FIELD_VALUE\n93\n0\n90\n0\n",
            0xA00 + k,
            if k < 15 { 256 } else { 0 }
        ));
    }
    let shared =
        src.replace("90\n1\n360\n93\n97\n0\n", "90\n1\n360\nA00\n97\n0\n").replace("98\n2\n0\nENDSEC\n", &format!("98\n2\n{chain}0\nENDSEC\n"));
    for (name, text) in [("cyclic", cyclic), ("shared", shared)] {
        let t = std::time::Instant::now();
        let d = cadcraft_io::read_dxf(text.as_bytes()).unwrap();
        let f = attrib(&d, REF).field.expect("the root field");
        assert!(nodes(&f) < 10_000, "{name}: {} fields", nodes(&f));
        let _ = cadcraft_io::write(&d, "door.dxf").unwrap();
        assert!(t.elapsed() < std::time::Duration::from_secs(20), "{name}: {:?}", t.elapsed());
    }
}
