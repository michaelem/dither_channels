// Dithering per RGB channel, plus the palettes that turn the three
// resulting bits per pixel back into colors. Shared by the CLI and the GUI.

use image::RgbImage;
use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

pub const CHANNEL_NAMES: [&str; 3] = ["red", "green", "blue"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Algorithm {
    // Error diffusion: round each pixel to black or white and push the
    // rounding error onto neighbours that haven't been visited yet.
    Atkinson,
    FloydSteinberg,
    JarvisJudiceNinke,
    Stucki,
    Burkes,
    Sierra,
    SierraLite,
    // Ordered: compare against a repeating threshold pattern (2, 4 or 8 pixels square).
    Bayer(usize),
    // Round dots on a rotated grid, like print; every channel gets its own screen angle.
    Halftone,
    // Compare against noise.
    Random,
    // Plain rounding, for comparison.
    Threshold,
}

pub const ALGORITHMS: [Algorithm; 13] = [
    Algorithm::Atkinson,
    Algorithm::FloydSteinberg,
    Algorithm::JarvisJudiceNinke,
    Algorithm::Stucki,
    Algorithm::Burkes,
    Algorithm::Sierra,
    Algorithm::SierraLite,
    Algorithm::Bayer(2),
    Algorithm::Bayer(4),
    Algorithm::Bayer(8),
    Algorithm::Halftone,
    Algorithm::Random,
    Algorithm::Threshold,
];

// Error diffusion kernels: (dx, dy, weight) of each neighbour, and what the weights are divided by.
// Atkinson's weights only add up to 6/8, so it loses some error, which keeps highlights and shadows crisp.
const ATKINSON: (&[(isize, usize, f64)], f64) = (&[(1, 0, 1.), (2, 0, 1.), (-1, 1, 1.), (0, 1, 1.), (1, 1, 1.), (0, 2, 1.)], 8.);
const FLOYD_STEINBERG: (&[(isize, usize, f64)], f64) = (&[(1, 0, 7.), (-1, 1, 3.), (0, 1, 5.), (1, 1, 1.)], 16.);
const JARVIS_JUDICE_NINKE: (&[(isize, usize, f64)], f64) = (
    &[
        (1, 0, 7.), (2, 0, 5.),
        (-2, 1, 3.), (-1, 1, 5.), (0, 1, 7.), (1, 1, 5.), (2, 1, 3.),
        (-2, 2, 1.), (-1, 2, 3.), (0, 2, 5.), (1, 2, 3.), (2, 2, 1.),
    ],
    48.,
);
const STUCKI: (&[(isize, usize, f64)], f64) = (
    &[
        (1, 0, 8.), (2, 0, 4.),
        (-2, 1, 2.), (-1, 1, 4.), (0, 1, 8.), (1, 1, 4.), (2, 1, 2.),
        (-2, 2, 1.), (-1, 2, 2.), (0, 2, 4.), (1, 2, 2.), (2, 2, 1.),
    ],
    42.,
);
const BURKES: (&[(isize, usize, f64)], f64) = (
    &[(1, 0, 8.), (2, 0, 4.), (-2, 1, 2.), (-1, 1, 4.), (0, 1, 8.), (1, 1, 4.), (2, 1, 2.)],
    32.,
);
const SIERRA: (&[(isize, usize, f64)], f64) = (
    &[
        (1, 0, 5.), (2, 0, 3.),
        (-2, 1, 2.), (-1, 1, 4.), (0, 1, 5.), (1, 1, 4.), (2, 1, 2.),
        (-1, 2, 2.), (0, 2, 3.), (1, 2, 2.),
    ],
    32.,
);
const SIERRA_LITE: (&[(isize, usize, f64)], f64) = (&[(1, 0, 2.), (-1, 1, 1.), (0, 1, 1.)], 4.);

// Halftone screen angles per channel, as in print: red is covered by cyan ink
// (15°), green by magenta (75°), blue by yellow (0°). Cells are this many pixels apart.
const HALFTONE_ANGLES: [f64; 3] = [15.0, 75.0, 0.0];
const HALFTONE_CELL: f64 = 6.0;

impl Algorithm {
    // Short name, as used by the CLI's --dither option.
    pub fn name(&self) -> String {
        match self {
            Algorithm::Atkinson => "atkinson".into(),
            Algorithm::FloydSteinberg => "floyd-steinberg".into(),
            Algorithm::JarvisJudiceNinke => "jarvis".into(),
            Algorithm::Stucki => "stucki".into(),
            Algorithm::Burkes => "burkes".into(),
            Algorithm::Sierra => "sierra".into(),
            Algorithm::SierraLite => "sierra-lite".into(),
            Algorithm::Bayer(n) => format!("bayer-{n}"),
            Algorithm::Halftone => "halftone".into(),
            Algorithm::Random => "random".into(),
            Algorithm::Threshold => "threshold".into(),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Algorithm::Atkinson => "Atkinson".into(),
            Algorithm::FloydSteinberg => "Floyd–Steinberg".into(),
            Algorithm::JarvisJudiceNinke => "Jarvis–Judice–Ninke".into(),
            Algorithm::Stucki => "Stucki".into(),
            Algorithm::Burkes => "Burkes".into(),
            Algorithm::Sierra => "Sierra".into(),
            Algorithm::SierraLite => "Sierra Lite".into(),
            Algorithm::Bayer(n) => format!("Bayer {n}×{n}"),
            Algorithm::Halftone => "Halftone".into(),
            Algorithm::Random => "Random".into(),
            Algorithm::Threshold => "Threshold".into(),
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        ALGORITHMS.into_iter().find(|a| a.name() == name)
    }

    fn kernel(&self) -> Option<(&'static [(isize, usize, f64)], f64)> {
        Some(match self {
            Algorithm::Atkinson => ATKINSON,
            Algorithm::FloydSteinberg => FLOYD_STEINBERG,
            Algorithm::JarvisJudiceNinke => JARVIS_JUDICE_NINKE,
            Algorithm::Stucki => STUCKI,
            Algorithm::Burkes => BURKES,
            Algorithm::Sierra => SIERRA,
            Algorithm::SierraLite => SIERRA_LITE,
            _ => return None,
        })
    }

    // Whether serpentine scanning applies (only error diffusion has a scan direction).
    pub fn is_error_diffusion(&self) -> bool {
        self.kernel().is_some()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DitherOptions {
    pub algorithm: Algorithm,
    // Error diffusion only: scan every other row right to left, which breaks up
    // the diagonal "worm" patterns that always going left to right leaves.
    pub serpentine: bool,
}

impl Default for DitherOptions {
    fn default() -> Self {
        DitherOptions { algorithm: Algorithm::Atkinson, serpentine: false }
    }
}

// Takes a flat slice of 0..255 values of one channel (0 = red, 1 = green, 2 = blue),
// returns a flat vec of 0s and 255s.
pub fn dither(values: &[u8], width: usize, height: usize, channel: usize, options: DitherOptions) -> Vec<u8> {
    let bit = |on: bool| if on { 255u8 } else { 0u8 };
    let pixels = (0..height).flat_map(|y| (0..width).map(move |x| (x, y)));
    match options.algorithm {
        Algorithm::Threshold => values.iter().map(|&v| bit(v >= 128)).collect(),
        Algorithm::Random => pixels
            .zip(values)
            .map(|((x, y), &v)| bit(v as u64 > noise(x, y, channel) % 256))
            .collect(),
        Algorithm::Bayer(n) => {
            let matrix = bayer_matrix(n);
            pixels
                .zip(values)
                .map(|((x, y), &v)| {
                    let threshold = (matrix[(y % n) * n + x % n] as f64 + 0.5) / (n * n) as f64 * 255.0;
                    bit(v as f64 > threshold)
                })
                .collect()
        }
        Algorithm::Halftone => {
            let (sin, cos) = HALFTONE_ANGLES[channel].to_radians().sin_cos();
            pixels
                .zip(values)
                .map(|((x, y), &v)| {
                    // Position inside the rotated cell, from -1 to 1 on both axes.
                    let (x, y) = (x as f64 + 0.5, y as f64 + 0.5);
                    let u = (x * cos + y * sin) / HALFTONE_CELL;
                    let w = (-x * sin + y * cos) / HALFTONE_CELL;
                    let (u, w) = ((u - u.floor()) * 2.0 - 1.0, (w - w.floor()) * 2.0 - 1.0);
                    // Classic round-dot spot function, from 1 in the middle of a cell to 0 at
                    // its corners: dark dots grow from the corners as the value drops, touch
                    // in a checkerboard at mid grey, and leave light dots in the middle.
                    let pi = std::f64::consts::PI;
                    let spot = ((pi * u).cos() + (pi * w).cos() + 2.0) / 4.0;
                    bit(v as f64 / 255.0 > 1.0 - spot)
                })
                .collect()
        }
        algorithm => {
            let (kernel, divisor) = algorithm.kernel().unwrap();
            error_diffusion(values, width, height, kernel, divisor, options.serpentine)
        }
    }
}

fn error_diffusion(
    values: &[u8],
    width: usize,
    height: usize,
    kernel: &[(isize, usize, f64)],
    divisor: f64,
    serpentine: bool,
) -> Vec<u8> {
    let mut buf: Vec<f64> = values.iter().map(|&v| v as f64).collect();
    let mut out = vec![0u8; buf.len()];
    for y in 0..height {
        let row = y * width;
        // Going right to left mirrors the kernel too.
        let reverse = serpentine && y % 2 == 1;
        for step in 0..width {
            let x = if reverse { width - 1 - step } else { step };
            let i = row + x;
            let old = buf[i];
            let new = if old < 128.0 { 0u8 } else { 255u8 };
            out[i] = new;
            let err = (old - new as f64) / divisor;
            if err == 0.0 {
                continue;
            }
            for &(dx, dy, weight) in kernel {
                let nx = if reverse { x as isize - dx } else { x as isize + dx };
                let ny = y + dy;
                if nx < 0 || nx as usize >= width || ny >= height {
                    continue;
                }
                buf[ny * width + nx as usize] += err * weight;
            }
        }
    }
    out
}

// Bayer matrix of size n (a power of two), as threshold ranks 0..n*n.
fn bayer_matrix(n: usize) -> Vec<usize> {
    let mut matrix = vec![0];
    let mut size = 1;
    while size < n {
        let mut next = vec![0; size * size * 4];
        for y in 0..size {
            for x in 0..size {
                let v = matrix[y * size + x] * 4;
                next[y * size * 2 + x] = v;
                next[y * size * 2 + x + size] = v + 2;
                next[(y + size) * size * 2 + x] = v + 3;
                next[(y + size) * size * 2 + x + size] = v + 1;
            }
        }
        matrix = next;
        size *= 2;
    }
    matrix
}

// Repeatable per-pixel noise, different for every channel (splitmix64).
fn noise(x: usize, y: usize, channel: usize) -> u64 {
    let mut z = (x as u64) << 32 ^ (y as u64) << 2 ^ channel as u64;
    z = z.wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
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

pub const PRESET_NAMES: [&str; 11] = [
    "rgb", "riso", "riso-sunrise", "riso-mint", "gameboy", "sepia", "pico-4", "zx", "cga", "anaglyph", "thermal",
];

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
        // Riso federal blue, bright red and sunflower inks on cream paper.
        "riso-sunrise" => Preset::Channels(Channels {
            mix: Mix::Ink,
            colors: [[0x3d, 0x55, 0x88], [0xf1, 0x50, 0x60], [0xff, 0xb5, 0x11]],
            background: [0xf6, 0xef, 0xdc],
        }),
        // Riso mint, purple and orange inks; their overlaps make browns and plums.
        "riso-mint" => Preset::Channels(Channels {
            mix: Mix::Ink,
            colors: [[0x82, 0xd8, 0xd5], [0x76, 0x5b, 0xa7], [0xff, 0x6c, 0x2f]],
            background: [0xf4, 0xef, 0xe4],
        }),
        "gameboy" => Preset::Palette(ramp_palette(&[[0x0f, 0x38, 0x0f], [0x9b, 0xbc, 0x0f]])),
        "sepia" => Preset::Palette(ramp_palette(&[[0x2b, 0x1d, 0x0e], [0xf2, 0xe2, 0xc4]])),
        // The PICO-8 colors closest to each slot's black, blue, green, ... white.
        "pico-4" => Preset::Palette([
            [0x00, 0x00, 0x00],
            [0x1d, 0x2b, 0x53],
            [0x00, 0x87, 0x51],
            [0x29, 0xad, 0xff],
            [0xff, 0x00, 0x4d],
            [0x7e, 0x25, 0x53],
            [0xff, 0xec, 0x27],
            [0xff, 0xf1, 0xe8],
        ]),
        // The ZX Spectrum's (non-bright) colors were built from these same bits.
        "zx" => Preset::Channels(Channels {
            mix: Mix::Light,
            colors: [[0xd7, 0, 0], [0, 0xd7, 0], [0, 0, 0xd7]],
            background: [0, 0, 0],
        }),
        // IBM CGA's low-intensity colors, also indexed by r, g, b bits,
        // except that the monitor turned dark yellow into brown.
        "cga" => Preset::Palette([
            [0x00, 0x00, 0x00],
            [0x00, 0x00, 0xaa],
            [0x00, 0xaa, 0x00],
            [0x00, 0xaa, 0xaa],
            [0xaa, 0x00, 0x00],
            [0xaa, 0x00, 0xaa],
            [0xaa, 0x55, 0x00],
            [0xaa, 0xaa, 0xaa],
        ]),
        // Red/cyan 3D-glasses look: red stays red, green and blue both add cyan
        // (weighted by how bright they look, together making full cyan).
        "anaglyph" => Preset::Channels(Channels {
            mix: Mix::Light,
            colors: [[0xff, 0, 0], [0, 0xb4, 0xb4], [0, 0x4b, 0x4b]],
            background: [0, 0, 0],
        }),
        // Infrared-camera gradient from black through purple and red to pale yellow.
        "thermal" => Preset::Palette(ramp_palette(&[
            [0x00, 0x00, 0x00],
            [0x2d, 0x0b, 0x59],
            [0x8c, 0x1d, 0x82],
            [0xe3, 0x46, 0x2a],
            [0xfc, 0xa6, 0x36],
            [0xff, 0xf6, 0xb0],
        ])),
        _ => return None,
    })
}

// Maps each palette slot's brightness onto a gradient through evenly spaced
// color stops, darkest first. Needs at least two stops.
pub fn ramp_palette(stops: &[[u8; 3]]) -> Palette {
    std::array::from_fn(|i| {
        let [r, g, b] = index_bits(i).map(|on| on as u8 as f32);
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let position = luma * (stops.len() - 1) as f32;
        let segment = (position.floor() as usize).min(stops.len() - 2);
        let t = position - segment as f32;
        let (dark, light) = (stops[segment], stops[segment + 1]);
        std::array::from_fn(|c| (dark[c] as f32 + (light[c] as f32 - dark[c] as f32) * t).round() as u8)
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

pub fn load_rgb(path: &Path) -> Result<RgbImage, Box<dyn Error>> {
    Ok(image::open(path)?.to_rgb8())
}

// Scales to the given width, keeping the aspect ratio. Dithering a smaller image
// makes the dots bigger relative to the picture.
pub fn resize_to_width(image: &RgbImage, width: u32) -> RgbImage {
    if width == image.width() {
        return image.clone();
    }
    let height = height_for_width(image, width);
    image::imageops::resize(image, width.max(1), height, image::imageops::FilterType::Lanczos3)
}

pub fn height_for_width(image: &RgbImage, width: u32) -> u32 {
    (image.height() as f64 * width as f64 / image.width() as f64).round().max(1.0) as u32
}

// An image dithered to 1 bit per channel; each channel holds 0s and 255s.
pub struct Dithered {
    pub width: usize,
    pub height: usize,
    pub channels: [Vec<u8>; 3],
}

impl Dithered {
    // Takes flat 8-bit RGB pixels.
    pub fn new(rgb: &[u8], width: usize, height: usize, options: DitherOptions) -> Self {
        let channels = std::array::from_fn(|c| {
            let values: Vec<u8> = rgb.iter().skip(c).step_by(3).copied().collect();
            dither(&values, width, height, c, options)
        });
        Dithered { width, height, channels }
    }

    pub fn from_image(image: &RgbImage, options: DitherOptions) -> Self {
        Self::new(image.as_raw(), image.width() as usize, image.height() as usize, options)
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
