// Applies an Atkinson dither to each RGB channel separately, then recombines them.
//
// Usage: dither_channels [input.png] [--palette NAME|HEX,HEX,...]...
// Writes <name>_dither_red.png, _green.png, _blue.png (1-bit each)
// and <name>_dither.png (the recombined 8-color image) next to the input.
//
// The recombined image has one of 8 colors per pixel, picked by its r, g, b bits.
// --palette swaps those colors for a preset (rgb, riso, gameboy, sepia) or for
// 8 hex colors in the order: black, blue, green, cyan, red, magenta, yellow, white.
// Repeat --palette to write several versions; non-rgb ones get the palette name
// in the file name (<name>_dither_riso.png, or _custom for hex lists).

use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

// Atkinson spreads 1/8 of the error to each of these neighbours (dx, dy),
// so only 6/8 of it is passed on, which keeps highlights and shadows crisp.
const ATKINSON: [(isize, usize); 6] = [(1, 0), (2, 0), (-1, 1), (0, 1), (1, 1), (0, 2)];

// Takes a flat slice of 0..255 values, returns a flat vec of 0s and 255s.
fn atkinson(values: &[u8], width: usize, height: usize) -> Vec<u8> {
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

type Palette = [[u8; 3]; 8];

// Bits of each palette index: r, g, b.
fn index_bits(i: usize) -> [bool; 3] {
    [i & 4 != 0, i & 2 != 0, i & 1 != 0]
}

// Simulates printing with three inks on paper: a channel that's off (dark)
// gets its ink, and overlapping inks multiply like on a risograph.
fn ink_palette(paper: [u8; 3], inks: [[u8; 3]; 3]) -> Palette {
    std::array::from_fn(|i| {
        let mut color = paper.map(|v| v as f32 / 255.0);
        for (on, ink) in index_bits(i).iter().zip(inks) {
            if !on {
                for c in 0..3 {
                    color[c] *= ink[c] as f32 / 255.0;
                }
            }
        }
        color.map(|v| (v * 255.0).round() as u8)
    })
}

// Maps each palette slot's brightness onto a gradient between two colors.
fn ramp_palette(dark: [u8; 3], light: [u8; 3]) -> Palette {
    std::array::from_fn(|i| {
        let [r, g, b] = index_bits(i).map(|on| on as u8 as f32);
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        std::array::from_fn(|c| (dark[c] as f32 + (light[c] as f32 - dark[c] as f32) * luma).round() as u8)
    })
}

fn parse_hex(hex: &str) -> Result<[u8; 3], Box<dyn Error>> {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return Err(format!("expected a 6-digit hex color, got {hex:?}").into());
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16);
    Ok([channel(0)?, channel(2)?, channel(4)?])
}

// Returns the name used in the output file and the colors.
fn parse_palette(arg: &str) -> Result<(String, Palette), Box<dyn Error>> {
    let palette = match arg {
        "rgb" => std::array::from_fn(|i| index_bits(i).map(|on| on as u8 * 255)),
        "riso" => ink_palette([0xf4, 0xef, 0xe4], [[0x00, 0x78, 0xbf], [0xff, 0x48, 0xb0], [0xff, 0xe8, 0x00]]),
        "gameboy" => ramp_palette([0x0f, 0x38, 0x0f], [0x9b, 0xbc, 0x0f]),
        "sepia" => ramp_palette([0x2b, 0x1d, 0x0e], [0xf2, 0xe2, 0xc4]),
        _ if arg.contains(',') => {
            let colors = arg.split(',').map(parse_hex).collect::<Result<Vec<_>, _>>()?;
            let palette = colors.try_into().map_err(|c: Vec<_>| format!("expected 8 colors, got {}", c.len()))?;
            return Ok(("custom".to_string(), palette));
        }
        _ => return Err(format!("unknown palette {arg:?} (try rgb, riso, gameboy, sepia or 8 hex colors)").into()),
    };
    Ok((arg.to_string(), palette))
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

// Writes 0/255 values as a 1-bit grayscale PNG (8 pixels per byte, rows padded).
fn save_1bit(path: &Path, values: &[u8], width: usize, height: usize) -> Result<(), Box<dyn Error>> {
    let stride = width.div_ceil(8);
    let mut packed = vec![0u8; stride * height];
    for y in 0..height {
        for x in 0..width {
            if values[y * width + x] != 0 {
                packed[y * stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    save_png(path, &packed, width, height, png::ColorType::Grayscale, png::BitDepth::One, None)
}

fn main() {
    if let Err(err) = run() {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut input = None;
    let mut palettes = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--palette" {
            palettes.push(parse_palette(&args.next().ok_or("--palette needs a value")?)?);
        } else {
            input = Some(arg);
        }
    }
    if palettes.is_empty() {
        palettes.push(parse_palette("rgb")?);
    }
    let input = input.unwrap_or_else(|| "_DSF6099.png".to_string());
    let input = Path::new(&input);
    let image = image::open(input)?.to_rgb8();
    let (w, h) = (image.width() as usize, image.height() as usize);
    let raw = image.as_raw();
    let stem = input.file_stem().unwrap().to_string_lossy();
    let base = |suffix: &str| -> PathBuf { input.with_file_name(format!("{stem}{suffix}.png")) };

    let mut dithered = Vec::with_capacity(3);
    for (c, name) in ["red", "green", "blue"].iter().enumerate() {
        let values: Vec<u8> = raw.iter().skip(c).step_by(3).copied().collect();
        let result = atkinson(&values, w, h);
        let path = base(&format!("_dither_{name}"));
        save_1bit(&path, &result, w, h)?;
        println!("Wrote {}", path.display());
        dithered.push(result);
    }

    // Only 8 colors are possible, so store them as a 4-bit indexed PNG where
    // the index bits are r, g, b (2 pixels per byte, rows padded).
    let (r, g, b) = (&dithered[0], &dithered[1], &dithered[2]);
    let stride = w.div_ceil(2);
    let mut packed = vec![0u8; stride * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let index = (r[i] & 4) | (g[i] & 2) | (b[i] & 1);
            packed[y * stride + x / 2] |= index << if x % 2 == 0 { 4 } else { 0 };
        }
    }
    for (name, palette) in palettes {
        let path = base(&if name == "rgb" { "_dither".to_string() } else { format!("_dither_{name}") });
        let palette = palette.concat();
        save_png(&path, &packed, w, h, png::ColorType::Indexed, png::BitDepth::Four, Some(palette))?;
        println!("Wrote {}", path.display());
    }
    Ok(())
}
