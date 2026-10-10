use std::collections::HashMap;
use std::path::Path;

use imager_core::Image;

use crate::bmp::{
    Frame, decode_error, frame_planes, gray_image, indexed_image, is_gray, lut_or_gray, rgb_image,
};
use crate::{Error, Format, Result};

pub(crate) struct Gif;

impl Format for Gif {
    fn name(&self) -> &'static str {
        "GIF"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["gif"]
    }

    fn matches(&self, header: &[u8]) -> bool {
        // The same four bytes `Opener.getFileType` tests.
        header.starts_with(b"GIF8")
    }

    /// Opens a GIF the way ImageJ's `GIF_Reader` does.
    ///
    /// A file with one image that fills the logical screen opens as an
    /// 8-bit plane of palette indices with the palette as the channel's
    /// lookup table; one that covers only part of the screen opens as RGB
    /// on black (ImageJ gets either from Java's own GIF decoder, though
    /// the row that decoder repeats below a partial interlaced image is
    /// not reproduced). A file
    /// with several images opens as a Z stack of composited RGB frames, or
    /// of 8-bit planes if every pixel of the first frame is grey
    /// (`Opener.convertGrayJpegTo8Bits`, which converts each frame with
    /// `(r + g + b) / 3`). Frame delays are discarded.
    ///
    /// Unlike ImageJ, a file that ends in the middle of its data is an
    /// error rather than a partly black image.
    fn read(&self, path: &Path) -> Result<Image> {
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        let decoded = decode(&bytes).map_err(|m| decode_error(path, m))?;
        let (width, height) = (decoded.width, decoded.height);
        if decoded.frames.len() == 1 {
            let first = decoded.first;
            if first.fills_screen {
                return indexed_image(path, width, height, first.indices, &first.palette);
            }
            // Transparent pixels keep their palette colour; only the part
            // of the screen the image does not cover is black.
            let rgb: Vec<u8> = first
                .indices
                .iter()
                .zip(&first.inside)
                .flat_map(|(&i, &inside)| match first.palette.get(usize::from(i)) {
                    Some(color) if inside => *color,
                    _ => [0; 3],
                })
                .collect();
            return rgb_image(path, width, height, std::slice::from_ref(&rgb));
        }
        let frames: Vec<Vec<u8>> = decoded
            .frames
            .iter()
            .map(|f| {
                f.iter()
                    .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
                    .collect()
            })
            .collect();
        if is_gray(&frames[0]) {
            // ImageJ's unweighted RGB to 8-bit conversion
            // (`TypeConverter.convertRGBToByte`), applied to every frame
            // although only the first was tested.
            let third = 1.0 / 3.0;
            let gray = frames
                .iter()
                .map(|f| {
                    f.as_chunks::<3>()
                        .0
                        .iter()
                        .map(|p| {
                            (f64::from(p[0]) * third
                                + f64::from(p[1]) * third
                                + f64::from(p[2]) * third
                                + 0.5) as u8
                        })
                        .collect()
                })
                .collect();
            gray_image(path, width, height, gray)
        } else {
            rgb_image(path, width, height, &frames)
        }
    }

    /// Writes 8-bit planes as palette indices with the channel's lookup
    /// table (or a grey ramp) as the palette, which loses nothing. A stack
    /// becomes an animated GIF laid out as ImageJ's `GifWriter` does: no
    /// global colour table, endless looping, and a 256-entry local table
    /// and a 140 ms delay (ImageJ's default 7 frames per second) for each
    /// frame.
    ///
    /// RGB frames are written exactly when they have at most 256 colours.
    /// ImageJ instead reduces any RGB image to 256 colours; that lossy step
    /// is left to an explicit conversion.
    fn write(&self, image: &Image, path: &Path) -> Result<()> {
        let meta = image.meta();
        let unsupported = |reason: String| Error::Unsupported {
            format: "GIF",
            reason,
        };
        let frames = frame_planes(meta, "GIF")?;
        let (width, height) = (meta.width(), meta.height());
        if width > 65535 || height > 65535 {
            return Err(unsupported(format!(
                "GIF is limited to 65535 x 65535 pixels, but this image is {width} x {height}"
            )));
        }
        let size = [(width as u16).to_le_bytes(), (height as u16).to_le_bytes()].concat();
        let animated = frames.len() > 1;

        let mut out = b"GIF89a".to_vec();
        out.extend(&size);
        if animated {
            // Colour resolution 7, no global colour table; then the
            // Netscape extension asking for endless repetition.
            out.extend([0x70, 0, 0]);
            out.extend([0x21, 0xff, 11]);
            out.extend(b"NETSCAPE2.0");
            out.extend([3, 1, 0, 0, 0]);
        } else {
            out.extend([0xf7, 0, 0]);
        }
        for frame in &frames {
            let (indices, palette) = match frame {
                Frame::Gray(plane) => (
                    image.read_plane(*plane)?.try_slice::<u8>()?.to_vec(),
                    *lut_or_gray(meta).table,
                ),
                Frame::Rgb(planes) => {
                    let [r, g, b] = [
                        image.read_plane(planes[0])?,
                        image.read_plane(planes[1])?,
                        image.read_plane(planes[2])?,
                    ];
                    index_colors(r.try_slice()?, g.try_slice()?, b.try_slice()?).ok_or_else(
                        || {
                            unsupported(
                                "GIF holds at most 256 colours per frame, but this RGB image has more; convert it to 8-bit colour first"
                                    .to_string(),
                            )
                        },
                    )?
                }
            };
            if animated {
                // Graphic control: no disposal, no transparency, delay in
                // hundredths of a second.
                out.extend([0x21, 0xf9, 4, 0, DELAY_CS, 0, 0, 0]);
            } else {
                out.extend(palette.as_flattened());
            }
            out.extend([0x2c, 0, 0, 0, 0]);
            out.extend(&size);
            if animated {
                out.push(0x87);
                out.extend(palette.as_flattened());
            } else {
                out.push(0);
            }
            encode_lzw(&indices, &mut out);
        }
        out.push(0x3b);
        std::fs::write(path, out).map_err(|e| Error::io(path, e))
    }
}

/// Frame delay of written animations in hundredths of a second:
/// `Math.round((int)(1000 / 7.0) / 10f)`, from `GifWriter.run` and
/// `AnimatedGifEncoder2.setDelay` at ImageJ's default animation rate.
const DELAY_CS: u8 = 14;

/// Maps an RGB frame to palette indices, numbering colours in order of
/// first appearance, or `None` if it has more than 256 colours.
fn index_colors(r: &[u8], g: &[u8], b: &[u8]) -> Option<(Vec<u8>, [[u8; 3]; 256])> {
    let mut palette = [[0u8; 3]; 256];
    let mut known: HashMap<[u8; 3], u8> = HashMap::new();
    let mut indices = Vec::with_capacity(r.len());
    for i in 0..r.len() {
        let color = [r[i], g[i], b[i]];
        let index = match known.get(&color) {
            Some(&index) => index,
            None => {
                let index = u8::try_from(known.len()).ok()?;
                palette[index as usize] = color;
                known.insert(color, index);
                index
            }
        };
        indices.push(index);
    }
    Some((indices, palette))
}

/// Appends GIF image data for 8-bit indices: the minimum code size, the
/// LZW codes in sub-blocks of up to 255 bytes, and the block terminator.
fn encode_lzw(indices: &[u8], out: &mut Vec<u8>) {
    const CLEAR: u16 = 256;
    const END: u16 = 257;
    const MAX_CODES: u16 = 4096;

    struct Bits {
        bytes: Vec<u8>,
        acc: u32,
        count: u32,
    }
    impl Bits {
        fn put(&mut self, code: u16, size: u32) {
            self.acc |= u32::from(code) << self.count;
            self.count += size;
            while self.count >= 8 {
                self.bytes.push(self.acc as u8);
                self.acc >>= 8;
                self.count -= 8;
            }
        }
    }

    let mut bits = Bits {
        bytes: Vec::new(),
        acc: 0,
        count: 0,
    };
    let mut table: HashMap<(u16, u8), u16> = HashMap::new();
    let mut next = END + 1;
    let mut size = 9;
    bits.put(CLEAR, size);
    let mut pixels = indices.iter().copied();
    if let Some(first) = pixels.next() {
        let mut current = u16::from(first);
        for pixel in pixels {
            if let Some(&code) = table.get(&(current, pixel)) {
                current = code;
                continue;
            }
            bits.put(current, size);
            if next < MAX_CODES {
                table.insert((current, pixel), next);
                next += 1;
                // The decoder defines each code one step later, so the
                // width grows once the code just defined no longer fits.
                if next > (1 << size) && size < 12 {
                    size += 1;
                }
            } else {
                bits.put(CLEAR, size);
                table.clear();
                next = END + 1;
                size = 9;
            }
            current = u16::from(pixel);
        }
        bits.put(current, size);
    }
    bits.put(END, size);
    if bits.count > 0 {
        bits.bytes.push(bits.acc as u8);
    }

    out.push(8);
    for block in bits.bytes.chunks(255) {
        out.push(block.len() as u8);
        out.extend(block);
    }
    out.push(0);
}

const TRUNCATED: &str = "the file ends in the middle of the GIF data";

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn byte(&mut self) -> std::result::Result<u8, String> {
        let b = *self.data.get(self.pos).ok_or(TRUNCATED)?;
        self.pos += 1;
        Ok(b)
    }

    fn u16(&mut self) -> std::result::Result<usize, String> {
        let bytes = self.take(2)?;
        Ok(usize::from(u16::from_le_bytes([bytes[0], bytes[1]])))
    }

    fn take(&mut self, n: usize) -> std::result::Result<&'a [u8], String> {
        let slice = self.data.get(self.pos..self.pos + n).ok_or(TRUNCATED)?;
        self.pos += n;
        Ok(slice)
    }

    /// Skips data sub-blocks up to and including the zero-length one.
    fn skip_blocks(&mut self) -> std::result::Result<(), String> {
        loop {
            let n = usize::from(self.byte()?);
            if n == 0 {
                return Ok(());
            }
            self.take(n)?;
        }
    }
}

/// A colour table as `GifDecoder` keeps it: 256 packed `0xffRRGGBB`
/// entries, zero beyond the table's length.
type Table = [u32; 256];

/// The first image of a file, as needed when it is the only one.
struct Indexed {
    /// Palette indices on the logical screen; 0 outside the image.
    indices: Vec<u8>,
    /// Which screen pixels the image covers.
    inside: Vec<bool>,
    palette: Vec<[u8; 3]>,
    fills_screen: bool,
}

struct Decoded {
    width: usize,
    height: usize,
    /// Composited frames as packed RGB; zero is transparent or untouched
    /// and shows as black.
    frames: Vec<Vec<u32>>,
    /// The first image as palette indices on the logical screen.
    first: Indexed,
}

/// The state of ImageJ's `GifDecoder` (in `GIF_Reader.java`), whose
/// compositing differs from a web browser's in ways noted below.
struct Decoder<'a> {
    cur: Cursor<'a>,
    width: usize,
    height: usize,
    gct: Option<(Table, usize)>,
    bg_index: usize,
    bg_color: u32,
    // Graphic control state. `GifDecoder.resetFrame` assigns to local
    // variables instead of these fields, so a frame without its own
    // graphic control extension inherits the previous frame's values.
    dispose: u8,
    transparency: bool,
    trans_index: usize,
    last_dispose: u8,
    last_rect: [usize; 4],
    last_bg_color: u32,
    frames: Vec<Vec<u32>>,
    first: Option<Indexed>,
}

fn decode(bytes: &[u8]) -> std::result::Result<Decoded, String> {
    let mut cur = Cursor {
        data: bytes,
        pos: 0,
    };
    if !cur.take(6)?.starts_with(b"GIF") {
        return Err("not a GIF file".to_string());
    }
    let (width, height) = (cur.u16()?, cur.u16()?);
    let packed = cur.byte()?;
    let bg_index = usize::from(cur.byte()?);
    cur.byte()?; // pixel aspect ratio
    if width == 0 || height == 0 {
        return Err(format!("invalid GIF size {width} x {height}"));
    }
    let mut decoder = Decoder {
        cur,
        width,
        height,
        gct: None,
        bg_index,
        bg_color: 0,
        dispose: 0,
        transparency: false,
        trans_index: 0,
        last_dispose: 0,
        last_rect: [0; 4],
        last_bg_color: 0,
        frames: Vec::new(),
        first: None,
    };
    if packed & 0x80 != 0 {
        let table = decoder.read_table(2 << (packed & 7))?;
        decoder.bg_color = table.0[bg_index];
        decoder.gct = Some(table);
    }
    decoder.check_size(width, height)?;
    decoder.read_contents()?;
    match decoder.first {
        Some(first) => Ok(Decoded {
            width,
            height,
            frames: decoder.frames,
            first,
        }),
        None => Err("the GIF file contains no image".to_string()),
    }
}

impl Decoder<'_> {
    fn read_table(&mut self, colors: usize) -> std::result::Result<(Table, usize), String> {
        let mut table = [0u32; 256];
        let bytes = self.cur.take(colors * 3)?;
        for (entry, c) in table.iter_mut().zip(bytes.as_chunks::<3>().0) {
            *entry = 0xff00_0000 | u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2]);
        }
        Ok((table, colors))
    }

    /// `GifDecoder.readContents`.
    fn read_contents(&mut self) -> std::result::Result<(), String> {
        loop {
            // Files without the trailer byte are common enough to accept
            // once at least one image has been read.
            if self.cur.at_end() && !self.frames.is_empty() {
                return Ok(());
            }
            match self.cur.byte()? {
                0x2c => self.read_image()?,
                0x21 => match self.cur.byte()? {
                    0xf9 => {
                        self.cur.byte()?; // block size
                        let packed = self.cur.byte()?;
                        // GifDecoder computes `(packed & 0x1c) >> 1`, twice
                        // the disposal method; `set_pixels` accounts for it.
                        self.dispose = (packed & 0x1c) >> 2;
                        self.transparency = packed & 1 != 0;
                        self.cur.u16()?; // delay
                        self.trans_index = usize::from(self.cur.byte()?);
                        self.cur.byte()?; // block terminator
                    }
                    _ => self.cur.skip_blocks()?,
                },
                0x3b => return Ok(()),
                other if self.frames.is_empty() => {
                    return Err(format!("unexpected GIF block type 0x{other:02x}"));
                }
                // GifDecoder stops at an unknown block and ImageJ shows the
                // frames read so far.
                _ => return Ok(()),
            }
        }
    }

    /// `GifDecoder.readImage`.
    fn read_image(&mut self) -> std::result::Result<(), String> {
        let rect = [
            self.cur.u16()?,
            self.cur.u16()?,
            self.cur.u16()?,
            self.cur.u16()?,
        ];
        let packed = self.cur.byte()?;
        let interlace = packed & 0x40 != 0;
        let (mut table, colors) = if packed & 0x80 != 0 {
            self.read_table(2 << (packed & 7))?
        } else {
            if self.bg_index == self.trans_index {
                self.bg_color = 0;
            }
            self.gct.ok_or("the GIF image has no colour table")?
        };
        let palette = table[..colors]
            .iter()
            .map(|c| [(c >> 16) as u8, (c >> 8) as u8, *c as u8])
            .collect();
        if self.transparency {
            table[self.trans_index] = 0;
        }
        let [_, _, iw, ih] = rect;
        self.check_size(iw, ih)?;
        let pixels = decode_lzw(&mut self.cur, iw * ih)?;

        if self.first.is_none() {
            let (indices, inside) = self.place_indices(rect, interlace, &pixels)?;
            self.first = Some(Indexed {
                indices,
                inside,
                palette,
                fills_screen: rect == [0, 0, self.width, self.height],
            });
        }
        let frame = self.set_pixels(rect, interlace, &pixels, &table)?;
        self.frames.push(frame);
        self.last_dispose = self.dispose;
        self.last_rect = rect;
        self.last_bg_color = self.bg_color;
        Ok(())
    }

    /// `GifDecoder.setPixels`: builds the next frame from the previous one
    /// and the decoded image.
    ///
    /// Where this differs from the GIF specification, it is because
    /// ImageJ does: after "no disposal" the frame starts black instead of
    /// keeping the previous frame; after "do not dispose" the previous
    /// image's rectangle is filled with the background colour (the code
    /// meant for "restore to background", selected by the doubled disposal
    /// value); after the other two methods the previous frame is kept
    /// whole; and transparent pixels are painted black rather than left
    /// alone.
    fn set_pixels(
        &self,
        [ix, iy, iw, ih]: [usize; 4],
        interlace: bool,
        pixels: &[u8],
        table: &Table,
    ) -> std::result::Result<Vec<u32>, String> {
        let (width, height) = (self.width, self.height);
        let mut dest = zeroed::<u32>(width * height)?;
        if self.last_dispose > 0
            && let Some(previous) = self.frames.last()
        {
            dest.copy_from_slice(previous);
            if self.last_dispose == 1 && self.last_bg_color != 0 {
                let [lx, ly, lw, lh] = self.last_rect;
                for y in ly..(ly + lh).min(height) {
                    let row = &mut dest[y * width..(y + 1) * width];
                    if lx < width {
                        row[lx..(lx + lw).min(width)].fill(self.last_bg_color);
                    }
                }
            }
        }

        let (mut pass, mut inc, mut iline) = (1, 8, 0);
        for i in 0..ih {
            let mut line = i;
            if interlace {
                // Checked once per row, as in GifDecoder; images shorter
                // than five rows therefore land on rows outside the image.
                if iline >= ih {
                    pass += 1;
                    match pass {
                        2 => iline = 4,
                        3 => (iline, inc) = (2, 4),
                        4 => (iline, inc) = (1, 2),
                        _ => {}
                    }
                }
                line = iline;
                iline += inc;
            }
            line += iy;
            if line < height {
                let start = line * width + ix;
                let end = (start + iw).min((line + 1) * width);
                if start < end {
                    for (d, &index) in dest[start..end].iter_mut().zip(&pixels[i * iw..]) {
                        *d = table[usize::from(index)];
                    }
                }
            }
        }
        Ok(dest)
    }

    /// Refuses a size that the file is too short to describe: LZW cannot
    /// pack more than 4096 pixels into a byte, so such a size can only
    /// come from a damaged header and would ask for a huge allocation.
    fn check_size(&self, width: usize, height: usize) -> std::result::Result<(), String> {
        if width * height > self.cur.data.len().saturating_mul(4096) {
            return Err(format!(
                "a {width} x {height} GIF image cannot fit in a file of {} bytes",
                self.cur.data.len()
            ));
        }
        Ok(())
    }

    /// The image's palette indices placed on the logical screen, with
    /// index 0 elsewhere, and which pixels it covers.
    fn place_indices(
        &self,
        [ix, iy, iw, ih]: [usize; 4],
        interlace: bool,
        pixels: &[u8],
    ) -> std::result::Result<(Vec<u8>, Vec<bool>), String> {
        let (width, height) = (self.width, self.height);
        let mut dest = zeroed::<u8>(width * height)?;
        let mut inside = zeroed::<bool>(width * height)?;
        let rows: Vec<usize> = if interlace {
            [(0, 8), (4, 8), (2, 4), (1, 2)]
                .into_iter()
                .flat_map(|(first, step)| (first..ih).step_by(step))
                .collect()
        } else {
            (0..ih).collect()
        };
        for (i, row) in rows.into_iter().enumerate() {
            let line = row + iy;
            if line < height && ix < width {
                let n = iw.min(width - ix);
                let start = line * width + ix;
                dest[start..start + n].copy_from_slice(&pixels[i * iw..i * iw + n]);
                inside[start..start + n].fill(true);
            }
        }
        Ok((dest, inside))
    }
}

/// A zero-filled vector, or an error instead of an abort when the size a
/// file claims cannot be allocated.
fn zeroed<T: Clone + Default>(len: usize) -> std::result::Result<Vec<T>, String> {
    let mut v = Vec::new();
    v.try_reserve_exact(len)
        .map_err(|_| format!("not enough memory for a GIF frame of {len} pixels"))?;
    v.resize(len, T::default());
    Ok(v)
}

/// `GifDecoder.decodeImageData`: decodes LZW image data to `npix` palette
/// indices. Like ImageJ it stops quietly at invalid codes and fills the
/// rest with index 0, but a file that ends before its data does is an
/// error.
fn decode_lzw(cur: &mut Cursor, npix: usize) -> std::result::Result<Vec<u8>, String> {
    const MAX_CODES: usize = 4096;
    let mut pixels: Vec<u8> = Vec::new();
    pixels
        .try_reserve_exact(npix)
        .map_err(|_| format!("not enough memory for a GIF image of {npix} pixels"))?;

    let data_size = usize::from(cur.byte()?);
    if data_size > 11 {
        return Err(format!("invalid GIF code size {data_size}"));
    }
    let clear = 1 << data_size;
    let end_of_information = clear + 1;
    let mut available = clear + 2;
    let mut old_code: Option<usize> = None;
    let mut code_size = data_size + 1;
    let mut code_mask = (1 << code_size) - 1;
    let mut prefix = [0u16; MAX_CODES];
    let mut suffix = [0u8; MAX_CODES];
    let mut stack = [0u8; MAX_CODES + 1];
    for (code, s) in suffix.iter_mut().enumerate().take(clear) {
        *s = code as u8;
    }

    let (mut datum, mut bits, mut first, mut top) = (0usize, 0, 0u8, 0);
    let mut block: &[u8] = &[];
    let mut terminated = false;
    while pixels.len() < npix {
        if top == 0 {
            if bits < code_size {
                if block.is_empty() {
                    let n = usize::from(cur.byte()?);
                    if n == 0 {
                        terminated = true;
                        break;
                    }
                    block = cur.take(n)?;
                }
                datum += usize::from(block[0]) << bits;
                bits += 8;
                block = &block[1..];
                continue;
            }
            let mut code = datum & code_mask;
            datum >>= code_size;
            bits -= code_size;
            if code > available || code == end_of_information {
                break;
            }
            if code == clear {
                code_size = data_size + 1;
                code_mask = (1 << code_size) - 1;
                available = clear + 2;
                old_code = None;
                continue;
            }
            let Some(old) = old_code else {
                stack[top] = suffix[code];
                top += 1;
                old_code = Some(code);
                first = code as u8;
                continue;
            };
            let in_code = code;
            if code == available {
                stack[top] = first;
                top += 1;
                code = old;
            }
            while code > clear {
                // A corrupt stream can make a code its own prefix.
                if top >= MAX_CODES {
                    return Err("corrupt GIF image data".to_string());
                }
                stack[top] = suffix[code];
                top += 1;
                code = usize::from(prefix[code]);
            }
            first = suffix[code];
            if available >= MAX_CODES {
                break;
            }
            stack[top] = first;
            top += 1;
            prefix[available] = old as u16;
            suffix[available] = first;
            available += 1;
            if available & code_mask == 0 && available < MAX_CODES {
                code_size += 1;
                code_mask += available;
            }
            old_code = Some(in_code);
        }
        top -= 1;
        pixels.push(stack[top]);
    }
    pixels.resize(npix, 0);
    if !terminated {
        cur.skip_blocks()?;
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lzw_round_trip(indices: &[u8]) {
        let mut data = Vec::new();
        encode_lzw(indices, &mut data);
        let mut cur = Cursor {
            data: &data,
            pos: 0,
        };
        assert_eq!(decode_lzw(&mut cur, indices.len()).unwrap(), indices);
        assert!(cur.at_end());
    }

    #[test]
    fn lzw_round_trips() {
        lzw_round_trip(&[]);
        lzw_round_trip(&[7]);
        lzw_round_trip(&[0; 5000]);
        // Enough distinct pairs to fill the code table several times.
        let mut state = 1u32;
        let noisy: Vec<u8> = (0..60_000)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (state >> 16) as u8
            })
            .collect();
        lzw_round_trip(&noisy);
        let ramp: Vec<u8> = (0..30_000).map(|i| (i % 251) as u8).collect();
        lzw_round_trip(&ramp);
    }

    #[test]
    fn indexes_up_to_256_colors() {
        let r: Vec<u8> = (0..=255).collect();
        let (indices, palette) = index_colors(&r, &r, &r).unwrap();
        assert_eq!(indices, r);
        assert_eq!(palette[200], [200; 3]);
        let mut g = r.clone();
        g.push(1);
        let mut r2 = r.clone();
        r2.push(0);
        assert!(index_colors(&r2, &g, &r2).is_none());
    }
}
