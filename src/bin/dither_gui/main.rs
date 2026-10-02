// Interactive preview: open an image, scale it down, pick a color per channel
// (or a preset, or edit the 8 final colors directly) and export the recombined PNG.
//
// Usage: dither_gui [input.png]   (or open / drop an image in the window)

use dither_channels::{
    height_for_width, index_bits, load_rgb, preset, resize_to_width, Channels, Dithered, Mix, Palette, Preset,
    Algorithm, DitherOptions, ALGORITHMS, CHANNEL_NAMES, PRESET_NAMES,
};
use eframe::egui;
use image::RgbImage;
use menu::{Command, State};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

mod menu;

fn main() -> eframe::Result {
    let input = std::env::args().nth(1).map(PathBuf::from);
    // eframe sets the Dock/taskbar icon at runtime (egui's logo unless told otherwise),
    // which would also override the icon of the macOS app bundle.
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../macos/icon.png")).expect("icon is a valid PNG");
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]).with_icon(icon);
    // A Mac-style sidebar: the content goes up under a transparent title bar, so the
    // sidebar runs the full height with the window buttons on top of it, and the
    // window is see-through so the sidebar material can show behind it.
    if NATIVE_SIDEBAR {
        viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_transparent(true);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "Dither Channels",
        options,
        Box::new(move |cc| {
            let mut app = App::new();
            if let Some(path) = input {
                app.open(path, &cc.egui_ctx);
            }
            #[cfg(target_os = "macos")]
            {
                add_sidebar_material(cc);
                use_system_font(&cc.egui_ctx);
                let native_menu = menu::NativeMenu::new(&cc.egui_ctx, |command| app.menu_state(command));
                app.native_menu = Some(native_menu);
            }
            Ok(Box::new(app))
        }),
    )
}

// How the 8 colors are chosen. Light and Ink derive them from one color per
// channel (see Mix); Palette sets all 8 directly.
#[derive(Clone, Copy, PartialEq)]
enum ColorMode {
    Light,
    Ink,
    Palette,
}

const COLOR_MODES: [(ColorMode, &str, &str); 3] = [
    (ColorMode::Light, "Light", "Channels that are on add their color, like a screen"),
    (ColorMode::Ink, "Ink", "Channels that are off print their color as ink, like a risograph"),
    (ColorMode::Palette, "Palette", "Pick all 8 colors directly"),
];

fn preset_mode(preset: &Preset) -> ColorMode {
    match preset {
        Preset::Channels(Channels { mix: Mix::Light, .. }) => ColorMode::Light,
        Preset::Channels(Channels { mix: Mix::Ink, .. }) => ColorMode::Ink,
        Preset::Palette(_) => ColorMode::Palette,
    }
}

// The color settings of one mode, kept so switching modes back and forth keeps edits.
struct ColorState {
    channels: Channels,
    palette: Palette,
    preset_name: String,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum View {
    Combined,
    Channel(usize),
}

struct Loaded {
    path: PathBuf,
    // The image as opened, kept so it can be resized and dithered again.
    source: Arc<RgbImage>,
    dithered: Dithered,
    // The settings `dithered` was made with.
    options: DitherOptions,
}

// Loading and dithering run on a background thread so big photos don't freeze the window.
struct Job {
    path: PathBuf,
    // Opening a new image (as opposed to re-dithering the current one at a new size).
    opening: bool,
    rx: Receiver<Result<Loaded, String>>,
}

struct App {
    loaded: Option<Loaded>,
    job: Option<Job>,
    // Width to dither at; whenever it differs from the current result, it's dithered again.
    width: u32,
    // Algorithm to dither with; like `width`, a change triggers dithering again.
    options: DitherOptions,
    mode: ColorMode,
    // Settings of the modes that aren't shown, indexed by ColorMode.
    stashed: [Option<ColorState>; 3],
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
    #[cfg(target_os = "macos")]
    native_menu: Option<menu::NativeMenu>,
}

const CHANNEL_LABELS: [&str; 3] = ["Red", "Green", "Blue"];

// On macOS the sidebar is translucent and starts under the title bar.
const NATIVE_SIDEBAR: bool = cfg!(target_os = "macos");
// Height of the macOS title bar, which the content now extends under.
const TITLE_BAR_HEIGHT: f32 = 28.0;

impl App {
    fn new() -> Self {
        let mut app = App {
            loaded: None,
            job: None,
            width: 0,
            options: DitherOptions::default(),
            mode: ColorMode::Light,
            stashed: [None, None, None],
            channels: Channels { mix: Mix::Light, colors: [[0; 3]; 3], background: [0; 3] },
            palette: [[0; 3]; 8],
            preset_name: String::new(),
            view: View::Combined,
            fit: true,
            zoom: 1.0,
            texture: None,
            texture_dirty: true,
            status: String::new(),
            #[cfg(target_os = "macos")]
            native_menu: None,
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

    fn set_mode(&mut self, mode: ColorMode) {
        if mode == self.mode {
            return;
        }
        let current = ColorState { channels: self.channels, palette: self.palette, preset_name: self.preset_name.clone() };
        self.stashed[self.mode as usize] = Some(current);
        self.mode = mode;
        match self.stashed[mode as usize].take() {
            Some(state) => {
                self.channels = state.channels;
                self.palette = state.palette;
                self.preset_name = state.preset_name;
                self.texture_dirty = true;
            }
            None => {
                // First visit: start from the mode's first preset.
                let name = PRESET_NAMES.iter().find(|name| preset(name).is_some_and(|p| preset_mode(&p) == mode));
                self.apply_preset(name.expect("every mode has a preset"));
            }
        }
    }

    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        let thread_path = path.clone();
        let options = self.options;
        self.status = format!("Opening {}…", file_name(&path));
        self.spawn(path, true, ctx, move || {
            let source = Arc::new(load_rgb(&thread_path).map_err(|e| e.to_string())?);
            let dithered = Dithered::from_image(&source, options);
            Ok(Loaded { path: thread_path, source, dithered, options })
        });
    }

    fn redither(&mut self, ctx: &egui::Context) {
        let Some(loaded) = &self.loaded else { return };
        let (path, source, width, options) = (loaded.path.clone(), loaded.source.clone(), self.width, self.options);
        self.spawn(path.clone(), false, ctx, move || {
            let dithered = Dithered::from_image(&resize_to_width(&source, width), options);
            Ok(Loaded { path, source, dithered, options })
        });
    }

    // Starting a job replaces any running one; the old result is then dropped.
    fn spawn(
        &mut self,
        path: PathBuf,
        opening: bool,
        ctx: &egui::Context,
        work: impl FnOnce() -> Result<Loaded, String> + Send + 'static,
    ) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
        self.job = Some(Job { path, opening, rx });
    }

    fn poll_job(&mut self) {
        let Some(job) = &self.job else { return };
        let Ok(result) = job.rx.try_recv() else { return };
        match result {
            Ok(loaded) => {
                if job.opening {
                    self.width = loaded.source.width();
                }
                let (source, d) = (&loaded.source, &loaded.dithered);
                self.status = format!("{} · {}×{}", file_name(&loaded.path), source.width(), source.height());
                if d.width as u32 != source.width() {
                    self.status += &format!(" → {}×{}", d.width, d.height);
                }
                self.loaded = Some(loaded);
                self.texture_dirty = true;
            }
            Err(err) => self.status = format!("Couldn't open {}: {err}", file_name(&job.path)),
        }
        self.job = None;
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
            // Palette mode has no channel colors, so show the channel in black and white.
            View::Channel(c) if self.mode == ColorMode::Palette => {
                std::array::from_fn(|i| if index_bits(i)[c] { [255; 3] } else { [0; 3] })
            }
            View::Channel(c) => self.channels.channel_palette(c),
        };
        // GPUs cap texture size, so very large images get previewed at every n-th pixel.
        let max_side = ctx.input(|i| i.max_texture_side);
        let step = d.width.max(d.height).div_ceil(max_side).max(1);
        let image = render(d, &palette, step);
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

    fn open_dialog(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg"]).pick_file() {
            self.open(path, ctx);
        }
    }

    // Output file name without extension, e.g. "photo_dither_halftone_riso_400px".
    // `part` replaces the preset name (used for the single channels).
    fn output_stem(loaded: &Loaded, part: &str) -> String {
        let stem = loaded.path.file_stem().unwrap_or_default().to_string_lossy();
        let mut name = format!("{stem}_dither");
        let options = loaded.options;
        if options != DitherOptions::default() {
            name += &format!("_{}", options.algorithm.name());
            if options.serpentine && options.algorithm.is_error_diffusion() {
                name += "-serpentine";
            }
        }
        if !part.is_empty() && part != "rgb" {
            name += &format!("_{part}");
        }
        if loaded.dithered.width as u32 != loaded.source.width() {
            name += &format!("_{}px", loaded.dithered.width);
        }
        name
    }

    fn export(&mut self) {
        let Some(loaded) = &self.loaded else { return };
        let mut dialog = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .set_file_name(format!("{}.png", Self::output_stem(loaded, &self.preset_name)));
        if let Some(dir) = loaded.path.parent() {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.save_file() else { return };
        self.status = match loaded.dithered.save(&path, &self.palette) {
            Ok(()) => format!("Saved {}", file_name(&path)),
            Err(err) => format!("Couldn't save: {err}"),
        };
    }

    // Saves each channel as a 1-bit black-and-white PNG into a chosen folder, like the CLI does.
    fn export_channels(&mut self) {
        let Some(loaded) = &self.loaded else { return };
        let mut dialog = rfd::FileDialog::new().set_title("Choose a folder for the channel images");
        if let Some(dir) = loaded.path.parent() {
            dialog = dialog.set_directory(dir);
        }
        let Some(dir) = dialog.pick_folder() else { return };
        let result = CHANNEL_NAMES.iter().enumerate().try_for_each(|(c, name)| {
            loaded.dithered.save_channel(c, &dir.join(format!("{}.png", Self::output_stem(loaded, name))))
        });
        self.status = match result {
            Ok(()) => format!("Saved red, green and blue channels to {}", file_name(&dir)),
            Err(err) => format!("Couldn't save: {err}"),
        };
    }

    // Copies the full-size result with the current colors, as Export would save it.
    fn copy_image(&mut self, ctx: &egui::Context) {
        let Some(loaded) = &self.loaded else { return };
        ctx.copy_image(render(&loaded.dithered, &self.palette, 1));
        self.status = "Copied image".to_string();
    }

    fn menu_state(&self, command: Command) -> State {
        let has_image = self.loaded.is_some();
        let (enabled, checked) = match command {
            Command::Open => (true, None),
            Command::Export | Command::ExportChannels | Command::CopyImage => (has_image, None),
            Command::View(view) => (true, Some(self.view == view)),
            Command::ZoomToFit => (has_image, Some(self.fit)),
            Command::ActualPixels | Command::ZoomIn | Command::ZoomOut => (has_image, None),
        };
        State { enabled, checked }
    }

    fn run(&mut self, command: Command, ctx: &egui::Context) {
        match command {
            Command::Open => self.open_dialog(ctx),
            Command::Export => self.export(),
            Command::ExportChannels => self.export_channels(),
            Command::CopyImage => self.copy_image(ctx),
            Command::View(view) => {
                self.texture_dirty |= self.view != view;
                self.view = view;
            }
            Command::ZoomToFit => self.fit = true,
            Command::ActualPixels => self.set_zoom(1.0),
            Command::ZoomIn => self.set_zoom(self.zoom * 1.25),
            Command::ZoomOut => self.set_zoom(self.zoom / 1.25),
        }
    }

    fn set_zoom(&mut self, zoom: f32) {
        self.fit = false;
        self.zoom = zoom.clamp(0.25, 8.0);
    }

    // Commands from the menu bar and its keyboard shortcuts.
    fn menu_commands(&mut self, ui: &mut egui::Ui) -> Vec<Command> {
        #[cfg(target_os = "macos")]
        if let Some(mut native_menu) = self.native_menu.take() {
            let clicked = native_menu.clicked();
            native_menu.update(|command| self.menu_state(command));
            self.native_menu = Some(native_menu);
            return clicked;
        }
        let mut commands = Vec::new();
        egui::Panel::top("menu_bar").show(ui, |ui| {
            commands.extend(menu::show_in_window(ui, |command| self.menu_state(command)));
        });
        commands.extend(menu::pressed_shortcut(ui.ctx(), |command| self.menu_state(command)));
        commands
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        if NATIVE_SIDEBAR {
            mac_controls(ui.visuals_mut());
            // egui's buttons hug their text (4 × 1 pt); Mac push buttons have more room.
            ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
        }
        ui.add_space(if NATIVE_SIDEBAR { 4.0 } else { 8.0 });
        if ui.button("Open image…").clicked() {
            self.open_dialog(ui.ctx());
        }
        ui.label(egui::RichText::new(&self.status).small().weak());

        section(ui, "Size");
        if let Some(loaded) = &self.loaded {
            let full = loaded.source.width();
            let height = height_for_width(&loaded.source, self.width);
            ui.add(egui::Slider::new(&mut self.width, full.min(16)..=full).logarithmic(true).suffix(" px"))
                .on_hover_text("Width to dither at. Smaller means bigger dots relative to the picture");
            ui.horizontal(|ui| {
                for (label, divisor) in [("⅛", 8), ("¼", 4), ("½", 2), ("Full", 1)] {
                    if ui.button(label).clicked() {
                        self.width = (full / divisor).max(1);
                    }
                }
                ui.label(egui::RichText::new(format!("{} × {height}", self.width)).weak());
                if self.job.as_ref().is_some_and(|job| !job.opening) {
                    ui.spinner();
                }
            });
        } else {
            ui.label(egui::RichText::new("Open an image first").weak());
        }

        section(ui, "Dither");
        egui::ComboBox::from_label("Algorithm").selected_text(self.options.algorithm.label()).show_ui(ui, |ui| {
            for (i, algorithm) in ALGORITHMS.into_iter().enumerate() {
                // Separate the error diffusion, ordered and other groups.
                if i > 0 && algorithm.is_error_diffusion() != ALGORITHMS[i - 1].is_error_diffusion()
                    || algorithm == Algorithm::Halftone
                {
                    ui.separator();
                }
                ui.selectable_value(&mut self.options.algorithm, algorithm, algorithm.label());
            }
        });
        ui.add_enabled(
            self.options.algorithm.is_error_diffusion(),
            egui::Checkbox::new(&mut self.options.serpentine, "Serpentine"),
        )
        .on_hover_text("Scan every other row right to left, which breaks up diagonal \"worm\" patterns");

        section(ui, "Colors");
        ui.horizontal(|ui| {
            ui.label("Mix");
            for (mode, label, hover) in COLOR_MODES {
                if ui.selectable_label(self.mode == mode, label).on_hover_text(hover).clicked() {
                    self.set_mode(mode);
                }
            }
        });
        egui::ComboBox::from_label("Preset").selected_text(&self.preset_name).show_ui(ui, |ui| {
            for name in PRESET_NAMES {
                let fits = preset(name).is_some_and(|p| preset_mode(&p) == self.mode);
                if fits && ui.selectable_label(self.preset_name == name, name).clicked() {
                    self.apply_preset(name);
                }
            }
        });

        if self.mode != ColorMode::Palette {
            let before = self.channels;
            egui::Grid::new("channels").num_columns(2).show(ui, |ui| {
                let background = if self.mode == ColorMode::Ink { "Paper" } else { "Background" };
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
        }

        ui.add_space(4.0);
        let palette_label = match self.mode {
            ColorMode::Palette => "Colors (by channels that are on)",
            _ => "Resulting colors (by channels that are on)",
        };
        ui.label(palette_label).on_hover_text("Click one to change it directly");
        egui::Grid::new("palette").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
            for i in 0..8 {
                // Centre the label under its swatch only. Unconstrained, the last column
                // reaches to the panel edge and its swatches drift off to the right.
                let swatch = ui.spacing().interact_size.x;
                ui.allocate_ui(egui::vec2(swatch, 0.0), |ui| ui.vertical_centered(|ui| {
                    if ui.color_edit_button_srgb(&mut self.palette[i]).changed() {
                        self.preset_name = "custom".to_string();
                        self.texture_dirty = true;
                    }
                    ui.label(egui::RichText::new(slot_label(i)).small().weak());
                }));
                if i % 4 == 3 {
                    ui.end_row();
                }
            }
        });

        section(ui, "Preview");
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
            self.set_zoom(1.0);
        }

        if NATIVE_SIDEBAR {
            ui.add_space(14.0);
        } else {
            ui.separator();
        }
        if ui.add_enabled(self.loaded.is_some(), egui::Button::new("Export PNG…")).clicked() {
            self.export();
        }
    }

    // The title is hidden from the real title bar (it would sit over the sidebar),
    // so show the file name in the title bar strip above the preview, like Mac apps do.
    fn title_bar(&self, ui: &mut egui::Ui) {
        let title = self.loaded.as_ref().map_or("Dither Channels".to_string(), |l| file_name(&l.path));
        let rect = ui.max_rect();
        let strip = egui::Rect::from_min_max(rect.min - egui::vec2(0.0, TITLE_BAR_HEIGHT), egui::pos2(rect.max.x, rect.min.y));
        ui.painter().text(
            strip.left_center() + egui::vec2(4.0, 0.0),
            egui::Align2::LEFT_CENTER,
            title,
            egui::FontId::proportional(13.0),
            ui.visuals().strong_text_color(),
        );
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        let (Some(texture), Some(_)) = (&self.texture, &self.loaded) else {
            if self.job.is_some() {
                ui.centered_and_justified(|ui| ui.spinner());
            } else {
                ui.centered_and_justified(|ui| ui.label("Open an image or drop one here"));
            }
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
    // Transparent, so the sidebar material isn't covered.
    #[cfg(target_os = "macos")]
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_job();
        let outdated =
            self.loaded.as_ref().is_some_and(|l| l.dithered.width as u32 != self.width || l.options != self.options);
        if outdated && self.job.is_none() {
            self.redither(ui.ctx());
        }
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.open(path, ui.ctx());
        }
        for command in self.menu_commands(ui) {
            self.run(command, ui.ctx());
        }

        let mut sidebar = egui::Panel::left("controls").resizable(false).default_size(260.0);
        let mut central = egui::CentralPanel::default_margins();
        if NATIVE_SIDEBAR {
            // Clear, so the sidebar material shows through; the window buttons need room above.
            let margin = egui::Margin { left: 12, right: 12, top: TITLE_BAR_HEIGHT as i8 + 8, bottom: 8 };
            sidebar = sidebar.show_separator_line(true).frame(egui::Frame::new().inner_margin(margin));
            let fill = ui.visuals().panel_fill;
            let margin = egui::Margin { top: TITLE_BAR_HEIGHT as i8, ..egui::Margin::same(8) };
            central = central.frame(egui::Frame::new().fill(fill).inner_margin(margin));
        }
        sidebar.show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.sidebar(ui));
        });
        self.update_texture(ui.ctx());
        central.show(ui, |ui| {
            if NATIVE_SIDEBAR {
                self.title_bar(ui);
            }
            self.preview(ui)
        });
    }
}

// Puts the translucent macOS sidebar material behind the whole window. The preview
// paints over it; only the sidebar is left transparent.
#[cfg(target_os = "macos")]
fn add_sidebar_material(cc: &eframe::CreationContext) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
        NSVisualEffectView, NSWindowOrderingMode,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = cc.window_handle() else { return };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else { return };
    let Some(mtm) = MainThreadMarker::new() else { return };
    // egui draws into a Metal layer inside winit's view, and a subview of that view
    // would cover it. So the effect view goes next to it instead, below it in the
    // window's frame view.
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    let Some(frame_view) = (unsafe { view.superview() }) else { return };
    let effect = NSVisualEffectView::initWithFrame(mtm.alloc(), frame_view.bounds());
    effect.setMaterial(NSVisualEffectMaterial::Sidebar);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::FollowsWindowActiveState);
    effect.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
    frame_view.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, Some(view));
}

// egui's own font (Ubuntu Light) is thin and small for its size next to the rest of
// macOS, so use San Francisco, read from the system rather than bundled. egui's fonts
// stay as a fallback for glyphs it lacks. Small text goes from 9 to 10 pt, closer to
// Mac captions.
#[cfg(target_os = "macos")]
fn use_system_font(ctx: &egui::Context) {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    let Ok(font) = std::fs::read("/System/Library/Fonts/SFNS.ttf") else { return };
    let family = InsertFontFamily { family: egui::FontFamily::Proportional, priority: FontPriority::Highest };
    ctx.add_font(FontInsert::new("San Francisco", egui::FontData::from_owned(font), vec![family]));
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(10.0));
    });
}

// egui's control fills are about the grey of the sidebar material, so they'd vanish
// into it. Mac sidebars use white controls with a faint edge instead (light, see-through
// ones in dark mode).
fn mac_controls(visuals: &mut egui::Visuals) {
    use egui::Color32;
    let (fill, hovered, pressed, edge) = if visuals.dark_mode {
        (Color32::from_white_alpha(28), Color32::from_white_alpha(40), Color32::from_white_alpha(56), Color32::from_white_alpha(18))
    } else {
        (Color32::WHITE, Color32::from_gray(248), Color32::from_gray(232), Color32::from_black_alpha(36))
    };
    let widgets = &mut visuals.widgets;
    for (state, fill) in [(&mut widgets.inactive, fill), (&mut widgets.hovered, hovered), (&mut widgets.active, pressed)] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = egui::Stroke::new(1.0, edge);
    }
}

// A sidebar section header: small, bold and grey like in Mac sidebars, or a
// separator and heading elsewhere.
fn section(ui: &mut egui::Ui, title: &str) {
    if NATIVE_SIDEBAR {
        ui.add_space(14.0);
        ui.label(egui::RichText::new(title).size(11.0).strong().color(ui.visuals().weak_text_color()));
        ui.add_space(2.0);
    } else {
        ui.separator();
        ui.heading(title);
    }
}

// Colors every n-th pixel (step) with the given palette.
fn render(d: &Dithered, palette: &Palette, step: usize) -> egui::ColorImage {
    let (w, h) = (d.width.div_ceil(step), d.height.div_ceil(step));
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            rgb.extend_from_slice(&palette[d.index(y * step * d.width + x * step) as usize]);
        }
    }
    egui::ColorImage::from_rgb([w, h], &rgb)
}

// Which channels are on for palette slot i, e.g. "R+B".
fn slot_label(i: usize) -> String {
    let on: Vec<&str> = index_bits(i).iter().zip(["R", "G", "B"]).filter(|(on, _)| **on).map(|(_, l)| l).collect();
    if on.is_empty() { "none".to_string() } else { on.join("+") }
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}
