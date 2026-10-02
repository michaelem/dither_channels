// Regression tests against the sample photo and the outputs it produced with the
// defaults (Atkinson, no serpentine, full size), once with the rgb preset and
// once with the PICO-8 colors as a custom hex palette.
//
// The PNGs are compared decoded (header, palette and packed pixel rows), so a
// different compressor in the png crate doesn't break them, but a changed pixel does.

use dither_channels::{load_rgb, parse_palette, DitherOptions, Dithered, CHANNEL_NAMES};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;

const PICO_HEX: &str = "000000,1d2b53,008751,29adff,ff004d,7e2553,ffec27,fff1e8";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

// A fresh directory per test, so tests running in parallel don't share files.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Decoded {
    width: u32,
    height: u32,
    color: png::ColorType,
    depth: png::BitDepth,
    palette: Option<Vec<u8>>,
    data: Vec<u8>,
}

// Decodes without expanding packed pixels or palettes.
fn decode(path: &Path) -> Decoded {
    let file = File::open(path).unwrap_or_else(|e| panic!("can't open {}: {e}", path.display()));
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder.read_info().unwrap();
    let mut data = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut data).unwrap();
    data.truncate(frame.buffer_size());
    let info = reader.info();
    Decoded {
        width: info.width,
        height: info.height,
        color: info.color_type,
        depth: info.bit_depth,
        palette: info.palette.as_ref().map(|p| p.to_vec()),
        data,
    }
}

fn assert_same_png(actual: &Path, expected: &Path) {
    let (a, e) = (decode(actual), decode(expected));
    let name = expected.file_name().unwrap().to_string_lossy();
    assert_eq!((a.width, a.height), (e.width, e.height), "{name}: size");
    assert_eq!((a.color, a.depth), (e.color, e.depth), "{name}: format");
    assert_eq!(a.palette, e.palette, "{name}: palette");
    // Comparing whole buffers would print megabytes on failure; count instead.
    let differing = a.data.iter().zip(&e.data).filter(|(a, e)| a != e).count();
    assert!(a.data.len() == e.data.len() && differing == 0, "{name}: {differing} bytes of pixels differ");
}

#[test]
fn library_matches_samples() {
    let dir = scratch("library_matches_samples");
    let image = load_rgb(&fixture("_DSF6099.png")).unwrap();
    let dithered = Dithered::from_image(&image, DitherOptions::default());
    for (c, name) in CHANNEL_NAMES.iter().enumerate() {
        let file = format!("_DSF6099_dither_{name}.png");
        dithered.save_channel(c, &dir.join(&file)).unwrap();
        assert_same_png(&dir.join(&file), &fixture(&file));
    }
    for (arg, file) in [("rgb", "_DSF6099_dither.png"), (PICO_HEX, "_DSF6099_dither_custom.png")] {
        let (_, palette) = parse_palette(arg).unwrap();
        dithered.save(&dir.join(file), &palette).unwrap();
        assert_same_png(&dir.join(file), &fixture(file));
    }
}

#[test]
fn cli_matches_samples() {
    let dir = scratch("cli_matches_samples");
    let input = dir.join("_DSF6099.png");
    std::fs::copy(fixture("_DSF6099.png"), &input).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dither_channels"))
        .arg(&input)
        .args(["--palette", "rgb", "--palette", PICO_HEX])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for suffix in ["_red", "_green", "_blue", "", "_custom"] {
        let file = format!("_DSF6099_dither{suffix}.png");
        assert!(stdout.contains(&file), "CLI didn't report writing {file}:\n{stdout}");
        assert_same_png(&dir.join(&file), &fixture(&file));
    }
}

#[test]
fn cli_reports_bad_arguments() {
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_dither_channels")).args(args).output().unwrap();
        assert!(!output.status.success(), "{args:?} should fail");
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    let input = fixture("_DSF6099.png");
    let input = input.to_str().unwrap();
    assert!(run(&[input, "--dither", "nope"]).contains("atkinson"));
    assert!(run(&[input, "--palette", "nope"]).contains("riso"));
    assert!(run(&[input, "--width", "wide"]).contains("number of pixels"));
    assert!(run(&[input, "--width"]).contains("--width needs a value"));
}
