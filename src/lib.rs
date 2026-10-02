// Atkinson dither per RGB channel, plus the palettes that turn the three
// resulting bits per pixel back into colors. Shared by the CLI and the GUI.

use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

pub const CHANNEL_NAMES: [&str; 3] = ["red", "green", "blue"];

// Atkinson spreads 1/8 of the error to each of these neighbours (dx, dy),
// so only 6/8 of it is passed on, which keeps highlights and shadows crisp.
const ATKINSON: [(isize, usize); 6] = [(1, 0), (2, 0), (-1, 1), (0, 1), (1, 1), (0, 2)];

// Takes a flat slice of 0..255 values, returns a flat vec of 0s and 255s.
pub fn atkinson(values: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut buf: Vec<f64> = values.iter().map(|&v| v as f64).collect();
    let mut out = vec![0u8; buf.len()];
    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let i = row + x;
            let old = buf[i];
            let new = if old < 128.0 { 0u8 } else { 255u8 };
            out[i] = new;
            let err = (old - new as f64) / 8.0;
            if err == 0.0 {
                continue;
            }
            for &(dx, dy) in &ATKINSON {
                let nx = x as isize + dx;
                let ny = y + dy;
                if nx < 0 || nx as usize >= width || ny >= height {
                    continue;
                }
                buf[ny * width + nx as usize] += err;
            }
        }
    }
    out
}

// One color per combination of the r, g, b bits, indexed by (r << 2 | g << 1 | b).
pub type Palette = [[u8; 3]; 8];

// Bits of each palette index: r, g, b.
pub fn index_bits(i: usize) -> [bool; 3] {
    [i & 4 != 0, i & 2 != 0, i & 1 != 0]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mix {
    // Like a screen: the colors of the channels that are on add up over the background.
    Light,
    // Like print: the colors of the channels that are off are inks that multiply
    // over the background (paper), so overlapping inks get darker.
    Ink,
}

// Builds a palette from one color per channel.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Channels {
    pub mix: Mix,
    pub colors: [[u8; 3]; 3],
    pub background: [u8; 3],
}

impl Channels {
    fn color(&self, bits: [bool; 3]) -> [u8; 3] {
        let mut color = self.background.map(|v| v as f32 / 255.0);
        for (on, channel) in bits.iter().zip(self.colors) {
            let channel = channel.map(|v| v as f32 / 255.0);
            for c in 0..3 {
                match self.mix {
                    Mix::Light if *on => color[c] += channel[c],
                    Mix::Ink if !on => color[c] *= channel[c],
                    _ => {}
                }
            }
        }
        color.map(|v| (v.min(1.0) * 255.0).round() as u8)
    }

    pub fn palette(&self) -> Palette {
        std::array::from_fn(|i| self.color(index_bits(i)))
    }

    // Palette that only shows one channel, as if the other two had no effect.
    pub fn channel_palette(&self, channel: usize) -> Palette {
        let neutral = self.mix == Mix::Ink;
        std::array::from_fn(|i| {
            let mut bits = [neutral; 3];
            bits[channel] = index_bits(i)[channel];
            self.color(bits)
        })
    }
}

pub enum Preset {
    Channels(Channels),
    Palette(Palette),
}

impl Preset {
    pub fn palette(&self) -> Palette {
        match self {
            Preset::Channels(channels) => channels.palette(),
            Preset::Palette(palette) => *palette,
        }
    }
}

pub const PRESET_NAMES: [&str; 4] = ["rgb", "riso", "gameboy", "sepia"];

pub fn preset(name: &str) -> Option<Preset> {
    Some(match name {
        "rgb" => Preset::Channels(Channels {
            mix: Mix::Light,
            colors: [[255, 0, 0], [0, 255, 0], [0, 0, 255]],
            background: [0, 0, 0],
        }),
        // Teal, fluorescent pink and yellow risograph inks on off-white paper.
        "riso" => Preset::Channels(Channels {
            mix: Mix::Ink,
            colors: [[0x00, 0x78, 0xbf], [0xff, 0x48, 0xb0], [0xff, 0xe8, 0x00]],
            background: [0xf4, 0xef, 0xe4],
        }),
        "gameboy" => Preset::Palette(ramp_palette([0x0f, 0x38, 0x0f], [0x9b, 0xbc, 0x0f])),
        "sepia" => Preset::Palette(ramp_palette([0x2b, 0x1d, 0x0e], [0xf2, 0xe2, 0xc4])),
        _ => return None,
    })
}

// Maps each palette slot's brightness onto a gradient between two colors.
pub fn ramp_palette(dark: [u8; 3], light: [u8; 3]) -> Palette {
    std::array::from_fn(|i| {
        let [r, g, b] = index_bits(i).map(|on| on as u8 as f32);
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        std::array::from_fn(|c| (dark[c] as f32 + (light[c] as f32 - dark[c] as f32) * luma).round() as u8)
    })
}

pub fn parse_hex(hex: &str) -> Result<[u8; 3], Box<dyn Error>> {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return Err(format!("expected a 6-digit hex color, got {hex:?}").into());
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16);
    Ok([channel(0)?, channel(2)?, channel(4)?])
}

// Accepts a preset name or 8 comma-separated hex colors.
// Returns the name used in output file names and the colors.
pub fn parse_palette(arg: &str) -> Result<(String, Palette), Box<dyn Error>> {
    if let Some(preset) = preset(arg) {
        return Ok((arg.to_string(), preset.palette()));
    }
    if !arg.contains(',') {
        let names = PRESET_NAMES.join(", ");
        return Err(format!("unknown palette {arg:?} (try {names} or 8 hex colors)").into());
    }
    let colors = arg.split(',').map(parse_hex).collect::<Result<Vec<_>, _>>()?;
    let palette = colors.try_into().map_err(|c: Vec<_>| format!("expected 8 colors, got {}", c.len()))?;
    Ok(("custom".to_string(), palette))
}

// An image dithered to 1 bit per channel; each channel holds 0s and 255s.
pub struct Dithered {
    pub width: usize,
    pub height: usize,
    pub channels: [Vec<u8>; 3],
}

impl Dithered {
    // Takes flat 8-bit RGB pixels.
    pub fn new(rgb: &[u8], width: usize, height: usize) -> Self {
        let channels = std::array::from_fn(|c| {
            let values: Vec<u8> = rgb.iter().skip(c).step_by(3).copied().collect();
            atkinson(&values, width, height)
        });
        Dithered { width, height, channels }
    }

    pub fn open(path: &Path) -> Result<Self, Box<dyn Error>> {
        let image = image::open(path)?.to_rgb8();
        Ok(Self::new(image.as_raw(), image.width() as usize, image.height() as usize))
    }

    // Palette index (r, g, b bits) of the pixel at flat position i.
    pub fn index(&self, i: usize) -> u8 {
        let [r, g, b] = &self.channels;
        (r[i] & 4) | (g[i] & 2) | (b[i] & 1)
    }

    // Writes one channel as a 1-bit grayscale PNG (8 pixels per byte, rows padded).
    pub fn save_channel(&self, channel: usize, path: &Path) -> Result<(), Box<dyn Error>> {
        let (w, h) = (self.width, self.height);
        let stride = w.div_ceil(8);
        let mut packed = vec![0u8; stride * h];
        for y in 0..h {
            for x in 0..w {
                if self.channels[channel][y * w + x] != 0 {
                    packed[y * stride + x / 8] |= 0x80 >> (x % 8);
                }
            }
        }
        save_png(path, &packed, w, h, png::ColorType::Grayscale, png::BitDepth::One, None)
    }

    // Only 8 colors are possible, so store them as a 4-bit indexed PNG
    // (2 pixels per byte, rows padded).
    pub fn save(&self, path: &Path, palette: &Palette) -> Result<(), Box<dyn Error>> {
        let (w, h) = (self.width, self.height);
        let stride = w.div_ceil(2);
        let mut packed = vec![0u8; stride * h];
        for y in 0..h {
            for x in 0..w {
                let index = self.index(y * w + x);
                packed[y * stride + x / 2] |= index << if x % 2 == 0 { 4 } else { 0 };
            }
        }
        let palette = palette.concat();
        save_png(path, &packed, w, h, png::ColorType::Indexed, png::BitDepth::Four, Some(palette))
    }
}

// Writes an encoded PNG with the given color type, bit depth and optional palette.
fn save_png(
    path: &Path,
    data: &[u8],
    width: usize,
    height: usize,
    color: png::ColorType,
    depth: png::BitDepth,
    palette: Option<Vec<u8>>,
) -> Result<(), Box<dyn Error>> {
    let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), width as u32, height as u32);
    encoder.set_color(color);
    encoder.set_depth(depth);
    // Row filters don't help with packed 1- and 4-bit pixels, they just bloat the output.
    // set_compression also picks a filter, so it has to come first.
    encoder.set_compression(png::Compression::High);
    encoder.set_filter(png::Filter::NoFilter);
    if let Some(palette) = palette {
        encoder.set_palette(palette);
    }
    encoder.write_header()?.write_image_data(data)?;
    Ok(())
}
