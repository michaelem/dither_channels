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
//
// For an interactive version with a preview, see the dither_gui binary.

use dither_channels::{parse_palette, Dithered, CHANNEL_NAMES};
use std::error::Error;
use std::path::{Path, PathBuf};

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
    let stem = input.file_stem().unwrap().to_string_lossy();
    let base = |suffix: &str| -> PathBuf { input.with_file_name(format!("{stem}{suffix}.png")) };

    let dithered = Dithered::open(input)?;
    for (c, name) in CHANNEL_NAMES.iter().enumerate() {
        let path = base(&format!("_dither_{name}"));
        dithered.save_channel(c, &path)?;
        println!("Wrote {}", path.display());
    }
    for (name, palette) in palettes {
        let path = base(&if name == "rgb" { "_dither".to_string() } else { format!("_dither_{name}") });
        dithered.save(&path, &palette)?;
        println!("Wrote {}", path.display());
    }
    Ok(())
}
