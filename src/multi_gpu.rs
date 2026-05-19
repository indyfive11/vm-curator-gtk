use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{Box as GtkBox, Button, CheckButton, Label, Orientation, ScrolledWindow, Spinner};
use libadwaita::{ActionRow, HeaderBar, PreferencesGroup, Toast, ToastOverlay, ToolbarView};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::hardware::{
    check_multi_gpu_passthrough_status, find_gpu_audio_pair, LookingGlassConfig,
    MultiGpuPassthroughStatus, PciDevice,
};
use vm_curator::vm::{save_pci_passthrough, DiscoveredVm};


fn badge_row(title: &str, ok: bool, subtitle: &str) -> ActionRow {
    let row = ActionRow::new();
    row.set_title(title);
    row.set_subtitle(subtitle);
    let lbl = Label::new(Some(if ok { "✓" } else { "✗" }));
    lbl.add_css_class(if ok { "success" } else { "error" });
    lbl.add_css_class("title-2");
    row.add_suffix(&lbl);
    row
}

pub fn show(parent: &impl IsA<gtk4::Widget>, vm: DiscoveredVm) {
    let dialog = libadwaita::Dialog::new();
    dialog.set_title("Multi-GPU / Looking Glass");
    dialog.set_content_width(580);
    dialog.set_content_height(540);

    let toast_overlay = ToastOverlay::new();

    // ── Spinner ────────────────────────────────────────────────────────────
    let spinner = Spinner::builder().spinning(true).halign(gtk4::Align::Center).build();
    let spinner_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .valign(gtk4::Align::Center)
        .spacing(8)
        .margin_top(32)
        .build();
    spinner_box.append(&spinner);
    let checking_lbl = Label::new(Some("Checking multi-GPU status…"));
    checking_lbl.add_css_class("dim-label");
    spinner_box.append(&checking_lbl);

    // ── Content ────────────────────────────────────────────────────────────
    let content_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    content_box.set_visible(false);

    let status_group = PreferencesGroup::new();
    status_group.set_title("System Status");
    content_box.append(&status_group);

    let gpu_group = PreferencesGroup::new();
    gpu_group.set_title("Passthrough GPUs");
    gpu_group.set_description(Some("Select a GPU to pass through to this VM"));
    content_box.append(&gpu_group);

    let errors_group = PreferencesGroup::new();
    errors_group.set_title("Warnings / Errors");
    errors_group.set_visible(false);
    content_box.append(&errors_group);

    // ── Apply button ───────────────────────────────────────────────────────
    let btn_box = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(4)
        .margin_bottom(12)
        .build();

    let apply_btn = Button::builder()
        .label("Apply")
        .sensitive(false)
        .build();
    apply_btn.add_css_class("suggested-action");

    let spacer = Label::new(None);
    spacer.set_hexpand(true);
    btn_box.append(&spacer);
    btn_box.append(&apply_btn);

    let scroll = ScrolledWindow::builder()
        .child(&content_box)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();

    let stack_box = GtkBox::builder().orientation(Orientation::Vertical).build();
    stack_box.append(&spinner_box);
    stack_box.append(&scroll);

    toast_overlay.set_child(Some(&stack_box));

    let main_box = GtkBox::new(Orientation::Vertical, 0);
    main_box.append(&toast_overlay);
    main_box.append(&btn_box);

    let header = HeaderBar::new();
    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&main_box));
    dialog.set_child(Some(&toolbar_view));

    // ── Background status check ────────────────────────────────────────────
    let (tx, rx) = mpsc::channel::<MultiGpuPassthroughStatus>();
    std::thread::spawn(move || {
        tx.send(check_multi_gpu_passthrough_status()).ok();
    });

    let vm = Rc::new(vm);
    let spinner_box_c = spinner_box.clone();
    let content_box_c = content_box.clone();
    let status_group_c = status_group.clone();
    let gpu_group_c = gpu_group.clone();
    let errors_group_c = errors_group.clone();
    let apply_btn_c = apply_btn.clone();
    let toast_overlay_c = toast_overlay.clone();
    let rx = Rc::new(RefCell::new(rx));

    // Track which CheckButton is selected and which GPU it corresponds to
    let selected_gpu: Rc<RefCell<Option<PciDevice>>> = Rc::new(RefCell::new(None));
    let gpu_buttons: Rc<RefCell<Vec<(CheckButton, PciDevice)>>> = Rc::new(RefCell::new(Vec::new()));

    gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
        match rx.borrow().try_recv() {
            Ok(status) => {
                spinner_box_c.set_visible(false);
                content_box_c.set_visible(true);

                // Status badges
                status_group_c.add(&badge_row(
                    "IOMMU",
                    status.iommu_enabled,
                    if status.iommu_enabled { "Enabled" } else { "Not enabled — add intel_iommu=on or amd_iommu=on" },
                ));
                status_group_c.add(&badge_row(
                    "VFIO",
                    status.vfio_loaded,
                    if status.vfio_loaded { "Modules loaded" } else { "Run: sudo modprobe vfio-pci" },
                ));
                let lg_path = LookingGlassConfig::find_client();
                let lg_found = lg_path.is_some();
                status_group_c.add(&badge_row(
                    "Looking Glass Client",
                    lg_found,
                    &lg_path
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Not found — install looking-glass-client".to_string()),
                ));

                // GPU list
                if status.passthrough_gpus.is_empty() {
                    let row = ActionRow::new();
                    row.set_title("No passthrough-capable GPUs found");
                    row.set_subtitle("A secondary GPU (non-boot VGA) with an IOMMU group is required");
                    gpu_group_c.add(&row);
                } else {
                    // Enumerate all PCI devices once for audio pair detection
                    let all_devices = vm_curator::hardware::enumerate_pci_devices()
                        .unwrap_or_default();

                    let mut first = true;
                    let mut group_btn: Option<CheckButton> = None;

                    for gpu in &status.passthrough_gpus {
                        let audio = find_gpu_audio_pair(gpu, &all_devices);
                        let row = ActionRow::new();
                        row.set_title(&format!("{} {}", gpu.vendor_name, gpu.device_name));
                        let mut sub = format!("PCI {}", gpu.address);
                        if let Some(group) = gpu.iommu_group {
                            sub.push_str(&format!(" · IOMMU group {group}"));
                        }
                        if let Some(ref a) = audio {
                            sub.push_str(&format!(" + audio {}", a.address));
                        }
                        row.set_subtitle(&sub);

                        let check = CheckButton::new();
                        check.set_valign(gtk4::Align::Center);
                        if first {
                            first = false;
                        } else if let Some(ref grp) = group_btn {
                            check.set_group(Some(grp));
                        }
                        if group_btn.is_none() {
                            group_btn = Some(check.clone());
                        }

                        row.add_prefix(&check);
                        row.set_activatable_widget(Some(&check));

                        let gpu_clone = gpu.clone();
                        let selected_gpu_ref = Rc::clone(&selected_gpu);
                        let apply_btn_ref = apply_btn_c.clone();
                        check.connect_toggled(move |btn| {
                            if btn.is_active() {
                                *selected_gpu_ref.borrow_mut() = Some(gpu_clone.clone());
                                apply_btn_ref.set_sensitive(true);
                            }
                        });

                        gpu_buttons.borrow_mut().push((check, gpu.clone()));
                        gpu_group_c.add(&row);
                    }
                }

                // Errors / warnings
                if !status.errors.is_empty() || !status.warnings.is_empty() {
                    errors_group_c.set_visible(true);
                    for e in &status.errors {
                        let row = ActionRow::new();
                        row.set_title(e.as_str());
                        row.add_css_class("error");
                        errors_group_c.add(&row);
                    }
                    for w in &status.warnings {
                        let row = ActionRow::new();
                        row.set_title(w.as_str());
                        errors_group_c.add(&row);
                    }
                }

                // Apply button handler (wired after GPU list is built)
                {
                    let vm_ref = Rc::clone(&vm);
                    let selected_gpu_ref = Rc::clone(&selected_gpu);
                    let toast_ref = toast_overlay_c.clone();
                    let all_devices = vm_curator::hardware::enumerate_pci_devices()
                        .unwrap_or_default();

                    apply_btn_c.connect_clicked(move |btn| {
                        let Some(gpu) = selected_gpu_ref.borrow().clone() else { return };
                        let audio = find_gpu_audio_pair(&gpu, &all_devices);
                        let mut devices = vec![gpu];
                        if let Some(a) = audio {
                            devices.push(a);
                        }

                        btn.set_sensitive(false);
                        btn.set_label("Applying…");

                        let vm_clone = (*vm_ref).clone();
                        let (tx2, rx2) = mpsc::channel::<Result<(), String>>();
                        std::thread::spawn(move || {
                            let result = save_pci_passthrough(&vm_clone, &devices)
                                .map_err(|e| e.to_string());
                            tx2.send(result).ok();
                        });

                        let btn2 = btn.clone();
                        let toast = toast_ref.clone();
                        let rx2 = Rc::new(RefCell::new(rx2));
                        gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
                            match rx2.borrow().try_recv() {
                                Ok(Ok(())) => {
                                    toast.add_toast(Toast::new("PCI passthrough saved."));
                                    btn2.set_sensitive(true);
                                    btn2.set_label("Apply");
                                    gtk4::glib::ControlFlow::Break
                                }
                                Ok(Err(e)) => {
                                    toast.add_toast(
                                        Toast::builder()
                                            .title(&format!("Failed: {e}"))
                                            .timeout(0)
                                            .build(),
                                    );
                                    btn2.set_sensitive(true);
                                    btn2.set_label("Apply");
                                    gtk4::glib::ControlFlow::Break
                                }
                                Err(mpsc::TryRecvError::Empty) => gtk4::glib::ControlFlow::Continue,
                                Err(mpsc::TryRecvError::Disconnected) => gtk4::glib::ControlFlow::Break,
                            }
                        });
                    });
                }

                gtk4::glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => gtk4::glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => gtk4::glib::ControlFlow::Break,
        }
    });

    dialog.present(Some(parent));
}
