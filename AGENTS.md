# AGENTS.md

Dither Channels: dithers each RGB channel of an image to 1 bit separately, then
recombines the three bits per pixel into one of 8 colors. The 8 colors come from
a palette, so the same dither can look like an RGB screen, a risograph print, a
Game Boy, etc. It started as Ruby scripts (`../dither_channels.rb`,
`../split_channels.rb`) and is now a Rust crate with a CLI and an egui GUI.

## Layout

- `src/lib.rs` — everything shared: dithering algorithms, palettes/presets, image
  loading/resizing and PNG output. Put new logic here, not in the binaries.
- `src/main.rs` — `dither_channels` CLI. Hand-rolled argument parsing (no clap);
  the usage text lives in the header comment.
- `src/bin/dither_gui/main.rs` — `dither_gui`, the eframe/egui app: sidebar
  controls, zoomable preview, export.
- `src/bin/dither_gui/menu.rs` — menus described once (`Command`, shortcuts), shown
  as the native macOS menu bar via `muda`, or as an in-window menu elsewhere.
- `macos/build-app.sh` — builds `target/macos/Dither Channels.app` (ad-hoc signed,
  optional `--universal`, `--dmg`). `macos/icon.png` is also embedded as the
  window icon.

## Core model

- **Algorithms** (`Algorithm`, `ALGORITHMS`): error diffusion (Atkinson — the
  default, Floyd–Steinberg, Jarvis–Judice–Ninke, Stucki, Burkes, Sierra, Sierra
  Lite) as `(dx, dy, weight)` kernels + divisor; ordered Bayer 2/4/8; halftone
  (round dots, per-channel print screen angles 15°/75°/0°); random (repeatable
  splitmix64 noise per channel); threshold. `serpentine` only applies to error
  diffusion. Each algorithm has a CLI `name()` and a GUI `label()`.
- **Palette** = `[[u8; 3]; 8]`, indexed by `r << 2 | g << 1 | b`, i.e. order
  black, blue, green, cyan, red, magenta, yellow, white.
- **Channels** builds a palette from one color per channel plus a background:
  `Mix::Light` adds the colors of channels that are on (screen); `Mix::Ink`
  multiplies the colors of channels that are off over the paper (print).
- **Presets** (`PRESET_NAMES`, `preset()`): either `Channels` (rgb, riso*, zx,
  anaglyph) or a fixed `Palette` (cga, pico-4, or `ramp_palette` gradients by
  luminance: gameboy, sepia, thermal). Adding one means updating both
  `PRESET_NAMES` and `preset()`, and the CLI header comment.
- **Output**: channels as 1-bit grayscale PNGs, the result as a 4-bit indexed PNG,
  with no row filters (they only bloat packed pixels).

## GUI notes

- Loading and dithering run on a background thread (`Job`); starting a new job
  replaces the running one. Changing width or algorithm re-dithers; changing
  colors only re-renders the texture.
- Color modes Light / Ink / Palette each keep their own settings when switching.
- Very large images are previewed at every n-th pixel (GPU texture limits);
  nearest filtering when zoomed in, mipmaps when zoomed out.
- Export names look like `photo_dither_halftone_riso_400px.png`.

## Commands

```sh
cargo run --release --bin dither_gui -- [image]
cargo run --release --bin dither_channels -- image.png --width 400 --dither bayer-4 --palette riso --palette gameboy
cargo build --no-default-features      # CLI only, without the GUI dependencies
macos/build-app.sh [--universal] [--dmg]
```

There are no tests; check changes by running the CLI on an image and/or the GUI.
Sample input/output images (`_DSF6099*.png`) sit in the parent directory and are
not part of this repo.

## Style

- Plain `//` comments that explain *why* or describe the visual effect, in short
  sentences; no `///` doc comments. Keep that density.
- Compact, iterator-heavy Rust; errors as `Box<dyn Error>` with readable messages
  that list valid options.
- Keep the CLI and GUI in sync when adding algorithms, presets or options.
