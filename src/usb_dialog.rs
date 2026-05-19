use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{Box as GtkBox, Button, CheckButton, Label, Orientation, ScrolledWindow, Spinner};
use libadwaita::{ActionRow, HeaderBar, Toast, ToastOverlay, ToolbarView};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::hardware::{
    enumerate_usb_devices, install_udev_rules, UdevInstallResult, UsbDevice,
};
use vm_curator::vm::{load_usb_passthrough, save_usb_passthrough, DiscoveredVm, UsbPassthrough};

enum LoadResult {
    Ok(Vec<UsbDevice>),
    Err(String),
}

pub fn show(parent: &impl IsA<gtk4::Widget>, vm: DiscoveredVm) {
    let dialog = libadwaita::Dialog::new();
    dialog.set_title(&format!("USB Passthrough — {}", vm.display_name()));
    dialog.set_content_width(520);
    dialog.set_content_height(520);

    // Load saved passthrough entries (fast, sync)
    let saved = load_usb_passthrough(&vm);

    // --- Spinner shown while enumerating ---
    let spinner = Spinner::builder().spinning(true).halign(gtk4::Align::Center).build();
    let spinner_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .valign(gtk4::Align::Center)
        .spacing(8)
        .margin_top(32)
        .margin_bottom(32)
        .build();
    spinner_box.append(&spinner);
    let loading_label = Label::new(Some("Enumerating USB devices…"));
    loading_label.add_css_class("dim-label");
    spinner_box.append(&loading_label);

    // --- Device list (populated after enumeration) ---
    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::None);
    list_box.set_visible(false);

    // Shared selected-device tracking: Vec<CheckButton> in same order as devices
    let check_buttons: Rc<RefCell<Vec<(CheckButton, UsbDevice)>>> =
        Rc::new(RefCell::new(Vec::new()));

    let content_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    content_box.append(&spinner_box);
    content_box.append(&list_box);

    let note = Label::builder()
        .label("Check devices to pass through to this VM. Requires VFIO and udev rules.\nDevice access persists as long as the VM is running.")
        .wrap(true)
        .halign(gtk4::Align::Start)
        .build();
    note.add_css_class("caption");
    note.add_css_class("dim-label");
    content_box.append(&note);

    let scroll = ScrolledWindow::builder()
        .child(&content_box)
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();

    let save_btn = Button::builder().label("Save").build();
    save_btn.add_css_class("suggested-action");

    let udev_btn = Button::builder().label("Install udev Rules").build();
    udev_btn.set_tooltip_text(Some("Write udev rules so the VM user can access selected devices"));

    let header = HeaderBar::new();
    header.pack_end(&save_btn);
    header.pack_start(&udev_btn);

    let toast_overlay = ToastOverlay::new();
    toast_overlay.set_child(Some(&scroll));

    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&toast_overlay));
    dialog.set_child(Some(&toolbar_view));

    // --- Background USB enumeration ---
    let (tx, rx) = mpsc::channel::<LoadResult>();
    std::thread::spawn(move || {
        let result = match enumerate_usb_devices() {
            Ok(devices) => LoadResult::Ok(devices),
            Err(e) => LoadResult::Err(e.to_string()),
        };
        tx.send(result).ok();
    });

    {
        let spinner_box = spinner_box.clone();
        let list_box = list_box.clone();
        let check_buttons = Rc::clone(&check_buttons);
        let toast_overlay = toast_overlay.clone();
        let rx = Rc::new(RefCell::new(rx));

        gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
            match rx.borrow().try_recv() {
                Ok(LoadResult::Ok(devices)) => {
                    spinner_box.set_visible(false);

                    if devices.is_empty() {
                        let empty = Label::builder()
                            .label("No USB devices found")
                            .halign(gtk4::Align::Center)
                            .margin_top(8)
                            .margin_bottom(8)
                            .build();
                        empty.add_css_class("dim-label");
                        list_box.set_placeholder(Some(&empty));
                    }

                    let mut buttons = check_buttons.borrow_mut();
                    for device in &devices {
                        let is_checked = saved.iter().any(|s| {
                            s.vendor_id == device.vendor_id && s.product_id == device.product_id
                        });

                        let check = CheckButton::builder().active(is_checked).build();
                        let row = ActionRow::new();
                        row.set_title(&format!("{} {}", device.vendor_name, device.product_name));
                        row.set_subtitle(&format!(
                            "{:?}  ·  {:04x}:{:04x}",
                            device.usb_version, device.vendor_id, device.product_id
                        ));
                        row.add_prefix(&check);
                        row.set_activatable_widget(Some(&check));
                        list_box.append(&row);
                        buttons.push((check, device.clone()));
                    }

                    list_box.set_visible(true);
                    gtk4::glib::ControlFlow::Break
                }
                Ok(LoadResult::Err(e)) => {
                    spinner_box.set_visible(false);
                    toast_overlay.add_toast(
                        Toast::builder()
                            .title(&format!("Enumeration failed: {e}"))
                            .timeout(0)
                            .build(),
                    );
                    gtk4::glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => gtk4::glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => gtk4::glib::ControlFlow::Break,
            }
        });
    }

    // --- Install udev Rules ---
    {
        let check_buttons = Rc::clone(&check_buttons);
        let toast_overlay = toast_overlay.clone();

        udev_btn.connect_clicked(move |_| {
            let selected: Vec<UsbDevice> = check_buttons
                .borrow()
                .iter()
                .filter(|(btn, _)| btn.is_active())
                .map(|(_, dev)| dev.clone())
                .collect();

            if selected.is_empty() {
                toast_overlay
                    .add_toast(Toast::new("Select at least one device first."));
                return;
            }

            match install_udev_rules(&selected) {
                UdevInstallResult::Success => {
                    toast_overlay.add_toast(Toast::new("udev rules installed."));
                }
                UdevInstallResult::NeedsReboot => {
                    toast_overlay.add_toast(
                        Toast::builder()
                            .title("Rules installed — reboot required to apply.")
                            .timeout(0)
                            .build(),
                    );
                }
                UdevInstallResult::PermissionDenied => {
                    toast_overlay.add_toast(
                        Toast::builder()
                            .title("Permission denied — run with sudo or install polkit.")
                            .timeout(0)
                            .build(),
                    );
                }
                UdevInstallResult::Error(e) => {
                    toast_overlay.add_toast(
                        Toast::builder()
                            .title(&format!("udev install failed: {e}"))
                            .timeout(0)
                            .build(),
                    );
                }
            }
        });
    }

    // --- Save ---
    {
        let dialog_ref = dialog.clone();
        let toast_overlay = toast_overlay.clone();

        save_btn.connect_clicked(move |_| {
            let devices: Vec<UsbPassthrough> = check_buttons
                .borrow()
                .iter()
                .filter(|(btn, _)| btn.is_active())
                .map(|(_, dev)| UsbPassthrough {
                    vendor_id: dev.vendor_id,
                    product_id: dev.product_id,
                    usb_version: dev.usb_version,
                })
                .collect();

            let vm = vm.clone();
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let result = save_usb_passthrough(&vm, &devices).map_err(|e| e.to_string());
                tx.send(result).ok();
            });

            let dialog_ref = dialog_ref.clone();
            let toast_overlay = toast_overlay.clone();
            let rx = Rc::new(RefCell::new(rx));
            gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
                match rx.borrow().try_recv() {
                    Ok(Ok(())) => {
                        dialog_ref.close();
                        gtk4::glib::ControlFlow::Break
                    }
                    Ok(Err(e)) => {
                        toast_overlay.add_toast(
                            Toast::builder()
                                .title(&format!("Save failed: {e}"))
                                .timeout(0)
                                .build(),
                        );
                        gtk4::glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => gtk4::glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => gtk4::glib::ControlFlow::Break,
                }
            });
        });
    }

    dialog.present(Some(parent));
}
