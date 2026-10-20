//! Dynamic block references: which dynamic block a block reference shows, the values it was given
//! and the custom properties those make.
//!
//! A reference whose values differ from the block's own refers to an anonymous block (`*U12`)
//! holding the result, and its extension dictionary says what it is: `AcDbBlockRepresentation`
//! has `AcDbRepData` (pointing at the dynamic block's BLOCK_RECORD) and an `AppDataCache` whose
//! `ACAD_ENHANCEDBLOCKDATA` dictionary holds one XRECORD per parameter, keyed by its graph id, with
//! the value in block coordinates. A reference to the dynamic block itself has the block's values.

use std::cell::OnceCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use cadcraft_geom::Vec3;
use serde::Serialize;

use super::{
    ActionKind, Cursor, DynamicBlock, ParameterKind, block_xdict, enhanced_data, entries, entry, objects, read_dynamic_block, section, sections,
};
use crate::foreign::ForeignObject;
use crate::{Drawing, EntityKind, Handle};

/// Most values one reference is read with.
const MAX_VALUES: usize = 10_000;

/// A parameter's value on one reference, in block coordinates.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Value {
    /// A visibility state's name or a lookup value.
    Text { value: String },
    /// A point parameter's position.
    Point { at: Vec3 },
    /// A two-point parameter's points.
    Points { base: Vec3, end: Vec3 },
}

/// A dynamic block reference.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    /// The dynamic block it shows (its name in the drawing).
    pub definition: String,
    /// The values it was given, by parameter graph id; parameters missing here have the block's
    /// own values.
    pub values: BTreeMap<i32, Value>,
}

/// One custom property of a reference, as the Properties palette shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomProperty {
    pub name: String,
    pub value: String,
    /// The values it may be set to, when it is a choice (visibility states, lookup values, a
    /// distance's list).
    pub choices: Vec<String>,
    /// CADCraft can change it ([`super::set_property`]): a visibility state, a distance, a
    /// position, or a lookup whose tables set only those.
    pub editable: bool,
}

/// A value XRECORD: its text (group 1) when it has one, else its points (10/20/30).
fn value(o: &ForeignObject) -> Option<Value> {
    if let Some(t) = o.tags.iter().find(|t| t.code == 1) {
        return Some(Value::Text { value: t.value.clone() });
    }
    let num = |code: i32, i: usize| {
        o.tags.iter().filter(|t| t.code == code).nth(i).and_then(|t| t.value.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
    };
    let point = |i: usize| Some(Vec3::new(num(10, i)?, num(20, i)?, num(30, i).unwrap_or(0.0)));
    match (point(0), point(1)) {
        (Some(base), Some(end)) => Some(Value::Points { base, end }),
        (Some(at), None) => Some(Value::Point { at }),
        _ => None,
    }
}

/// The dynamic block reference `insert` is, or `None` when it isn't one.
pub fn reference(d: &Drawing, insert: Handle) -> Option<Reference> {
    Reader::new(d).read_reference(insert)
}

/// A parameter's value as a field reads it (`AcObjProp … Parameter(N).Property`).
#[derive(Clone, Debug, PartialEq)]
pub enum ParamValue {
    Number(f64),
    Text(String),
}

/// Property `property` of parameter `node` of dynamic block reference `insert`, as a field reads
/// it: a distance (`UpdatedDistance`, `Distance`) in drawing units — the length between the
/// parameter's points, across for a horizontal constraint and up for a vertical one — a user
/// parameter's value (`UserVariable`), a visibility state or a lookup value.
pub fn parameter_value(d: &Drawing, insert: Handle, node: i32, property: &str) -> Option<ParamValue> {
    Reader::new(d).parameter_value(insert, node, property)
}

/// The `AcDbBlockRepresentation` dictionary in the extension dictionary of block reference
/// `insert`, when it has one.
fn representation(d: &Drawing, objects: &HashMap<Handle, &ForeignObject>, insert: Handle) -> Option<Handle> {
    d.foreign.entities.get(&insert).and_then(|x| x.xdict).and_then(|x| entry(objects, x, "AcDbBlockRepresentation"))
}

/// The XRECORD reference `insert` keeps parameter `node`'s value in, if it keeps one.
fn value_record<'a>(d: &Drawing, objects: &HashMap<Handle, &'a ForeignObject>, insert: Handle, node: i32) -> Option<&'a ForeignObject> {
    let data = enhanced_data(objects, representation(d, objects, insert)?)?;
    let h = entries(objects, data).into_iter().take(MAX_VALUES).find(|(k, _)| k.trim().parse::<i32>().ok() == Some(node))?.1;
    objects.get(&h).copied()
}

/// Reads the dynamic block references of one state of a drawing, for reading many of them
/// together (every attribute field of the drawing): the kept objects are indexed once, and each
/// reference and each dynamic block is read once.
pub(crate) struct Reader<'a> {
    d: &'a Drawing,
    /// The kept objects by handle, indexed when first needed.
    objects: OnceCell<HashMap<Handle, &'a ForeignObject>>,
    /// Each reference read, by handle (`None`: not a dynamic block reference).
    references: HashMap<Handle, Option<Rc<Reference>>>,
    /// Each dynamic block read, by its name in upper case (`None`: not a dynamic block).
    blocks: HashMap<String, Option<Rc<DynamicBlock>>>,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(d: &'a Drawing) -> Self {
        Self { d, objects: OnceCell::new(), references: HashMap::new(), blocks: HashMap::new() }
    }

    pub(crate) fn drawing(&self) -> &'a Drawing {
        self.d
    }

    fn objects(&self) -> &HashMap<Handle, &'a ForeignObject> {
        self.objects.get_or_init(|| objects(self.d))
    }

    /// The dynamic block data of block `name`, as [`super::dynamic_block`] reads it.
    fn block(&mut self, name: &str) -> Option<Rc<DynamicBlock>> {
        let key = name.trim().to_ascii_uppercase();
        if let Some(m) = self.blocks.get(&key) {
            return m.clone();
        }
        let m = block_xdict(self.d, name).and_then(|x| read_dynamic_block(self.objects(), x)).map(Rc::new);
        self.blocks.insert(key, m.clone());
        m
    }

    /// The dynamic block reference `insert` is, as [`reference`] reads it.
    pub(crate) fn reference(&mut self, insert: Handle) -> Option<Rc<Reference>> {
        if let Some(r) = self.references.get(&insert) {
            return r.clone();
        }
        let r = self.read_reference(insert).map(Rc::new);
        self.references.insert(insert, r.clone());
        r
    }

    fn read_reference(&mut self, insert: Handle) -> Option<Reference> {
        let d = self.d;
        let EntityKind::Insert(ins) = &d.entity(insert)?.kind else { return None };
        let Some(rep) = representation(d, self.objects(), insert) else {
            // No representation: a reference to a dynamic block with its own values.
            return self.block(&ins.block).is_some().then(|| Reference {
                definition: d.block(&ins.block).map_or_else(|| ins.block.clone(), |b| b.name.clone()),
                values: BTreeMap::new(),
            });
        };
        let objects = self.objects();
        let data = entry(objects, rep, "AcDbRepData").and_then(|h| objects.get(&h).copied())?;
        let target = data.tags.iter().find(|t| t.code == 340).and_then(|t| Handle::parse_hex(&t.value))?;
        let (kind, name) = d.foreign.targets.get(&target)?;
        let definition = d.block(name).filter(|_| kind == "BLOCK_RECORD")?.name.clone();
        let mut values = BTreeMap::new();
        for (key, h) in enhanced_data(objects, rep).map(|c| entries(objects, c)).unwrap_or_default().into_iter().take(MAX_VALUES) {
            if let (Ok(node), Some(v)) = (key.trim().parse::<i32>(), objects.get(&h).and_then(|o| value(o))) {
                values.insert(node, v);
            }
        }
        self.block(&definition)?;
        Some(Reference { definition, values })
    }

    /// Property `property` of parameter `node` of reference `insert`, as [`parameter_value`]
    /// reads it.
    pub(crate) fn parameter_value(&mut self, insert: Handle, node: i32, property: &str) -> Option<ParamValue> {
        let d = self.d;
        let r = self.reference(insert)?;
        let EntityKind::Insert(ins) = &d.entity(insert)?.kind else { return None };
        let m = self.block(&r.definition)?;
        let p = m.parameters.iter().find(|p| p.node == node)?;
        let (sx, sy) = (ins.scale.x.abs(), ins.scale.y.abs());
        match property.to_ascii_lowercase().as_str() {
            "updateddistance" | "distance" => {
                let (b, e) = match (r.values.get(&node), &p.kind) {
                    (Some(Value::Points { base, end }), _) => (*base, *end),
                    (_, ParameterKind::Linear { base, end, .. }) => (*base, *end),
                    _ => {
                        // A constraint parameter has its points where a linear one has them.
                        let o = self.objects().get(&p.handle)?;
                        let secs = sections(&o.tags);
                        let mut c = Cursor::new(section(&secs, "AcDbBlock2PtParameter"));
                        (c.point(1010), c.point(1011))
                    }
                };
                let kind = match &p.kind {
                    ParameterKind::Other { dxf_type } => dxf_type.as_str(),
                    _ => "",
                };
                let v = if kind.contains("HORIZONTAL") {
                    (e.x - b.x).abs() * sx
                } else if kind.contains("VERTICAL") {
                    (e.y - b.y).abs() * sy
                } else {
                    (e - b).len() * sx
                };
                v.is_finite().then_some(ParamValue::Number(v))
            }
            "uservariable" | "value" => {
                let o = value_record(d, self.objects(), insert, node)?;
                if let Some(t) = o.tags.iter().find(|t| t.code == 1) {
                    return Some(ParamValue::Text(t.value.clone()));
                }
                o.tags
                    .iter()
                    .find(|t| t.code == 40)
                    .and_then(|t| t.value.trim().parse::<f64>().ok())
                    .filter(|v| v.is_finite())
                    .map(ParamValue::Number)
            }
            "visibilitystate" => Current { m: &m, values: &r.values }.property(node, "VisibilityState").map(ParamValue::Text),
            "lookupstring" => Some(ParamValue::Text(Current { m: &m, values: &r.values }.lookup(node).0)),
            _ => None,
        }
    }
}

/// A number as the drawing shows lengths (LUPREC decimals).
fn number(d: &Drawing, v: f64) -> String {
    let prec = d.header.length_precision();
    format!("{v:.prec$}")
}

/// Whether table cell `cell` reads as `value` (numbers compare as numbers).
fn same(cell: &str, value: &str) -> bool {
    match (cell.trim().parse::<f64>(), value.trim().parse::<f64>()) {
        (Ok(a), Ok(b)) => (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0),
        _ => cell.trim().eq_ignore_ascii_case(value.trim()),
    }
}

/// The current values of a reference's parameters, for evaluating its lookup tables.
struct Current<'a> {
    m: &'a DynamicBlock,
    values: &'a BTreeMap<i32, Value>,
}

impl Current<'_> {
    /// Parameter `node`'s value of property `property`, as table cells write it.
    fn property(&self, node: i32, property: &str) -> Option<String> {
        let p = self.m.parameters.iter().find(|p| p.node == node)?;
        let v = self.values.get(&node);
        match (&p.kind, property.to_ascii_lowercase().as_str()) {
            (ParameterKind::Visibility { states, .. }, "visibilitystate") => match v {
                Some(Value::Text { value }) => Some(value.clone()),
                _ => states.first().map(|s| s.name.clone()),
            },
            (ParameterKind::Linear { base, end, .. }, "updateddistance" | "distance") => {
                let (b, e) = match v {
                    Some(Value::Points { base, end }) => (*base, *end),
                    _ => (*base, *end),
                };
                Some(format!("{}", (e - b).len()))
            }
            (ParameterKind::Lookup { .. }, "lookupstring") => match v {
                Some(Value::Text { value }) => Some(value.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// Lookup parameter `node`'s value: the value every lookup table that sets it gives for the
    /// current inputs, or the tables' "no match" name ("Custom") when they don't agree or a table
    /// has no matching row. Also the values it may be set to.
    fn lookup(&self, node: i32) -> (String, Vec<String>) {
        let mut found: Option<Option<String>> = None;
        let mut unmatched = String::from("Custom");
        let mut choices: Vec<String> = Vec::new();
        for a in &self.m.actions {
            let ActionKind::Lookup { columns, rows } = &a.kind else { continue };
            let Some(out) = columns.iter().position(|c| c.lookup && c.node == node) else { continue };
            if rows.is_empty() {
                continue;
            }
            if let Some(c) = columns.get(out) {
                unmatched = c.unmatched.clone();
            }
            for r in rows {
                if let Some(v) = r.get(out).filter(|v| !choices.contains(v)) {
                    choices.push(v.clone());
                }
            }
            let hit = rows.iter().find(|r| {
                columns.iter().enumerate().filter(|(_, c)| !c.lookup).all(|(i, c)| match (r.get(i), self.property(c.node, &c.property)) {
                    (Some(cell), Some(v)) => same(cell, &v),
                    _ => false,
                })
            });
            let v = hit.and_then(|r| r.get(out).cloned());
            found = Some(match found {
                None => v,
                Some(prev) if prev == v => prev,
                Some(_) => None,
            });
        }
        (found.flatten().unwrap_or(unmatched), choices)
    }
}

/// The custom properties block reference `insert` shows (the Properties palette's Custom
/// section): its dynamic block's name and, in parameter order, each parameter shown in Properties
/// with its value. Lengths and positions are in drawing units (block units × the reference's
/// scale); positions are relative to the block's base point.
pub fn custom_properties(d: &Drawing, insert: Handle) -> Option<(String, Vec<CustomProperty>)> {
    let mut reader = Reader::new(d);
    let r = reader.reference(insert)?;
    let EntityKind::Insert(ins) = &d.entity(insert)?.kind else { return None };
    let m = reader.block(&r.definition)?;
    let base = d.block(&r.definition).map(|b| b.base).unwrap_or_default();
    let (sx, sy) = (ins.scale.x.abs(), ins.scale.y.abs());
    let cur = Current { m: &m, values: &r.values };
    let mut out = Vec::new();
    for p in m.parameters.iter().filter(|p| p.show_properties) {
        let v = r.values.get(&p.node);
        match &p.kind {
            ParameterKind::Linear { base: b, end: e, label, values, .. } => {
                let (b, e) = match v {
                    Some(Value::Points { base, end }) => (*base, *end),
                    _ => (*b, *e),
                };
                let choices = if values.is_list() { values.list.iter().map(|x| number(d, x * sx)).collect() } else { Vec::new() };
                out.push(CustomProperty { name: label.clone(), value: number(d, (e - b).len() * sx), choices, editable: true });
            }
            ParameterKind::Point { at, label, .. } => {
                let at = match v {
                    Some(Value::Point { at }) => *at,
                    _ => *at,
                };
                out.push(CustomProperty { name: format!("{label} X"), value: number(d, (at.x - base.x) * sx), choices: Vec::new(), editable: true });
                out.push(CustomProperty { name: format!("{label} Y"), value: number(d, (at.y - base.y) * sy), choices: Vec::new(), editable: true });
            }
            ParameterKind::Visibility { label, states, .. } => {
                let value = cur.property(p.node, "VisibilityState").unwrap_or_default();
                out.push(CustomProperty { name: label.clone(), value, choices: states.iter().map(|s| s.name.clone()).collect(), editable: true });
            }
            ParameterKind::Lookup { label, .. } => {
                let (value, choices) = cur.lookup(p.node);
                let editable = lookup_sets_what_changes(&m, p.node);
                out.push(CustomProperty { name: label.clone(), value, choices, editable });
            }
            _ => {}
        }
    }
    Some((r.definition.clone(), out))
}

/// Whether every lookup table that sets lookup parameter `node` sets only what
/// [`super::set_property`] can apply with it: visibility states and distances.
fn lookup_sets_what_changes(m: &DynamicBlock, node: i32) -> bool {
    let mut any = false;
    for a in &m.actions {
        let ActionKind::Lookup { columns, rows } = &a.kind else { continue };
        if rows.is_empty() || !columns.iter().any(|c| c.lookup && c.node == node) {
            continue;
        }
        any = true;
        if !columns
            .iter()
            .all(|c| matches!(c.property.to_ascii_lowercase().as_str(), "lookupstring" | "visibilitystate" | "updateddistance" | "distance"))
        {
            return false;
        }
    }
    any
}
