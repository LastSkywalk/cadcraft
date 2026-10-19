//! EXPORT and Save-as-PDF write the space you are in: on a layout tab the picture is that layout
//! (plotted from its own page setup), as AutoCAD's Save-as-PDF does, not model space (issue #1321,
//! item 15). Model space still goes through the file writer, as do DXF and DWG.

use std::sync::Mutex;

use cadcraft_doc::{Drawing, Space};
use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, set_io};
use serde_json::{Value, json};

/// What the plotter was asked for, in order.
static PLOTTED: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// What the file writer was asked for, in order.
static WRITTEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The tests below watch the same two process-wide logs, so they take turns and start empty.
static SERIAL: Mutex<()> = Mutex::new(());

fn plot(d: &Drawing, space: &Space, opts: &Value) -> Result<Vec<u8>, String> {
    let output = opts["output"].as_str().unwrap_or("").to_string();
    PLOTTED.lock().map_err(|_| "the plot log is poisoned".to_string())?.push(format!("{space:?} {output}"));
    Ok(format!("%PDF-1.4 plotted {output} of {}", d.layouts.len()).into_bytes())
}

fn written(_: &Drawing, name: &str) -> Result<Vec<u8>, String> {
    WRITTEN.lock().map_err(|_| "the write log is poisoned".to_string())?.push(name.to_string());
    Ok(b"0\nEOF\n".to_vec())
}

fn log(m: &Mutex<Vec<String>>) -> Vec<String> {
    m.lock().map(|v| v.clone()).unwrap_or_default()
}

/// Take the turn and start with empty logs. A lock poisoned by an earlier failed test still
/// serialises, and the returned guard is what holds the turn.
fn begin() -> std::sync::MutexGuard<'static, ()> {
    let turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(mut v) = PLOTTED.lock() {
        v.clear();
    }
    if let Ok(mut v) = WRITTEN.lock() {
        v.clear();
    }
    turn
}

fn session() -> Session {
    set_io(IoHooks { read: |_, _| Err("no reader in this test".into()), write: written, plot: Some(plot) });
    let mut s = Session::new();
    s.cmdline("circle 0,0 5").unwrap();
    s
}

#[test]
fn a_picture_of_a_layout_tab_is_that_layout() {
    let _turn = begin();
    let mut s = session();
    let layout = s.state().unwrap().drawing().layouts[0].name.clone();
    s.execute("layout.set", &json!({ "name": layout })).unwrap();

    for format in ["pdf", "png", "svg"] {
        let r = s.execute("export", &json!({ "format": format })).unwrap();
        assert!(r["data"].is_string(), "{format}: the plot comes back as data");
        // The picture is the layout, not model space.
        assert_eq!(log(&PLOTTED).last().map(String::as_str), Some(format!("Paper(\"{layout}\") {format}").as_str()), "{format}");
    }
    // `layout` picks the space whatever tab is active.
    s.execute("layout.set", &json!({ "name": "Model" })).unwrap();
    s.execute("export", &json!({ "format": "pdf", "layout": layout })).unwrap();
    assert_eq!(log(&PLOTTED).last().map(String::as_str), Some(format!("Paper(\"{layout}\") pdf").as_str()));
}

#[test]
fn model_space_and_drawing_files_go_through_the_file_writer() {
    let _turn = begin();
    let mut s = session();
    let layout = s.state().unwrap().drawing().layouts[0].name.clone();

    // Model space: the writer frames the drawing, the plotter is not asked.
    s.execute("export", &json!({ "format": "png" })).unwrap();
    assert!(log(&PLOTTED).is_empty(), "model space is not plotted");
    assert_eq!(log(&WRITTEN).last().map(String::as_str), Some("export.png"));

    // A drawing file holds the whole drawing, on any tab.
    s.execute("layout.set", &json!({ "name": layout })).unwrap();
    s.execute("export", &json!({ "format": "dxf" })).unwrap();
    assert!(log(&PLOTTED).is_empty(), "a drawing file is not plotted");
    assert_eq!(log(&WRITTEN).last().map(String::as_str), Some("export.dxf"));

    // SAVEAS to a picture path is the same export.
    let dir = std::env::temp_dir().join(format!("cadcraft-export-layout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    s.execute("saveas", &json!({ "path": dir.join("plate.pdf").to_string_lossy() })).unwrap();
    assert_eq!(log(&PLOTTED).last().map(String::as_str), Some(format!("Paper(\"{layout}\") pdf").as_str()));
    let _ = std::fs::remove_dir_all(&dir);
}
