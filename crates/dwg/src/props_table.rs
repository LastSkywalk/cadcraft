//! Block properties tables (`BLOCKPROPERTIESTABLE`) of dynamic blocks, decoded from their DWG
//! records. The DWG reader keeps them as raw record bytes, which the DXF rendition can't carry,
//! so a dynamic block that sets its user parameters from a properties table (VetCAD's rebar
//! blocks: class and diameter → the values their attributes read) lost the table and, saved
//! again, made AutoCAD show `##` in those attributes (#1321). Decoded here into the groups
//! AutoCAD writes for the object in DXF, the table goes through like any other object.
//!
//! The record after the object header: the evaluation expression, block element, parameter and
//! one-point parameter every one-point block parameter has, then the table: its version, name,
//! description and columns (each the parameter and property it sets, the parameter's node, a
//! default and an unmatched value, five flags, a format and a second reference), its rows (an
//! index and one value per column) and three flags. Values are typed: a code, then the value
//! (1 text, 40 real, 90 long, 70 short; -9999 none). Checked against AutoCAD's own DXF of the
//! same drawings, every group equal.

use acadrust::io::dwg::dwg_stream_readers::merged_reader::DwgMergedReader;
use acadrust::objects::ObjectType;
use acadrust::types::DxfVersion;

/// Most columns, rows and property connections a table is decoded with.
const MAX_COLUMNS: i32 = 256;
const MAX_ROWS: i32 = 100_000;
const MAX_CONNECTIONS: i32 = 256;
const MAX_EED: usize = 4096;

/// Groups of one decoded record.
type Groups = Vec<(i32, String)>;

fn real(v: f64) -> String {
    if v.is_finite() { format!("{v:?}") } else { "0.0".into() }
}

fn hex(h: u64) -> String {
    format!("{h:X}")
}

/// A typed value: its code (170), then the value in the group its type has.
fn value(r: &mut DwgMergedReader, out: &mut Groups) -> Option<()> {
    let code = r.read_bit_short();
    out.push((170, code.to_string()));
    match code {
        1 => out.push((300, r.read_variable_text())),
        40 => out.push((140, real(r.read_bit_double()))),
        90 => out.push((90, r.read_bit_long().to_string())),
        70 => out.push((70, r.read_bit_short().to_string())),
        -9999 => {}
        // A type not seen in AutoCAD's files: the rest can't be read.
        _ => return None,
    }
    Some(())
}

/// Decode one table record (`data`, its handle stream `handle_bits` long) into its DXF groups,
/// or `None` when it isn't one this decoder reads completely.
pub(crate) fn decode(data: &[u8], handle_bits: i64, version: DxfVersion) -> Option<Groups> {
    let total = i64::try_from(data.len()).ok()?.checked_mul(8)?;
    let handles_at = total.checked_sub(handle_bits).filter(|b| *b > 0)?;
    let mut r = DwgMergedReader::new(data.to_vec(), version, handles_at);
    r.setup_text_and_handle(handles_at);
    let mut out: Groups = Vec::new();
    // Object header: type, handle, extended data (skipped), owner, reactors, dictionary.
    r.read_object_type();
    let handle = r.main_mut().read_handle();
    r.set_ref_handle(handle);
    for _ in 0..MAX_EED {
        let size = r.read_bit_short();
        if size <= 0 {
            break;
        }
        r.main_mut().read_handle();
        r.read_bytes(usize::try_from(size).ok()?);
    }
    r.read_handle();
    let reactors = r.read_bit_long();
    if !(0..=MAX_CONNECTIONS).contains(&reactors) {
        return None;
    }
    let reactors: Vec<u64> = (0..reactors).map(|_| r.read_handle()).collect();
    if !reactors.is_empty() {
        out.push((102, "{ACAD_REACTORS".into()));
        out.extend(reactors.iter().map(|h| (330, hex(*h))));
        out.push((102, "}".into()));
    }
    let xdict = if r.read_bit() { None } else { Some(r.read_handle()) };
    if let Some(x) = xdict {
        out.push((102, "{ACAD_XDICTIONARY".into()));
        out.push((360, hex(x)));
        out.push((102, "}".into()));
    }
    if r.read_bit() {
        // Binary data after the object (2013+): not in these records.
        return None;
    }
    // Evaluation expression: parent, version, value (none in a table), node.
    r.read_bit_long();
    let (major, minor) = (r.read_bit_long(), r.read_bit_long());
    if r.read_bit_short() != -9999 {
        return None;
    }
    let node = r.read_bit_long();
    out.extend([(100, "AcDbEvalExpr".into()), (90, node.to_string()), (98, major.to_string()), (99, minor.to_string())]);
    // Block element.
    let name = r.read_variable_text();
    let (emajor, eminor, flags) = (r.read_bit_long(), r.read_bit_long(), r.read_bit_long());
    out.extend([(100, "AcDbBlockElement".into()), (300, name), (98, emajor.to_string()), (99, eminor.to_string()), (1071, flags.to_string())]);
    // Block parameter and one-point parameter.
    let (show, chain) = (r.read_bit(), r.read_bit());
    out.extend([(100, "AcDbBlockParameter".into()), (280, i32::from(show).to_string()), (281, i32::from(chain).to_string())]);
    let p = r.read_3bit_double();
    out.extend([(100, "AcDbBlock1PtParameter".into()), (1010, real(p.x)), (1020, real(p.y)), (1030, real(p.z))]);
    let mut connections = [0, 0];
    for c in &mut connections {
        *c = r.read_bit_long();
        // Tables have none; their groups aren't known.
        if *c != 0 {
            return None;
        }
    }
    let properties = r.read_bit_long();
    out.extend([(93, properties.to_string()), (170, "0".into()), (171, "0".into())]);
    // The table.
    out.push((100, "AcDbBlockPropertiesTable".into()));
    out.push((90, r.read_bit_long().to_string()));
    out.push((300, r.read_variable_text()));
    out.push((301, r.read_variable_text()));
    let columns = r.read_bit_long();
    if !(0..=MAX_COLUMNS).contains(&columns) {
        return None;
    }
    out.push((91, columns.to_string()));
    for _ in 0..columns {
        out.push((340, hex(r.read_handle())));
        out.push((170, r.read_bit_short().to_string()));
        out.push((171, r.read_bit_short().to_string()));
        out.push((300, r.read_variable_text()));
        out.push((301, r.read_variable_text()));
        out.push((90, r.read_bit_long().to_string()));
        value(&mut r, &mut out)?;
        value(&mut r, &mut out)?;
        for code in 290..=294 {
            out.push((code, i32::from(r.read_bit()).to_string()));
        }
        out.push((302, r.read_variable_text()));
        out.push((340, hex(r.read_handle())));
    }
    let rows = r.read_bit_long();
    if !(0..=MAX_ROWS).contains(&rows) {
        return None;
    }
    out.push((92, rows.to_string()));
    for _ in 0..rows {
        out.push((90, r.read_bit_long().to_string()));
        for _ in 0..columns {
            value(&mut r, &mut out)?;
            if r.check_error().is_err() {
                return None;
            }
        }
    }
    out.push((93, r.read_bit_long().to_string()));
    for code in 290..=292 {
        out.push((code, i32::from(r.read_bit()).to_string()));
    }
    // Read to its end and no further: anything else isn't this layout.
    (r.check_error().is_ok() && r.main_remaining_bits() < 8).then_some(out)
}

/// Decode every properties table the reader kept as raw record bytes into its DXF groups; how
/// many were decoded.
pub(crate) fn decode_all(doc: &mut acadrust::CadDocument) -> usize {
    let mut n = 0;
    for obj in doc.objects.values_mut() {
        let ObjectType::Unknown { type_name, raw_dxf_codes, raw_dwg_data: Some(data), raw_dwg_handle_bits, raw_dwg_version: Some(version), .. } = obj
        else {
            continue;
        };
        if !type_name.eq_ignore_ascii_case("BLOCKPROPERTIESTABLE") || raw_dxf_codes.is_some() || *version < DxfVersion::AC1024 {
            continue;
        }
        let (data, bits, version) = (data.clone(), *raw_dwg_handle_bits, *version);
        // The reader's own guards stop at the record's end; this one guards against the rest.
        if let Ok(Some(groups)) = std::panic::catch_unwind(|| decode(&data, bits, version)) {
            *raw_dxf_codes = Some(groups);
            n += 1;
        }
    }
    n
}
