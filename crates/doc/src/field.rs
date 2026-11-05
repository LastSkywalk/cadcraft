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

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::dynblock::Reader;
use crate::{Attrib, Drawing, EntityKind, Handle, RawTag};

/// Deepest field tree evaluated.
pub const MAX_FIELD_DEPTH: usize = 16;
/// Most children one field keeps.
pub const MAX_FIELD_CHILDREN: usize = 256;
/// Longest expression an `AcExpr` field is evaluated with.
const MAX_EXPR: usize = 4096;
/// Deepest nesting of an expression's parentheses and operators.
const MAX_EXPR_DEPTH: usize = 64;
/// Longest text a `_text` field makes (bytes): a code repeating its children's placeholders
/// can't multiply its text level by level.
pub const MAX_FIELD_TEXT: usize = 65_536;

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
    /// The file groups between the object references and the value (evaluation flags and the
    /// evaluator's own data), as read. CADCraft doesn't interpret them; they are written back as
    /// they are, so a saved field is the field that was read and evaluates the same in other
    /// programs. A handle in them of the object the field is on is [`HOST`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<RawTag>,
    #[serde(default)]
    pub value: FieldValue,
}

/// The handle [`Field::data`] has for the object the field is on: that object's handle changes
/// when it is copied or saved, so the data names it by this and a save writes the handle it has
/// then.
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
    /// An object named by a handle that doesn't read as one: it is never found, so a field
    /// reading it keeps the value it was read with.
    Missing,
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
    /// A real an expression makes: with no format it shows six decimals, as in AutoCAD.
    Real(f64),
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
        self.update_at(&mut Reader::new(d), host, 0)
    }

    fn update_at(&mut self, r: &mut Reader, host: Handle, depth: usize) -> bool {
        if depth >= MAX_FIELD_DEPTH {
            return false;
        }
        let mut changed = false;
        for c in self.children.iter_mut().take(MAX_FIELD_CHILDREN) {
            changed |= c.update_at(r, host, depth + 1);
        }
        let Some(v) = self.evaluate(r, host) else { return changed };
        let value = self.make_value(r.drawing(), v);
        if value != self.value {
            self.value = value;
            changed = true;
        }
        changed
    }

    /// The value this field's evaluator gives, from its children's values.
    fn evaluate(&self, r: &mut Reader, host: Handle) -> Option<Val> {
        let ev = self.evaluator.to_ascii_lowercase();
        if ev == "_text" {
            return Some(Val::Text(self.join()));
        }
        if ev == "acexpr" {
            let body = self.body();
            let expr = body.strip_prefix("\\AcExpr").unwrap_or(body);
            let expr = self.substitute(expr)?;
            return expression(&expr).map(|n| if n.whole { Val::Num(n.value) } else { Val::Real(n.value) });
        }
        if ev.starts_with("acobjprop") {
            return self.object_property(r, host);
        }
        None
    }

    /// A `_text` field's text: its code with each child's text in place of `%<\_FldIdx N>%`, cut
    /// at [`MAX_FIELD_TEXT`].
    fn join(&self) -> String {
        let mut out = String::new();
        let mut rest = self.code.as_str();
        while let Some(i) = rest.find("%<\\_FldIdx ") {
            if !push_capped(&mut out, rest.get(..i).unwrap_or("")) {
                return out;
            }
            let after = rest.get(i + 11..).unwrap_or("");
            let Some(end) = after.find(">%") else {
                rest = rest.get(i..).unwrap_or("");
                break;
            };
            let child = after.get(..end).and_then(|n| n.trim().parse::<usize>().ok()).and_then(|n| self.children.get(n));
            if !push_capped(&mut out, child.map_or("", |c| c.value.text())) {
                return out;
            }
            rest = after.get(end + 2..).unwrap_or("");
        }
        push_capped(&mut out, rest);
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
            if out.len() > MAX_EXPR {
                return None;
            }
            rest = after.get(end + 2..)?;
        }
        out.push_str(rest);
        Some(out)
    }

    /// An `AcObjProp` field's value: a property of the object it reads. CADCraft reads the
    /// parameters of a dynamic block reference (`Parameter(N).UpdatedDistance`, `.UserVariable`,
    /// `.VisibilityState`, `.LookupString`).
    fn object_property(&self, r: &mut Reader, host: Handle) -> Option<Val> {
        let body = self.body();
        let at = body.find("Object(")?;
        let rest = body.get(at + 7..)?;
        let close = rest.find(')')?;
        let inner = rest.get(..close)?;
        // `Object(%<\_ObjIdx 0>%,1)`: the object, then how it is read (1: in the block reference,
        // the only mode evaluated; a field with another keeps the value it was read with).
        let (inner, mode) = inner.split_once(',').unwrap_or((inner, "1"));
        if mode.trim() != "1" {
            return None;
        }
        let idx = inner.trim().strip_prefix("%<\\_ObjIdx ")?.strip_suffix(">%")?.trim().parse::<usize>().ok()?;
        let object = match self.objects.get(idx)? {
            FieldObject::Host => host,
            FieldObject::Object { handle } => *handle,
            FieldObject::Missing => return None,
        };
        let path = rest.get(close + 1..)?.trim().trim_start_matches('.');
        let Some(param) = path.strip_prefix("Parameter(") else {
            let property = path.split_whitespace().next()?;
            return curve_length(r, host, object, property).map(Val::Num);
        };
        let close = param.find(')')?;
        let node = param.get(..close)?.trim().parse::<i32>().ok()?;
        let property = param.get(close + 1..)?.trim().trim_start_matches('.').split_whitespace().next()?;
        match r.parameter_value(object, node, property)? {
            crate::dynblock::ParamValue::Number(v) => Some(Val::Num(v)),
            crate::dynblock::ParamValue::Text(t) => Some(Val::Text(t)),
        }
    }

    /// The value to keep for `v`: numbers formatted by the field's format, the text cut at
    /// [`MAX_FIELD_TEXT`] (a case change can lengthen it, `İ` lower-casing to two characters, and
    /// a number format can add any prefix and suffix).
    fn make_value(&self, d: &Drawing, v: Val) -> FieldValue {
        let capped = |s: String| {
            let mut out = String::new();
            push_capped(&mut out, &s);
            out
        };
        match v {
            Val::Text(text) => FieldValue::Text { text: capped(format_text(&text, self.format().unwrap_or(""))) },
            Val::Num(n) => {
                let n = if n == 0.0 { 0.0 } else { n };
                match self.format() {
                    Some(f) => FieldValue::Double { value: n, text: capped(format_number(d, n, f)) },
                    // An expression with no format whose result is whole is a whole number.
                    None if n.fract() == 0.0 && n.abs() < 9.0e15 => FieldValue::Long { value: n as i64, text: format!("{}", n as i64) },
                    None => FieldValue::Double { value: n, text: format_number(d, n, "") },
                }
            }
            Val::Real(n) => {
                let n = if n == 0.0 { 0.0 } else { n };
                let text = match self.format() {
                    Some(f) => capped(format_number(d, n, f)),
                    None => format!("{n:.6}"),
                };
                FieldValue::Double { value: n, text }
            }
        }
    }
}

/// Append `s` to `out` up to [`MAX_FIELD_TEXT`] (cut at a character boundary); false when it
/// didn't all fit.
fn push_capped(out: &mut String, s: &str) -> bool {
    let room = MAX_FIELD_TEXT.saturating_sub(out.len());
    if s.len() <= room {
        out.push_str(s);
        return true;
    }
    let mut cut = room;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    out.push_str(s.get(..cut).unwrap_or(""));
    false
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
fn curve_length(r: &mut Reader, host: Handle, object: Handle, property: &str) -> Option<f64> {
    let p = property.to_ascii_lowercase();
    if p != "length" && p != "arclength" {
        return None;
    }
    let d = r.drawing();
    let (kind, scale) = match d.entity(object) {
        Some(e) => (&e.kind, 1.0),
        None => {
            let EntityKind::Insert(ins) = &d.entity(host)?.kind else { return None };
            let definition = r.reference(host).map_or_else(|| ins.block.clone(), |r| r.definition.clone());
            let at = d.block(&definition)?.entities.iter().position(|e| e.handle == object)?;
            let e = d.block(&ins.block)?.entities.iter().nth(at)?;
            // Uniform scale only: a length has no single scale otherwise.
            let (sx, sy) = (ins.scale.x.abs(), ins.scale.y.abs());
            if (sx - sy).abs() > 1e-9 * sx.max(sy).max(1.0) {
                return None;
            }
            (&e.kind, sx)
        }
    };
    let len = match kind {
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
    let mut prec = d.header.length_precision();
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

/// A number an `AcExpr` expression makes: AutoCAD keeps whole numbers (32-bit) apart from reals,
/// and shows a real with no format with six decimals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExprNumber {
    pub value: f64,
    /// A whole number: integer literals and what `+ - *`, `round` and `trunc` make of them.
    pub whole: bool,
}

impl ExprNumber {
    fn real(value: f64) -> Option<Self> {
        value.is_finite().then_some(Self { value, whole: false })
    }
    /// `value`, whole when both operands were: a whole result outside 32 bits is an error, as
    /// in AutoCAD.
    fn of(value: f64, whole: bool) -> Option<Self> {
        if whole { (value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX)).then_some(Self { value, whole }) } else { Self::real(value) }
    }
}

/// Evaluate an `AcExpr` expression as AutoCAD does: numbers, `+ - * / ^` (`^` from left to
/// right, a sign before a power applying to its base: `-2^2` is 4), parentheses, `pi` and the
/// functions `round`, `trunc`, `abs`, `sqrt`, `sin`, `cos`, `tan` (degrees). `/`, `^`, a decimal
/// literal and every function but `round` and `trunc` make a real.
pub fn expression(s: &str) -> Option<ExprNumber> {
    if s.len() > MAX_EXPR {
        return None;
    }
    let mut p = Parser { s: s.as_bytes(), i: 0 };
    let v = p.sum(0)?;
    p.space();
    (p.i == p.s.len()).then_some(v)
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
    fn sum(&mut self, depth: usize) -> Option<ExprNumber> {
        if depth > MAX_EXPR_DEPTH {
            return None;
        }
        let mut v = self.product(depth + 1)?;
        loop {
            if self.eat(b'+') {
                let r = self.product(depth + 1)?;
                v = ExprNumber::of(v.value + r.value, v.whole && r.whole)?;
            } else if self.eat(b'-') {
                let r = self.product(depth + 1)?;
                v = ExprNumber::of(v.value - r.value, v.whole && r.whole)?;
            } else {
                return Some(v);
            }
        }
    }
    fn product(&mut self, depth: usize) -> Option<ExprNumber> {
        let mut v = self.power(depth + 1)?;
        loop {
            if self.eat(b'*') {
                let r = self.power(depth + 1)?;
                v = ExprNumber::of(v.value * r.value, v.whole && r.whole)?;
            } else if self.eat(b'/') {
                let r = self.power(depth + 1)?;
                if r.value == 0.0 {
                    return None;
                }
                v = ExprNumber::real(v.value / r.value)?;
            } else {
                return Some(v);
            }
        }
    }
    fn power(&mut self, depth: usize) -> Option<ExprNumber> {
        let mut v = self.unary(depth + 1)?;
        while self.eat(b'^') {
            let e = self.unary(depth + 1)?;
            v = ExprNumber::real(v.value.powf(e.value))?;
        }
        Some(v)
    }
    fn unary(&mut self, depth: usize) -> Option<ExprNumber> {
        if depth > MAX_EXPR_DEPTH {
            return None;
        }
        if self.eat(b'-') {
            let v = self.unary(depth + 1)?;
            return Some(ExprNumber { value: -v.value, ..v });
        }
        if self.eat(b'+') {
            return self.unary(depth + 1);
        }
        self.atom(depth + 1)
    }
    fn atom(&mut self, depth: usize) -> Option<ExprNumber> {
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
            let text = std::str::from_utf8(self.s.get(start..self.i)?).ok()?;
            let value = text.parse::<f64>().ok()?;
            return ExprNumber::of(value, text.bytes().all(|c| c.is_ascii_digit()));
        }
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
            self.i += 1;
        }
        let name = std::str::from_utf8(self.s.get(start..self.i)?).ok()?.to_ascii_lowercase();
        if name.is_empty() {
            return None;
        }
        if name == "pi" {
            return ExprNumber::real(std::f64::consts::PI);
        }
        if !self.eat(b'(') {
            return None;
        }
        let a = self.sum(depth + 1)?.value;
        if !self.eat(b')') {
            return None;
        }
        match name.as_str() {
            "round" => ExprNumber::of(a.round(), true),
            "trunc" => ExprNumber::of(a.trunc(), true),
            "abs" => ExprNumber::real(a.abs()),
            "sqrt" => ExprNumber::real(Some(a).filter(|v| *v >= 0.0)?.sqrt()),
            "sin" => ExprNumber::real(a.to_radians().sin()),
            "cos" => ExprNumber::real(a.to_radians().cos()),
            "tan" => ExprNumber::real(a.to_radians().tan()),
            // AutoCAD has no others (`floor`, `ceil`, `min`, `max` show as an error there).
            _ => None,
        }
    }
}

/// Evaluate the fields of `attribs`, the attributes of block reference `host`, and put each
/// one's value in its text; the number of attributes whose text changed.
pub fn update_attribs(d: &Drawing, host: Handle, attribs: &mut [Attrib]) -> usize {
    evaluate_attribs(&mut Reader::new(d), host, attribs)
}

fn evaluate_attribs(r: &mut Reader, host: Handle, attribs: &mut [Attrib]) -> usize {
    let mut n = 0;
    for a in attribs.iter_mut() {
        let Some(f) = a.field.as_mut() else { continue };
        f.update_at(r, host, 0);
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
    update_inserts(d, &[insert]) > 0
}

/// Evaluate the fields of the attributes of block references `inserts` again and put each one's
/// value in its text; the number of references whose attribute text changed. Every field is
/// evaluated on the drawing as it was before any of them changed, reading each reference's
/// dynamic block data once: a field reads parameters and geometry, never attribute text, so the
/// order the references come in makes no difference.
pub fn update_inserts(d: &mut Drawing, inserts: &[Handle]) -> usize {
    let changes: Vec<(Handle, Vec<Attrib>, usize)> = {
        let mut r = Reader::new(d);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for &h in inserts {
            let Some(e) = r.drawing().entity(h) else { continue };
            let EntityKind::Insert(ins) = &e.kind else { continue };
            if !seen.insert(h) || !ins.attribs.iter().any(|a| a.field.is_some()) {
                continue;
            }
            let mut attribs = ins.attribs.clone();
            let n = evaluate_attribs(&mut r, h, &mut attribs);
            if attribs != ins.attribs {
                out.push((h, attribs, n));
            }
        }
        out
    };
    let mut changed = 0;
    for (h, attribs, n) in changes {
        let set = d.modify_entity(h, |e| {
            if let EntityKind::Insert(i) = &mut e.kind {
                i.attribs = attribs;
            }
        });
        if set.is_ok() && n > 0 {
            changed += 1;
        }
    }
    changed
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
    update_inserts(d, &inserts)
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
        let n = |s: &str| expression(s).map(|n| n.value);
        assert_eq!(n("((round ((5548.2)/5))*5)"), Some(5550.0));
        assert_eq!(n("round (378.4/5)*0.5"), Some(38.0));
        assert_eq!(n("round((10-2*3)/5)*5"), Some(5.0));
        assert_eq!(n("-(2)+abs(-4)"), Some(2.0));
        assert_eq!(n("1/0"), None);
        assert_eq!(n("round(1"), None);
        assert_eq!(n("nosuch(1)"), None);
        assert_eq!(n(&"(".repeat(10_000)), None);
        assert_eq!(n(""), None);
    }

    #[test]
    fn expressions_show_what_autocad_shows() {
        // Each `\AcExpr (…)` with no format as AutoCAD 2024 showed it after UPDATEFIELD: a sign
        // binds tighter than `^`, `^` goes from left to right, whole numbers stay whole under
        // `+ - *`, `round` and `trunc`, and anything real shows six decimals.
        let d = Drawing::new_metric();
        for (expr, shown) in [
            ("-2^2", "4.000000"),
            ("0-2^2", "-4.000000"),
            ("(-2)^2", "4.000000"),
            ("-(2^2)", "-4.000000"),
            ("10-2^2", "6.000000"),
            ("2^3^2", "64.000000"),
            ("2^2^0", "1.000000"),
            ("2^-1", "0.500000"),
            ("-2*3", "-6"),
            ("2*-3", "-6"),
            ("7-7", "0"),
            ("-7", "-7"),
            ("+3", "3"),
            ("(2)", "2"),
            ("5/2", "2.500000"),
            ("4/2", "2.000000"),
            ("10/4*2", "5.000000"),
            ("1.5+1", "2.500000"),
            ("2.0*3", "6.000000"),
            ("0.5+0.5", "1.000000"),
            ("2*2.5", "5.000000"),
            ("2^2*1", "4.000000"),
            ("1e3", "1000.000000"),
            ("round(2.5)", "3"),
            ("round(2.4)*5", "10"),
            ("trunc(2.7)", "2"),
            ("abs(-2)", "2.000000"),
            ("abs(-2.5)", "2.500000"),
            ("sqrt(4)", "2.000000"),
            ("pi", "3.141593"),
        ] {
            let mut f = field("AcExpr", &format!("\\AcExpr ({expr})"), Vec::new());
            f.update(&d, Handle(1));
            assert_eq!(f.value.text(), shown, "{expr}");
        }
        // AutoCAD shows an error for these: the field keeps the value it has.
        for expr in ["floor(2.7)", "ceil(2.2)", "min(1,2)", "max(1,2.5)", "100000*100000"] {
            let mut f = field("AcExpr", &format!("\\AcExpr ({expr})"), Vec::new());
            f.value = FieldValue::Text { text: "#".into() };
            assert!(!f.update(&d, Handle(1)), "{expr}");
            assert_eq!(f.value.text(), "#", "{expr}");
        }
    }

    #[test]
    fn formats() {
        let d = Drawing::new_metric();
        assert_eq!(format_number(&d, 20.000000000001, "%lu2%pr0"), "20");
        assert_eq!(format_number(&d, 142.84787767753, "%lu2%pr2"), "142.85");
        assert_eq!(format_number(&d, 1465.0, "%lu6"), "1465.0000");
        assert_eq!(format_number(&d, -0.0001, "%lu2%pr0"), "0");
        // A half rounds away from zero: 100.5 → 101, -2.25 → -2.3.
        assert_eq!(format_number(&d, 100.5, "%lu2%pr0"), "101");
        assert_eq!(format_number(&d, -2.25, "%lu2%pr1"), "-2.3");
        assert_eq!(format_number(&d, 2.5, "%lu2%pr3%zs8"), "2.5");
        assert_eq!(format_number(&d, 2.5, "%lu2%pr1%ds44"), "2,5");
        assert_eq!(format_number(&d, 2500.0, "%lu2%pr1%ct8[0.001]%ps[L=,m]"), "L=2.5m");
    }

    #[test]
    fn text_case() {
        // Upper, lower, first capital and title case, Latin and Cyrillic alike.
        assert_eq!(format_text("A3 x 3", "%tc4"), "A3 X 3");
        assert_eq!(format_text("north wall", "%tc1"), "NORTH WALL");
        assert_eq!(format_text("текст", "%tc1"), "ТЕКСТ");
        assert_eq!(format_text("SLAB", "%tc3"), "Slab");
        assert_eq!(format_text("Slab", "%tc2"), "slab");
        assert_eq!(format_text("Slab", ""), "Slab");
    }

    #[test]
    fn a_text_field_joins_its_children() {
        // A position mark: a prefix, a bar diameter, a rounded length / 10 and a rounded width / 10.
        let d = Drawing::new_metric();
        let len = field("AcExpr", "\\AcExpr (round (%<\\_FldIdx 0>%/5)*0.5) \\f \"%lu2%pr0\"", vec![number(5548.2)]);
        let wid = field("AcExpr", "\\AcExpr (round (%<\\_FldIdx 0>%/5)*0.5) \\f \"%lu2%pr0\"", vec![number(378.4)]);
        let mut dia = number(20.000000000001);
        dia.code = "\\AcObjProp.16.2 Object(%<\\_ObjIdx 0>%).Parameter(39).UpdatedDistance \\f \"%lu2%pr0\"".into();
        dia.value = FieldValue::Double { value: 20.000000000001, text: "20".into() };
        let mut f = field("_text", "P%<\\_FldIdx 0>%.%<\\_FldIdx 1>%.%<\\_FldIdx 2>%", vec![dia, len, wid]);
        assert!(f.update(&d, Handle(1)));
        assert_eq!(f.value.text(), "P20.555.38");
        // The parameter's reference doesn't exist: that child keeps the value it was read with.
        assert_eq!(f.children[0].value.text(), "20");
    }

    #[test]
    fn an_expression_with_no_format_is_a_whole_number() {
        let d = Drawing::new_metric();
        let mut f = field("AcExpr", "\\AcExpr ((round ((%<\\_FldIdx 0>%)/5))*5)", vec![number(5548.2)]);
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

    /// A drawing with `n` references to dynamic block W, whose distance parameter 2 runs from
    /// (0,0) to (10,0) along its line and whose user parameter 7 keeps a number: every other one
    /// refers to it with its own values (an anonymous block whose line is as long as its width,
    /// width 12 + i and user value 100 + i), the rest to W itself. References are scaled 1..=3.
    /// Each has the attributes W (the width, one decimal), U (the user value, two decimals) and
    /// L (the length of the block's line, as the reference shows it).
    fn many_references(n: usize) -> (Drawing, Vec<Handle>) {
        use crate::foreign::{Foreign, ForeignExtra, ForeignObject};
        use crate::{Block, Common, Entity, HAlign, Insert, Line, Space, Text, VAlign};
        use cadcraft_geom::Vec3;
        use std::sync::Arc;

        let tag = |code: i32, value: &str| RawTag { code, value: value.to_string() };
        let dict = |h: u64, entries: &[(&str, u64)]| ForeignObject {
            handle: Handle(h),
            kind: "DICTIONARY".into(),
            tags: entries.iter().flat_map(|(k, v)| [tag(3, k), tag(360, &Handle(*v).hex())]).collect(),
        };
        let record = |h: u64, tags: Vec<RawTag>| ForeignObject { handle: Handle(h), kind: "XRECORD".into(), tags };
        let line = |h: u64, len: f64| Entity::new(Handle(h), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(len, 0.0, 0.0) }));
        let mut d = Drawing::new_metric();
        let mut f = Foreign::default();
        // Block W, its record (target 0x1F) and its evaluation graph.
        const LINE: u64 = 0x10_0030;
        let mut w = Block::new("W");
        w.entities.push(line(LINE, 10.0));
        d.blocks.insert("W".into(), Arc::new(w));
        f.records.insert(("BLOCK_RECORD".into(), "W".into()), ForeignExtra { xdict: Some(Handle(0x10_0100)), ..Default::default() });
        f.targets.insert(Handle(0x1F), ("BLOCK_RECORD".into(), "W".into()));
        f.objects.push(dict(0x10_0100, &[("ACAD_ENHANCEDBLOCK", 0x10_0101)]));
        f.objects.push(ForeignObject {
            handle: Handle(0x10_0101),
            kind: "ACAD_EVALUATION_GRAPH".into(),
            tags: vec![tag(100, "AcDbEvalGraph"), tag(360, &Handle(0x10_0102).hex()), tag(360, &Handle(0x10_0103).hex())],
        });
        let point = |x: &str| [tag(1010, x), tag(1020, "0"), tag(1030, "0")];
        let mut linear =
            vec![tag(100, "AcDbEvalExpr"), tag(90, "2"), tag(100, "AcDbBlockParameter"), tag(280, "1"), tag(100, "AcDbBlock2PtParameter")];
        linear.extend(point("0"));
        linear.extend(point("10").map(|t| RawTag { code: t.code + 1, ..t }));
        linear.extend([tag(100, "AcDbBlockLinearParameter"), tag(305, "Width")]);
        f.objects.push(ForeignObject { handle: Handle(0x10_0102), kind: "BLOCKLINEARPARAMETER".into(), tags: linear });
        f.objects.push(ForeignObject {
            handle: Handle(0x10_0103),
            kind: "BLOCKUSERPARAMETER".into(),
            tags: vec![tag(100, "AcDbEvalExpr"), tag(90, "7")],
        });

        let field_of = |code: &str, objects: Vec<FieldObject>| Field { objects, ..field("AcObjProp.16.2", code, Vec::new()) };
        let attrib = |name: &str, field: Field| Attrib {
            field: Some(field),
            tag: name.into(),
            text: Text {
                insert: Vec3::ZERO,
                align_pt: None,
                height: 1.0,
                value: "?".into(),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: HAlign::Left,
                valign: VAlign::Baseline,
            },
            invisible: false,
            constant: false,
            prompt: String::new(),
            props: Default::default(),
            multiline: None,
            modes: Default::default(),
        };
        let width = field_of("\\AcObjProp.16.2 Object(%<\\_ObjIdx 0>%).Parameter(2).UpdatedDistance \\f \"%lu2%pr1\"", vec![FieldObject::Host]);
        let attribs = vec![
            attrib("W", field("_text", "W=%<\\_FldIdx 0>%", vec![width])),
            attrib("U", field_of("\\AcObjProp.16.2 Object(%<\\_ObjIdx 0>%).Parameter(7).UserVariable \\f \"%lu2%pr2\"", vec![FieldObject::Host])),
            attrib(
                "L",
                field_of("\\AcObjProp.16.2 Object(%<\\_ObjIdx 0>%,1).Length \\f \"%lu2%pr0\"", vec![FieldObject::Object { handle: Handle(LINE) }]),
            ),
        ];
        let mut handles = Vec::new();
        for i in 0..n {
            let k = u64::try_from(i).unwrap_or(0);
            let s = (i % 3 + 1) as f64;
            let own = i % 2 == 1;
            let block = if own { format!("*U{i}") } else { "W".into() };
            let ins = Insert {
                block: block.clone(),
                insert: Vec3::new(i as f64 * 50.0, 0.0, 0.0),
                scale: Vec3::new(s, s, 1.0),
                rotation: 0.0,
                attribs: attribs.clone(),
                cols: 1,
                rows: 1,
                clip: None,
                col_spacing: 0.0,
                row_spacing: 0.0,
            };
            let h = d.add(&Space::Model, Common::default(), EntityKind::Insert(ins)).unwrap();
            handles.push(h);
            if !own {
                continue;
            }
            let width = 12.0 + i as f64;
            let mut u = Block::new(&block);
            u.entities.push(line(0x20_0000 + k, width));
            d.blocks.insert(block, Arc::new(u));
            // xdict → representation → (data → W's record, cache → values).
            let base = 0x30_0000 + k * 16;
            f.entities.insert(h, ForeignExtra { xdict: Some(Handle(base)), ..Default::default() });
            f.objects.push(dict(base, &[("AcDbBlockRepresentation", base + 1)]));
            f.objects.push(dict(base + 1, &[("AcDbRepData", base + 2), ("AppDataCache", base + 3)]));
            f.objects.push(record(base + 2, vec![tag(340, "1F")]));
            f.objects.push(dict(base + 3, &[("ACAD_ENHANCEDBLOCKDATA", base + 4)]));
            f.objects.push(dict(base + 4, &[("2", base + 5), ("7", base + 6)]));
            let w = width.to_string();
            f.objects.push(record(base + 5, [point("0"), point(&w)].concat().into_iter().map(|t| RawTag { code: t.code - 1000, ..t }).collect()));
            f.objects.push(record(base + 6, vec![tag(40, &(100.0 + i as f64).to_string())]));
        }
        d.foreign = Arc::new(f);
        (d, handles)
    }

    fn texts(d: &Drawing, h: Handle) -> Vec<String> {
        match &d.entity(h).unwrap().kind {
            EntityKind::Insert(i) => i.attribs.iter().map(|a| a.text.value.clone()).collect(),
            k => panic!("{k:?}"),
        }
    }

    #[test]
    fn update_all_gives_what_updating_each_reference_gives() {
        let (d, handles) = many_references(300);
        let mut each = d.clone();
        let changed = handles.iter().filter(|h| update_insert(&mut each, **h)).count();
        let mut all = d.clone();
        assert_eq!(update_all(&mut all), changed);
        assert_eq!(changed, 300);
        for h in &handles {
            assert_eq!(texts(&all, *h), texts(&each, *h));
        }
        // W itself at scale 1, its own values at scale 2 (width 13), W itself at scale 3.
        assert_eq!(texts(&all, handles[0]), ["W=10.0", "?", "10"]);
        assert_eq!(texts(&all, handles[1]), ["W=26.0", "101.00", "26"]);
        assert_eq!(texts(&all, handles[2]), ["W=30.0", "?", "30"]);
        // Nothing changed since: evaluating again changes nothing.
        assert_eq!(update_all(&mut all), 0);
        assert_eq!(all.model.iter().map(|e| e.kind.clone()).collect::<Vec<_>>(), each.model.iter().map(|e| e.kind.clone()).collect::<Vec<_>>());
        // The single-reference readers agree with the batch.
        assert_eq!(crate::dynblock::parameter_value(&d, handles[1], 2, "UpdatedDistance"), Some(crate::dynblock::ParamValue::Number(26.0)));
        assert_eq!(crate::dynblock::reference(&d, handles[0]).map(|r| r.definition), Some("W".into()));
    }

    #[test]
    fn a_read_mode_other_than_1_or_a_missing_object_keeps_the_value() {
        let (mut d, handles) = many_references(1);
        let h = handles[0];
        // As read, the curve's length evaluates (mode 1: in the block reference).
        let mut ok = d.clone();
        update_insert(&mut ok, h);
        assert_eq!(texts(&ok, h)[2], "10");
        // Another mode, or an object whose handle didn't read: the value it was read with.
        d.modify_entity(h, |e| {
            if let EntityKind::Insert(i) = &mut e.kind {
                let f = i.attribs[2].field.as_mut().unwrap();
                f.code = f.code.replace(">%,1)", ">%,2)");
                let mut missing = f.clone();
                missing.code = missing.code.replace(">%,2)", ">%,1)");
                missing.objects = vec![FieldObject::Missing];
                i.attribs[0].field = Some(missing);
            }
        })
        .unwrap();
        update_insert(&mut d, h);
        assert_eq!(texts(&d, h)[0], "?");
        assert_eq!(texts(&d, h)[2], "?");
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

    #[test]
    fn repeated_placeholders_dont_multiply_the_text() {
        // Each level repeats its one child 100 times: 100^5 bytes uncapped.
        let d = Drawing::new_metric();
        let mut f = field("_text", "x", Vec::new());
        let code = "%<\\_FldIdx 0>%".repeat(100);
        for _ in 0..5 {
            f = field("_text", &code, vec![f]);
        }
        f.update(&d, Handle(1));
        assert_eq!(f.value.text().len(), MAX_FIELD_TEXT);
        // Lower-casing `İ` (2 bytes) gives 3 bytes: the formatted text is capped too.
        let lower = field("AcObjProp", "\\AcObjProp Object(%<\\_ObjIdx 0>%).Name \\f \"%tc2\"", Vec::new());
        let v = lower.make_value(&d, Val::Text("İ".repeat(MAX_FIELD_TEXT / 2)));
        assert!(v.text().len() <= MAX_FIELD_TEXT && v.text().starts_with('i'), "{}", v.text().len());
        // So is a number with a long prefix and suffix.
        let fix = "x".repeat(MAX_FIELD_TEXT);
        let num = field("AcExpr", &format!("\\AcExpr (1) \\f \"%ps[{fix},{fix}]\""), Vec::new());
        let v = num.make_value(&d, Val::Num(1.0));
        assert!(v.text().len() <= MAX_FIELD_TEXT && v.text().starts_with('x'), "{}", v.text().len());
    }
}
