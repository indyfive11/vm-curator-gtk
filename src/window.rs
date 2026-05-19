use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{
    glib, Box as GtkBox, Button, Label, Orientation, Paned, ScrolledWindow, SearchEntry,
    SelectionMode, Separator,
};
use libadwaita::{
    ActionRow, AlertDialog, ApplicationWindow, HeaderBar, ResponseAppearance, Toast,
    ToastOverlay, ToolbarView,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::config::Config;
use vm_curator::vm::{
    delete_vm, detect_qemu_processes, discover_vms, ensure_qmp_in_script,
    force_stop_vm, is_vm_paused, launch_vm_with_error_check, pause_vm,
    rename_vm, reset_vm, resume_vm, stop_vm_by_pid,
    BootMode, DiscoveredVm, LaunchOptions,
};

pub fn build_and_show(app: &libadwaita::Application) {
    let config: Rc<RefCell<Config>> =
        Rc::new(RefCell::new(Config::load().unwrap_or_default()));

    // --- State ---
    let vms: Rc<RefCell<Vec<DiscoveredVm>>> = Rc::new(RefCell::new(Vec::new()));
    let selected: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));
    let running_pids: Rc<RefCell<Vec<Option<u32>>>> = Rc::new(RefCell::new(Vec::new()));
    let vm_paused: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
    let rows: Rc<RefCell<Vec<ActionRow>>> = Rc::new(RefCell::new(Vec::new()));

    // --- Left panel ---
    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(SelectionMode::Single);
    list_box.add_css_class("navigation-sidebar");

    let search_entry = SearchEntry::builder()
        .placeholder_text("Filter VMs…")
        .margin_start(8)
        .margin_end(8)
        .margin_top(8)
        .margin_bottom(4)
        .build();

    let list_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .child(&list_box)
        .vexpand(true)
        .build();

    let left_box = GtkBox::new(Orientation::Vertical, 0);
    left_box.set_width_request(240);
    left_box.append(&search_entry);
    left_box.append(&list_scroll);

    // --- Right panel widgets ---
    let detail_label = Label::builder()
        .label("Select a VM from the list.")
        .halign(gtk4::Align::Start)
        .valign(gtk4::Align::Start)
        .wrap(true)
        .margin_start(16)
        .margin_top(16)
        .margin_end(16)
        .margin_bottom(4)
        .build();
    detail_label.add_css_class("dim-label");

    let notes_label = Label::builder()
        .halign(gtk4::Align::Start)
        .wrap(true)
        .margin_start(16)
        .margin_end(16)
        .margin_bottom(8)
        .visible(false)
        .build();
    notes_label.add_css_class("dim-label");

    // Primary actions
    let launch_btn = make_btn("Launch VM", false);
    launch_btn.add_css_class("suggested-action");
    let stop_btn = make_btn("Stop VM", false);
    let pause_btn = make_btn("Pause", false);
    let force_stop_btn = make_btn("Force Stop", false);
    force_stop_btn.add_css_class("destructive-action");

    // VM dialog buttons
    let snapshots_btn = make_btn("Snapshots…", false);
    let boot_btn = make_btn("Boot Options…", false);

    // VM config buttons (stubs — wired in later tiers)
    let network_btn = make_btn("Network Settings…", false);
    let folders_btn = make_btn("Shared Folders…", false);
    let display_btn = make_btn("Display Options…", false);
    let usb_btn = make_btn("USB Passthrough…", false);
    let pci_btn = make_btn("PCI Passthrough…", false);
    let raw_config_btn = make_btn("Edit Raw Config…", false);
    let single_gpu_btn = make_btn("Single GPU Setup…", false);
    let multi_gpu_btn = make_btn("Multi-GPU Setup…", false);

    // VM metadata buttons
    let notes_btn = make_btn("Edit Notes…", false);
    let rename_btn = make_btn("Rename…", false);
    let reset_btn = make_btn("Reset VM…", false);
    let delete_btn = make_btn("Delete VM…", false);
    delete_btn.add_css_class("destructive-action");

    // --- Right panel layout ---
    let right_box = GtkBox::new(Orientation::Vertical, 0);
    right_box.set_hexpand(true);
    right_box.set_vexpand(true);

    right_box.append(&detail_label);
    right_box.append(&notes_label);
    right_box.append(&Separator::new(Orientation::Horizontal));

    right_box.append(&btn_section(&[
        &[&launch_btn, &stop_btn],
        &[&pause_btn, &force_stop_btn],
    ], 8, 4));

    right_box.append(&Separator::new(Orientation::Horizontal));
    right_box.append(&btn_section(&[
        &[&snapshots_btn, &boot_btn],
    ], 8, 8));

    right_box.append(&Separator::new(Orientation::Horizontal));
    right_box.append(&btn_section(&[
        &[&network_btn, &folders_btn],
        &[&display_btn, &usb_btn],
        &[&pci_btn, &raw_config_btn],
        &[&single_gpu_btn, &multi_gpu_btn],
    ], 8, 8));

    right_box.append(&Separator::new(Orientation::Horizontal));
    right_box.append(&btn_section(&[
        &[&notes_btn, &rename_btn],
        &[&reset_btn, &delete_btn],
    ], 8, 12));

    let right_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .child(&right_box)
        .hexpand(true)
        .vexpand(true)
        .build();

    // --- Toast overlay ---
    let toast_overlay = ToastOverlay::new();

    // --- refresh_vm_list ---
    let refresh_vm_list = {
        let list_box = list_box.clone();
        let vms = Rc::clone(&vms);
        let rows = Rc::clone(&rows);
        let running_pids = Rc::clone(&running_pids);
        let vm_paused = Rc::clone(&vm_paused);
        let config = Rc::clone(&config);

        move || {
            list_box.unselect_all();
            while let Some(child) = list_box.first_child() {
                list_box.remove(&child);
            }
            let new_vms =
                discover_vms(&config.borrow().vm_library_path).unwrap_or_default();
            let count = new_vms.len();
            *vms.borrow_mut() = new_vms;
            *running_pids.borrow_mut() = vec![None; count];
            *vm_paused.borrow_mut() = vec![false; count];

            let mut new_rows: Vec<ActionRow> = Vec::with_capacity(count);
            if vms.borrow().is_empty() {
                let placeholder = Label::new(Some("No VMs found.\nConfigure vm-curator first."));
                placeholder.set_justify(gtk4::Justification::Center);
                placeholder.add_css_class("dim-label");
                placeholder.set_margin_top(24);
                placeholder.set_margin_bottom(24);
                list_box.append(&placeholder);
            } else {
                for vm in vms.borrow().iter() {
                    let row = ActionRow::new();
                    row.set_title(&vm.display_name());
                    row.set_subtitle(&vm.id);
                    list_box.append(&row);
                    new_rows.push(row);
                }
            }
            *rows.borrow_mut() = new_rows;
        }
    };

    // --- Row selection ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        let running_pids = Rc::clone(&running_pids);
        let vm_paused = Rc::clone(&vm_paused);
        let detail_label = detail_label.clone();
        let notes_label = notes_label.clone();
        let launch_btn = launch_btn.clone();
        let stop_btn = stop_btn.clone();
        let pause_btn = pause_btn.clone();
        let force_stop_btn = force_stop_btn.clone();
        let snapshots_btn = snapshots_btn.clone();
        let boot_btn = boot_btn.clone();
        let network_btn = network_btn.clone();
        let folders_btn = folders_btn.clone();
        let display_btn = display_btn.clone();
        let usb_btn = usb_btn.clone();
        let pci_btn = pci_btn.clone();
        let raw_config_btn = raw_config_btn.clone();
        let single_gpu_btn = single_gpu_btn.clone();
        let multi_gpu_btn = multi_gpu_btn.clone();
        let notes_btn = notes_btn.clone();
        let rename_btn = rename_btn.clone();
        let reset_btn = reset_btn.clone();
        let delete_btn = delete_btn.clone();

        list_box.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let idx = row.index() as usize;
                let vm = {
                    let vms = vms.borrow();
                    if idx >= vms.len() {
                        return;
                    }
                    vms[idx].clone()
                };
                *selected.borrow_mut() = Some(idx);
                let is_running = running_pids.borrow()[idx].is_some();

                detail_label.set_label(&format!(
                    "<b>{}</b>\n\nCores: {}   RAM: {} MB   KVM: {}\nPath: {}",
                    vm.display_name(),
                    vm.config.cpu_cores,
                    vm.config.memory_mb,
                    vm.config.enable_kvm,
                    vm.path.display(),
                ));
                detail_label.set_use_markup(true);
                detail_label.remove_css_class("dim-label");

                if let Some(ref notes) = vm.notes {
                    notes_label.set_label(&format!(
                        "<i>{}</i>",
                        glib::markup_escape_text(notes)
                    ));
                    notes_label.set_use_markup(true);
                    notes_label.set_visible(true);
                } else {
                    notes_label.set_visible(false);
                }

                let is_paused = vm_paused.borrow().get(idx).copied().unwrap_or(false);
                launch_btn.set_sensitive(!is_running);
                launch_btn.set_label(if is_running { "Running" } else { "Launch VM" });
                stop_btn.set_sensitive(is_running);
                pause_btn.set_sensitive(is_running);
                pause_btn.set_label(if is_paused { "Resume" } else { "Pause" });
                force_stop_btn.set_sensitive(is_running);

                snapshots_btn.set_sensitive(!vm.config.disks.is_empty());
                boot_btn.set_sensitive(true);
                network_btn.set_sensitive(true);
                folders_btn.set_sensitive(true);
                display_btn.set_sensitive(true);
                usb_btn.set_sensitive(true);
                pci_btn.set_sensitive(true);
                raw_config_btn.set_sensitive(true);
                single_gpu_btn.set_sensitive(true);
                multi_gpu_btn.set_sensitive(true);
                notes_btn.set_sensitive(true);
                rename_btn.set_sensitive(true);
                reset_btn.set_sensitive(true);
                delete_btn.set_sensitive(true);
            } else {
                *selected.borrow_mut() = None;
                detail_label.set_label("Select a VM from the list.");
                detail_label.set_use_markup(false);
                detail_label.add_css_class("dim-label");
                notes_label.set_visible(false);

                launch_btn.set_sensitive(false);
                launch_btn.set_label("Launch VM");
                stop_btn.set_sensitive(false);
                pause_btn.set_sensitive(false);
                pause_btn.set_label("Pause");
                force_stop_btn.set_sensitive(false);
                snapshots_btn.set_sensitive(false);
                boot_btn.set_sensitive(false);
                network_btn.set_sensitive(false);
                folders_btn.set_sensitive(false);
                display_btn.set_sensitive(false);
                usb_btn.set_sensitive(false);
                pci_btn.set_sensitive(false);
                raw_config_btn.set_sensitive(false);
                single_gpu_btn.set_sensitive(false);
                multi_gpu_btn.set_sensitive(false);
                notes_btn.set_sensitive(false);
                rename_btn.set_sensitive(false);
                reset_btn.set_sensitive(false);
                delete_btn.set_sensitive(false);
            }
        });
    }

    // --- Shared launch logic (used by Launch VM button and Boot Options dialog) ---
    let do_launch: Rc<dyn Fn(DiscoveredVm, LaunchOptions)> = {
        let toast_overlay = toast_overlay.clone();
        let launch_btn = launch_btn.clone();
        Rc::new(move |vm: DiscoveredVm, options: LaunchOptions| {
            launch_btn.set_sensitive(false);
            launch_btn.set_label("Launching…");

            // Ensure QMP socket is present in the launch script (idempotent)
            if let Err(e) = ensure_qmp_in_script(&vm.path) {
                log::warn!("Could not ensure QMP in launch script for {}: {}", vm.display_name(), e);
            }

            // Detect SPICE mode before moving vm into the thread
            let is_spice = std::fs::read_to_string(&vm.launch_script)
                .map(|s| s.contains("spice.sock"))
                .unwrap_or(false);
            let spice_sock_path = vm.path.join("spice.sock");

            let (tx, rx) = mpsc::channel::<(String, bool, Option<String>)>();
            let rx = Rc::new(RefCell::new(rx));
            std::thread::spawn(move || {
                let result = launch_vm_with_error_check(&vm, &options);
                tx.send((result.vm_name, result.success, result.error)).ok();
            });

            let launch_btn = launch_btn.clone();
            let toast_overlay = toast_overlay.clone();
            glib::timeout_add_local(Duration::from_millis(200), move || {
                match rx.borrow().try_recv() {
                    Ok((vm_name, success, error)) => {
                        if success {
                            if is_spice {
                                let sock = spice_sock_path.clone();
                                std::thread::spawn(move || {
                                    std::thread::sleep(Duration::from_secs(2));
                                    let _ = std::process::Command::new("remote-viewer")
                                        .arg(format!("spice+unix://{}", sock.display()))
                                        .spawn();
                                });
                                toast_overlay.add_toast(Toast::new(
                                    &format!("{vm_name} launched — opening SPICE viewer…"),
                                ));
                            } else {
                                toast_overlay
                                    .add_toast(Toast::new(&format!("{vm_name} launched")));
                            }
                        } else {
                            launch_btn.set_sensitive(true);
                            launch_btn.set_label("Launch VM");
                            let msg = error.as_deref().unwrap_or("unknown error");
                            toast_overlay.add_toast(
                                Toast::builder()
                                    .title(&format!("Failed to launch {vm_name}: {msg}"))
                                    .timeout(0)
                                    .build(),
                            );
                        }
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        launch_btn.set_sensitive(true);
                        launch_btn.set_label("Launch VM");
                        glib::ControlFlow::Break
                    }
                }
            });
        })
    };

    // --- Launch (Normal boot) ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        let do_launch = Rc::clone(&do_launch);

        launch_btn.connect_clicked(move |_| {
            let idx = match *selected.borrow() {
                Some(i) => i,
                None => return,
            };
            let vm = {
                let vms = vms.borrow();
                if idx >= vms.len() {
                    return;
                }
                vms[idx].clone()
            };
            do_launch(vm, LaunchOptions {
                boot_mode: BootMode::Normal,
                extra_args: Vec::new(),
                usb_devices: Vec::new(),
            });
        });
    }

    // --- Stop ---
    {
        let selected = Rc::clone(&selected);
        let running_pids = Rc::clone(&running_pids);
        let toast_overlay = toast_overlay.clone();

        stop_btn.connect_clicked(move |_| {
            let idx = match *selected.borrow() {
                Some(i) => i,
                None => return,
            };
            let pid = match running_pids.borrow()[idx] {
                Some(p) => p,
                None => return,
            };
            match stop_vm_by_pid(pid) {
                Ok(_) => toast_overlay.add_toast(Toast::new("Shutdown signal sent.")),
                Err(e) => toast_overlay.add_toast(
                    Toast::builder()
                        .title(&format!("Stop failed: {e}"))
                        .timeout(0)
                        .build(),
                ),
            }
        });
    }

    // --- Force Stop ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        let running_pids = Rc::clone(&running_pids);
        let toast_overlay = toast_overlay.clone();

        force_stop_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() {
                Some(i) => i,
                None => return,
            };
            let pid = match running_pids.borrow()[idx] {
                Some(p) => p,
                None => return,
            };
            let vm_name = {
                let vms = vms.borrow();
                if idx >= vms.len() {
                    return;
                }
                vms[idx].display_name()
            };

            let alert = AlertDialog::builder()
                .heading("Force Stop VM?")
                .body(&format!(
                    "Force kill \"{vm_name}\"? The guest OS will not shut down cleanly."
                ))
                .build();
            alert.add_response("cancel", "Cancel");
            alert.add_response("force", "Force Stop");
            alert.set_response_appearance("force", ResponseAppearance::Destructive);
            alert.set_default_response(Some("cancel"));
            alert.set_close_response("cancel");

            let toast_overlay = toast_overlay.clone();
            alert.connect_response(None, move |_, response| {
                if response != "force" {
                    return;
                }
                match force_stop_vm(pid) {
                    Ok(_) => toast_overlay.add_toast(Toast::new("VM force stopped.")),
                    Err(e) => toast_overlay.add_toast(
                        Toast::builder()
                            .title(&format!("Force stop failed: {e}"))
                            .timeout(0)
                            .build(),
                    ),
                }
            });

            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                alert.present(Some(&win));
            }
        });
    }

    // --- Pause / Resume ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        let vm_paused = Rc::clone(&vm_paused);

        pause_btn.connect_clicked(move |_| {
            let idx = match *selected.borrow() { Some(i) => i, None => return };
            let vm_path = {
                let vms = vms.borrow();
                if idx >= vms.len() { return; }
                vms[idx].path.clone()
            };
            let paused = vm_paused.borrow().get(idx).copied().unwrap_or(false);
            std::thread::spawn(move || {
                let result = if paused { resume_vm(&vm_path) } else { pause_vm(&vm_path) };
                if let Err(e) = result {
                    log::warn!("{} failed: {}", if paused { "resume_vm" } else { "pause_vm" }, e);
                }
            });
        });
    }

    // --- Delete ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        let toast_overlay = toast_overlay.clone();
        let refresh_vm_list = refresh_vm_list.clone();

        delete_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() {
                Some(i) => i,
                None => return,
            };
            let vm = {
                let vms = vms.borrow();
                if idx >= vms.len() {
                    return;
                }
                vms[idx].clone()
            };
            let vm_name = vm.display_name();

            let alert = AlertDialog::builder()
                .heading("Delete VM?")
                .body(&format!("What do you want to do with \"{vm_name}\"?"))
                .build();
            alert.add_response("cancel", "Cancel");
            alert.add_response("archive", "Move to Trash");
            alert.add_response("delete", "Delete Permanently");
            alert.set_response_appearance("delete", ResponseAppearance::Destructive);
            alert.set_default_response(Some("cancel"));
            alert.set_close_response("cancel");

            let toast_overlay = toast_overlay.clone();
            let refresh_vm_list = refresh_vm_list.clone();
            alert.connect_response(None, move |_, response| {
                let permanent = if response == "archive" {
                    false
                } else if response == "delete" {
                    true
                } else {
                    return;
                };
                let (tx, rx) = mpsc::channel::<Result<(), String>>();
                let vm = vm.clone();
                std::thread::spawn(move || {
                    let result = delete_vm(&vm, permanent).map_err(|e| e.to_string());
                    tx.send(result).ok();
                });

                let toast_overlay = toast_overlay.clone();
                let refresh_vm_list = refresh_vm_list.clone();
                let rx = Rc::new(RefCell::new(rx));
                glib::timeout_add_local(Duration::from_millis(200), move || {
                    match rx.borrow().try_recv() {
                        Ok(Ok(())) => {
                            toast_overlay.add_toast(Toast::new("VM deleted."));
                            refresh_vm_list();
                            glib::ControlFlow::Break
                        }
                        Ok(Err(e)) => {
                            toast_overlay.add_toast(
                                Toast::builder()
                                    .title(&format!("Delete failed: {e}"))
                                    .timeout(0)
                                    .build(),
                            );
                            glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                    }
                });
            });

            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                alert.present(Some(&win));
            }
        });
    }

    // --- Rename ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        let toast_overlay = toast_overlay.clone();
        let refresh_vm_list = refresh_vm_list.clone();

        rename_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() {
                Some(i) => i,
                None => return,
            };
            let vm = {
                let vms = vms.borrow();
                if idx >= vms.len() {
                    return;
                }
                vms[idx].clone()
            };

            let entry = gtk4::Entry::builder()
                .text(&vm.display_name())
                .activates_default(true)
                .build();

            let alert = AlertDialog::builder()
                .heading("Rename VM")
                .build();
            alert.set_extra_child(Some(&entry));
            alert.add_response("cancel", "Cancel");
            alert.add_response("rename", "Rename");
            alert.set_response_appearance("rename", ResponseAppearance::Suggested);
            alert.set_default_response(Some("rename"));
            alert.set_close_response("cancel");

            let toast_overlay = toast_overlay.clone();
            let refresh_vm_list = refresh_vm_list.clone();
            alert.connect_response(None, move |_, response| {
                if response != "rename" {
                    return;
                }
                let new_name = entry.text().trim().to_string();
                if new_name.is_empty() {
                    return;
                }
                let (tx, rx) = mpsc::channel::<Result<(), String>>();
                let vm = vm.clone();
                let name = new_name.clone();
                std::thread::spawn(move || {
                    let result = rename_vm(&vm, &name).map_err(|e| e.to_string());
                    tx.send(result).ok();
                });

                let toast_overlay = toast_overlay.clone();
                let refresh_vm_list = refresh_vm_list.clone();
                let rx = Rc::new(RefCell::new(rx));
                glib::timeout_add_local(Duration::from_millis(200), move || {
                    match rx.borrow().try_recv() {
                        Ok(Ok(())) => {
                            toast_overlay.add_toast(Toast::new("VM renamed."));
                            refresh_vm_list();
                            glib::ControlFlow::Break
                        }
                        Ok(Err(e)) => {
                            toast_overlay.add_toast(
                                Toast::builder()
                                    .title(&format!("Rename failed: {e}"))
                                    .timeout(0)
                                    .build(),
                            );
                            glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                    }
                });
            });

            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                alert.present(Some(&win));
            }
        });
    }

    // --- Reset ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        let toast_overlay = toast_overlay.clone();

        reset_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() {
                Some(i) => i,
                None => return,
            };
            let vm = {
                let vms = vms.borrow();
                if idx >= vms.len() {
                    return;
                }
                vms[idx].clone()
            };
            let vm_name = vm.display_name();

            let alert = AlertDialog::builder()
                .heading("Reset VM?")
                .body(&format!(
                    "Reset \"{vm_name}\" to its initial disk state? This cannot be undone."
                ))
                .build();
            alert.add_response("cancel", "Cancel");
            alert.add_response("reset", "Reset");
            alert.set_response_appearance("reset", ResponseAppearance::Destructive);
            alert.set_default_response(Some("cancel"));
            alert.set_close_response("cancel");

            let toast_overlay = toast_overlay.clone();
            alert.connect_response(None, move |_, response| {
                if response != "reset" {
                    return;
                }
                let (tx, rx) = mpsc::channel::<Result<(), String>>();
                let vm = vm.clone();
                std::thread::spawn(move || {
                    let result = reset_vm(&vm).map_err(|e| e.to_string());
                    tx.send(result).ok();
                });

                let toast_overlay = toast_overlay.clone();
                let rx = Rc::new(RefCell::new(rx));
                glib::timeout_add_local(Duration::from_millis(200), move || {
                    match rx.borrow().try_recv() {
                        Ok(Ok(())) => {
                            toast_overlay.add_toast(Toast::new("VM reset."));
                            glib::ControlFlow::Break
                        }
                        Ok(Err(e)) => {
                            toast_overlay.add_toast(
                                Toast::builder()
                                    .title(&format!("Reset failed: {e}"))
                                    .timeout(0)
                                    .build(),
                            );
                            glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                    }
                });
            });

            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                alert.present(Some(&win));
            }
        });
    }

    // --- Snapshots ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);

        snapshots_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() {
                Some(i) => i,
                None => return,
            };
            let vm = {
                let vms = vms.borrow();
                if idx >= vms.len() {
                    return;
                }
                vms[idx].clone()
            };
            let disk_path = match vm.config.disks.first() {
                Some(d) => d.path.clone(),
                None => return,
            };
            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                crate::snapshot::show(&win, &vm.display_name(), disk_path);
            }
        });
    }

    // --- Boot Options ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        let do_launch = Rc::clone(&do_launch);

        boot_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() {
                Some(i) => i,
                None => return,
            };
            let vm = {
                let vms = vms.borrow();
                if idx >= vms.len() {
                    return;
                }
                vms[idx].clone()
            };
            let do_launch = Rc::clone(&do_launch);
            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                crate::boot_dialog::show(&win, move |options| {
                    do_launch(vm.clone(), options);
                });
            }
        });
    }

    // --- Edit Notes ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        let notes_label = notes_label.clone();

        notes_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() {
                Some(i) => i,
                None => return,
            };
            let vm = {
                let vms = vms.borrow();
                if idx >= vms.len() {
                    return;
                }
                vms[idx].clone()
            };
            let vms = Rc::clone(&vms);
            let notes_label = notes_label.clone();
            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                crate::notes_dialog::show(&win, &vm, move |new_notes: Option<String>| {
                    // Update in-memory VM so label stays consistent without full refresh
                    if idx < vms.borrow().len() {
                        vms.borrow_mut()[idx].notes = new_notes.clone();
                    }
                    if let Some(ref notes) = new_notes {
                        notes_label.set_label(&format!(
                            "<i>{}</i>",
                            glib::markup_escape_text(notes)
                        ));
                        notes_label.set_use_markup(true);
                        notes_label.set_visible(true);
                    } else {
                        notes_label.set_visible(false);
                    }
                });
            }
        });
    }

    // --- Network Settings ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        network_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() { Some(i) => i, None => return };
            let vm = { let v = vms.borrow(); if idx >= v.len() { return; } v[idx].clone() };
            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                crate::network_dialog::show(&win, vm);
            }
        });
    }

    // --- Shared Folders ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        folders_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() { Some(i) => i, None => return };
            let vm = { let v = vms.borrow(); if idx >= v.len() { return; } v[idx].clone() };
            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                crate::folders_dialog::show(&win, vm);
            }
        });
    }

    // --- Display Options ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        display_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() { Some(i) => i, None => return };
            let vm = { let v = vms.borrow(); if idx >= v.len() { return; } v[idx].clone() };
            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                crate::display_dialog::show(&win, vm);
            }
        });
    }

    // --- Raw Config Editor ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        raw_config_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() { Some(i) => i, None => return };
            let vm = { let v = vms.borrow(); if idx >= v.len() { return; } v[idx].clone() };
            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                crate::config_editor::show(&win, vm);
            }
        });
    }

    // --- USB Passthrough ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        usb_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() { Some(i) => i, None => return };
            let vm = { let v = vms.borrow(); if idx >= v.len() { return; } v[idx].clone() };
            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                crate::usb_dialog::show(&win, vm);
            }
        });
    }

    // --- PCI Passthrough ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        pci_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() { Some(i) => i, None => return };
            let vm = { let v = vms.borrow(); if idx >= v.len() { return; } v[idx].clone() };
            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                crate::pci_dialog::show(&win, vm);
            }
        });
    }

    // --- Single GPU Setup ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        single_gpu_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() { Some(i) => i, None => return };
            let vm = { let v = vms.borrow(); if idx >= v.len() { return; } v[idx].clone() };
            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                crate::single_gpu::show(&win, vm);
            }
        });
    }

    // --- Multi-GPU / Looking Glass ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        multi_gpu_btn.connect_clicked(move |btn| {
            let idx = match *selected.borrow() { Some(i) => i, None => return };
            let vm = { let v = vms.borrow(); if idx >= v.len() { return; } v[idx].clone() };
            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                crate::multi_gpu::show(&win, vm);
            }
        });
    }

    // --- Running VM detection (every 3 s) ---
    {
        let vms = Rc::clone(&vms);
        let rows = Rc::clone(&rows);
        let running_pids = Rc::clone(&running_pids);
        let vm_paused = Rc::clone(&vm_paused);
        let selected = Rc::clone(&selected);
        let launch_btn = launch_btn.clone();
        let stop_btn = stop_btn.clone();
        let pause_btn = pause_btn.clone();
        let force_stop_btn = force_stop_btn.clone();

        glib::timeout_add_seconds_local(3, move || {
            let processes = detect_qemu_processes();
            let mut pids = running_pids.borrow_mut();
            let vms = vms.borrow();
            let rows = rows.borrow();

            for (i, vm) in vms.iter().enumerate() {
                if i >= rows.len() {
                    break;
                }
                let new_pid = processes
                    .iter()
                    .find(|p| p.cwd.as_deref() == Some(vm.path.as_path()))
                    .map(|p| p.pid);

                if new_pid != pids[i] {
                    pids[i] = new_pid;
                    if let Some(pid) = new_pid {
                        rows[i].set_subtitle(&format!("Running  ·  PID {pid}"));
                    } else {
                        rows[i].set_subtitle(&vm.id);
                    }
                    if selected.borrow().as_ref() == Some(&i) {
                        launch_btn.set_sensitive(new_pid.is_none());
                        launch_btn.set_label(if new_pid.is_some() { "Running" } else { "Launch VM" });
                        stop_btn.set_sensitive(new_pid.is_some());
                        force_stop_btn.set_sensitive(new_pid.is_some());
                    }
                }
            }

            // Update paused state for each running VM
            {
                let mut paused_vec = vm_paused.borrow_mut();
                for (i, vm) in vms.iter().enumerate() {
                    if i >= paused_vec.len() {
                        break;
                    }
                    paused_vec[i] = if pids.get(i).and_then(|p| *p).is_some() {
                        is_vm_paused(&vm.path)
                    } else {
                        false
                    };
                }
            }

            // Refresh pause button for selected VM
            if let Some(idx) = *selected.borrow() {
                let is_running = pids.get(idx).and_then(|p| *p).is_some();
                let is_paused = vm_paused.borrow().get(idx).copied().unwrap_or(false);
                pause_btn.set_sensitive(is_running);
                pause_btn.set_label(if is_paused { "Resume" } else { "Pause" });
            }

            glib::ControlFlow::Continue
        });
    }

    // --- Search filter ---
    {
        let list_box = list_box.clone();
        let search_entry_filter = search_entry.clone();
        let search_entry_changed = search_entry.clone();

        list_box.set_filter_func(move |row| {
            let query = search_entry_filter.text();
            let query = query.trim();
            if query.is_empty() {
                return true;
            }
            let query = query.to_lowercase();
            if let Some(action_row) = row.downcast_ref::<ActionRow>() {
                action_row.title().to_lowercase().contains(&query)
            } else {
                true
            }
        });

        search_entry_changed.connect_search_changed(move |_| {
            list_box.invalidate_filter();
        });
    }

    // --- Settings button ---
    {
        let config_s = Rc::clone(&config);
        let refresh_vm_list_settings = refresh_vm_list.clone();
        let refresh_vm_list_initial = refresh_vm_list.clone();

        // Stored as a Rc so it can be moved into the connect_clicked closure
        // and the HeaderBar construction below.
        let settings_btn = Button::from_icon_name("preferences-system-symbolic");
        settings_btn.set_tooltip_text(Some("Settings"));

        settings_btn.connect_clicked(move |btn| {
            let current = config_s.borrow().clone();
            let config = Rc::clone(&config_s);
            let refresh_vm_list = refresh_vm_list_settings.clone();
            if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                crate::settings::show(&win, current, move |new_config| {
                    *config.borrow_mut() = new_config;
                    refresh_vm_list();
                });
            }
        });

        // --- Create VM button ---
        let create_btn = Button::from_icon_name("list-add-symbolic");
        create_btn.set_tooltip_text(Some("Create VM"));
        {
            let config = Rc::clone(&config);
            let refresh_vm_list = refresh_vm_list.clone();
            create_btn.connect_clicked(move |btn| {
                let cfg = config.borrow().clone();
                let refresh = refresh_vm_list.clone();
                if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                    crate::create_wizard::show(&win, cfg, move || refresh());
                }
            });
        }

        // --- Import VM button ---
        let import_btn = Button::from_icon_name("document-open-symbolic");
        import_btn.set_tooltip_text(Some("Import VM"));
        {
            let config = Rc::clone(&config);
            let refresh_vm_list = refresh_vm_list.clone();
            import_btn.connect_clicked(move |btn| {
                let cfg = config.borrow().clone();
                let refresh = refresh_vm_list.clone();
                if let Some(win) = btn.root().and_downcast::<gtk4::Window>() {
                    crate::import_wizard::show(&win, cfg, move || refresh());
                }
            });
        }

        // Store button so we can pass it to HeaderBar below.
        // We build the HeaderBar here instead of inline so we can pack_end.
        let header_bar = HeaderBar::new();
        header_bar.pack_end(&settings_btn);
        header_bar.pack_start(&create_btn);
        header_bar.pack_start(&import_btn);

        // --- Initial load ---
        refresh_vm_list_initial();

        // --- Main layout ---
        let paned = Paned::new(Orientation::Horizontal);
        paned.set_start_child(Some(&left_box));
        paned.set_end_child(Some(&right_scroll));
        paned.set_position(260);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);

        toast_overlay.set_child(Some(&paned));

        let toolbar_view = ToolbarView::new();
        toolbar_view.add_top_bar(&header_bar);
        toolbar_view.set_content(Some(&toast_overlay));

        let window = ApplicationWindow::builder()
            .application(app)
            .title("VM Curator")
            .default_width(960)
            .default_height(640)
            .content(&toolbar_view)
            .build();

        window.present();
    }
}

fn make_btn(label: &str, sensitive: bool) -> Button {
    Button::builder()
        .label(label)
        .sensitive(sensitive)
        .hexpand(true)
        .build()
}

/// Build a two-column grid section for a group of buttons.
/// Each inner slice is a row; a single-item row spans both columns.
/// `column_homogeneous(true)` guarantees the centre seam is pixel-perfect
/// across every row regardless of label length.
fn btn_section(rows: &[&[&Button]], margin_top: i32, margin_bottom: i32) -> gtk4::Grid {
    let grid = gtk4::Grid::builder()
        .column_homogeneous(true)
        .column_spacing(8)
        .row_spacing(4)
        .margin_start(8)
        .margin_end(8)
        .margin_top(margin_top)
        .margin_bottom(margin_bottom)
        .hexpand(true)
        .build();
    for (r, row) in rows.iter().enumerate() {
        let span = if row.len() == 1 { 2 } else { 1 };
        for (c, btn) in row.iter().enumerate() {
            grid.attach(*btn, c as i32, r as i32, span, 1);
        }
    }
    grid
}
