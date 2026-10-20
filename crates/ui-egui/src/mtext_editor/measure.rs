//! A dimension's measured value in the in-place editor, as AutoCAD shows it: one gray piece of
//! the text that follows the dimension, with what is written around it staying put. The caret
//! doesn't go inside it, and an edit that reaches into it removes it whole (the text is then
//! fixed). These functions keep its place, in characters, as the text changes.

/// The measured value's range (`start..end`, characters) after the text went from `old` to `new`
/// by one edit, with `new` corrected: an edit reaching into the value removes the rest of it too.
/// Returns the text, the value's range (`None` once removed) and where the caret goes when the
/// text was corrected.
pub(super) fn follow(old: &str, new: &str, (s, e): (usize, usize)) -> (String, Option<(usize, usize)>, Option<usize>) {
    let o: Vec<char> = old.chars().collect();
    let n: Vec<char> = new.chars().collect();
    if e > o.len() || s >= e {
        return (new.to_string(), None, None);
    }
    if o == n {
        return (new.to_string(), Some((s, e)), None);
    }
    let pre = o.iter().zip(&n).take_while(|(a, b)| a == b).count();
    let suf = o.iter().rev().zip(n.iter().rev()).take_while(|(a, b)| a == b).count();
    let kept = (pre + suf).min(o.len()).min(n.len());
    // One edit: `removed` characters at `p` became `inserted` ones. Within a run of equal
    // characters (typing 4 before 4400) `p` is ambiguous: any of lo..=hi; it is taken outside
    // the value when it can be.
    let (removed, inserted) = (o.len() - kept, n.len() - kept);
    let (lo, hi) = (kept.saturating_sub(suf), pre.min(kept));
    // Before the value: it moves.
    if s.checked_sub(removed).is_some_and(|p| p.min(hi) >= lo) {
        return (new.to_string(), Some((s + inserted - removed, e + inserted - removed)), None);
    }
    // After it: it stays.
    if lo.max(e) <= hi {
        return (new.to_string(), Some((s, e)), None);
    }
    // Into it: what is left of it goes too.
    let p = s.clamp(lo, hi);
    let before = s..p.min(e).max(s);
    let after = if e > p + removed { (p + removed).max(s) + inserted - removed..e + inserted - removed } else { 0..0 };
    let text: String = n.iter().enumerate().filter(|(i, _)| !before.contains(i) && !after.contains(i)).map(|(_, c)| c).collect();
    (text, None, Some(p + inserted - before.len()))
}

/// Where a caret at `at` goes: never inside the value, to its nearer end.
pub(super) fn outside(at: usize, (s, e): (usize, usize)) -> usize {
    if at > s && at < e { if at - s <= e - at { s } else { e } } else { at }
}

/// `text` with the characters `range` replaced by `with`.
pub(super) fn splice(text: &str, (s, e): (usize, usize), with: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let s = s.min(c.len());
    let e = e.clamp(s, c.len());
    c[..s].iter().chain(with.chars().collect::<Vec<_>>().iter()).chain(c[e..].iter()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_written_before_or_after_moves_or_keeps_the_value() {
        // "=4400": the value is 1..5.
        assert_eq!(follow("=4400", "x=4400", (1, 5)), ("x=4400".into(), Some((2, 6)), None));
        assert_eq!(follow("=4400", "=4400 mm", (1, 5)), ("=4400 mm".into(), Some((1, 5)), None));
        assert_eq!(follow("200x22=4400", "200x2=4400", (7, 11)), ("200x2=4400".into(), Some((6, 10)), None));
    }

    #[test]
    fn equal_characters_next_to_the_value_are_taken_as_outside_it() {
        // Typing 4 right before 4400, or 0 right after it.
        assert_eq!(follow("=4400", "=44400", (1, 5)), ("=44400".into(), Some((2, 6)), None));
        assert_eq!(follow("=4400", "=44000", (1, 5)), ("=44000".into(), Some((1, 5)), None));
        // Deleting a 4 just before it.
        assert_eq!(follow("44400", "4400", (1, 5)), ("4400".into(), Some((0, 4)), None));
    }

    #[test]
    fn an_edit_into_the_value_removes_it_whole() {
        // Backspace at its end.
        assert_eq!(follow("a=4400", "a=440", (2, 6)), ("a=".into(), None, Some(2)));
        // A character typed over its first digit (selected): the character stays.
        assert_eq!(follow("a=4400", "a=X400", (2, 6)), ("a=X".into(), None, Some(3)));
        // Everything selected and replaced.
        assert_eq!(follow("4400", "new", (0, 4)), ("new".into(), None, Some(3)));
    }

    #[test]
    fn the_caret_stays_out_of_the_value() {
        assert_eq!(outside(3, (2, 6)), 2);
        assert_eq!(outside(5, (2, 6)), 6);
        assert_eq!(outside(2, (2, 6)), 2);
        assert_eq!(outside(7, (2, 6)), 7);
    }

    #[test]
    fn splice_replaces_characters() {
        assert_eq!(splice("200x22=4400", (7, 11), "<>"), "200x22=<>");
        assert_eq!(splice("Ø4400", (1, 5), "<>"), "Ø<>");
    }
}
