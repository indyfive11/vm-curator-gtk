use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{Box as GtkBox, Button, CheckButton, Label, Orientation, ScrolledWindow, Spinner};
use libadwaita::{ActionRow, HeaderBar, PreferencesGroup, Toast, ToastOverlay, ToolbarView};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::hardware::{enumerate_pci_devices, PciDevice};
use vm_curator::vm::{load_pci_passthrough, save_pci_passthrough, DiscoveredVm};

enum LoadResult {
    Ok(Vec<PciDevice>),
    Err(String),
}

/// Extract PCI address from a raw arg like "-device vfio-pci,host=0000:01:00.0"
fn parse_saved_address(arg: &str) -> Option<&str> {
    arg.find("host=").map(|i| {
        let rest = &arg[i + 5..];
        rest.split(|c: char| c.is_whitespace() || c == ',')
            .next()
            .unwrap_or(rest)
    })
}

pub fn show(parent: &impl IsA<gtk4::Widget>, vm: DiscoveredVm) {
    let dialog = libadwaita::Dialog::new();
    dialog.set_title(&format!("PCI Passthrough — {}", vm.display_name()));
    dialog.set_content_width(580);
    dialog.set_content_height(580);

    // Load saved addresses synchronously (just file read)
    let saved_args = load_pci_passthrough(&vm);
    let saved_addrs: Vec<String> = saved_args
        .iter()
        .filter_map(|a| parse_saved_address(a).map(|s| s.to_string()))
        .collect();

    // --- Loading state ---
    let spinner = Spinner::builder().spinning(true).halign(gtk4::Align::Center).build();
    let spinner_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .valign(gtk4::Align::Center)
        .spacing(8)
        .margin_top(32)
        .margin_bottom(32)
        .build();
    spinner_box.append(&spinner);
    let loading_lbl = Label::new(Some("Enumerating PCI devices…"));
    loading_lbl.add_css_class("dim-label");
    spinner_box.append(&loading_lbl);

    let groups_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .build();
    groups_box.set_visible(false);

    // Shared: Vec<(CheckButton, PciDevice, iommu_group)>
    let check_buttons: Rc<RefCell<Vec<(CheckButton, PciDevice)>>> =
        Rc::new(RefCell::new(Vec::new()));

    let content_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    content_box.append(&spinner_box);
    content_box.append(&groups_box);

    let note = Label::builder()
        .label("All devices in an IOMMU group must be passed together. GPU audio companions are auto-checked when the GPU is selected.")
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

    let header = HeaderBar::new();
    header.pack_end(&save_btn);

    let toast_overlay = ToastOverlay::new();
    toast_overlay.set_child(Some(&scroll));

    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&toast_overlay));
    dialog.set_child(Some(&toolbar_view));

    // --- Background enumeration ---
    let (tx, rx) = mpsc::channel::<LoadResult>();
    std::thread::spawn(move || {
        let result = match enumerate_pci_devices() {
            Ok(devs) => LoadResult::Ok(devs),
            Err(e) => LoadResult::Err(e.to_string()),
        };
        tx.send(result).ok();
    });

    {
        let spinner_box = spinner_box.clone();
        let groups_box = groups_box.clone();
        let check_buttons = Rc::clone(&check_buttons);
        let toast_overlay = toast_overlay.clone();
        let rx = Rc::new(RefCell::new(rx));

        gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
            match rx.borrow().try_recv() {
                Ok(LoadResult::Ok(all_devices)) => {
                    spinner_box.set_visible(false);
                    populate_pci_list(
                        &all_devices,
                        &saved_addrs,
                        &groups_box,
                        &check_buttons,
                    );
                    groups_box.set_visible(true);
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

    // --- Save ---
    {
        let dialog_ref = dialog.clone();
        let toast_overlay = toast_overlay.clone();

        save_btn.connect_clicked(move |_| {
            let selected: Vec<PciDevice> = check_buttons
                .borrow()
                .iter()
                .filter(|(btn, _)| btn.is_active())
                .map(|(_, dev)| dev.clone())
                .collect();

            let vm = vm.clone();
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let result = save_pci_passthrough(&vm, &selected).map_err(|e| e.to_string());
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

fn populate_pci_list(
    all_devices: &[PciDevice],
    saved_addrs: &[String],
    groups_box: &GtkBox,
    check_buttons: &Rc<RefCell<Vec<(CheckButton, PciDevice)>>>,
) {
    // Only show passthrough candidates (has IOMMU group, not boot VGA)
    let candidates: Vec<&PciDevice> =
        all_devices.iter().filter(|d| d.can_passthrough()).collect();

    if candidates.is_empty() {
        let empty = Label::builder()
            .label("No passthrough-capable PCI devices found.\nEnsure IOMMU is enabled in firmware and kernel (intel_iommu=on / amd_iommu=on).")
            .wrap(true)
            .halign(gtk4::Align::Center)
            .margin_top(16)
            .build();
        empty.add_css_class("dim-label");
        groups_box.append(&empty);
        return;
    }

    // Group by IOMMU group number
    let mut by_group: BTreeMap<u32, Vec<&PciDevice>> = BTreeMap::new();
    for dev in &candidates {
        if let Some(g) = dev.iommu_group {
            by_group.entry(g).or_default().push(dev);
        }
    }

    // Build a flat index: address → index in check_buttons, for group auto-check
    // We need to wire this up after all buttons are created.
    // Strategy: collect (group_id, Vec<usize>) alongside button creation.
    let mut group_indices: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    let mut buttons_snapshot: Vec<(CheckButton, PciDevice)> = Vec::new();

    for (&group_id, devices) in &by_group {
        let group_widget = PreferencesGroup::new();
        group_widget.set_title(&format!("IOMMU Group {group_id}"));

        let multi_device = devices.len() > 1;

        let _start_idx = buttons_snapshot.len();
        for dev in devices {
            let is_checked = saved_addrs.contains(&dev.address);
            let check = CheckButton::builder().active(is_checked).build();
            let row = ActionRow::new();
            row.set_title(&dev.display_name());

            let driver_text = dev
                .driver
                .as_deref()
                .unwrap_or("no driver");
            let vfio_note = if dev.is_vfio_bound() { " (vfio-pci ✓)" } else { "" };
            row.set_subtitle(&format!("{} · {}{}", dev.address, driver_text, vfio_note));
            row.add_prefix(&check);
            row.set_activatable_widget(Some(&check));

            group_widget.add(&row);
            group_indices.entry(group_id).or_default().push(buttons_snapshot.len());
            buttons_snapshot.push((check, (*dev).clone()));
        }

        if multi_device {
            group_widget.set_description(Some(
                "All devices in this group must be passed together",
            ));
        }

        groups_box.append(&group_widget);
    }

    // Wire auto-check: when any device in a group is checked, check all group mates
    // We need the full buttons list first, then connect signals.
    let all_buttons: Rc<Vec<(CheckButton, PciDevice)>> = Rc::new(buttons_snapshot);
    let group_indices: Rc<BTreeMap<u32, Vec<usize>>> = Rc::new(group_indices);

    for (&group_id, indices) in group_indices.iter() {
        if indices.len() <= 1 {
            continue; // No auto-check needed for single-device groups
        }
        for &idx in indices {
            let all_buttons = Rc::clone(&all_buttons);
            let group_indices = Rc::clone(&group_indices);
            let (ref btn, _) = all_buttons[idx];
            let btn_clone = btn.clone();
            btn_clone.connect_toggled(move |b| {
                if b.is_active() {
                    if let Some(peers) = group_indices.get(&group_id) {
                        for &peer_idx in peers {
                            if peer_idx != idx {
                                all_buttons[peer_idx].0.set_active(true);
                            }
                        }
                    }
                }
            });
        }
    }

    // Move buttons into the shared Rc<RefCell<>>
    let mut shared = check_buttons.borrow_mut();
    for entry in Rc::try_unwrap(all_buttons)
        .unwrap_or_else(|rc| (*rc).clone())
        .into_iter()
    {
        shared.push(entry);
    }
}
