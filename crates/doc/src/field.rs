//! Fields: text computed from other data, which AutoCAD keeps on an object and evaluates again
//! when what it reads changes. CADCraft keeps the fields of block attributes and evaluates the
//! kinds dynamic blocks use: a reference's own parameter values (`AcObjProp`), arithmetic on them
//! (`AcExpr`) and text joining them (`_text`). A field CADCraft can't evaluate keeps the value it
//! was read with.
//!
//! A field is a tree. Its code names its children as `%<\_FldIdx N>%` and the objects it reads as
//! `%<\_ObjIdx N>%`, for example
//! `\AcObjProp.16.2 Object(%<\_ObjIdx 0>%).Parameter(39).UpdatedDistance \f "%lu2%pr0"`: the
//! distance of parameter 39 of the block reference the attribute is on, in decimal units with no
//! decimals (DXF Reference, FIELD).

use serde::{Deserialize, Serialize};

use crate::{Attrib, Drawing, EntityKind, Handle, RawTag};

/// Deepest field tree evaluated.
pub const MAX_FIELD_DEPTH: usize = 16;
/// Most children one field keeps.
pub const MAX_FIELD_CHILDREN: usize = 256;
/// Longest expression an `AcExpr` field is evaluated with.
const MAX_EXPR: usize = 4096;
/// Deepest nesting of an expression's parentheses and operators.
const MAX_EXPR_DEPTH: usize = 64;

/// A field: how its value is made and the value it has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    /// The evaluator: `_text`, `AcExpr`, `AcObjProp.16.2`, `AcVar`…
    pub evaluator: String,
    /// The field code.
    pub code: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Field>,
    /// The objects the code reads, by `_ObjIdx`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<FieldObject>,
    /// The groups between the object references and the value, as read (evaluation flags and
    /// the evaluator's own data). A reference to the object the field is on is [`HOST`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<RawTag>,
    #[serde(default)]
    pub value: FieldValue,
}

/// The handle a field's data uses for the object the field is on, whatever handle that has.
pub const HOST: &str = "*HOST";

/// An object a field reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FieldObject {
    /// The object the field is on (for an attribute's field, its block reference): a copy of it
    /// reads the copy.
    Host,
    Object {
        handle: Handle,
    },
}

/// A field's value: what it shows, and the number it is when it is one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FieldValue {
    /// Not evaluated (AutoCAD keeps none for a text field: the text is on the object).
    #[default]
    None,
    Long {
        value: i64,
        text: String,
    },
    Double {
        value: f64,
        text: String,
    },
    Text {
        text: String,
    },
}

impl FieldValue {
    /// What the field shows.
    pub fn text(&self) -> &str {
        match self {
            Self::None => "",
            Self::Long { text, .. } | Self::Double { text, .. } | Self::Text { text } => text,
        }
    }
    /// The number it is, when it is one.
    pub fn number(&self) -> Option<f64> {
        match self {
            Self::Long { value, .. } => Some(*value as f64),
            Self::Double { value, .. } => Some(*value),
            Self::Text { text } => text.trim().parse::<f64>().ok().filter(|v| v.is_finite()),
            Self::None => None,
        }
    }
}

/// A value an evaluator makes.
#[derive(Clone, Debug, PartialEq)]
enum Val {
    Num(f64),
    Text(String),
}

impl Field {
    /// The format of the code (`\f "%lu2%pr0"`), if it has one.
    pub fn format(&self) -> Option<&str> {
        let i = self.code.rfind("\\f \"")?;
        let rest = self.code.get(i + 4..)?;
        rest.find('"').and_then(|j| rest.get(..j))
    }

    /// The code without its format.
    fn body(&self) -> &str {
        match self.code.rfind("\\f \"") {
            Some(i) => self.code.get(..i).unwrap_or(&self.code).trim_end(),
            None => self.code.trim_end(),
        }
    }

    /// Evaluate the field again on the object `host` it is on; whether its value changed. Each
    /// child is evaluated first; a field CADCraft can't evaluate keeps its value.
    pub fn update(&mut self, d: &Drawing, host: Handle) -> bool {
        self.update_at(d, host, 0)
    }

    fn update_at(&mut self, d: &Drawing, host: Handle, depth: usize) -> bool {
        if depth >= MAX_FIELD_DEPTH {
            return false;
        }
        let mut changed = false;
        for c in self.children.iter_mut().take(MAX_FIELD_CHILDREN) {
            changed |= c.update_at(d, host, depth + 1);
        }
        let Some(v) = self.evaluate(d, host) else { return changed };
        let value = self.make_value(d, v);
        if value != self.value {
            self.value = value;
            changed = true;
        }
        changed
    }

    /// The value this field's evaluator gives, from its children's values.
    fn evaluate(&self, d: &Drawing, host: Handle) -> Option<Val> {
        let ev = self.evaluator.to_ascii_lowercase();
        if ev == "_text" {
            return Some(Val::Text(self.join()));
        }
        if ev == "acexpr" {
            let body = self.body();
            let expr = body.strip_prefix("\\AcExpr").unwrap_or(body);
            let expr = self.substitute(expr)?;
            return expression(&expr).map(Val::Num);
        }
        if ev.starts_with("acobjprop") {
            return self.object_property(d, host);
        }
        None
    }

    /// A `_text` field's text: its code with each child's text in place of `%<\_FldIdx N>%`.
    fn join(&self) -> String {
        let mut out = String::new();
        let mut rest = self.code.as_str();
        while let Some(i) = rest.find("%<\\_FldIdx ") {
            out.push_str(rest.get(..i).unwrap_or(""));
            let after = rest.get(i + 11..).unwrap_or("");
            let Some(end) = after.find(">%") else {
                rest = rest.get(i..).unwrap_or("");
                break;
            };
            let child = after.get(..end).and_then(|n| n.trim().parse::<usize>().ok()).and_then(|n| self.children.get(n));
            out.push_str(child.map_or("", |c| c.value.text()));
            rest = after.get(end + 2..).unwrap_or("");
        }
        out.push_str(rest);
        out
    }

    /// An expression with each child's number in place of `%<\_FldIdx N>%` (`None` when a child
    /// isn't a number).
    fn substitute(&self, expr: &str) -> Option<String> {
        let mut out = String::new();
        let mut rest = expr;
        while let Some(i) = rest.find("%<\\_FldIdx ") {
            out.push_str(rest.get(..i)?);
            let after = rest.get(i + 11..)?;
            let end = after.find(">%")?;
            let n = after.get(..end)?.trim().parse::<usize>().ok()?;
            let v = self.children.get(n)?.value.number()?;
            out.push_str(&format!("({v:?})"));
            rest = after.get(end + 2..)?;
        }
        out.push_str(rest);
        Some(out)
    }

    /// An `AcObjProp` field's value: a property of the object it reads. CADCraft reads the
    /// parameters of a dynamic block reference (`Parameter(N).UpdatedDistance`, `.UserVariable`,
    /// `.VisibilityState`, `.LookupString`).
    fn object_property(&self, d: &Drawing, host: Handle) -> Option<Val> {
        let body = self.body();
        let at = body.find("Object(")?;
        let rest = body.get(at + 7..)?;
        let close = rest.find(')')?;
        let inner = rest.get(..close)?;
        // `Object(%<\_ObjIdx 0>%,1)`: the object, then how it is read (1: in the block reference).
        let inner = inner.split(',').next().unwrap_or(inner);
        let idx = inner.trim().strip_prefix("%<\\_ObjIdx ")?.strip_suffix(">%")?.trim().parse::<usize>().ok()?;
        let object = match self.objects.get(idx)? {
            FieldObject::Host => host,
            FieldObject::Object { handle } => *handle,
        };
        let path = rest.get(close + 1..)?.trim().trim_start_matches('.');
        let Some(param) = path.strip_prefix("Parameter(") else {
            let property = path.split_whitespace().next()?;
            return curve_length(d, host, object, property).map(Val::Num);
        };
        let close = param.find(')')?;
        let node = param.get(..close)?.trim().parse::<i32>().ok()?;
        let property = param.get(close + 1..)?.trim().trim_start_matches('.').split_whitespace().next()?;
        match crate::dynblock::parameter_value(d, object, node, property)? {
            crate::dynblock::ParamValue::Number(v) => Some(Val::Num(v)),
            crate::dynblock::ParamValue::Text(t) => Some(Val::Text(t)),
        }
    }

    /// The value to keep for `v`: numbers formatted by the field's format.
    fn make_value(&self, d: &Drawing, v: Val) -> FieldValue {
        match v {
            Val::Text(text) => FieldValue::Text { text: format_text(&text, self.format().unwrap_or("")) },
            Val::Num(n) => {
                let n = if n == 0.0 { 0.0 } else { n };
                match self.format() {
                    Some(f) => FieldValue::Double { value: n, text: format_number(d, n, f) },
                    // An expression with no format whose result is whole is a whole number.
                    None if n.fract() == 0.0 && n.abs() < 9.0e15 => FieldValue::Long { value: n as i64, text: format!("{}", n as i64) },
                    None => FieldValue::Double { value: n, text: format_number(d, n, "") },
                }
            }
        }
    }
}

/// Text formatted by a field format's case (`%tc1` upper, `%tc2` lower, `%tc3` sentence case,
/// `%tc4` title case).
pub fn format_text(text: &str, format: &str) -> String {
    let case = format.find("%tc").and_then(|i| format.get(i + 3..)).and_then(|r| r.chars().next()).and_then(|c| c.to_digit(10));
    match case {
        Some(1) => text.to_uppercase(),
        Some(2) => text.to_lowercase(),
        Some(3) => {
            let lower = text.to_lowercase();
            let mut c = lower.chars();
            c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
        }
        Some(4) => {
            let mut out = String::with_capacity(text.len());
            let mut start = true;
            for ch in text.chars() {
                if start {
                    out.extend(ch.to_uppercase());
                } else {
                    out.extend(ch.to_lowercase());
                }
                start = ch.is_whitespace();
            }
            out
        }
        _ => text.to_string(),
    }
}

/// Property `property` (`Length`, `ArcLength`) of entity `object`: a line's, arc's, circle's or
/// polyline's length. An object of a block definition is read in block reference `host`, the
/// way AutoCAD reads a dynamic block's own geometry for its attributes (`Object(…,1)`): the
/// entity at the same place in the block the reference shows, scaled with it.
fn curve_length(d: &Drawing, host: Handle, object: Handle, property: &str) -> Option<f64> {
    let p = property.to_ascii_lowercase();
    if p != "length" && p != "arclength" {
        return None;
    }
    let (kind, scale) = match d.entity(object) {
        Some(e) => (e.kind.clone(), 1.0),
        None => {
            let EntityKind::Insert(ins) = &d.entity(host)?.kind else { return None };
            let definition = crate::dynblock::reference(d, host).map_or_else(|| ins.block.clone(), |r| r.definition);
            let at = d.block(&definition)?.entities.iter().position(|e| e.handle == object)?;
            let e = d.block(&ins.block)?.entities.iter().nth(at)?;
            // Uniform scale only: a length has no single scale otherwise.
            let (sx, sy) = (ins.scale.x.abs(), ins.scale.y.abs());
            if (sx - sy).abs() > 1e-9 * sx.max(sy).max(1.0) {
                return None;
            }
            (e.kind.clone(), sx)
        }
    };
    let len = match &kind {
        EntityKind::Line(l) if p == "length" => (l.b - l.a).len(),
        EntityKind::Arc(a) => {
            let sweep = (a.end - a.start).rem_euclid(std::f64::consts::TAU);
            a.radius * if sweep == 0.0 { std::f64::consts::TAU } else { sweep }
        }
        EntityKind::Circle(c) if p == "length" => c.radius * std::f64::consts::TAU,
        EntityKind::LwPolyline(l) if p == "length" => cadcraft_geom::Polyline { vertices: l.vertices.clone(), closed: l.closed }.len(),
        EntityKind::Polyline3d(l) if p == "length" => {
            let mut pts: Vec<_> = l.points.clone();
            if l.closed
                && let Some(f) = pts.first().copied()
            {
                pts.push(f);
            }
            pts.windows(2)
                .map(|w| match w {
                    [a, b] => (*b - *a).len(),
                    _ => 0.0,
                })
                .sum()
        }
        _ => return None,
    };
    Some(len * scale).filter(|v| v.is_finite())
}

/// `v` formatted by a field format: `%lu` units (decimal is what CADCraft writes; the others
/// fall back to it), `%pr` decimals (else LUPREC), `%zs8` no trailing zeros, `%ds` decimal
/// separator, `%ct8[f]` a conversion factor and `%ps[prefix,suffix]`.
pub fn format_number(d: &Drawing, v: f64, format: &str) -> String {
    let mut prec = usize::try_from(d.header.i64("LUPREC", 4).clamp(0, 8)).unwrap_or(4);
    let mut trailing = true;
    let mut sep = '.';
    let mut factor = 1.0;
    let mut scientific = false;
    let (mut prefix, mut suffix) = (String::new(), String::new());
    let mut rest = format;
    while let Some(i) = rest.find('%') {
        let tail = rest.get(i + 1..).unwrap_or("");
        let key = tail.get(..2).unwrap_or("");
        let after = tail.get(2..).unwrap_or("");
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        let n = digits.parse::<i64>().ok();
        let mut next = after.get(digits.len()..).unwrap_or("");
        let bracket = next.strip_prefix('[').and_then(|b| b.find(']').map(|j| (b.get(..j).unwrap_or(""), b.get(j + 1..).unwrap_or(""))));
        match key {
            "pr" => prec = n.map_or(prec, |p| usize::try_from(p.clamp(0, 8)).unwrap_or(prec)),
            "lu" => scientific = n == Some(1),
            "zs" => trailing = n.is_none_or(|z| z & 8 == 0),
            "ds" => sep = n.and_then(|c| u32::try_from(c).ok()).and_then(char::from_u32).unwrap_or('.'),
            "ct" => {
                if let Some((f, after)) = bracket {
                    factor = f.trim().parse::<f64>().ok().filter(|f| f.is_finite()).unwrap_or(1.0);
                    next = after;
                }
            }
            "ps" => {
                if let Some((p, after)) = bracket {
                    let (a, b) = p.split_once(',').unwrap_or((p, ""));
                    (prefix, suffix) = (a.to_string(), b.to_string());
                    next = after;
                }
            }
            _ => {}
        }
        rest = next;
    }
    let v = v * factor;
    // Halves round away from zero, as AutoCAD rounds (Rust's formatting rounds them to even).
    let scale = 10f64.powi(i32::try_from(prec).unwrap_or(4));
    let v = if scientific || (v * scale).abs() >= 9.0e15 { v } else { (v * scale).round() / scale };
    let mut s = if scientific { format!("{v:.prec$E}") } else { format!("{v:.prec$}") };
    if s.starts_with('-') && s.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') {
        s.remove(0);
    }
    if !trailing && s.contains('.') && !scientific {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if sep != '.' {
        s = s.replace('.', &sep.to_string());
    }
    format!("{prefix}{s}{suffix}")
}

/// Evaluate an `AcExpr` expression: numbers, `+ - * / ^`, parentheses, `pi` and the functions
/// `round`, `trunc`, `floor`, `ceil`, `abs`, `sqrt`, `min`, `max`, `sin`, `cos`, `tan` (degrees).
pub fn expression(s: &str) -> Option<f64> {
    if s.len() > MAX_EXPR {
        return None;
    }
    let mut p = Parser { s: s.as_bytes(), i: 0 };
    let v = p.sum(0)?;
    p.space();
    (p.i == p.s.len() && v.is_finite()).then_some(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        self.space();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn sum(&mut self, depth: usize) -> Option<f64> {
        if depth > MAX_EXPR_DEPTH {
            return None;
        }
        let mut v = self.product(depth + 1)?;
        loop {
            if self.eat(b'+') {
                v += self.product(depth + 1)?;
            } else if self.eat(b'-') {
                v -= self.product(depth + 1)?;
            } else {
                return Some(v);
            }
        }
    }
    fn product(&mut self, depth: usize) -> Option<f64> {
        let mut v = self.power(depth + 1)?;
        loop {
            if self.eat(b'*') {
                v *= self.power(depth + 1)?;
            } else if self.eat(b'/') {
                let r = self.power(depth + 1)?;
                if r == 0.0 {
                    return None;
                }
                v /= r;
            } else {
                return Some(v);
            }
        }
    }
    fn power(&mut self, depth: usize) -> Option<f64> {
        let b = self.unary(depth + 1)?;
        if self.eat(b'^') { Some(b.powf(self.power(depth + 1)?)).filter(|v| v.is_finite()) } else { Some(b) }
    }
    fn unary(&mut self, depth: usize) -> Option<f64> {
        if depth > MAX_EXPR_DEPTH {
            return None;
        }
        if self.eat(b'-') {
            return Some(-self.unary(depth + 1)?);
        }
        if self.eat(b'+') {
            return self.unary(depth + 1);
        }
        self.atom(depth + 1)
    }
    fn atom(&mut self, depth: usize) -> Option<f64> {
        if self.eat(b'(') {
            let v = self.sum(depth + 1)?;
            return self.eat(b')').then_some(v);
        }
        self.space();
        let start = self.i;
        if self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
            while self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
                self.i += 1;
            }
            if self.s.get(self.i).is_some_and(|c| *c == b'e' || *c == b'E') {
                let save = self.i;
                self.i += 1;
                if self.s.get(self.i).is_some_and(|c| *c == b'+' || *c == b'-') {
                    self.i += 1;
                }
                if self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
                    while self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
                        self.i += 1;
                    }
                } else {
                    self.i = save;
                }
            }
            return std::str::from_utf8(self.s.get(start..self.i)?).ok()?.parse::<f64>().ok();
        }
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
            self.i += 1;
        }
        let name = std::str::from_utf8(self.s.get(start..self.i)?).ok()?.to_ascii_lowercase();
        if name.is_empty() {
            return None;
        }
        if name == "pi" {
            return Some(std::f64::consts::PI);
        }
        if !self.eat(b'(') {
            return None;
        }
        let mut args = vec![self.sum(depth + 1)?];
        while self.eat(b',') {
            if args.len() >= 16 {
                return None;
            }
            args.push(self.sum(depth + 1)?);
        }
        if !self.eat(b')') {
            return None;
        }
        let one = || if let [a] = args.as_slice() { Some(*a) } else { None };
        let v = match name.as_str() {
            "round" => one()?.round(),
            "trunc" => one()?.trunc(),
            "floor" => one()?.floor(),
            "ceil" => one()?.ceil(),
            "abs" => one()?.abs(),
            "sqrt" => Some(one()?).filter(|v| *v >= 0.0)?.sqrt(),
            "sin" => one()?.to_radians().sin(),
            "cos" => one()?.to_radians().cos(),
            "tan" => one()?.to_radians().tan(),
            "min" => args.iter().copied().fold(f64::INFINITY, f64::min),
            "max" => args.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            _ => return None,
        };
        Some(v).filter(|v| v.is_finite())
    }
}

/// Evaluate the fields of `attribs`, the attributes of block reference `host`, and put each
/// one's value in its text; the number of attributes whose text changed.
pub fn update_attribs(d: &Drawing, host: Handle, attribs: &mut [Attrib]) -> usize {
    let mut n = 0;
    for a in attribs.iter_mut() {
        let Some(f) = a.field.as_mut() else { continue };
        f.update(d, host);
        if f.value == FieldValue::None {
            continue;
        }
        if a.text.value != f.value.text() {
            a.text.value = f.value.text().to_string();
            n += 1;
        }
    }
    n
}

/// Evaluate the fields of block reference `insert`'s attributes again; whether an attribute's
/// text changed.
pub fn update_insert(d: &mut Drawing, insert: Handle) -> bool {
    let Some(e) = d.entity(insert) else { return false };
    let EntityKind::Insert(ins) = &e.kind else { return false };
    if !ins.attribs.iter().any(|a| a.field.is_some()) {
        return false;
    }
    let mut attribs = ins.attribs.clone();
    let before = attribs.clone();
    let n = update_attribs(d, insert, &mut attribs);
    if attribs == before {
        return false;
    }
    d.modify_entity(insert, |e| {
        if let EntityKind::Insert(i) = &mut e.kind {
            i.attribs = attribs;
        }
    })
    .is_ok()
        && n > 0
}

/// The block references in model space and the layouts with attribute fields.
fn with_fields(d: &Drawing) -> impl Iterator<Item = Handle> + '_ {
    std::iter::once(&d.model)
        .chain(d.layouts.iter().map(|l| &l.entities))
        .flat_map(|s| s.iter())
        .filter(|e| matches!(&e.kind, EntityKind::Insert(i) if i.attribs.iter().any(|a| a.field.is_some())))
        .map(|e| e.handle)
}

/// Whether any block reference has attribute fields.
pub fn needs_update(d: &Drawing) -> bool {
    with_fields(d).next().is_some()
}

/// Evaluate the fields of every block reference in model space and the layouts again; the
/// number of references whose attribute text changed.
pub fn update_all(d: &mut Drawing) -> usize {
    let inserts: Vec<Handle> = with_fields(d).collect();
    inserts.into_iter().filter(|h| update_insert(d, *h)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(evaluator: &str, code: &str, children: Vec<Field>) -> Field {
        Field { evaluator: evaluator.into(), code: code.into(), children, objects: Vec::new(), data: Vec::new(), value: FieldValue::None }
    }
    fn number(v: f64) -> Field {
        Field { value: FieldValue::Double { value: v, text: String::new() }, ..field("AcObjProp.16.2", "", Vec::new()) }
    }

    #[test]
    fn expressions() {
        assert_eq!(expression("((round ((5547.830448673572)/5))*5)"), Some(5550.0));
        assert_eq!(expression("round (377.9987/5)*0.5"), Some(38.0));
        assert_eq!(expression("round((10-2*3)/5)*5"), Some(5.0));
        assert_eq!(expression("2^3^2"), Some(512.0));
        assert_eq!(expression("-(2)+max(1, 4, 3)"), Some(2.0));
        assert_eq!(expression("1/0"), None);
        assert_eq!(expression("round(1"), None);
        assert_eq!(expression("nosuch(1)"), None);
        assert_eq!(expression(&"(".repeat(10_000)), None);
        assert_eq!(expression(""), None);
    }

    #[test]
    fn formats() {
        let d = Drawing::new_metric();
        assert_eq!(format_number(&d, 20.000000000001, "%lu2%pr0"), "20");
        assert_eq!(format_number(&d, 142.84787767753, "%lu2%pr2"), "142.85");
        assert_eq!(format_number(&d, 1465.0, "%lu6"), "1465.0000");
        assert_eq!(format_number(&d, -0.0001, "%lu2%pr0"), "0");
        // A half rounds away from zero (VetCAD's "Пн10.101…": 1007.15 / 10 = 100.5 → 101).
        assert_eq!(format_number(&d, 100.5, "%lu2%pr0"), "101");
        assert_eq!(format_number(&d, -2.25, "%lu2%pr1"), "-2.3");
        assert_eq!(format_number(&d, 2.5, "%lu2%pr3%zs8"), "2.5");
        assert_eq!(format_number(&d, 2.5, "%lu2%pr1%ds44"), "2,5");
        assert_eq!(format_number(&d, 2500.0, "%lu2%pr1%ct8[0.001]%ps[L=,м]"), "L=2.5м");
    }

    #[test]
    fn text_case() {
        // A title block's sheet format, "A3 x 3" in the lookup table, shows as AutoCAD shows it.
        assert_eq!(format_text("A3 x 3", "%tc4"), "A3 X 3");
        assert_eq!(format_text("ш. стены", "%tc1"), "Ш. СТЕНЫ");
        assert_eq!(format_text("ПЛИТА", "%tc3"), "Плита");
        assert_eq!(format_text("Плита", "%tc2"), "плита");
        assert_eq!(format_text("Плита", ""), "Плита");
    }

    #[test]
    fn a_text_field_joins_its_children() {
        // VetCAD's position: "Г" + bar diameter + rounded length / 10 + rounded width / 10.
        let d = Drawing::new_metric();
        let len = field("AcExpr", "\\AcExpr (round (%<\\_FldIdx 0>%/5)*0.5) \\f \"%lu2%pr0\"", vec![number(5547.830448673572)]);
        let wid = field("AcExpr", "\\AcExpr (round (%<\\_FldIdx 0>%/5)*0.5) \\f \"%lu2%pr0\"", vec![number(377.9987298678607)]);
        let mut dia = number(20.000000000001);
        dia.code = "\\AcObjProp.16.2 Object(%<\\_ObjIdx 0>%).Parameter(39).UpdatedDistance \\f \"%lu2%pr0\"".into();
        dia.value = FieldValue::Double { value: 20.000000000001, text: "20".into() };
        let mut f = field("_text", "Г%<\\_FldIdx 0>%.%<\\_FldIdx 1>%.%<\\_FldIdx 2>%", vec![dia, len, wid]);
        assert!(f.update(&d, Handle(1)));
        assert_eq!(f.value.text(), "Г20.555.38");
        // The parameter's reference doesn't exist: that child keeps the value it was read with.
        assert_eq!(f.children[0].value.text(), "20");
    }

    #[test]
    fn an_expression_with_no_format_is_a_whole_number() {
        let d = Drawing::new_metric();
        let mut f = field("AcExpr", "\\AcExpr ((round ((%<\\_FldIdx 0>%)/5))*5)", vec![number(5547.830448673572)]);
        f.update(&d, Handle(1));
        assert_eq!(f.value, FieldValue::Long { value: 5550, text: "5550".into() });
    }

    #[test]
    fn a_field_it_cant_evaluate_keeps_its_value() {
        let d = Drawing::new_metric();
        let mut f = field("AcVar", "\\AcVar Date \\f \"M/d/yyyy\"", Vec::new());
        f.value = FieldValue::Text { text: "10/19/2026".into() };
        assert!(!f.update(&d, Handle(1)));
        assert_eq!(f.value.text(), "10/19/2026");
        // An expression on a child that isn't a number.
        let mut e = field(
            "AcExpr",
            "\\AcExpr (%<\\_FldIdx 0>%*2)",
            vec![Field { value: FieldValue::Text { text: "abc".into() }, ..field("AcVar", "", Vec::new()) }],
        );
        e.value = FieldValue::Long { value: 7, text: "7".into() };
        assert!(!e.update(&d, Handle(1)));
        assert_eq!(e.value.text(), "7");
    }

    #[test]
    fn hostile_codes_dont_panic() {
        let d = Drawing::new_metric();
        for code in [
            "%<\\_FldIdx ",
            "%<\\_FldIdx 99>%",
            "%<\\_FldIdx -1>%x",
            "\\f \"",
            "\\AcObjProp Object(",
            "Object(%<\\_ObjIdx 9>%).Parameter(x).",
            "\\f \"%pr999%ct8[\"",
        ] {
            for ev in ["_text", "AcExpr", "AcObjProp.16.2"] {
                let mut f = field(ev, code, Vec::new());
                f.update(&d, Handle(1));
            }
        }
        let mut deep = number(1.0);
        for _ in 0..100 {
            deep = field("AcExpr", "\\AcExpr (%<\\_FldIdx 0>%+1)", vec![deep]);
        }
        deep.update(&d, Handle(1));
    }
}
