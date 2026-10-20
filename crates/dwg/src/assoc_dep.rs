//! Geometry dependencies of associative networks (`ACDBASSOCGEOMDEPENDENCY`) the DWG reader
//! couldn't decode. It reads the ones whose persistent subentity is a single edge or an ACIS one,
//! and drops the others — those naming a subentity by type and index
//! (`AcDbAssocSimplePersSubentId`), which every constraint-driven dynamic block has. Without them AutoCAD can't evaluate the constraint
//! parameters of a drawing CADCraft saved, and shows `##` in the attribute fields that read them
//! (#1321).
//!
//! The reader reports where each skipped record is in the objects section, so they are read
//! again from there: the record (its size, its handle stream's size, its bits), the object
//! header, the dependency (as the reader reads it for the dependencies it decodes), the geometry
//! dependency's version and flag, the subentity class (2: type, index and two flags), and the
//! compound-object flag. They are written as AutoCAD writes them in DXF.

use std::collections::{HashMap, HashSet};

use acadrust::io::dwg::dwg_stream_readers::merged_reader::DwgMergedReader;
use acadrust::objects::ObjectType;
use acadrust::types::{DxfVersion, Handle};

use crate::record::{Groups, hex};

/// The class name, as classes and type codes name it.
const CLASS: &str = "ACDBASSOCGEOMDEPENDENCY";
/// Most records recovered from one file.
const MAX_RECORDS: usize = 1_000_000;
/// The persistent subentity class this decodes (`AcDbAssocSimplePersSubentId`).
const SIMPLE: i32 = 2;

fn bit(b: bool) -> String {
    i32::from(b).to_string()
}

/// A record in the objects section at `at`: its bytes and its handle stream's size in bits.
fn record(objects: &[u8], at: usize) -> Option<(&[u8], i64)> {
    // Size: a modular short (15 bits per little-endian word, high bit continues).
    let (mut size, mut shift, mut p) = (0usize, 0u32, at);
    loop {
        let w = usize::from(u16::from_le_bytes([*objects.get(p)?, *objects.get(p + 1)?]));
        p += 2;
        size |= (w & 0x7fff).checked_shl(shift)?;
        if w & 0x8000 == 0 {
            break;
        }
        shift += 15;
        if shift > 45 {
            return None;
        }
    }
    // Handle stream size (2010+): an unsigned modular char (7 bits per byte, high bit continues).
    let (mut bits, mut shift) = (0i64, 0u32);
    loop {
        let b = i64::from(*objects.get(p)?);
        p += 1;
        bits |= (b & 0x7f).checked_shl(shift)?;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 49 {
            return None;
        }
    }
    Some((objects.get(p..p.checked_add(size)?)?, bits))
}

/// Decode one record into (handle, owner, groups), or `None` when it isn't one this reads whole.
pub(crate) fn decode(data: &[u8], handle_bits: i64, version: DxfVersion) -> Option<(u64, u64, Groups)> {
    let (mut r, header) = crate::record::open(data, handle_bits, version)?;
    let (handle, owner) = (header.handle, header.owner);
    let mut g = header.groups();
    // The dependency.
    g.push((100, "AcDbAssocDependency".into()));
    g.push((90, r.read_bit_short().to_string()));
    g.push((90, r.read_bit_long().to_string()));
    for _ in 0..4 {
        g.push((290, bit(r.read_bit())));
    }
    g.push((90, r.read_bit_long().to_string()));
    g.push((330, hex(r.read_handle())));
    let named = r.read_bit();
    g.push((290, bit(named)));
    if named {
        g.push((1, r.read_variable_text()));
    }
    g.push((330, hex(r.read_handle())));
    g.push((330, hex(r.read_handle())));
    g.push((360, hex(r.read_handle())));
    g.push((90, r.read_bit_long().to_string()));
    // The geometry dependency and its persistent subentity.
    g.push((100, "AcDbAssocGeomDependency".into()));
    g.push((90, r.read_bit_short().to_string()));
    g.push((290, bit(r.read_bit())));
    if r.read_bit() || r.read_bit_long() != SIMPLE {
        return None;
    }
    g.push((1, "AcDbAssocSimplePersSubentId".into()));
    for _ in 0..3 {
        g.push((90, r.read_bit_long().to_string()));
    }
    g.push((290, bit(r.read_bit())));
    g.push((290, bit(r.read_bit())));
    (r.check_error().is_ok() && r.main_remaining_bits() < 8).then_some((handle, owner, g))
}

/// The file's objects section and handle map, read again for the records the reader skipped or
/// misread.
pub(crate) struct Raw {
    objects: Vec<u8>,
    handles: HashMap<u64, i64>,
    version: DxfVersion,
}

impl Raw {
    /// Read them from `bytes` when `doc` has associative objects (else there is nothing to fix).
    pub(crate) fn load(bytes: &[u8], doc: &acadrust::CadDocument) -> Option<Raw> {
        if doc.version < DxfVersion::AC1024 || !doc.classes.iter().any(|c| c.dxf_name.to_ascii_uppercase().starts_with("ACDBASSOC")) {
            return None;
        }
        let mut reader = acadrust::DwgReader::from_stream_with_options(std::io::Cursor::new(bytes), acadrust::DwgReadOptions::failsafe());
        let info = reader.read_file_header().ok()?;
        let objects = reader.get_section_buffer("AcDb:AcDbObjects", &info).ok()?;
        let map = reader.get_section_buffer("AcDb:Handles", &info).ok()?;
        let handles = acadrust::io::dwg::dwg_stream_readers::handle_reader::read_handles(&map).ok()?;
        Some(Raw { objects, handles, version: doc.version })
    }

    /// The record of `handle` at section offset `at`, read up to its object header: the reader
    /// positioned at the object's own data.
    fn object(&self, at: usize, handle: u64) -> Option<DwgMergedReader> {
        let (data, bits) = record(&self.objects, at)?;
        let (r, header) = crate::record::open(data, bits, self.version)?;
        (header.handle == handle).then_some(r)
    }
}

/// Recover the geometry dependencies the read skipped into `doc`; their handles.
pub(crate) fn recover(raw: &Raw, stats: &acadrust::ReadStats, doc: &mut acadrust::CadDocument) -> HashSet<u64> {
    let mut out = HashSet::new();
    let codes: HashSet<String> = doc.classes.iter().filter(|c| c.dxf_name.eq_ignore_ascii_case(CLASS)).map(|c| c.class_number.to_string()).collect();
    let wanted: HashMap<u64, u64> = stats
        .diagnostics
        .iter()
        .filter(|d| d.section.as_deref() == Some("AcDb:AcDbObjects") && d.source_offset_basis.as_deref() == Some("object-section-byte"))
        .filter(|d| d.record_type.as_ref().is_some_and(|t| codes.contains(t)))
        .filter_map(|d| Some((d.record_handle?, d.source_offset?)))
        .take(MAX_RECORDS)
        .collect();
    for (h, at) in wanted {
        let Some((data, bits)) = usize::try_from(at).ok().and_then(|at| record(&raw.objects, at)) else { continue };
        let Some((handle, owner, groups)) = decode(data, bits, raw.version).filter(|(handle, _, _)| *handle == h) else { continue };
        let key = Handle::from(handle);
        if doc.objects.contains_key(&key) {
            continue;
        }
        doc.objects.insert(
            key,
            ObjectType::Unknown {
                type_name: CLASS.into(),
                handle: key,
                owner: Handle::from(owner),
                raw_dxf_codes: Some(groups),
                raw_dwg_data: None,
                raw_dwg_handle_bits: 0,
                raw_dwg_version: None,
            },
        );
        out.insert(handle);
    }
    out
}

/// The persistent subentity class of a geometry dependency the reader decodes without naming
/// it: an edge by its three indices (`AcDbAssocEdgePersSubentId`, class 3). Named, it is written
/// as AutoCAD writes it; unnamed, AutoCAD rejects the constraint group it belongs to.
pub(crate) fn name_edges(doc: &mut acadrust::CadDocument) {
    for obj in doc.objects.values_mut() {
        if let ObjectType::Associative(a) = obj
            && let acadrust::objects::AssociativeData::GeomDependency(g) = &mut a.data
            && g.persistent_subent.class_name.is_empty()
            && g.persistent_subent.class_code == 3
            && g.persistent_subent.values.len() == 3
        {
            g.persistent_subent.class_name = "AcDbAssocEdgePersSubentId".into();
        }
    }
}

/// Most dependencies and values read from an action to reach a constraint group's own data.
const MAX_LIST: i32 = 100_000;

/// The node count of each 2D constraint group, read from its record: the reader keeps the
/// nodes, not this count, and writes their number in its place, which makes AutoCAD drop the
/// group's constraints (`##` in the attributes reading a constrained parameter).
pub(crate) fn node_counts(raw: &Raw, doc: &acadrust::CadDocument) -> HashMap<u64, i32> {
    let mut out = HashMap::new();
    for (h, obj) in &doc.objects {
        let ObjectType::Associative(a) = obj else { continue };
        if !matches!(a.data, acadrust::objects::AssociativeData::ConstraintGroup(_)) {
            continue;
        }
        let Some(at) = raw.handles.get(&h.value()).and_then(|a| usize::try_from(*a).ok()) else { continue };
        let Some(mut r) = raw.object(at, h.value()) else { continue };
        // The action: version, status, network, body, index, highest dependency index, the
        // dependencies, then (version 2 and later) owned parameters and values.
        let version = r.read_bit_short();
        r.read_bit_long();
        r.read_handle();
        r.read_handle();
        r.read_bit_long();
        r.read_bit_long();
        let deps = r.read_bit_long();
        if !(0..=MAX_LIST).contains(&deps) {
            continue;
        }
        for _ in 0..deps {
            r.read_bit();
            r.read_handle();
        }
        if version > 1 {
            r.read_bit_short();
            let params = r.read_bit_long();
            if !(0..=MAX_LIST).contains(&params) {
                continue;
            }
            for _ in 0..params {
                r.read_handle();
            }
            r.read_bit_short();
            // Values have a layout of their own; groups with them aren't fixed.
            if r.read_bit_long() != 0 {
                continue;
            }
        }
        // The group: version, flag, work plane, dependency, the actions, then the node count.
        r.read_bit_long();
        r.read_bit();
        for _ in 0..3 {
            r.read_3bit_double();
        }
        r.read_handle();
        let actions = r.read_bit_long();
        if !(0..=MAX_LIST).contains(&actions) {
            continue;
        }
        for _ in 0..actions {
            r.read_handle();
        }
        let nodes = r.read_bit_long();
        if r.check_error().is_ok() && nodes >= 0 {
            out.insert(h.value(), nodes);
        }
    }
    out
}

/// Put each constraint group's node count (by handle) into the DXF the bridge wrote: the first
/// 90 after its actions (`AcDbAssoc2dConstraintGroup`: version 90, flag 70, three points, the
/// dependency 360, the action count 90 and the actions).
pub(crate) fn patch_node_counts(dxf: Vec<u8>, counts: &HashMap<u64, i32>) -> (Vec<u8>, usize) {
    if counts.is_empty() {
        return (dxf, 0);
    }
    // Lines as byte ranges, values trimmed.
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    for (i, b) in dxf.iter().enumerate() {
        if *b == b'\n' {
            lines.push((start, i));
            start = i + 1;
        }
    }
    let text = |r: (usize, usize)| -> &[u8] { dxf.get(r.0..r.1).unwrap_or(&[]).trim_ascii() };
    let mut edits: Vec<((usize, usize), Vec<u8>)> = Vec::new();
    let mut i = 0;
    while i + 1 < lines.len() {
        let (Some(&c), Some(&v)) = (lines.get(i), lines.get(i + 1)) else { break };
        if text(c) != b"0" || text(v) != b"ACDBASSOC2DCONSTRAINTGROUP" {
            i += 2;
            continue;
        }
        // The record's pairs up to the next one.
        let mut pairs: Vec<((usize, usize), (usize, usize))> = Vec::new();
        let mut j = i + 2;
        while j + 1 < lines.len() {
            let (Some(&c), Some(&v)) = (lines.get(j), lines.get(j + 1)) else { break };
            if text(c) == b"0" {
                break;
            }
            pairs.push((c, v));
            j += 2;
        }
        i = j;
        let code = |k: usize| pairs.get(k).map(|p| text(p.0));
        let value = |k: usize| pairs.get(k).map(|p| text(p.1));
        let handle =
            pairs.iter().find(|p| text(p.0) == b"5").and_then(|p| std::str::from_utf8(text(p.1)).ok()).and_then(|h| u64::from_str_radix(h, 16).ok());
        let Some(n) = handle.and_then(|h| counts.get(&h)) else { continue };
        let Some(m) = pairs.iter().position(|p| text(p.1) == b"AcDbAssoc2dConstraintGroup") else { continue };
        // 90 version, 70 flag, nine coordinates, 360 dependency, 90 count.
        let at = m + 1 + 1 + 1 + 9 + 1;
        if code(at) != Some(b"90") {
            continue;
        }
        let Some(count) = value(at).and_then(|v| std::str::from_utf8(v).ok()).and_then(|v| v.parse::<usize>().ok()) else { continue };
        let Some(target) = count.checked_add(at + 1) else { continue };
        if code(target) != Some(b"90") || (at + 1..target).any(|k| code(k) != Some(b"360")) {
            continue;
        }
        if let Some(p) = pairs.get(target) {
            edits.push((p.1, n.to_string().into_bytes()));
        }
    }
    let n = edits.len();
    if n == 0 {
        return (dxf, 0);
    }
    // The value lines replaced, keeping each line's own indentation and ending.
    edits.sort_by_key(|((s, _), _)| *s);
    let mut out = Vec::with_capacity(dxf.len() + n * 4);
    let mut at = 0;
    for ((s, e), new) in edits {
        let line = dxf.get(s..e).unwrap_or(&[]);
        let lead = line.iter().take_while(|b| b.is_ascii_whitespace()).count();
        let trail = line.iter().rev().take_while(|b| b.is_ascii_whitespace()).count();
        out.extend_from_slice(dxf.get(at..s + lead).unwrap_or(&[]));
        out.extend_from_slice(&new);
        at = e.saturating_sub(trail).max(s + lead);
    }
    out.extend_from_slice(dxf.get(at..).unwrap_or(&[]));
    (out, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_is_its_size_its_handle_bits_and_its_bytes() {
        // Size 5 (a one-word modular short), 3 handle bits (a one-byte modular char), 5 bytes.
        let objects = [0u8, 0, 5, 0, 3, 1, 2, 3, 4, 5, 9];
        assert_eq!(record(&objects, 2), Some((&[1u8, 2, 3, 4, 5][..], 3)));
        // A two-word size (0x8001, 0x0001 → 1 + 2^15) runs past the end.
        assert_eq!(record(&[0x01, 0x80, 0x01, 0x00, 0x00, 1, 2], 0), None);
        assert_eq!(record(&[5, 0], 0), None);
        assert_eq!(record(&[], 0), None);
        // A modular char that never ends.
        assert_eq!(record(&[1, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff], 0), None);
    }

    #[test]
    fn hostile_records_decode_to_nothing() {
        // A fixed pseudo-random stream: no record of this layout, and no panic.
        let mut x: u32 = 0x1234_5678;
        for len in [0usize, 1, 7, 64, 513] {
            for bits in [-5i64, 0, 7, 64, 100_000] {
                let data: Vec<u8> = (0..len)
                    .map(|_| {
                        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        (x >> 24) as u8
                    })
                    .collect();
                let _ = decode(&data, bits, DxfVersion::AC1032);
                let _ = crate::props_table::decode(&data, bits, DxfVersion::AC1032);
            }
        }
    }

    fn group(code: i32, value: &str) -> String {
        format!("{code:>3}\r\n{value}\r\n")
    }

    /// A constraint group as the DWG bridge writes it, with `actions` action handles.
    fn constraint_group(handle: &str, actions: usize, nodes: &str) -> String {
        let mut s = group(0, "ACDBASSOC2DCONSTRAINTGROUP") + &group(5, handle) + &group(100, "AcDbAssocAction") + &group(90, "     2");
        s += &group(100, "AcDbAssoc2dConstraintGroup");
        s += &group(90, "     2");
        s += &group(70, "     0");
        for c in [10, 20, 30, 10, 20, 30, 10, 20, 30] {
            s += &group(c, "0.0");
        }
        s += &group(360, "0");
        s += &group(90, &format!("{actions:>6}"));
        for i in 0..actions {
            s += &group(360, &format!("{:X}", 0x100 + i));
        }
        s += &group(90, nodes);
        s += &group(90, "     0");
        s
    }

    #[test]
    fn node_counts_go_where_the_bridge_wrote_the_node_number() {
        let dxf = constraint_group("1A", 3, "    12") + &constraint_group("1B", 1, "     4") + &group(0, "ENDSEC");
        let counts: HashMap<u64, i32> = [(0x1A, 369), (0xFF, 7)].into_iter().collect();
        let (out, n) = patch_node_counts(dxf.clone().into_bytes(), &counts);
        assert_eq!(n, 1);
        let out = String::from_utf8(out).unwrap_or_default();
        // The first group's count is AutoCAD's (wider than before), the second's untouched.
        assert_eq!(out, dxf.replacen("\r\n    12\r\n", "\r\n    369\r\n", 1));
        // A record whose action count doesn't match its actions is left alone.
        let odd = constraint_group("1A", 2, "    12").replace("     2\r\n360\r\n100", "     3\r\n360\r\n100");
        assert_ne!(odd, constraint_group("1A", 2, "    12"));
        assert_eq!(patch_node_counts(odd.clone().into_bytes(), &counts), (odd.into_bytes(), 0));
    }

    #[test]
    fn an_action_count_past_the_end_of_memory_is_left_alone() {
        let counts: HashMap<u64, i32> = [(0x1A, 369)].into_iter().collect();
        let dxf = constraint_group("1A", 1, "    12");
        let huge = dxf.replace("     1\r\n360", &format!("{}\r\n360", usize::MAX));
        assert_ne!(huge, dxf);
        assert_eq!(patch_node_counts(huge.clone().into_bytes(), &counts), (huge.into_bytes(), 0));
    }
}
