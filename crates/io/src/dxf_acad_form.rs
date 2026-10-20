//! Dynamic block objects in the form AutoCAD's DXF reader takes. A drawing read from a DWG keeps
//! them as the DWG bridge wrote them, and AutoCAD rejects a few of its groups outright
//! ("Unexpected DXF group code", the whole file fails to open), so a DXF CADCraft saves writes
//! them as AutoCAD writes them itself (#1321). Measured against AutoCAD's own DXF of the same
//! drawings:
//!
//! - an evaluation expression with no value has no value group (the bridge writes `70 -9999`);
//!   a user parameter's empty value is `309 AcDbEvalVariant NULL = -9999`;
//! - constraint parameters keep their value set in 141–143 (the bridge: 128–130), an alignment
//!   grip its orientation in 141/142 (the bridge: 150/160);
//! - lookup parameters and grips are `AcDbBlockLookUp…` with a capital U, the parameter's
//!   label and description before its lookup action (94);
//! - a properties table grip, a dynamic block proxy node and a constraint group have no subclass
//!   of their own;
//! - move, stretch and polar stretch actions interleave their connections (`92 301 93 302…`);
//! - an associative variable lists its dependencies after their count (the bridge writes the
//!   count alone): each dependency it owns, with its index.
//!
//! The DWG bridge reads the bridge's own form, so this is for DXF output only.

use cadcraft_dxf::{Tag, Value};

use crate::dxf_ext::owner;

/// Most groups moved within one subclass.
const MAX_RUN: usize = 64;

fn is_marker(t: &Tag, name: &str) -> bool {
    t.code == 100 && t.str() == name
}

/// Rewrite the dynamic block objects among `tags` (a whole file) in AutoCAD's form.
pub(crate) fn normalize(tags: &mut Vec<Tag>) {
    if !tags.iter().any(|t| t.code == 100 && matches!(t.str().as_str(), "AcDbEvalExpr" | "AcDbAssocAction" | "AcDbAssocDependency")) {
        return;
    }
    let deps = dependencies(tags);
    let mut out = Vec::with_capacity(tags.len());
    for rec in records(tags) {
        let kind = rec.first().map(Tag::str).unwrap_or_default();
        if kind == "ACDBASSOCVARIABLE" {
            out.extend(variable(rec, &deps));
        } else if (kind.starts_with("BLOCK") && rec.iter().any(|t| is_marker(t, "AcDbEvalExpr")))
            || matches!(kind.as_str(), "ACDB_DYNAMICBLOCKPROXYNODE" | "ACDBASSOC2DCONSTRAINTGROUP")
        {
            out.extend(record(rec));
        } else {
            out.extend_from_slice(rec);
        }
    }
    *tags = out;
}

/// The records of a run of tags, each from its `0` group up to the next.
fn records(tags: &[Tag]) -> impl Iterator<Item = &[Tag]> {
    tags.chunk_by(|_, t| t.code != 0)
}

/// The associative dependencies of each owner (an associative variable): handles in file order.
fn dependencies(tags: &[Tag]) -> std::collections::HashMap<String, Vec<String>> {
    let mut out: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for rec in records(tags) {
        let kind = rec.first().map(Tag::str).unwrap_or_default();
        if kind.starts_with("ACDBASSOC")
            && kind.ends_with("DEPENDENCY")
            && let (Some(o), Some(h)) = (owner(rec), rec.iter().find(|t| t.code == 5).map(|t| t.str().trim().to_ascii_uppercase()))
        {
            let list = out.entry(o.trim().to_ascii_uppercase()).or_default();
            if list.len() < MAX_RUN {
                list.push(h);
            }
        }
    }
    out
}

/// An associative variable: AutoCAD follows its dependency count with each dependency and its
/// index (`90 n`, then `360 h 90 i` n times); the bridge writes the count alone, so the
/// dependencies the variable owns are put back.
fn variable(rec: &[Tag], deps: &std::collections::HashMap<String, Vec<String>>) -> Vec<Tag> {
    let mut out = rec.to_vec();
    let Some(var) = out.iter().position(|t| is_marker(t, "AcDbAssocVariable")) else { return out };
    let truncated = out.last().is_some_and(|t| t.code == 90 && t.i64() > 0) && out.get(out.len().saturating_sub(2)).is_some_and(|t| t.code == 290);
    if !truncated || out.len() <= var {
        return out;
    }
    let me = out.iter().find(|t| t.code == 5).map(|t| t.str().trim().to_ascii_uppercase()).unwrap_or_default();
    let list = deps.get(&me).cloned().unwrap_or_default();
    out.pop();
    out.push(Tag::i(90, list.len() as i64));
    for (i, h) in list.into_iter().enumerate() {
        out.push(Tag::s(360, h));
        out.push(Tag::i(90, i as i64));
    }
    out
}

/// The record split into its subclasses: the groups before the first marker, then each marker
/// with the groups after it.
fn subclasses(rec: &[Tag]) -> Vec<Vec<Tag>> {
    let mut out: Vec<Vec<Tag>> = vec![Vec::new()];
    for t in rec {
        if t.code == 100 {
            out.push(vec![t.clone()]);
        } else if let Some(last) = out.last_mut() {
            last.push(t.clone());
        }
    }
    out
}

fn real(code: i32, t: &Tag) -> Tag {
    Tag::new(code, Value::Real(t.f64()))
}

fn record(rec: &[Tag]) -> Vec<Tag> {
    let mut out = Vec::with_capacity(rec.len() + 2);
    for mut sub in subclasses(rec) {
        let name = sub.first().filter(|t| t.code == 100).map(Tag::str).unwrap_or_default();
        match name.as_str() {
            "AcDbEvalExpr" => {
                // A null value is no value.
                if sub.last().is_some_and(|t| t.code == 70 && t.i64() == -9999) {
                    sub.pop();
                }
            }
            "AcDbBlockUserParameter" => {
                for t in &mut sub {
                    if t.code == 70 && t.i64() == -9999 {
                        *t = Tag::s(309, "AcDbEvalVariant NULL = -9999");
                    }
                }
            }
            "AcDbBlockAlignmentGrip" => {
                for t in &mut sub {
                    match t.code {
                        150 => *t = real(141, t),
                        160 => *t = real(142, t),
                        _ => {}
                    }
                }
            }
            "AcDbBlockPropertiesTableGrip" | "AcDbDynamicBlockProxyNode" | "AcDbAssoc2dConstraintGroup" => {
                // No subclass of its own: anything after the marker belongs to the grip before.
                sub.remove(0);
            }
            "AcDbBlockLookupGrip" => {
                if let Some(t) = sub.first_mut() {
                    *t = Tag::s(100, "AcDbBlockLookUpGrip");
                }
            }
            "AcDbBlockLookupParameter" => {
                if let Some(t) = sub.first_mut() {
                    *t = Tag::s(100, "AcDbBlockLookUpParameter");
                }
                // Label and description, then the lookup action.
                if let Some(a) = sub.iter().position(|t| t.code == 94)
                    && let Some(d) = sub.iter().position(|t| t.code == 304)
                    && a < d
                {
                    let action = sub.remove(a);
                    sub.insert(d, action);
                }
            }
            "AcDbBlockMoveAction" | "AcDbBlockStretchAction" | "AcDbBlockPolarStretchAction" => interleave(&mut sub),
            n if n.ends_with("ConstraintParameter") => {
                for t in &mut sub {
                    match t.code {
                        128 => *t = real(141, t),
                        129 => *t = real(142, t),
                        130 => *t = real(143, t),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        out.extend(sub);
    }
    out
}

/// Connections as AutoCAD writes them: each graph id (92, 93…) followed by its name (301, 302…),
/// where the bridge writes every id and then every name.
fn interleave(sub: &mut [Tag]) {
    let Some(start) = sub.iter().position(|t| t.code == 92) else { return };
    let ids = sub.get(start..).unwrap_or(&[]).iter().take_while(|t| (92..=99).contains(&t.code)).count().min(MAX_RUN);
    let names = sub.get(start + ids..).unwrap_or(&[]).iter().take_while(|t| (301..=309).contains(&t.code)).count();
    if ids < 2 || ids != names {
        return;
    }
    let run: Vec<Tag> = sub.get(start..start + 2 * ids).map(<[Tag]>::to_vec).unwrap_or_default();
    let mut k = start;
    for j in 0..ids {
        if let (Some(id), Some(name)) = (run.get(j), run.get(ids + j)) {
            if let Some(slot) = sub.get_mut(k) {
                *slot = id.clone();
            }
            if let Some(slot) = sub.get_mut(k + 1) {
                *slot = name.clone();
            }
            k += 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(src: &[(i32, &str)]) -> Vec<Tag> {
        src.iter().map(|(c, v)| Tag::s(*c, *v)).collect()
    }
    fn codes(t: &[Tag]) -> Vec<(i32, String)> {
        t.iter().map(|t| (t.code, t.str())).collect()
    }

    #[test]
    fn bridge_form_becomes_autocads() {
        let mut t = tags(&[
            (0, "BLOCKVISIBILITYPARAMETER"),
            (100, "AcDbEvalExpr"),
            (90, "65"),
            (98, "33"),
            (99, "175"),
            (70, "-9999"),
            (100, "AcDbBlockElement"),
            (0, "BLOCKMOVEACTION"),
            (100, "AcDbEvalExpr"),
            (90, "5"),
            (100, "AcDbBlockMoveAction"),
            (92, "1"),
            (93, "1"),
            (301, "XDelta"),
            (302, "YDelta"),
            (140, "1.0"),
            (0, "BLOCKLOOKUPPARAMETER"),
            (100, "AcDbEvalExpr"),
            (90, "16"),
            (100, "AcDbBlockLookupParameter"),
            (94, "20"),
            (303, "Lookup"),
            (304, ""),
            (1001, "ACAUTHENVIRON"),
            (0, "BLOCKUSERPARAMETER"),
            (100, "AcDbEvalExpr"),
            (90, "47"),
            (100, "AcDbBlockUserParameter"),
            (90, "0"),
            (301, ""),
            (70, "-9999"),
            (170, "5"),
            (0, "ENDSEC"),
        ]);
        normalize(&mut t);
        let c = codes(&t);
        let s = |a: &[(i32, &str)]| a.iter().map(|(c, v)| (*c, (*v).to_string())).collect::<Vec<_>>();
        assert_eq!(
            &c[..6],
            &s(&[(0, "BLOCKVISIBILITYPARAMETER"), (100, "AcDbEvalExpr"), (90, "65"), (98, "33"), (99, "175"), (100, "AcDbBlockElement")])[..]
        );
        let mv = c.iter().position(|x| x.1 == "AcDbBlockMoveAction").unwrap();
        assert_eq!(&c[mv + 1..mv + 5], &s(&[(92, "1"), (301, "XDelta"), (93, "1"), (302, "YDelta")])[..]);
        let lk = c.iter().position(|x| x.1 == "AcDbBlockLookUpParameter").unwrap();
        assert_eq!(&c[lk + 1..lk + 4], &s(&[(303, "Lookup"), (304, ""), (94, "20")])[..]);
        assert!(c.contains(&(309, "AcDbEvalVariant NULL = -9999".to_string())));
        assert!(!c.contains(&(70, "-9999".to_string())));
    }

    #[test]
    fn other_records_are_untouched() {
        let src = tags(&[(0, "LINE"), (100, "AcDbEntity"), (70, "-9999"), (0, "ENDSEC")]);
        let mut t = src.clone();
        normalize(&mut t);
        assert_eq!(t, src);
    }
}
