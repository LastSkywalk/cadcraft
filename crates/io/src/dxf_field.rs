//! Block attribute fields in DXF (DXF Reference, FIELD). An ATTRIB's extension dictionary has an
//! `ACAD_FIELD` dictionary whose `TEXT` entry is the root FIELD; a FIELD lists its children (360)
//! and the objects it reads (331), then evaluation data, then its cached value (`7
//! ACFD_FIELD_VALUE` … `304 ACVALUE_END`, `301`, `98`).

use std::collections::HashMap;

use cadcraft_doc::field::{Field, FieldObject, FieldValue, HOST, MAX_FIELD_CHILDREN, MAX_FIELD_DEPTH};
use cadcraft_doc::{Handle, RawTag};
use cadcraft_dxf::Tag;

/// Most objects a field reads.
const MAX_FIELD_OBJECTS: usize = 256;
/// Most groups kept from one field's evaluation data.
const MAX_FIELD_DATA: usize = 4096;
/// Longest code or value group: DXF strings are split at 250 characters (code 3 continues 2).
const CHUNK: usize = 250;

/// The OBJECTS section's dictionaries and FIELD objects, by handle (upper case).
#[derive(Default)]
pub(crate) struct FieldIndex {
    dicts: HashMap<String, Vec<(String, String)>>,
    fields: HashMap<String, Vec<Tag>>,
}

fn up(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

/// The dictionaries and fields of a file's OBJECTS section (empty when it has no FIELD).
pub(crate) fn index(secs: &[cadcraft_dxf::Section]) -> FieldIndex {
    let mut ix = FieldIndex::default();
    let objects: Vec<&cadcraft_dxf::Section> = secs.iter().filter(|s| s.name == "OBJECTS").collect();
    if !objects.iter().any(|s| s.tags.iter().any(|t| t.code == 0 && t.str() == "FIELD")) {
        return ix;
    }
    for s in objects {
        for (kind, tags) in cadcraft_dxf::records(&s.tags) {
            let Some(h) = tags.iter().find(|t| t.code == 5).map(|t| up(&t.str())) else { continue };
            match kind.as_str() {
                "DICTIONARY" if ix.dicts.len() < crate::dxf_read::MAX_OBJECTS => {
                    let mut entries = Vec::new();
                    let mut key: Option<String> = None;
                    for t in &tags {
                        match t.code {
                            3 => key = Some(t.str()),
                            350 | 360 => {
                                if let Some(k) = key.take() {
                                    entries.push((k, up(&t.str())));
                                }
                            }
                            _ => {}
                        }
                    }
                    ix.dicts.insert(h, entries);
                }
                "FIELD" if ix.fields.len() < crate::dxf_read::MAX_OBJECTS => {
                    ix.fields.insert(h, tags);
                }
                _ => {}
            }
        }
    }
    ix
}

impl FieldIndex {
    fn entry(&self, dict: &str, key: &str) -> Option<&str> {
        self.dicts.get(dict)?.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, h)| h.as_str())
    }

    /// The field of an attribute record (its extension dictionary's `ACAD_FIELD` → `TEXT`), on
    /// block reference `host`.
    pub(crate) fn attrib_field(&self, tags: &[Tag], host: Option<Handle>) -> Option<Field> {
        if self.fields.is_empty() {
            return None;
        }
        let i = tags.iter().position(|t| t.code == 102 && t.str().trim() == "{ACAD_XDICTIONARY")?;
        let xdict = tags.get(i + 1..)?.iter().find(|t| t.code == 360).map(|t| up(&t.str()))?;
        let fields = self.entry(&xdict, "ACAD_FIELD")?;
        let root = self.entry(fields, "TEXT")?;
        self.field(root, host.map(Handle::hex), 0)
    }

    fn field(&self, h: &str, host: Option<String>, depth: usize) -> Option<Field> {
        if depth >= MAX_FIELD_DEPTH {
            return None;
        }
        let tags = self.fields.get(h)?;
        let start = tags.iter().position(|t| t.code == 100 && t.str() == "AcDbField").map_or(0, |i| i + 1);
        let tags = tags.get(start..)?;
        let is_host = |v: &str| host.as_deref().is_some_and(|h| Handle::parse_hex(v).map(Handle::hex).as_deref() == Some(h));
        let mut f = Field {
            evaluator: String::new(),
            code: String::new(),
            children: Vec::new(),
            objects: Vec::new(),
            data: Vec::new(),
            value: FieldValue::None,
        };
        let mut i = 0;
        // Evaluator, code (with its continuations), children, objects.
        let mut counted = false;
        while let Some(t) = tags.get(i) {
            match t.code {
                1 if f.evaluator.is_empty() => f.evaluator = t.str(),
                2 | 3 => f.code.push_str(&t.str()),
                90 if !counted => counted = true,
                360 => {
                    if f.children.len() < MAX_FIELD_CHILDREN
                        && let Some(c) = self.field(&up(&t.str()), host.clone(), depth + 1)
                    {
                        f.children.push(c);
                    }
                }
                97 => {}
                331 => {
                    if f.objects.len() < MAX_FIELD_OBJECTS {
                        let v = t.str();
                        f.objects.push(if is_host(&v) {
                            FieldObject::Host
                        } else {
                            Handle::parse_hex(&v).map_or(FieldObject::Object { handle: Handle(0) }, |handle| FieldObject::Object { handle })
                        });
                    }
                }
                _ => break,
            }
            i += 1;
        }
        // Evaluation data up to the cached value.
        while let Some(t) = tags.get(i) {
            if t.code == 7 && t.str() == "ACFD_FIELD_VALUE" {
                break;
            }
            if f.data.len() < MAX_FIELD_DATA {
                let v = t.str();
                let v = if t.code == 330 && is_host(&v) { HOST.to_string() } else { v };
                f.data.push(RawTag { code: t.code, value: v });
            }
            i += 1;
        }
        f.value = value(tags.get(i..).unwrap_or(&[]));
        Some(f)
    }
}

/// A cached value: its data type (90: 1 long, 2 double, 4 text), the value and what it shows
/// (301, else 302).
fn value(tags: &[Tag]) -> FieldValue {
    let find = |code: i32| tags.iter().find(|t| t.code == code);
    let shown = find(301).map(Tag::str).filter(|s| !s.is_empty()).or_else(|| find(302).map(Tag::str)).unwrap_or_default();
    match find(90).map(Tag::i64) {
        Some(1) => find(91).map_or(FieldValue::None, |v| FieldValue::Long { value: v.i64(), text: shown }),
        Some(2) => find(140).map(Tag::f64).filter(|v| v.is_finite()).map_or(FieldValue::None, |value| FieldValue::Double { value, text: shown }),
        Some(4) => FieldValue::Text { text: find(1).map(Tag::str).unwrap_or(shown) },
        _ => FieldValue::None,
    }
}

/// A string split into 250-character DXF groups.
fn chunks(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars.chunks(CHUNK).map(|c| c.iter().collect()).collect()
}

/// The FIELD objects of an attribute's field, as (handle, groups) records owned by the
/// dictionary `owner`, with `next` giving new handles; `host` is the block reference's handle.
pub(crate) fn field_records(f: &Field, owner: &str, host: &str, next: &mut dyn FnMut() -> String, out: &mut Vec<(String, Vec<Tag>)>) -> String {
    write_field(f, owner, host, next, out, 0)
}

fn write_field(f: &Field, owner: &str, host: &str, next: &mut dyn FnMut() -> String, out: &mut Vec<(String, Vec<Tag>)>, depth: usize) -> String {
    let h = next();
    let at = out.len();
    out.push((h.clone(), Vec::new()));
    let children: Vec<String> = if depth + 1 < MAX_FIELD_DEPTH {
        f.children.iter().take(MAX_FIELD_CHILDREN).map(|c| write_field(c, &h, host, next, out, depth + 1)).collect()
    } else {
        Vec::new()
    };
    let mut t = vec![Tag::s(0, "FIELD"), Tag::s(5, h.clone()), Tag::s(330, owner), Tag::s(100, "AcDbField"), Tag::s(1, f.evaluator.clone())];
    // A code longer than one group goes on in overflow groups (3).
    let parts = chunks(&f.code);
    if let Some((first, rest)) = parts.split_first() {
        t.push(Tag::s(2, first.clone()));
        t.extend(rest.iter().map(|p| Tag::s(3, p.clone())));
    }
    t.push(Tag::i(90, children.len() as i64));
    t.extend(children.iter().map(|c| Tag::s(360, c.clone())));
    t.push(Tag::i(97, f.objects.len() as i64));
    for o in &f.objects {
        t.push(Tag::s(
            331,
            match o {
                FieldObject::Host => host.to_string(),
                FieldObject::Object { handle } => handle.hex(),
            },
        ));
    }
    if f.data.is_empty() {
        // Evaluation flags, filing, state, status and error: what AutoCAD writes for a field
        // it has evaluated.
        t.extend([Tag::i(91, 63), Tag::i(92, 0), Tag::i(94, 59), Tag::i(95, 2), Tag::i(96, 0), Tag::s(300, ""), Tag::i(93, 0)]);
    } else {
        for r in &f.data {
            let v = if r.code == 330 && r.value == HOST { host.to_string() } else { r.value.clone() };
            t.push(crate::dxf_raw::tag(&RawTag { code: r.code, value: v }));
        }
    }
    t.push(Tag::s(7, "ACFD_FIELD_VALUE"));
    let format = f.format().unwrap_or("").to_string();
    let shown = f.value.text().to_string();
    match &f.value {
        FieldValue::None => t.extend([Tag::i(93, 0), Tag::i(90, 0), Tag::i(94, 0), Tag::s(300, ""), Tag::s(302, ""), Tag::s(304, "ACVALUE_END")]),
        FieldValue::Long { value, .. } => {
            t.extend([
                Tag::i(93, 4),
                Tag::i(90, 1),
                Tag::i(91, *value),
                Tag::i(94, 0),
                Tag::s(300, format),
                Tag::s(302, shown.clone()),
                Tag::s(304, "ACVALUE_END"),
            ]);
        }
        FieldValue::Double { value, .. } => {
            t.extend([
                Tag::i(93, 4),
                Tag::i(90, 2),
                Tag::f(140, *value),
                Tag::i(94, 0),
                Tag::s(300, format),
                Tag::s(302, shown.clone()),
                Tag::s(304, "ACVALUE_END"),
            ]);
        }
        FieldValue::Text { text } => {
            t.extend([
                Tag::i(93, 4),
                Tag::i(90, 4),
                Tag::s(1, text.clone()),
                Tag::i(94, 0),
                Tag::s(300, format),
                Tag::s(302, shown.clone()),
                Tag::s(304, "ACVALUE_END"),
            ]);
        }
    }
    let shown_parts = chunks(&shown);
    if let Some((first, rest)) = shown_parts.split_first() {
        t.push(Tag::s(301, first.clone()));
        t.extend(rest.iter().map(|p| Tag::s(9, p.clone())));
    }
    t.push(Tag::i(98, shown.chars().count() as i64));
    if let Some(slot) = out.get_mut(at) {
        slot.1 = t;
    }
    h
}
