//! CAD colours: the 256-entry colour index (ACI), 24-bit true colour, and the logical
//! ByLayer / ByBlock values. The index palette is built from its structure (24 hues × 5
//! brightnesses × 2 saturations, plus primaries and greys) with channel levels measured on a
//! plot of every index, not copied from any file.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// An 8-bit sRGB colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn to_u32(self) -> u32 {
        (u32::from(self.0) << 16) | (u32::from(self.1) << 8) | u32::from(self.2)
    }
    pub fn from_u32(v: u32) -> Rgb {
        Rgb(((v >> 16) & 0xff) as u8, ((v >> 8) & 0xff) as u8, (v & 0xff) as u8)
    }
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
    pub fn parse_hex(s: &str) -> Option<Rgb> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(s, 16).ok().map(Rgb::from_u32)
    }
    /// Relative luminance (0..1) for contrast decisions.
    pub fn luma(self) -> f64 {
        (0.2126 * f64::from(self.0) + 0.7152 * f64::from(self.1) + 0.0722 * f64::from(self.2)) / 255.0
    }
}

/// An entity or layer colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum Color {
    #[default]
    ByLayer,
    ByBlock,
    /// ACI 1..=255.
    Index(u8),
    True(Rgb),
}

impl Color {
    pub const WHITE: Color = Color::Index(7);
    /// DXF group 62 value: 0 = ByBlock, 256 = ByLayer.
    pub fn to_aci(self) -> i16 {
        match self {
            Color::ByLayer => 256,
            Color::ByBlock => 0,
            Color::Index(i) => i16::from(i),
            Color::True(rgb) => i16::from(nearest_aci(rgb)),
        }
    }
    pub fn from_aci(v: i16) -> Color {
        match v {
            0 => Color::ByBlock,
            1..=255 => Color::Index(v as u8),
            _ => Color::ByLayer,
        }
    }
    /// Resolve to RGB given the layer and block colours.
    pub fn resolve(self, layer: Color, block: Color) -> Rgb {
        match self {
            Color::ByLayer => match layer {
                Color::ByLayer | Color::ByBlock => aci_rgb(7),
                c => c.resolve(Color::Index(7), Color::Index(7)),
            },
            Color::ByBlock => match block {
                Color::ByLayer | Color::ByBlock => aci_rgb(7),
                c => c.resolve(Color::Index(7), Color::Index(7)),
            },
            Color::Index(i) => aci_rgb(i),
            Color::True(c) => c,
        }
    }
    /// Short display name like AutoCAD's Properties palette ("ByLayer", "Red", "Color 34", "255,128,0").
    pub fn name(self) -> String {
        match self {
            Color::ByLayer => "ByLayer".into(),
            Color::ByBlock => "ByBlock".into(),
            Color::Index(i) => match i {
                1 => "Red".into(),
                2 => "Yellow".into(),
                3 => "Green".into(),
                4 => "Cyan".into(),
                5 => "Blue".into(),
                6 => "Magenta".into(),
                7 => "White".into(),
                _ => format!("Color {i}"),
            },
            Color::True(Rgb(r, g, b)) => format!("{r},{g},{b}"),
        }
    }
    /// Parse "red", "7", "bylayer", "255,0,0", "#ff0000".
    pub fn parse(s: &str) -> Option<Color> {
        let t = s.trim().to_ascii_lowercase();
        let named = ["", "red", "yellow", "green", "cyan", "blue", "magenta", "white"];
        if let Some(i) = named.iter().position(|n| !n.is_empty() && *n == t) {
            return Some(Color::Index(i as u8));
        }
        match t.as_str() {
            "bylayer" => return Some(Color::ByLayer),
            "byblock" => return Some(Color::ByBlock),
            "black" => return Some(Color::Index(7)),
            _ => {}
        }
        if let Some(rest) = t.strip_prefix("color ") {
            return rest.trim().parse::<u8>().ok().filter(|i| *i >= 1).map(Color::Index);
        }
        if let Ok(i) = t.parse::<u16>() {
            return match i {
                0 => Some(Color::ByBlock),
                256 => Some(Color::ByLayer),
                1..=255 => Some(Color::Index(i as u8)),
                _ => None,
            };
        }
        if t.starts_with('#') {
            return Rgb::parse_hex(&t).map(Color::True);
        }
        let parts: Vec<_> = t.split(',').map(|p| p.trim().parse::<u8>()).collect();
        if let [Ok(r), Ok(g), Ok(b)] = parts.as_slice() {
            return Some(Color::True(Rgb(*r, *g, *b)));
        }
        None
    }
}

/// The RGB of ACI `i` (0 and out-of-range map to white/7).
pub fn aci_rgb(i: u8) -> Rgb {
    PALETTE.get(usize::from(i)).copied().unwrap_or(Rgb(255, 255, 255))
}

/// The AutoCAD Color Index as AutoCAD itself draws and plots it, measured on the reference
/// application: a chart of one line per index, each with its own index colour, plotted with no
/// plot style table (so the colour reaches the paper unchanged) and read back from the plot.
/// Entries 1..=7 are the named colours and 8 and 9 two greys; entry 0 is the ByBlock placeholder
/// and entry 7 is white — AutoCAD prints that one black on white paper, which [`display_rgb`]
/// does. Entries 10..=249 are 24 hues 15° apart around the wheel, ten entries each: five
/// brightnesses, each followed by its half-tone (every channel half-way to that brightness).
/// 250..=255 are the grey ramp.
pub static PALETTE: [Rgb; 256] = build_palette();

/// The brightnesses of the hues 10..=249, brightest first, each with the channel levels it was
/// measured at for 1/4, 1/2, 5/8, 3/4 and 7/8 of it (a hue's channels are whole, none or those
/// fractions of its brightness).
const SHADES: [(u8, [u8; 5]); 5] = [
    (255, [63, 127, 159, 191, 223]),
    (221, [55, 110, 138, 165, 193]),
    (184, [46, 92, 115, 138, 161]),
    (149, [37, 74, 93, 112, 131]),
    (114, [28, 57, 71, 86, 100]),
];

/// The palette, built when the program is compiled (every index below is a constant in range).
const fn build_palette() -> [Rgb; 256] {
    let named = [
        Rgb(0, 0, 0),
        Rgb(255, 0, 0),
        Rgb(255, 255, 0),
        Rgb(0, 255, 0),
        Rgb(0, 255, 255),
        Rgb(0, 0, 255),
        Rgb(255, 0, 255),
        Rgb(255, 255, 255),
        Rgb(128, 128, 128),
        Rgb(220, 220, 220),
    ];
    let mut out = [Rgb(0, 0, 0); 256];
    let mut i = 0;
    while i < named.len() {
        out[i] = named[i];
        i += 1;
    }
    let mut i = 10;
    while i < 250 {
        let (r, g, b) = hue_quarters((i - 10) / 10);
        let (shade, half) = ((i % 10) / 2, i % 2 == 1);
        out[i] = Rgb(level(shade, r, half), level(shade, g, half), level(shade, b, half));
        i += 1;
    }
    let greys: [u8; 6] = [84, 118, 152, 186, 219, 250];
    let mut k = 0;
    while k < greys.len() {
        out[250 + k] = Rgb(greys[k], greys[k], greys[k]);
        k += 1;
    }
    out
}

/// The channel level of brightness `shade` ([`SHADES`]) for a channel that is `quarters` / 4 of
/// it, or half-way from that to the brightness for a half-tone.
const fn level(shade: usize, quarters: u8, half: bool) -> u8 {
    let (full, levels) = SHADES[shade];
    let eighths = if half { 4 + quarters } else { 2 * quarters };
    match eighths {
        0 => 0,
        2 => levels[0],
        4 => levels[1],
        5 => levels[2],
        6 => levels[3],
        7 => levels[4],
        _ => full,
    }
}

/// Channel fractions, in quarters, of hue step 0..24 (red → yellow → green → cyan → blue →
/// magenta → red).
const fn hue_quarters(h: usize) -> (u8, u8, u8) {
    let t = (h % 4) as u8;
    match h / 4 {
        0 => (4, t, 0),
        1 => (4 - t, 4, 0),
        2 => (0, 4, t),
        3 => (0, 4 - t, 4),
        4 => (t, 0, 4),
        _ => (4, 0, 4 - t),
    }
}

/// The closest index colour to an RGB value (1..=255).
pub fn nearest_aci(c: Rgb) -> u8 {
    let mut best = (u32::MAX, 7u8);
    for i in 1..=255u8 {
        let p = aci_rgb(i);
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2) as u32;
        let dist = d(p.0, c.0) + d(p.1, c.1) + d(p.2, c.2);
        if dist < best.0 {
            best = (dist, i);
        }
    }
    best.1
}

/// Display colour of `c` on `background`. Colour 7 (`aci7`) flips to contrast with the
/// background, white on dark and black on light (paper), as CAD programs draw it; every other
/// colour, including a true-colour or index white or black, keeps its RGB.
pub fn display_rgb(c: Rgb, aci7: bool, background: Rgb) -> Rgb {
    if !aci7 {
        c
    } else if background.luma() < 0.5 {
        Rgb(255, 255, 255)
    } else {
        Rgb(0, 0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_rejects_non_hex_digits() {
        assert_eq!(Rgb::parse_hex("+12345"), None);
        assert_eq!(Rgb::parse_hex("-12345"), None);
        assert_eq!(Color::parse("#+12345"), None);
        assert_eq!(Rgb::parse_hex("#012345"), Some(Rgb(1, 35, 69)));
    }

    #[test]
    fn palette_known_entries() {
        // Measured from AutoCAD's own plot of a chart of every index.
        assert_eq!(aci_rgb(1), Rgb(255, 0, 0));
        assert_eq!(aci_rgb(7), Rgb(255, 255, 255));
        assert_eq!(aci_rgb(8), Rgb(128, 128, 128));
        assert_eq!(aci_rgb(9), Rgb(220, 220, 220));
        assert_eq!(aci_rgb(10), Rgb(255, 0, 0));
        assert_eq!(aci_rgb(11), Rgb(255, 127, 127));
        assert_eq!(aci_rgb(12), Rgb(221, 0, 0));
        assert_eq!(aci_rgb(13), Rgb(221, 110, 110));
        assert_eq!(aci_rgb(18), Rgb(114, 0, 0));
        assert_eq!(aci_rgb(20), Rgb(255, 63, 0));
        assert_eq!(aci_rgb(30), Rgb(255, 127, 0));
        assert_eq!(aci_rgb(50), Rgb(255, 255, 0));
        assert_eq!(aci_rgb(74), Rgb(92, 184, 0));
        assert_eq!(aci_rgb(90), Rgb(0, 255, 0));
        assert_eq!(aci_rgb(126), Rgb(0, 149, 112));
        assert_eq!(aci_rgb(130), Rgb(0, 255, 255));
        assert_eq!(aci_rgb(170), Rgb(0, 0, 255));
        assert_eq!(aci_rgb(184), Rgb(46, 0, 184));
        assert_eq!(aci_rgb(210), Rgb(255, 0, 255));
        assert_eq!(aci_rgb(250), Rgb(84, 84, 84));
        assert_eq!(aci_rgb(255), Rgb(250, 250, 250));
    }

    /// Every entry as measured (`0xRRGGBB`), index order.
    #[rustfmt::skip]
    const MEASURED: [u32; 256] = [
        0x000000, 0xff0000, 0xffff00, 0x00ff00, 0x00ffff, 0x0000ff, 0xff00ff, 0xffffff,
        0x808080, 0xdcdcdc, 0xff0000, 0xff7f7f, 0xdd0000, 0xdd6e6e, 0xb80000, 0xb85c5c,
        0x950000, 0x954a4a, 0x720000, 0x723939, 0xff3f00, 0xff9f7f, 0xdd3700, 0xdd8a6e,
        0xb82e00, 0xb8735c, 0x952500, 0x955d4a, 0x721c00, 0x724739, 0xff7f00, 0xffbf7f,
        0xdd6e00, 0xdda56e, 0xb85c00, 0xb88a5c, 0x954a00, 0x95704a, 0x723900, 0x725639,
        0xffbf00, 0xffdf7f, 0xdda500, 0xddc16e, 0xb88a00, 0xb8a15c, 0x957000, 0x95834a,
        0x725600, 0x726439, 0xffff00, 0xffff7f, 0xdddd00, 0xdddd6e, 0xb8b800, 0xb8b85c,
        0x959500, 0x95954a, 0x727200, 0x727239, 0xbfff00, 0xdfff7f, 0xa5dd00, 0xc1dd6e,
        0x8ab800, 0xa1b85c, 0x709500, 0x83954a, 0x567200, 0x647239, 0x7fff00, 0xbfff7f,
        0x6edd00, 0xa5dd6e, 0x5cb800, 0x8ab85c, 0x4a9500, 0x70954a, 0x397200, 0x567239,
        0x3fff00, 0x9fff7f, 0x37dd00, 0x8add6e, 0x2eb800, 0x73b85c, 0x259500, 0x5d954a,
        0x1c7200, 0x477239, 0x00ff00, 0x7fff7f, 0x00dd00, 0x6edd6e, 0x00b800, 0x5cb85c,
        0x009500, 0x4a954a, 0x007200, 0x397239, 0x00ff3f, 0x7fff9f, 0x00dd37, 0x6edd8a,
        0x00b82e, 0x5cb873, 0x009525, 0x4a955d, 0x00721c, 0x397247, 0x00ff7f, 0x7fffbf,
        0x00dd6e, 0x6edda5, 0x00b85c, 0x5cb88a, 0x00954a, 0x4a9570, 0x007239, 0x397256,
        0x00ffbf, 0x7fffdf, 0x00dda5, 0x6eddc1, 0x00b88a, 0x5cb8a1, 0x009570, 0x4a9583,
        0x007256, 0x397264, 0x00ffff, 0x7fffff, 0x00dddd, 0x6edddd, 0x00b8b8, 0x5cb8b8,
        0x009595, 0x4a9595, 0x007272, 0x397272, 0x00bfff, 0x7fdfff, 0x00a5dd, 0x6ec1dd,
        0x008ab8, 0x5ca1b8, 0x007095, 0x4a8395, 0x005672, 0x396472, 0x007fff, 0x7fbfff,
        0x006edd, 0x6ea5dd, 0x005cb8, 0x5c8ab8, 0x004a95, 0x4a7095, 0x003972, 0x395672,
        0x003fff, 0x7f9fff, 0x0037dd, 0x6e8add, 0x002eb8, 0x5c73b8, 0x002595, 0x4a5d95,
        0x001c72, 0x394772, 0x0000ff, 0x7f7fff, 0x0000dd, 0x6e6edd, 0x0000b8, 0x5c5cb8,
        0x000095, 0x4a4a95, 0x000072, 0x393972, 0x3f00ff, 0x9f7fff, 0x3700dd, 0x8a6edd,
        0x2e00b8, 0x735cb8, 0x250095, 0x5d4a95, 0x1c0072, 0x473972, 0x7f00ff, 0xbf7fff,
        0x6e00dd, 0xa56edd, 0x5c00b8, 0x8a5cb8, 0x4a0095, 0x704a95, 0x390072, 0x563972,
        0xbf00ff, 0xdf7fff, 0xa500dd, 0xc16edd, 0x8a00b8, 0xa15cb8, 0x700095, 0x834a95,
        0x560072, 0x643972, 0xff00ff, 0xff7fff, 0xdd00dd, 0xdd6edd, 0xb800b8, 0xb85cb8,
        0x950095, 0x954a95, 0x720072, 0x723972, 0xff00bf, 0xff7fdf, 0xdd00a5, 0xdd6ec1,
        0xb8008a, 0xb85ca1, 0x950070, 0x954a83, 0x720056, 0x723964, 0xff007f, 0xff7fbf,
        0xdd006e, 0xdd6ea5, 0xb8005c, 0xb85c8a, 0x95004a, 0x954a70, 0x720039, 0x723956,
        0xff003f, 0xff7f9f, 0xdd0037, 0xdd6e8a, 0xb8002e, 0xb85c73, 0x950025, 0x954a5d,
        0x72001c, 0x723947, 0x545454, 0x767676, 0x989898, 0xbababa, 0xdbdbdb, 0xfafafa,
    ];

    #[test]
    fn the_built_palette_is_the_measured_one() {
        for (i, (p, m)) in PALETTE.iter().zip(MEASURED).enumerate() {
            assert_eq!(p.to_u32(), m, "index {i}");
        }
    }

    #[test]
    fn aci_roundtrip_and_parse() {
        assert_eq!(Color::from_aci(256), Color::ByLayer);
        assert_eq!(Color::from_aci(0), Color::ByBlock);
        assert_eq!(Color::Index(5).to_aci(), 5);
        assert_eq!(Color::parse("Red"), Some(Color::Index(1)));
        assert_eq!(Color::parse("bylayer"), Some(Color::ByLayer));
        assert_eq!(Color::parse("255,128,0"), Some(Color::True(Rgb(255, 128, 0))));
        assert_eq!(Color::parse("#00ff00"), Some(Color::True(Rgb(0, 255, 0))));
        assert_eq!(Color::parse("300"), None);
        assert_eq!(Color::parse("color 34"), Some(Color::Index(34)));
        assert_eq!(nearest_aci(Rgb(250, 2, 3)), 1);
    }

    #[test]
    fn white_flips_on_light_background() {
        assert_eq!(display_rgb(Rgb(255, 255, 255), true, Rgb(255, 255, 255)), Rgb(0, 0, 0));
        assert_eq!(display_rgb(Rgb(255, 255, 255), true, Rgb(33, 40, 48)), Rgb(255, 255, 255));
        // Only colour 7 flips: ACI 255 / true-colour white and dark index colours keep their RGB.
        assert_eq!(display_rgb(Rgb(255, 255, 255), false, Rgb(255, 255, 255)), Rgb(255, 255, 255));
        assert_eq!(display_rgb(aci_rgb(250), false, Rgb(33, 40, 48)), aci_rgb(250));
    }
}
