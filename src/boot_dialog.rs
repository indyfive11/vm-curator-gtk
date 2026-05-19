use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{Box as GtkBox, Button, CheckButton, FileFilter, Label, Orientation, Separator};
use libadwaita::{HeaderBar, ToolbarView};
use std::cell::RefCell;
use std::rc::Rc;

use vm_curator::vm::{BootMode, LaunchOptions};

pub fn show(parent: &impl IsA<gtk4::Widget>, on_launch: impl Fn(LaunchOptions) + 'static) {
    let dialog = libadwaita::Dialog::new();
    dialog.set_title("Boot Options");
    dialog.set_content_width(380);
    dialog.set_content_height(320);

    // Radio group
    let normal_btn = CheckButton::builder()
        .label("Normal Boot")
        .active(true)
        .build();
    let install_btn = CheckButton::builder()
        .label("Install Mode  (boot from install media)")
        .group(&normal_btn)
        .build();
    let cdrom_btn = CheckButton::builder()
        .label("Custom ISO…")
        .group(&normal_btn)
        .build();

    // ISO chooser row (hidden until cdrom_btn active)
    let iso_name_label = Label::builder()
        .label("No file selected")
        .halign(gtk4::Align::Start)
        .hexpand(true)
        .ellipsize(gtk4::pango::EllipsizeMode::Start)
        .build();
    iso_name_label.add_css_class("dim-label");

    let browse_btn = Button::builder().label("Browse…").build();

    let iso_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(28)
        .margin_top(4)
        .visible(false)
        .build();
    iso_row.append(&iso_name_label);
    iso_row.append(&browse_btn);

    let launch_btn = Button::builder().label("Launch").hexpand(true).build();
    launch_btn.add_css_class("suggested-action");

    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(6)
        .margin_start(16)
        .margin_end(16)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    content.append(&normal_btn);
    content.append(&install_btn);
    content.append(&cdrom_btn);
    content.append(&iso_row);
    content.append(&Separator::new(Orientation::Horizontal));

    let btn_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .margin_top(4)
        .build();
    btn_row.append(&launch_btn);
    content.append(&btn_row);

    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&HeaderBar::new());
    toolbar_view.set_content(Some(&content));
    dialog.set_child(Some(&toolbar_view));

    // Show/hide ISO row; disable Launch until a file is actually chosen
    {
        let iso_row = iso_row.clone();
        let launch_btn = launch_btn.clone();
        cdrom_btn.connect_toggled(move |btn| {
            iso_row.set_visible(btn.is_active());
            if btn.is_active() {
                launch_btn.set_sensitive(false);
            } else {
                launch_btn.set_sensitive(true);
            }
        });
    }

    // ISO file chooser
    let iso_path: Rc<RefCell<Option<std::path::PathBuf>>> = Rc::new(RefCell::new(None));
    {
        let iso_path = Rc::clone(&iso_path);
        let iso_name_label = iso_name_label.clone();
        let dialog_ref = dialog.clone();
        let launch_btn_ref = launch_btn.clone();

        browse_btn.connect_clicked(move |_| {
            let filter = FileFilter::new();
            filter.set_name(Some("Disk images"));
            filter.add_pattern("*.iso");
            filter.add_pattern("*.img");
            filter.add_pattern("*.dmg");

            let filters = gtk4::gio::ListStore::new::<FileFilter>();
            filters.append(&filter);

            let file_dialog = gtk4::FileDialog::builder()
                .title("Select ISO image")
                .accept_label("Select")
                .filters(&filters)
                .build();

            let iso_path = Rc::clone(&iso_path);
            let iso_name_label = iso_name_label.clone();
            let launch_btn_inner = launch_btn_ref.clone();
            let parent_win = dialog_ref.root().and_downcast::<gtk4::Window>();
            file_dialog.open(
                parent_win.as_ref(),
                gtk4::gio::Cancellable::NONE,
                move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            let name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default();
                            iso_name_label.set_label(&name);
                            iso_name_label.remove_css_class("dim-label");
                            *iso_path.borrow_mut() = Some(path);
                            launch_btn_inner.set_sensitive(true);
                        }
                    }
                },
            );
        });
    }

    // Launch button: build LaunchOptions and call back
    {
        let normal_btn = normal_btn.clone();
        let install_btn = install_btn.clone();
        let cdrom_btn = cdrom_btn.clone();
        let iso_path = Rc::clone(&iso_path);
        let dialog_ref = dialog.clone();

        launch_btn.connect_clicked(move |_| {
            let boot_mode = if normal_btn.is_active() {
                BootMode::Normal
            } else if install_btn.is_active() {
                BootMode::Install
            } else if cdrom_btn.is_active() {
                match iso_path.borrow().clone() {
                    Some(path) => BootMode::Cdrom(path),
                    None => return, // no ISO chosen yet
                }
            } else {
                BootMode::Normal
            };

            on_launch(LaunchOptions {
                boot_mode,
                extra_args: Vec::new(),
                usb_devices: Vec::new(),
            });
            dialog_ref.close();
        });
    }

    dialog.present(Some(parent));
}
