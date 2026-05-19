use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{Box as GtkBox, Button, Label, Orientation, ScrolledWindow, Spinner};
use libadwaita::{ActionRow, HeaderBar, PreferencesGroup, Toast, ToastOverlay, ToolbarView};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::hardware::{check_single_gpu_support, scripts_exist, SingleGpuConfig, SingleGpuSupport};
use vm_curator::vm::{delete_scripts, generate_single_gpu_scripts, DiscoveredVm};

enum SupportResult {
    Ok(SingleGpuSupport),
    Err(String),
}

fn status_icon(ok: bool) -> &'static str {
    if ok { "✓" } else { "✗" }
}

fn status_css(ok: bool) -> &'static str {
    if ok { "success" } else { "error" }
}

fn make_status_row(title: &str, ok: bool, subtitle: &str) -> ActionRow {
    let row = ActionRow::new();
    row.set_title(title);
    row.set_subtitle(subtitle);
    let icon = Label::new(Some(status_icon(ok)));
    icon.add_css_class(status_css(ok));
    icon.add_css_class("title-2");
    row.add_suffix(&icon);
    row
}

pub fn show(parent: &impl IsA<gtk4::Widget>, vm: DiscoveredVm) {
    let dialog = libadwaita::Dialog::new();
    dialog.set_title("Single GPU Passthrough");
    dialog.set_content_width(560);
    dialog.set_content_height(500);

    let toast_overlay = ToastOverlay::new();

    // ── Spinner (shown while checking support) ──────────────────────────────
    let spinner = Spinner::builder().spinning(true).halign(gtk4::Align::Center).build();
    let spinner_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .valign(gtk4::Align::Center)
        .spacing(8)
        .margin_top(32)
        .build();
    spinner_box.append(&spinner);
    let checking_lbl = Label::new(Some("Checking system support…"));
    checking_lbl.add_css_class("dim-label");
    spinner_box.append(&checking_lbl);

    // ── Content area (shown after check) ───────────────────────────────────
    let content_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    content_box.set_visible(false);

    let prereq_group = PreferencesGroup::new();
    prereq_group.set_title("Prerequisites");
    content_box.append(&prereq_group);

    let gpu_group = PreferencesGroup::new();
    gpu_group.set_title("GPU Information");
    gpu_group.set_visible(false);
    content_box.append(&gpu_group);

    let script_group = PreferencesGroup::new();
    script_group.set_title("Scripts");
    script_group.set_visible(false);
    content_box.append(&script_group);

    // ── Buttons ────────────────────────────────────────────────────────────
    let btn_box = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(4)
        .margin_bottom(12)
        .build();

    let generate_btn = Button::builder()
        .label("Generate Scripts")
        .sensitive(false)
        .build();
    generate_btn.add_css_class("suggested-action");

    let delete_btn = Button::builder()
        .label("Delete Scripts")
        .sensitive(false)
        .build();
    delete_btn.add_css_class("destructive-action");

    let spacer = Label::new(None);
    spacer.set_hexpand(true);
    btn_box.append(&spacer);
    btn_box.append(&delete_btn);
    btn_box.append(&generate_btn);

    let scroll = ScrolledWindow::builder()
        .child(&content_box)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();

    let stack_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .build();
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

    // ── Background support check ───────────────────────────────────────────
    let (tx, rx) = mpsc::channel::<SupportResult>();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(|| check_single_gpu_support());
        match result {
            Ok(s) => tx.send(SupportResult::Ok(s)).ok(),
            Err(_) => tx.send(SupportResult::Err("Panic during support check".into())).ok(),
        };
    });

    let spinner_box_c = spinner_box.clone();
    let content_box_c = content_box.clone();
    let prereq_group_c = prereq_group.clone();
    let gpu_group_c = gpu_group.clone();
    let script_group_c = script_group.clone();
    let generate_btn_c = generate_btn.clone();
    let delete_btn_c = delete_btn.clone();
    let toast_overlay_c = toast_overlay.clone();
    let rx = Rc::new(std::cell::RefCell::new(rx));

    let vm_for_generate = Rc::new(vm);

    gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
        match rx.borrow().try_recv() {
            Ok(SupportResult::Ok(support)) => {
                spinner_box_c.set_visible(false);
                content_box_c.set_visible(true);

                // Prerequisites
                prereq_group_c.add(&make_status_row(
                    "IOMMU",
                    support.iommu_enabled,
                    if support.iommu_enabled { "Enabled in kernel" } else { "Add intel_iommu=on or amd_iommu=on to kernel parameters" },
                ));
                prereq_group_c.add(&make_status_row(
                    "VFIO Modules",
                    support.vfio_available,
                    if support.vfio_available { "vfio-pci loaded" } else { "Run: sudo modprobe vfio-pci" },
                ));
                let has_gpu = support.boot_vga.is_some();
                prereq_group_c.add(&make_status_row(
                    "Boot GPU",
                    has_gpu,
                    if has_gpu { "Boot VGA device found" } else { "No boot VGA device detected" },
                ));

                if support.is_supported() {
                    gpu_group_c.set_visible(true);
                    script_group_c.set_visible(true);

                    // GPU info
                    if let Some(ref gpu) = support.boot_vga {
                        let gpu_row = ActionRow::new();
                        gpu_row.set_title(&format!("{} {}", gpu.vendor_name, gpu.device_name));
                        gpu_row.set_subtitle(&format!("PCI {}", gpu.address));
                        gpu_group_c.add(&gpu_row);

                        if let Some(ref dm) = support.display_manager {
                            let dm_row = ActionRow::new();
                            dm_row.set_title("Display Manager");
                            dm_row.set_subtitle(dm.display_name());
                            gpu_group_c.add(&dm_row);
                        }

                        // Construct config for generate
                        let all_devices = vm_curator::hardware::enumerate_pci_devices()
                            .unwrap_or_default();
                        let config = SingleGpuConfig::new(gpu.clone(), &all_devices);
                        let vm_ref = Rc::clone(&vm_for_generate);
                        let toast_ref = toast_overlay_c.clone();
                        let generate_btn_ref = generate_btn_c.clone();
                        let delete_btn_ref2 = delete_btn_c.clone();
                        let script_group_ref = script_group_c.clone();

                        // Scripts info row
                        let scripts_path_row = ActionRow::new();
                        scripts_path_row.set_title("Start script");
                        scripts_path_row.set_subtitle(
                            &vm_ref.path.join("single-gpu-start.sh").to_string_lossy()
                        );
                        script_group_ref.add(&scripts_path_row);

                        let restore_path_row = ActionRow::new();
                        restore_path_row.set_title("Restore script");
                        restore_path_row.set_subtitle(
                            &vm_ref.path.join("single-gpu-restore.sh").to_string_lossy()
                        );
                        script_group_ref.add(&restore_path_row);

                        let have_scripts = scripts_exist(&vm_ref.path);
                        generate_btn_c.set_sensitive(true);
                        delete_btn_c.set_sensitive(have_scripts);

                        // Generate button
                        {
                            let vm_ref = Rc::clone(&vm_ref);
                            let toast_ref = toast_ref.clone();
                            let config = config.clone();
                            let delete_btn_ref = delete_btn_ref2.clone();
                            generate_btn_ref.connect_clicked(move |btn| {
                                btn.set_sensitive(false);
                                btn.set_label("Generating…");
                                let vm_owned = (*vm_ref).clone();
                                let config = config.clone();
                                let (tx2, rx2) = mpsc::channel::<Result<(), String>>();
                                std::thread::spawn(move || {
                                    let result = generate_single_gpu_scripts(&vm_owned, &config)
                                        .map(|_| ())
                                        .map_err(|e| e.to_string());
                                    tx2.send(result).ok();
                                });
                                let btn2 = btn.clone();
                                let toast = toast_ref.clone();
                                let del_btn = delete_btn_ref.clone();
                                let rx2 = Rc::new(std::cell::RefCell::new(rx2));
                                gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
                                    match rx2.borrow().try_recv() {
                                        Ok(Ok(())) => {
                                            toast.add_toast(Toast::new("Scripts generated successfully."));
                                            btn2.set_sensitive(true);
                                            btn2.set_label("Generate Scripts");
                                            del_btn.set_sensitive(true);
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
                                            btn2.set_label("Generate Scripts");
                                            gtk4::glib::ControlFlow::Break
                                        }
                                        Err(mpsc::TryRecvError::Empty) => gtk4::glib::ControlFlow::Continue,
                                        Err(mpsc::TryRecvError::Disconnected) => gtk4::glib::ControlFlow::Break,
                                    }
                                });
                            });
                        }

                        // Delete button
                        {
                            let vm_ref = Rc::clone(&vm_ref);
                            let toast_ref = toast_ref.clone();
                            delete_btn_ref2.connect_clicked(move |btn| {
                                let path = vm_ref.path.clone();
                                let (tx3, rx3) = mpsc::channel::<Result<(), String>>();
                                std::thread::spawn(move || {
                                    let result = delete_scripts(&path)
                                        .map_err(|e| e.to_string());
                                    tx3.send(result).ok();
                                });
                                let btn2 = btn.clone();
                                let toast = toast_ref.clone();
                                let rx3 = Rc::new(std::cell::RefCell::new(rx3));
                                gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
                                    match rx3.borrow().try_recv() {
                                        Ok(Ok(())) => {
                                            toast.add_toast(Toast::new("Scripts deleted."));
                                            btn2.set_sensitive(false);
                                            gtk4::glib::ControlFlow::Break
                                        }
                                        Ok(Err(e)) => {
                                            toast.add_toast(
                                                Toast::builder()
                                                    .title(&format!("Delete failed: {e}"))
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
                    }
                } else {
                    // Not supported — show summary label
                    let summary_row = ActionRow::new();
                    summary_row.set_title("Status");
                    summary_row.set_subtitle(&support.summary());
                    prereq_group_c.add(&summary_row);
                }

                gtk4::glib::ControlFlow::Break
            }
            Ok(SupportResult::Err(e)) => {
                spinner_box_c.set_visible(false);
                toast_overlay_c.add_toast(
                    Toast::builder().title(&format!("Check failed: {e}")).timeout(0).build(),
                );
                gtk4::glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => gtk4::glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => gtk4::glib::ControlFlow::Break,
        }
    });

    dialog.present(Some(parent));
}
