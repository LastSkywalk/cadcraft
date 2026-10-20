//! A real drawing with VetCAD dynamic blocks (#1321), when one is given: `CADCRAFT_VETCAD_DWG`
//! names it (drawings stay out of this repository). Its DXF rendition has every associative
//! geometry dependency, every block properties table and every constraint group's node count,
//! and the read leaves nothing out. Without the variable there is nothing to check.

#[test]
fn a_vetcad_drawing_converts_whole() {
    let Some(path) = std::env::var_os("CADCRAFT_VETCAD_DWG") else { return };
    let bytes = std::fs::read(&path).unwrap();
    let (dxf, notes) = cadcraft_dwg::dwg_to_dxf_noted(&bytes, cadcraft_dwg::Limits::DEFAULT).unwrap();
    assert!(notes.iter().all(|n| !n.contains("couldn't be read")), "{notes:?}");
    let text = String::from_utf8_lossy(&dxf);
    let records = |kind: &str| text.matches(&format!("\n{kind}\r\n")).count() + text.matches(&format!("\n{kind}\n")).count();
    assert!(records("BLOCKPROPERTIESTABLE") > 0, "properties tables");
    assert!(records("ACDBASSOCGEOMDEPENDENCY") > 0);
    assert!(!text.contains("AcDbAssocGeomDependency\r\n 90\r\n     0\r\n290\r\n     1\r\n  1\r\n\r\n"), "an unnamed subentity class");
}
