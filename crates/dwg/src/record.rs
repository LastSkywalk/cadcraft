//! The object header every non-entity record in the objects section starts with, for the
//! records CADCraft decodes itself: the type, the handle, extended data (skipped), the owner,
//! the reactors, the extension dictionary and the binary-data flag.

use acadrust::io::dwg::dwg_stream_readers::merged_reader::DwgMergedReader;
use acadrust::types::DxfVersion;

/// Most extended-data blocks skipped in an object header.
const MAX_EED: usize = 4096;
/// Most reactors an object header is read with.
const MAX_REACTORS: i32 = 256;

/// DXF groups of one decoded record.
pub(crate) type Groups = Vec<(i32, String)>;

/// A handle as DXF writes it.
pub(crate) fn hex(h: u64) -> String {
    format!("{h:X}")
}

/// An object header.
pub(crate) struct Header {
    pub(crate) handle: u64,
    pub(crate) owner: u64,
    pub(crate) reactors: Vec<u64>,
    pub(crate) xdict: Option<u64>,
}

impl Header {
    /// The groups DXF writes for the reactors and the extension dictionary, when there are any.
    pub(crate) fn groups(&self) -> Groups {
        let mut g: Groups = Vec::new();
        if !self.reactors.is_empty() {
            g.push((102, "{ACAD_REACTORS".into()));
            g.extend(self.reactors.iter().map(|h| (330, hex(*h))));
            g.push((102, "}".into()));
        }
        if let Some(x) = self.xdict {
            g.push((102, "{ACAD_XDICTIONARY".into()));
            g.push((360, hex(x)));
            g.push((102, "}".into()));
        }
        g
    }
}

/// Read the object header of a record (`data`, its handle stream `handle_bits` long): the
/// reader positioned at the object's own data, and the header. `None` when the handle stream
/// doesn't fit the record, an extended-data size or the reactor count is out of range, or the
/// object has binary data after it (2013+), which these records don't.
pub(crate) fn open(data: &[u8], handle_bits: i64, version: DxfVersion) -> Option<(DwgMergedReader, Header)> {
    let total = i64::try_from(data.len()).ok()?.checked_mul(8)?;
    let handles_at = total.checked_sub(handle_bits).filter(|b| *b > 0)?;
    let mut r = DwgMergedReader::new(data.to_vec(), version, handles_at);
    r.setup_text_and_handle(handles_at);
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
    let owner = r.read_handle();
    let reactors = r.read_bit_long();
    if !(0..=MAX_REACTORS).contains(&reactors) {
        return None;
    }
    let reactors: Vec<u64> = (0..reactors).map(|_| r.read_handle()).collect();
    let xdict = if r.read_bit() { None } else { Some(r.read_handle()) };
    if r.read_bit() {
        return None;
    }
    Some((r, Header { handle, owner, reactors, xdict }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_groups_are_the_reactors_then_the_dictionary() {
        let h = Header { handle: 1, owner: 2, reactors: vec![0x1A, 0xFF], xdict: Some(0x2B) };
        let want: Groups = vec![
            (102, "{ACAD_REACTORS".into()),
            (330, "1A".into()),
            (330, "FF".into()),
            (102, "}".into()),
            (102, "{ACAD_XDICTIONARY".into()),
            (360, "2B".into()),
            (102, "}".into()),
        ];
        assert_eq!(h.groups(), want);
        assert!(Header { handle: 1, owner: 2, reactors: Vec::new(), xdict: None }.groups().is_empty());
    }

    #[test]
    fn a_handle_stream_that_does_not_fit_opens_nothing() {
        for bits in [64i64, 65, i64::MAX, i64::MIN] {
            assert!(open(&[0u8; 8], bits, DxfVersion::AC1032).is_none(), "{bits}");
        }
        assert!(open(&[], 0, DxfVersion::AC1032).is_none());
    }
}
