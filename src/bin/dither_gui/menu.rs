// The menus, described once and shown either in the native macOS menu bar
// or, on other platforms, as a menu bar inside the window.

use crate::View;
use eframe::egui;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Command {
    Open,
    Export,
    ExportChannels,
    CopyImage,
    View(View),
    ZoomToFit,
    ActualPixels,
    ZoomIn,
    ZoomOut,
}

// Always Cmd on macOS / Ctrl elsewhere, optionally with Shift.
pub struct Shortcut {
    shift: bool,
    key: egui::Key,
    // The same key as named in muda's accelerator strings.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    accelerator_key: &'static str,
}

impl Shortcut {
    pub fn egui(&self) -> egui::KeyboardShortcut {
        let shift = if self.shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
        egui::KeyboardShortcut::new(egui::Modifiers::COMMAND | shift, self.key)
    }
}

pub struct Item {
    pub command: Command,
    pub label: &'static str,
    pub shortcut: Option<Shortcut>,
}

pub enum Entry {
    Item(Item),
    Separator,
}

const fn cmd(command: Command, label: &'static str, key: egui::Key, accelerator_key: &'static str) -> Entry {
    Entry::Item(Item { command, label, shortcut: Some(Shortcut { shift: false, key, accelerator_key }) })
}

const fn cmd_shift(command: Command, label: &'static str, key: egui::Key, accelerator_key: &'static str) -> Entry {
    Entry::Item(Item { command, label, shortcut: Some(Shortcut { shift: true, key, accelerator_key }) })
}

pub const MENUS: &[(&str, &[Entry])] = &[
    (
        "File",
        &[
            cmd(Command::Open, "Open…", egui::Key::O, "O"),
            Entry::Separator,
            cmd(Command::Export, "Export PNG…", egui::Key::E, "E"),
            cmd_shift(Command::ExportChannels, "Export Channels…", egui::Key::E, "E"),
        ],
    ),
    ("Edit", &[cmd_shift(Command::CopyImage, "Copy Image", egui::Key::C, "C")]),
    (
        "View",
        &[
            cmd(Command::View(View::Combined), "All Channels", egui::Key::Num1, "1"),
            cmd(Command::View(View::Channel(0)), "Red Channel", egui::Key::Num2, "2"),
            cmd(Command::View(View::Channel(1)), "Green Channel", egui::Key::Num3, "3"),
            cmd(Command::View(View::Channel(2)), "Blue Channel", egui::Key::Num4, "4"),
            Entry::Separator,
            cmd(Command::ZoomToFit, "Zoom to Fit", egui::Key::Num9, "9"),
            cmd(Command::ActualPixels, "Actual Pixels", egui::Key::Num0, "0"),
            cmd(Command::ZoomIn, "Zoom In", egui::Key::Equals, "="),
            cmd(Command::ZoomOut, "Zoom Out", egui::Key::Minus, "-"),
        ],
    ),
];

// How a command currently looks in the menu.
pub struct State {
    pub enabled: bool,
    // Some for commands that show a check mark.
    pub checked: Option<bool>,
}

pub fn items() -> impl Iterator<Item = &'static Item> {
    MENUS.iter().flat_map(|(_, entries)| entries.iter()).filter_map(|entry| match entry {
        Entry::Item(item) => Some(item),
        Entry::Separator => None,
    })
}

// In-window menu bar for platforms without a global one. Returns the clicked command.
pub fn show_in_window(ui: &mut egui::Ui, state: impl Fn(Command) -> State) -> Option<Command> {
    let mut clicked = None;
    egui::MenuBar::new().ui(ui, |ui| {
        for (title, entries) in MENUS {
            ui.menu_button(*title, |ui| {
                for entry in *entries {
                    let Entry::Item(item) = entry else {
                        ui.separator();
                        continue;
                    };
                    let State { enabled, checked } = state(item.command);
                    let mut button = egui::Button::new(item.label).selected(checked == Some(true));
                    if let Some(shortcut) = &item.shortcut {
                        button = button.shortcut_text(ui.ctx().format_shortcut(&shortcut.egui()));
                    }
                    if ui.add_enabled(enabled, button).clicked() {
                        clicked = Some(item.command);
                    }
                }
            });
        }
    });
    clicked
}

// Keyboard shortcuts for the in-window menu (a native menu handles its own).
pub fn pressed_shortcut(ctx: &egui::Context, state: impl Fn(Command) -> State) -> Option<Command> {
    items().find_map(|item| {
        let shortcut = item.shortcut.as_ref()?.egui();
        let pressed = state(item.command).enabled && ctx.input_mut(|i| i.consume_shortcut(&shortcut));
        pressed.then_some(item.command)
    })
}

#[cfg(target_os = "macos")]
pub use native::NativeMenu;

#[cfg(target_os = "macos")]
mod native {
    use super::{items, Command, Entry, State, MENUS};
    use eframe::egui;
    use muda::accelerator::Accelerator;
    use muda::{AboutMetadata, CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
    use std::sync::mpsc::{channel, Receiver};

    enum NativeItem {
        Plain(MenuItem),
        Check(CheckMenuItem),
    }

    impl NativeItem {
        fn id(&self) -> &MenuId {
            match self {
                NativeItem::Plain(item) => item.id(),
                NativeItem::Check(item) => item.id(),
            }
        }

        fn as_menu_item(&self) -> &dyn IsMenuItem {
            match self {
                NativeItem::Plain(item) => item,
                NativeItem::Check(item) => item,
            }
        }
    }

    // The macOS menu bar: the standard app and Window menus plus ours.
    pub struct NativeMenu {
        // The menu stops working once dropped.
        _menu: Menu,
        items: Vec<(Command, NativeItem)>,
        // Last state pushed to each item, to only touch AppKit when something changes.
        shown: Vec<(bool, Option<bool>)>,
        clicks: Receiver<MenuId>,
    }

    impl NativeMenu {
        pub fn new(ctx: &egui::Context, state: impl Fn(Command) -> State) -> Self {
            // Menu clicks arrive outside egui's event loop, so wake it up for them.
            let (tx, clicks) = channel();
            let ctx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let _ = tx.send(event.id);
                ctx.request_repaint();
            }));

            let items: Vec<(Command, NativeItem)> = items()
                .map(|item| {
                    let accelerator = item.shortcut.as_ref().map(|shortcut| {
                        let shift = if shortcut.shift { "Shift+" } else { "" };
                        format!("Cmd+{shift}{}", shortcut.accelerator_key)
                            .parse::<Accelerator>()
                            .expect("menu shortcuts are valid accelerators")
                    });
                    let State { enabled, checked } = state(item.command);
                    let native = match checked {
                        Some(checked) => NativeItem::Check(CheckMenuItem::new(item.label, enabled, checked, accelerator)),
                        None => NativeItem::Plain(MenuItem::new(item.label, enabled, accelerator)),
                    };
                    (item.command, native)
                })
                .collect();
            let shown = items.iter().map(|(command, _)| state(*command)).map(|s| (s.enabled, s.checked)).collect();

            let menu = Menu::new();
            let name = "Dither Channels";
            let about = AboutMetadata {
                name: Some(name.to_string()),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
                ..Default::default()
            };
            let app_menu = Submenu::with_items(
                name,
                true,
                &[
                    &PredefinedMenuItem::about(None, Some(about)),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::services(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::hide(None),
                    &PredefinedMenuItem::hide_others(None),
                    &PredefinedMenuItem::show_all(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::quit(None),
                ],
            )
            .expect("app menu");
            menu.append(&app_menu).expect("app menu");

            let mut ours = items.iter();
            for (title, entries) in MENUS {
                let submenu = Submenu::new(*title, true);
                for entry in *entries {
                    match entry {
                        Entry::Item(_) => submenu.append(ours.next().unwrap().1.as_menu_item()),
                        Entry::Separator => submenu.append(&PredefinedMenuItem::separator()),
                    }
                    .expect("menu item");
                }
                // Standard macOS items that belong in these menus.
                match *title {
                    "File" => submenu.append_items(&[&PredefinedMenuItem::separator(), &PredefinedMenuItem::close_window(None)]),
                    "View" => submenu.append_items(&[&PredefinedMenuItem::separator(), &PredefinedMenuItem::fullscreen(None)]),
                    _ => Ok(()),
                }
                .expect("menu item");
                menu.append(&submenu).expect("menu");
            }

            let window_menu = Submenu::with_items(
                "Window",
                true,
                &[
                    &PredefinedMenuItem::minimize(None),
                    &PredefinedMenuItem::maximize(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::bring_all_to_front(None),
                ],
            )
            .expect("window menu");
            menu.append(&window_menu).expect("window menu");
            window_menu.set_as_windows_menu_for_nsapp();

            menu.init_for_nsapp();
            NativeMenu { _menu: menu, items, shown, clicks }
        }

        // Returns the commands clicked (or triggered by shortcut) since the last call.
        pub fn clicked(&self) -> Vec<Command> {
            self.clicks
                .try_iter()
                .filter_map(|id| self.items.iter().find(|(_, item)| *item.id() == id).map(|(command, _)| *command))
                .collect()
        }

        pub fn update(&mut self, state: impl Fn(Command) -> State) {
            for ((command, item), shown) in self.items.iter().zip(&mut self.shown) {
                let State { enabled, checked } = state(*command);
                if (enabled, checked) == *shown {
                    continue;
                }
                *shown = (enabled, checked);
                match item {
                    NativeItem::Plain(item) => item.set_enabled(enabled),
                    NativeItem::Check(item) => {
                        item.set_enabled(enabled);
                        item.set_checked(checked == Some(true));
                    }
                }
            }
        }
    }
}
