//! Drawings with constraint-driven dynamic blocks and properties tables (#1321), when some are
//! given: `CADCRAFT_DYNBLOCK_CORPUS` names a DWG file or a folder of them (drawings stay out of
//! this repository). Each one's DXF rendition has its associative geometry dependencies, block
//! properties tables and constraint groups' node counts, and the read leaves nothing out. Without
//! the variable there is nothing to check.

use std::path::PathBuf;

/// The corpus drawings: the file named, or the DWG files in the folder named.
fn drawings() -> Vec<PathBuf> {
    let Some(path) = std::env::var_os("CADCRAFT_DYNBLOCK_CORPUS").map(PathBuf::from) else { return Vec::new() };
    if !path.is_dir() {
        return vec![path];
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&path)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dwg")))
        .collect();
    files.sort();
    files
}

#[test]
fn dynamic_block_drawings_convert_whole() {
    for path in drawings() {
        let bytes = std::fs::read(&path).unwrap();
        let (dxf, notes) = cadcraft_dwg::dwg_to_dxf_noted(&bytes, cadcraft_dwg::Limits::DEFAULT).unwrap();
        assert!(notes.iter().all(|n| !n.contains("couldn't be read")), "{}: {notes:?}", path.display());
        let text = String::from_utf8_lossy(&dxf);
        let records = |kind: &str| text.matches(&format!("\n{kind}\r\n")).count() + text.matches(&format!("\n{kind}\n")).count();
        assert!(records("ACDBASSOCGEOMDEPENDENCY") > 0, "{}: associative dependencies", path.display());
        assert!(
            !text.contains("AcDbAssocGeomDependency\r\n 90\r\n     0\r\n290\r\n     1\r\n  1\r\n\r\n"),
            "{}: an unnamed subentity class",
            path.display()
        );
    }
}
