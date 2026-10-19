//! A layout plots 1:1 with paper space (0,0) at the printable area's lower-left corner: the page
//! setup's margins and plot origin place the paper around it, and the plot is clipped to the
//! printable area. AutoCAD does the same — a viewport fitted to the sheet spans paper space
//! (0,0)..(printable size), its `PLOTSETTINGS` 46/47 hold the plot offset in mm from that corner,
//! and a 1:1 Layout-area plot lands the sheet content exactly that far inside the paper
//! (issue #1321, item 11).

use cadcraft_doc::{Common, Drawing, EntityKind, Line, PageSetup, Space};
use cadcraft_geom::Vec3;
use cadcraft_io::{plotter, read_dxf, write_dxf};
use serde_json::json;

const PT_PER_MM: f64 = 72.0 / 25.4;

/// A metric drawing with one layout (A4 portrait, 10/20/12/18 mm margins) holding a 50 mm line
/// from paper-space (0,0) along the X axis.
fn drawing(plot_origin: [f64; 2]) -> Drawing {
    let mut d = Drawing::new_metric();
    let name = d.layouts[0].name.clone();
    if let Some(l) = d.layouts.iter_mut().find(|l| l.name == name) {
        l.page = PageSetup {
            paper: "ISO A4 (210.00 x 297.00 MM)".into(),
            width_mm: 210.0,
            height_mm: 297.0,
            margins_mm: [10.0, 20.0, 12.0, 18.0],
            plot_origin_mm: plot_origin,
            landscape: false,
            ..PageSetup::default()
        };
    }
    d.add(&Space::Paper(name), Common::default(), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(50.0, 0.0, 0.0) })).unwrap();
    d
}

/// The uncompressed PDF of a layout.
fn pdf(d: &Drawing, layout: &str) -> String {
    let bytes = plotter::plot(d, &Space::Paper(layout.into()), &json!({"compress": false})).unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The plot area's clip rectangle (`x y w h re W n`) in points.
fn clip(pdf: &str) -> [f64; 4] {
    let l = pdf.lines().find(|l| l.ends_with(" re W n")).unwrap_or("");
    let v: Vec<f64> = l.split_whitespace().take(4).filter_map(|t| t.parse().ok()).collect();
    assert_eq!(v.len(), 4, "no clip rectangle in {l:?}");
    [v[0], v[1], v[2], v[3]]
}

/// The first stroked point of the content stream, in points.
fn first_point(pdf: &str) -> [f64; 2] {
    let l = pdf.lines().find(|l| l.ends_with(" m")).unwrap_or("");
    let v: Vec<f64> = l.split_whitespace().take(2).filter_map(|t| t.parse().ok()).collect();
    assert_eq!(v.len(), 2, "no first point in {l:?}");
    [v[0], v[1]]
}

/// The PDF writes its numbers rounded to a thousandth of a point.
fn mm(points: &[f64]) -> Vec<f64> {
    points.iter().map(|p| (p / PT_PER_MM * 1000.0).round() / 1000.0).collect()
}

#[test]
fn a_layout_plots_from_the_printable_corner() {
    let d = drawing([0.0, 0.0]);
    let name = d.layouts[0].name.clone();
    let out = pdf(&d, &name);
    // The media is the whole sheet.
    assert!(out.contains("/MediaBox [0 0 595.276 841.89]"), "A4 portrait");
    // The plot area is the printable area: it starts 10/20 mm in from the page's corner and is
    // 188 × 259 mm — the A4 sheet less the 10/12 and 20/18 mm margins (`x y w h`).
    assert_eq!(mm(&clip(&out)), [10.0, 20.0, 188.0, 259.0]);
    // Paper space (0,0) — the line's start — lands 10 mm right and 20 mm up of the sheet corner.
    assert_eq!(mm(&first_point(&out)), [10.0, 20.0]);
}

#[test]
fn the_plot_origin_moves_the_sheet_under_the_drawing() {
    let d = drawing([5.0, 3.0]);
    let name = d.layouts[0].name.clone();
    let out = pdf(&d, &name);
    // The printable area stays 10/20 mm in from the page's corner: the plot origin moves the
    // sheet's content under it, not the paper.
    assert_eq!(mm(&clip(&out)), [10.0, 20.0, 188.0, 259.0]);
    // The drawing moves with it: the sheet's corner is now 15 mm left and 23 mm below paper (0,0).
    assert_eq!(mm(&first_point(&out)), [15.0, 23.0]);
}

#[test]
fn the_sheet_bounds_hold_the_paper_and_its_printable_area() {
    let d = drawing([5.0, 3.0]);
    let name = d.layouts[0].name.clone();
    let sheet = cadcraft_render::sheet(&d, &name).unwrap();
    // The paper rectangle reaches 15 mm left and 23 mm below paper space (0,0).
    assert_eq!(sheet.bounds().min, Vec3::new(-15.0, -23.0, 0.0).xy());
    assert_eq!(sheet.bounds().max, Vec3::new(195.0, 274.0, 0.0).xy());
    assert_eq!(sheet.printable.min, Vec3::new(-5.0, -3.0, 0.0).xy());
    assert_eq!(sheet.printable.max, Vec3::new(183.0, 256.0, 0.0).xy());
    // Without a plot origin the printable area's corner is paper space (0,0), as AutoCAD's.
    let plain = cadcraft_render::sheet(&drawing([0.0, 0.0]), &name).unwrap();
    assert_eq!(plain.printable.min, Vec3::ZERO.xy());
    assert_eq!(plain.bounds().min, Vec3::new(-10.0, -20.0, 0.0).xy());
}

#[test]
fn the_plot_origin_survives_a_dxf_round_trip() {
    let d = drawing([5.0, -3.5]);
    let back = read_dxf(write_dxf(&d).as_bytes()).unwrap();
    assert_eq!(back.layouts[0].page.plot_origin_mm, [5.0, -3.5]);
}
