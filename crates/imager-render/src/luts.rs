//! The lookup tables ImageJ generates in code (`ij.plugin.LutLoader`).
//!
//! ImageJ's other lookup tables are `.lut` files shipped beside the
//! application; they are not built in here.

use imager_core::Lut;

/// A lookup table that is generated in code rather than loaded from a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinLut {
    /// Lower-case identifier for commands and macros, e.g. `"rgb332"`.
    pub id: &'static str,
    /// The name ImageJ shows in Image > Lookup Tables, e.g. `"3-3-2 RGB"`.
    pub name: &'static str,
}

/// Every built-in lookup table, in ImageJ's menu order.
pub const BUILTIN_LUTS: [BuiltinLut; 12] = [
    BuiltinLut::new("fire", "Fire"),
    BuiltinLut::new("grays", "Grays"),
    BuiltinLut::new("ice", "Ice"),
    BuiltinLut::new("spectrum", "Spectrum"),
    BuiltinLut::new("rgb332", "3-3-2 RGB"),
    BuiltinLut::new("red", "Red"),
    BuiltinLut::new("green", "Green"),
    BuiltinLut::new("blue", "Blue"),
    BuiltinLut::new("cyan", "Cyan"),
    BuiltinLut::new("magenta", "Magenta"),
    BuiltinLut::new("yellow", "Yellow"),
    BuiltinLut::new("redgreen", "Red/Green"),
];

impl BuiltinLut {
    const fn new(id: &'static str, name: &'static str) -> BuiltinLut {
        BuiltinLut { id, name }
    }

    /// Generates the table. Its [`Lut::name`] is the ImageJ name.
    pub fn lut(&self) -> Lut {
        let table: [[u8; 3]; 256] = match self.id {
            "fire" => interpolate(&FIRE),
            "ice" => interpolate(&ICE),
            "spectrum" => std::array::from_fn(spectrum),
            "rgb332" => std::array::from_fn(|i| {
                [
                    (i & 0xe0) as u8,
                    ((i << 3) & 0xe0) as u8,
                    ((i << 6) & 0xc0) as u8,
                ]
            }),
            "red" => primary(true, false, false),
            "green" => primary(false, true, false),
            "blue" => primary(false, false, true),
            "cyan" => primary(false, true, true),
            "magenta" => primary(true, false, true),
            "yellow" => primary(true, true, false),
            // `LutLoader.redGreen`: red rises over the first half; the
            // second half is green from `(byte)(i*2)`, which wraps to 0.
            "redgreen" => std::array::from_fn(|i| {
                let ramp = ((i * 2) & 255) as u8;
                if i < 128 { [ramp, 0, 0] } else { [0, ramp, 0] }
            }),
            _ => primary(true, true, true),
        };
        Lut {
            name: self.name.to_string(),
            table: table.into(),
        }
    }
}

/// The built-in lookup tables, for menus.
pub fn builtin_luts() -> &'static [BuiltinLut] {
    &BUILTIN_LUTS
}

/// Looks a built-in lookup table up by its identifier, its ImageJ name or
/// the argument ImageJ passes to `LutLoader` (`"3-3-2 rgb"`, `"red/green"`),
/// ignoring case.
pub fn builtin_lut(name: &str) -> Option<Lut> {
    let wanted = name.trim().to_lowercase();
    BUILTIN_LUTS
        .iter()
        .find(|l| l.id == wanted || l.name.to_lowercase() == wanted)
        .map(BuiltinLut::lut)
}

/// The table read backwards, as Image > Lookup Tables > Invert LUT does
/// (`ImageProcessor.invertLut`, `LUT.createInvertedLut`): entry `i` becomes
/// entry `255 - i`. Sample values are not changed.
pub fn inverted_lut(lut: &Lut) -> Lut {
    lut.inverted()
}

/// A ramp from black to `color` (`LUT.createLutFromColor`): entry `i` is
/// `(int)(i * component / 255.0)` per component.
pub fn lut_from_color(name: impl Into<String>, color: [u8; 3]) -> Lut {
    Lut::from_fn(name, |i| {
        color.map(|component| (i as f64 * (component as f64 / 255.0)) as u8)
    })
}

/// The lookup table ImageJ gives channel `channel` (from 0) of a composite
/// image that does not specify one (`CompositeImage.colors`): red, green,
/// blue, white, cyan, magenta, yellow, then white for every further channel.
pub fn default_channel_lut(channel: usize) -> Lut {
    const IDS: [&str; 7] = ["red", "green", "blue", "grays", "cyan", "magenta", "yellow"];
    builtin_lut(IDS.get(channel).copied().unwrap_or("grays")).unwrap_or_else(Lut::grays)
}

/// `LutLoader.primaryColor`: a linear ramp in the selected components.
fn primary(red: bool, green: bool, blue: bool) -> [[u8; 3]; 256] {
    std::array::from_fn(|i| [red, green, blue].map(|on| if on { i as u8 } else { 0 }))
}

/// `LutLoader.spectrum`: `Color.getHSBColor(i/255f, 1f, 1f)`, with Java's
/// single-precision HSB to RGB conversion for full saturation and brightness.
fn spectrum(i: usize) -> [u8; 3] {
    let hue = i as f32 / 255.0;
    let h = (hue - hue.floor()) * 6.0;
    let f = h - h.floor();
    let level = |v: f32| (v * 255.0 + 0.5) as u8;
    // With saturation 1 and brightness 1: p = 0, q = 1 - f, t = f.
    let (p, q, t, full) = (0, level(1.0 - f), level(1.0 - (1.0 - f)), 255);
    match h as u32 {
        0 => [full, t, p],
        1 => [q, full, p],
        2 => [p, full, t],
        3 => [p, q, full],
        4 => [t, p, full],
        _ => [full, p, q],
    }
}

/// `LutLoader.interpolate`: stretches 32 colours to 256 entries by linear
/// interpolation, truncating each component.
fn interpolate(colors: &[[u8; 3]; 32]) -> [[u8; 3]; 256] {
    let n = colors.len();
    let scale = n as f64 / 256.0;
    std::array::from_fn(|i| {
        let i1 = (i as f64 * scale) as usize;
        let i2 = (i1 + 1).min(n - 1);
        let fraction = i as f64 * scale - i1 as f64;
        std::array::from_fn(|c| {
            ((1.0 - fraction) * colors[i1][c] as f64 + fraction * colors[i2][c] as f64) as u8
        })
    })
}

const fn zip3(r: [u8; 32], g: [u8; 32], b: [u8; 32]) -> [[u8; 3]; 32] {
    let mut out = [[0u8; 3]; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = [r[i], g[i], b[i]];
        i += 1;
    }
    out
}

/// The 32 colours of `LutLoader.fire`.
const FIRE: [[u8; 3]; 32] = zip3(
    [
        0, 0, 1, 25, 49, 73, 98, 122, 146, 162, 173, 184, 195, 207, 217, 229, 240, 252, 255, 255,
        255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    ],
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 14, 35, 57, 79, 101, 117, 133, 147, 161, 175, 190,
        205, 219, 234, 248, 255, 255, 255, 255,
    ],
    [
        0, 61, 96, 130, 165, 192, 220, 227, 210, 181, 151, 122, 93, 64, 35, 5, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 35, 98, 160, 223, 255,
    ],
);

/// The 32 colours of `LutLoader.ice`.
const ICE: [[u8; 3]; 32] = zip3(
    [
        0, 0, 0, 0, 0, 0, 19, 29, 50, 48, 79, 112, 134, 158, 186, 201, 217, 229, 242, 250, 250,
        250, 250, 251, 250, 250, 250, 250, 251, 251, 243, 230,
    ],
    [
        156, 165, 176, 184, 190, 196, 193, 184, 171, 162, 146, 125, 107, 93, 81, 87, 92, 97, 95,
        93, 93, 90, 85, 69, 64, 54, 47, 35, 19, 0, 4, 0,
    ],
    [
        140, 147, 158, 166, 170, 176, 209, 220, 234, 225, 236, 246, 250, 251, 250, 250, 245, 230,
        230, 222, 202, 180, 163, 142, 123, 114, 106, 94, 84, 64, 26, 27,
    ],
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_accepts_ids_and_imagej_names() {
        for entry in builtin_luts() {
            assert_eq!(builtin_lut(entry.id).unwrap().name, entry.name);
            assert_eq!(builtin_lut(entry.name).unwrap(), entry.lut());
        }
        assert_eq!(builtin_lut("3-3-2 rgb").unwrap().name, "3-3-2 RGB");
        assert_eq!(builtin_lut(" RED/green ").unwrap().name, "Red/Green");
        assert!(builtin_lut("viridis").is_none());
        assert_eq!(builtin_lut("grays").unwrap().table, Lut::grays().table);
    }

    #[test]
    fn fire_interpolates_its_32_colours() {
        // i = 12: i1 = (int)(12 * 0.125) = 1, fraction 0.5, between colours
        // (0,0,61) and (1,0,96): (int)0.5 = 0, 0, (int)78.5 = 78.
        let fire = builtin_lut("fire").unwrap();
        assert_eq!(fire.table[12], [0, 0, 78]);
        assert_eq!(fire.table[0], [0, 0, 0]);
        // The last entries interpolate colour 31 with itself.
        assert_eq!(fire.table[255], [255, 255, 255]);
        assert_eq!(builtin_lut("ice").unwrap().table[0], [0, 156, 140]);
    }

    #[test]
    fn code_generated_tables() {
        let t = |name: &str| builtin_lut(name).unwrap().table;
        assert_eq!(
            t("rgb332")[0b1011_0110],
            [0b1010_0000, 0b1010_0000, 0b1000_0000]
        );
        assert_eq!(t("cyan")[200], [0, 200, 200]);
        assert_eq!(t("magenta")[7], [7, 0, 7]);
        assert_eq!(t("yellow")[255], [255, 255, 0]);
        assert_eq!(t("redgreen")[100], [200, 0, 0]);
        // (byte)(128 * 2) is 0: green restarts from black.
        assert_eq!(t("redgreen")[128], [0, 0, 0]);
        assert_eq!(t("redgreen")[255], [0, 254, 0]);
        // Hue 0 and hue 1 are both red; a third of the way round is green.
        assert_eq!(t("spectrum")[0], [255, 0, 0]);
        assert_eq!(t("spectrum")[255], [255, 0, 0]);
        assert_eq!(t("spectrum")[85], [0, 255, 0]);
        assert_eq!(t("spectrum")[1], [255, 6, 0]);
    }

    #[test]
    fn inverted_and_colour_ramps() {
        let fire = builtin_lut("fire").unwrap();
        let inverted = inverted_lut(&fire);
        assert!((0..256).all(|i| inverted.table[i] == fire.table[255 - i]));
        assert_eq!(
            lut_from_color("Orange", [255, 128, 0]).table[100],
            [100, 50, 0]
        );
        assert_eq!(default_channel_lut(0).name, "Red");
        assert_eq!(default_channel_lut(3).name, "Grays");
        assert_eq!(default_channel_lut(6).name, "Yellow");
        assert_eq!(default_channel_lut(20).name, "Grays");
        assert_eq!(
            lut_from_color("r", [255, 0, 0]).table,
            builtin_lut("red").unwrap().table
        );
    }
}
