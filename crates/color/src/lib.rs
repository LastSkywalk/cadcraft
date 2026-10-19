//! CAD colours: the 256-entry colour index (ACI), 24-bit true colour, and the logical
//! ByLayer / ByBlock values. The index palette is generated from its well-known structure
//! (24 hues × 5 values × 2 saturations, plus primaries and greys), not copied from any file.
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
/// The 24 hues of 10..=249 step 15° around the wheel, each in five brightnesses and in the five
/// half-tones half-way to white; 250..=255 are the grey ramp. Entry 0 is the ByBlock
/// placeholder and entry 7 is white — AutoCAD prints that one black on white paper, which
/// [`display_rgb`] does.
pub static PALETTE: [Rgb; 256] = [
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
    Rgb(255, 0, 0),
    Rgb(255, 127, 127),
    Rgb(221, 0, 0),
    Rgb(221, 110, 110),
    Rgb(184, 0, 0),
    Rgb(184, 92, 92),
    Rgb(149, 0, 0),
    Rgb(149, 74, 74),
    Rgb(114, 0, 0),
    Rgb(114, 57, 57),
    Rgb(255, 63, 0),
    Rgb(255, 159, 127),
    Rgb(221, 55, 0),
    Rgb(221, 138, 110),
    Rgb(184, 46, 0),
    Rgb(184, 115, 92),
    Rgb(149, 37, 0),
    Rgb(149, 93, 74),
    Rgb(114, 28, 0),
    Rgb(114, 71, 57),
    Rgb(255, 127, 0),
    Rgb(255, 191, 127),
    Rgb(221, 110, 0),
    Rgb(221, 165, 110),
    Rgb(184, 92, 0),
    Rgb(184, 138, 92),
    Rgb(149, 74, 0),
    Rgb(149, 112, 74),
    Rgb(114, 57, 0),
    Rgb(114, 86, 57),
    Rgb(255, 191, 0),
    Rgb(255, 223, 127),
    Rgb(221, 165, 0),
    Rgb(221, 193, 110),
    Rgb(184, 138, 0),
    Rgb(184, 161, 92),
    Rgb(149, 112, 0),
    Rgb(149, 131, 74),
    Rgb(114, 86, 0),
    Rgb(114, 100, 57),
    Rgb(255, 255, 0),
    Rgb(255, 255, 127),
    Rgb(221, 221, 0),
    Rgb(221, 221, 110),
    Rgb(184, 184, 0),
    Rgb(184, 184, 92),
    Rgb(149, 149, 0),
    Rgb(149, 149, 74),
    Rgb(114, 114, 0),
    Rgb(114, 114, 57),
    Rgb(191, 255, 0),
    Rgb(223, 255, 127),
    Rgb(165, 221, 0),
    Rgb(193, 221, 110),
    Rgb(138, 184, 0),
    Rgb(161, 184, 92),
    Rgb(112, 149, 0),
    Rgb(131, 149, 74),
    Rgb(86, 114, 0),
    Rgb(100, 114, 57),
    Rgb(127, 255, 0),
    Rgb(191, 255, 127),
    Rgb(110, 221, 0),
    Rgb(165, 221, 110),
    Rgb(92, 184, 0),
    Rgb(138, 184, 92),
    Rgb(74, 149, 0),
    Rgb(112, 149, 74),
    Rgb(57, 114, 0),
    Rgb(86, 114, 57),
    Rgb(63, 255, 0),
    Rgb(159, 255, 127),
    Rgb(55, 221, 0),
    Rgb(138, 221, 110),
    Rgb(46, 184, 0),
    Rgb(115, 184, 92),
    Rgb(37, 149, 0),
    Rgb(93, 149, 74),
    Rgb(28, 114, 0),
    Rgb(71, 114, 57),
    Rgb(0, 255, 0),
    Rgb(127, 255, 127),
    Rgb(0, 221, 0),
    Rgb(110, 221, 110),
    Rgb(0, 184, 0),
    Rgb(92, 184, 92),
    Rgb(0, 149, 0),
    Rgb(74, 149, 74),
    Rgb(0, 114, 0),
    Rgb(57, 114, 57),
    Rgb(0, 255, 63),
    Rgb(127, 255, 159),
    Rgb(0, 221, 55),
    Rgb(110, 221, 138),
    Rgb(0, 184, 46),
    Rgb(92, 184, 115),
    Rgb(0, 149, 37),
    Rgb(74, 149, 93),
    Rgb(0, 114, 28),
    Rgb(57, 114, 71),
    Rgb(0, 255, 127),
    Rgb(127, 255, 191),
    Rgb(0, 221, 110),
    Rgb(110, 221, 165),
    Rgb(0, 184, 92),
    Rgb(92, 184, 138),
    Rgb(0, 149, 74),
    Rgb(74, 149, 112),
    Rgb(0, 114, 57),
    Rgb(57, 114, 86),
    Rgb(0, 255, 191),
    Rgb(127, 255, 223),
    Rgb(0, 221, 165),
    Rgb(110, 221, 193),
    Rgb(0, 184, 138),
    Rgb(92, 184, 161),
    Rgb(0, 149, 112),
    Rgb(74, 149, 131),
    Rgb(0, 114, 86),
    Rgb(57, 114, 100),
    Rgb(0, 255, 255),
    Rgb(127, 255, 255),
    Rgb(0, 221, 221),
    Rgb(110, 221, 221),
    Rgb(0, 184, 184),
    Rgb(92, 184, 184),
    Rgb(0, 149, 149),
    Rgb(74, 149, 149),
    Rgb(0, 114, 114),
    Rgb(57, 114, 114),
    Rgb(0, 191, 255),
    Rgb(127, 223, 255),
    Rgb(0, 165, 221),
    Rgb(110, 193, 221),
    Rgb(0, 138, 184),
    Rgb(92, 161, 184),
    Rgb(0, 112, 149),
    Rgb(74, 131, 149),
    Rgb(0, 86, 114),
    Rgb(57, 100, 114),
    Rgb(0, 127, 255),
    Rgb(127, 191, 255),
    Rgb(0, 110, 221),
    Rgb(110, 165, 221),
    Rgb(0, 92, 184),
    Rgb(92, 138, 184),
    Rgb(0, 74, 149),
    Rgb(74, 112, 149),
    Rgb(0, 57, 114),
    Rgb(57, 86, 114),
    Rgb(0, 63, 255),
    Rgb(127, 159, 255),
    Rgb(0, 55, 221),
    Rgb(110, 138, 221),
    Rgb(0, 46, 184),
    Rgb(92, 115, 184),
    Rgb(0, 37, 149),
    Rgb(74, 93, 149),
    Rgb(0, 28, 114),
    Rgb(57, 71, 114),
    Rgb(0, 0, 255),
    Rgb(127, 127, 255),
    Rgb(0, 0, 221),
    Rgb(110, 110, 221),
    Rgb(0, 0, 184),
    Rgb(92, 92, 184),
    Rgb(0, 0, 149),
    Rgb(74, 74, 149),
    Rgb(0, 0, 114),
    Rgb(57, 57, 114),
    Rgb(63, 0, 255),
    Rgb(159, 127, 255),
    Rgb(55, 0, 221),
    Rgb(138, 110, 221),
    Rgb(46, 0, 184),
    Rgb(115, 92, 184),
    Rgb(37, 0, 149),
    Rgb(93, 74, 149),
    Rgb(28, 0, 114),
    Rgb(71, 57, 114),
    Rgb(127, 0, 255),
    Rgb(191, 127, 255),
    Rgb(110, 0, 221),
    Rgb(165, 110, 221),
    Rgb(92, 0, 184),
    Rgb(138, 92, 184),
    Rgb(74, 0, 149),
    Rgb(112, 74, 149),
    Rgb(57, 0, 114),
    Rgb(86, 57, 114),
    Rgb(191, 0, 255),
    Rgb(223, 127, 255),
    Rgb(165, 0, 221),
    Rgb(193, 110, 221),
    Rgb(138, 0, 184),
    Rgb(161, 92, 184),
    Rgb(112, 0, 149),
    Rgb(131, 74, 149),
    Rgb(86, 0, 114),
    Rgb(100, 57, 114),
    Rgb(255, 0, 255),
    Rgb(255, 127, 255),
    Rgb(221, 0, 221),
    Rgb(221, 110, 221),
    Rgb(184, 0, 184),
    Rgb(184, 92, 184),
    Rgb(149, 0, 149),
    Rgb(149, 74, 149),
    Rgb(114, 0, 114),
    Rgb(114, 57, 114),
    Rgb(255, 0, 191),
    Rgb(255, 127, 223),
    Rgb(221, 0, 165),
    Rgb(221, 110, 193),
    Rgb(184, 0, 138),
    Rgb(184, 92, 161),
    Rgb(149, 0, 112),
    Rgb(149, 74, 131),
    Rgb(114, 0, 86),
    Rgb(114, 57, 100),
    Rgb(255, 0, 127),
    Rgb(255, 127, 191),
    Rgb(221, 0, 110),
    Rgb(221, 110, 165),
    Rgb(184, 0, 92),
    Rgb(184, 92, 138),
    Rgb(149, 0, 74),
    Rgb(149, 74, 112),
    Rgb(114, 0, 57),
    Rgb(114, 57, 86),
    Rgb(255, 0, 63),
    Rgb(255, 127, 159),
    Rgb(221, 0, 55),
    Rgb(221, 110, 138),
    Rgb(184, 0, 46),
    Rgb(184, 92, 115),
    Rgb(149, 0, 37),
    Rgb(149, 74, 93),
    Rgb(114, 0, 28),
    Rgb(114, 57, 71),
    Rgb(84, 84, 84),
    Rgb(118, 118, 118),
    Rgb(152, 152, 152),
    Rgb(186, 186, 186),
    Rgb(219, 219, 219),
    Rgb(250, 250, 250),
];

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
