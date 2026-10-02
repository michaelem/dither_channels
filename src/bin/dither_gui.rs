// Interactive preview: open an image, pick a color per channel (or a preset,
// or edit the 8 final colors directly) and export the recombined PNG.
//
// Usage: dither_gui [input.png]   (or open / drop an image in the window)

use dither_channels::{index_bits, preset, Channels, Dithered, Mix, Palette, Preset, PRESET_NAMES};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

fn main() -> eframe::Result {
    let input = std::env::args().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Dither Channels",
        options,
        Box::new(move |cc| {
            let mut app = App::new();
            if let Some(path) = input {
                app.open(path, &cc.egui_ctx);
            }
            Ok(Box::new(app))
        }),
    )
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Combined,
    Channel(usize),
}

struct Loaded {
    path: PathBuf,
    dithered: Dithered,
}

struct App {
    loaded: Option<Loaded>,
    // Dithering runs on a background thread so big photos don't freeze the window.
    loading: Option<(PathBuf, Receiver<Result<Dithered, String>>)>,
    channels: Channels,
    palette: Palette,
    // Name of the preset the palette came from, or "custom" once edited.
    preset_name: String,
    view: View,
    fit: bool,
    // Image pixels per screen pixel.
    zoom: f32,
    texture: Option<egui::TextureHandle>,
    texture_dirty: bool,
    status: String,
}

const CHANNEL_LABELS: [&str; 3] = ["Red", "Green", "Blue"];

impl App {
    fn new() -> Self {
        let mut app = App {
            loaded: None,
            loading: None,
            channels: Channels { mix: Mix::Light, colors: [[0; 3]; 3], background: [0; 3] },
            palette: [[0; 3]; 8],
            preset_name: String::new(),
            view: View::Combined,
            fit: true,
            zoom: 1.0,
            texture: None,
            texture_dirty: true,
            status: String::new(),
        };
        app.apply_preset("rgb");
        app
    }

    fn apply_preset(&mut self, name: &str) {
        match preset(name) {
            Some(Preset::Channels(channels)) => {
                self.channels = channels;
                self.palette = channels.palette();
            }
            Some(Preset::Palette(palette)) => self.palette = palette,
            None => return,
        }
        self.preset_name = name.to_string();
        self.texture_dirty = true;
    }

    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let thread_path = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Dithered::open(&thread_path).map_err(|e| e.to_string()));
            ctx.request_repaint();
        });
        self.status = format!("Dithering {}…", file_name(&path));
        self.loading = Some((path, rx));
    }

    fn poll_loading(&mut self) {
        let Some((path, rx)) = &self.loading else { return };
        let Ok(result) = rx.try_recv() else { return };
        match result {
            Ok(dithered) => {
                self.status = format!("{} · {}×{}", file_name(path), dithered.width, dithered.height);
                self.loaded = Some(Loaded { path: path.clone(), dithered });
                self.texture_dirty = true;
            }
            Err(err) => self.status = format!("Couldn't open {}: {err}", file_name(path)),
        }
        self.loading = None;
    }

    fn update_texture(&mut self, ctx: &egui::Context) {
        if !self.texture_dirty {
            return;
        }
        self.texture_dirty = false;
        let Some(loaded) = &self.loaded else { return };
        let d = &loaded.dithered;
        let palette = match self.view {
            View::Combined => self.palette,
            View::Channel(c) => self.channels.channel_palette(c),
        };
        // GPUs cap texture size, so very large images get previewed at every n-th pixel.
        let max_side = ctx.input(|i| i.max_texture_side);
        let step = d.width.max(d.height).div_ceil(max_side).max(1);
        let (w, h) = (d.width.div_ceil(step), d.height.div_ceil(step));
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                rgb.extend_from_slice(&palette[d.index(y * step * d.width + x * step) as usize]);
            }
        }
        let image = egui::ColorImage::from_rgb([w, h], &rgb);
        // Nearest when zoomed in keeps dither dots sharp; mipmaps when zoomed out
        // average them, which is closer to how the dither reads at a distance.
        let options = egui::TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Linear,
            mipmap_mode: Some(egui::TextureFilter::Linear),
            ..Default::default()
        };
        match &mut self.texture {
            Some(texture) => texture.set(image, options),
            None => self.texture = Some(ctx.load_texture("preview", image, options)),
        }
    }

    fn export(&mut self) {
        let Some(loaded) = &self.loaded else { return };
        let stem = loaded.path.file_stem().unwrap_or_default().to_string_lossy();
        let suffix = if self.preset_name == "rgb" { String::new() } else { format!("_{}", self.preset_name) };
        let mut dialog = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .set_file_name(format!("{stem}_dither{suffix}.png"));
        if let Some(dir) = loaded.path.parent() {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.save_file() else { return };
        self.status = match loaded.dithered.save(&path, &self.palette) {
            Ok(()) => format!("Saved {}", file_name(&path)),
            Err(err) => format!("Couldn't save: {err}"),
        };
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        if ui.button("Open image…").clicked() {
            if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg"]).pick_file() {
                self.open(path, ui.ctx());
            }
        }
        ui.label(egui::RichText::new(&self.status).small().weak());

        ui.separator();
        ui.heading("Colors");
        egui::ComboBox::from_label("Preset").selected_text(&self.preset_name).show_ui(ui, |ui| {
            for name in PRESET_NAMES {
                if ui.selectable_label(self.preset_name == name, name).clicked() {
                    self.apply_preset(name);
                }
            }
        });

        let before = self.channels;
        ui.horizontal(|ui| {
            ui.label("Mix");
            ui.selectable_value(&mut self.channels.mix, Mix::Light, "Light")
                .on_hover_text("Channels that are on add their color, like a screen");
            ui.selectable_value(&mut self.channels.mix, Mix::Ink, "Ink")
                .on_hover_text("Channels that are off print their color as ink, like a risograph");
        });
        egui::Grid::new("channels").num_columns(2).show(ui, |ui| {
            let background = if self.channels.mix == Mix::Ink { "Paper" } else { "Background" };
            ui.label(background);
            ui.color_edit_button_srgb(&mut self.channels.background);
            ui.end_row();
            for (c, label) in CHANNEL_LABELS.iter().enumerate() {
                ui.label(format!("{label} channel"));
                ui.color_edit_button_srgb(&mut self.channels.colors[c]);
                ui.end_row();
            }
        });
        if self.channels != before {
            self.palette = self.channels.palette();
            self.preset_name = "custom".to_string();
            self.texture_dirty = true;
        }

        ui.add_space(4.0);
        ui.label("Resulting colors (channels that are on)").on_hover_text("Click one to override it directly");
        egui::Grid::new("palette").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
            for i in 0..8 {
                ui.vertical_centered(|ui| {
                    if ui.color_edit_button_srgb(&mut self.palette[i]).changed() {
                        self.preset_name = "custom".to_string();
                        self.texture_dirty = true;
                    }
                    ui.label(egui::RichText::new(slot_label(i)).small().weak());
                });
                if i % 4 == 3 {
                    ui.end_row();
                }
            }
        });

        ui.separator();
        ui.heading("Preview");
        ui.horizontal(|ui| {
            let before = self.view;
            ui.selectable_value(&mut self.view, View::Combined, "All");
            for (c, label) in CHANNEL_LABELS.iter().enumerate() {
                ui.selectable_value(&mut self.view, View::Channel(c), *label);
            }
            self.texture_dirty |= self.view != before;
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.fit, "Fit");
            ui.add_enabled(
                !self.fit,
                egui::Slider::new(&mut self.zoom, 0.25..=8.0).logarithmic(true).suffix("×"),
            );
        });
        if ui.add_enabled(!self.fit, egui::Button::new("Actual pixels")).clicked() {
            self.zoom = 1.0;
        }

        ui.separator();
        if ui.add_enabled(self.loaded.is_some(), egui::Button::new("Export PNG…")).clicked() {
            self.export();
        }
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        if self.loading.is_some() {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        }
        let (Some(texture), Some(_)) = (&self.texture, &self.loaded) else {
            ui.centered_and_justified(|ui| ui.label("Open an image or drop one here"));
            return;
        };
        // Sizes are in points; on a Retina screen one point is two pixels.
        let pixels_per_point = ui.ctx().pixels_per_point();
        let image_size = texture.size_vec2() / pixels_per_point;
        if self.fit {
            let available = ui.available_size();
            self.zoom = (available.x / image_size.x).min(available.y / image_size.y);
        }
        let size = image_size * self.zoom;
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            ui.add(egui::Image::new(egui::load::SizedTexture::new(texture.id(), size)));
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_loading();
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.open(path, ui.ctx());
        }

        egui::Panel::left("controls").resizable(false).default_size(260.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.sidebar(ui));
        });
        self.update_texture(ui.ctx());
        egui::CentralPanel::default_margins().show(ui, |ui| self.preview(ui));
    }
}

// Which channels are on for palette slot i, e.g. "R+B".
fn slot_label(i: usize) -> String {
    let on: Vec<&str> = index_bits(i).iter().zip(["R", "G", "B"]).filter(|(on, _)| **on).map(|(_, l)| l).collect();
    if on.is_empty() { "none".to_string() } else { on.join("+") }
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}
