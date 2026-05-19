use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{
    Box as GtkBox, Button, CheckButton, FileFilter, Label, Orientation, ScrolledWindow,
    SearchEntry, Stack, StringList,
};
use libadwaita::{ActionRow, ComboRow, EntryRow, HeaderBar, PreferencesGroup, SwitchRow, Toast,
                 ToastOverlay, ToolbarView};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::config::Config;
use vm_curator::metadata::QemuProfileStore;
use vm_curator::vm::create_vm;
use vm_curator::wizard_types::CreateWizardState;

const DISPLAYS: &[&str] = &["gtk", "sdl", "spice-app", "vnc", "none"];
const NET_MODELS: &[&str] = &["virtio-net-pci", "e1000", "rtl8139", "none"];
const NET_BACKENDS: &[&str] = &["user", "passt", "bridge", "none"];

pub fn show(
    parent: &impl IsA<gtk4::Widget>,
    config: Config,
    on_created: impl Fn() + 'static,
) {
    let dialog = libadwaita::Dialog::new();
    dialog.set_title("Create VM");
    dialog.set_content_width(620);
    dialog.set_content_height(560);

    let store = Rc::new(QemuProfileStore::load_embedded());
    let profiles: Rc<Vec<(String, vm_curator::metadata::QemuProfile)>> = Rc::new(
        store.list_all().into_iter().map(|(id, p)| (id.clone(), p.clone())).collect(),
    );

    let current_step: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    const LAST: usize = 4; // 0-indexed, 5 steps total

    // Persists selected OS ID and VM name across steps
    let state: Rc<RefCell<CreateWizardState>> =
        Rc::new(RefCell::new(CreateWizardState::default()));

    // ── Step-indicator label in header ────────────────────────────────────
    let step_label = Label::new(Some("Step 1 of 5"));
    step_label.add_css_class("caption");
    step_label.add_css_class("dim-label");

    // ── Shared nav buttons ─────────────────────────────────────────────────
    let back_btn = Button::builder().label("Back").build();
    back_btn.set_sensitive(false);
    let next_btn = Button::builder().label("Next").build();
    next_btn.add_css_class("suggested-action");

    let nav_bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(12)
        .build();
    nav_bar.append(&back_btn);
    let spacer = Label::new(None);
    spacer.set_hexpand(true);
    nav_bar.append(&spacer);
    nav_bar.append(&next_btn);

    // ══════════════════════════════════════════════════════════════════════
    // Page 1 — OS & Name
    // ══════════════════════════════════════════════════════════════════════
    let name_row = EntryRow::new();
    name_row.set_title("VM Name");

    let name_group = PreferencesGroup::new();
    name_group.set_title("Identity");
    name_group.add(&name_row);

    let os_search = SearchEntry::builder().placeholder_text("Search OS…").build();
    let os_list_box = gtk4::ListBox::new();
    os_list_box.add_css_class("boxed-list");
    os_list_box.set_selection_mode(gtk4::SelectionMode::Single);

    let profile_ids: Rc<Vec<String>> =
        Rc::new(profiles.iter().map(|(id, _)| id.clone()).collect());
    let profile_names: Rc<Vec<String>> = Rc::new(
        profiles.iter().map(|(_, p)| p.display_name.to_lowercase()).collect(),
    );

    for (_, profile) in profiles.iter() {
        let row = ActionRow::new();
        row.set_title(&profile.display_name);
        row.set_subtitle(QemuProfileStore::category_display_name(&profile.category));
        os_list_box.append(&row);
    }

    {
        let names = Rc::clone(&profile_names);
        let entry = os_search.clone();
        let lb2 = os_list_box.clone();
        os_list_box.set_filter_func(move |row| {
            let q = entry.text();
            let q = q.as_str().trim().to_lowercase();
            if q.is_empty() {
                return true;
            }
            let idx = row.index() as usize;
            names.get(idx).map_or(true, |n| n.contains(&q))
        });
        os_search.connect_search_changed(move |_| lb2.invalidate_filter());
    }

    let os_scroll = ScrolledWindow::builder()
        .child(&os_list_box)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .min_content_height(200)
        .build();

    let os_group_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .build();
    let os_hdr = Label::builder()
        .label("Operating System")
        .halign(gtk4::Align::Start)
        .build();
    os_hdr.add_css_class("heading");
    os_group_box.append(&os_hdr);
    os_group_box.append(&os_search);
    os_group_box.append(&os_scroll);

    let page1 = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    page1.append(&name_group);
    page1.append(&os_group_box);

    // ══════════════════════════════════════════════════════════════════════
    // Page 2 — Resources
    // ══════════════════════════════════════════════════════════════════════
    let mem_row = EntryRow::new();
    mem_row.set_title("Memory (MB)");
    mem_row.set_text("2048");

    let cpu_row = EntryRow::new();
    cpu_row.set_title("CPU Cores");
    cpu_row.set_text("2");

    let disk_row = EntryRow::new();
    disk_row.set_title("Disk Size (GB)");
    disk_row.set_text("32");

    let kvm_row = SwitchRow::new();
    kvm_row.set_title("Enable KVM");
    kvm_row.set_active(true);

    let uefi_row = SwitchRow::new();
    uefi_row.set_title("UEFI Firmware");

    let tpm_row = SwitchRow::new();
    tpm_row.set_title("TPM 2.0");

    let res_group = PreferencesGroup::new();
    res_group.set_title("Resources");
    res_group.add(&mem_row);
    res_group.add(&cpu_row);
    res_group.add(&disk_row);

    let fw_group = PreferencesGroup::new();
    fw_group.set_title("Firmware");
    fw_group.add(&kvm_row);
    fw_group.add(&uefi_row);
    fw_group.add(&tpm_row);

    let page2_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .build();
    let page2 = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    page2.append(&res_group);
    page2.append(&fw_group);
    page2_scroll.set_child(Some(&page2));

    // ══════════════════════════════════════════════════════════════════════
    // Page 3 — Boot Media
    // ══════════════════════════════════════════════════════════════════════
    let no_iso_btn = CheckButton::builder().label("No install media").active(true).build();
    let iso_btn = CheckButton::builder()
        .label("Install ISO…")
        .group(&no_iso_btn)
        .build();

    let iso_path: Rc<RefCell<Option<std::path::PathBuf>>> = Rc::new(RefCell::new(None));
    let iso_name_lbl = Label::builder()
        .label("No file selected")
        .halign(gtk4::Align::Start)
        .hexpand(true)
        .ellipsize(gtk4::pango::EllipsizeMode::Start)
        .build();
    iso_name_lbl.add_css_class("dim-label");
    let browse_iso_btn = Button::builder().label("Browse…").build();
    let iso_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(24)
        .visible(false)
        .build();
    iso_row.append(&iso_name_lbl);
    iso_row.append(&browse_iso_btn);

    {
        let iso_row = iso_row.clone();
        iso_btn.connect_toggled(move |b| iso_row.set_visible(b.is_active()));
    }

    let boot_group = PreferencesGroup::new();
    boot_group.set_title("Boot Media");

    let media_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    media_box.append(&no_iso_btn);
    media_box.append(&iso_btn);
    media_box.append(&iso_row);

    let page3 = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    page3.append(&media_box);

    // ISO browse
    {
        let iso_path = Rc::clone(&iso_path);
        let iso_name_lbl = iso_name_lbl.clone();
        let dialog_ref = dialog.clone();
        browse_iso_btn.connect_clicked(move |_| {
            let filt = FileFilter::new();
            filt.set_name(Some("Disk images"));
            filt.add_pattern("*.iso");
            filt.add_pattern("*.img");
            let filters = gtk4::gio::ListStore::new::<FileFilter>();
            filters.append(&filt);
            let fd = gtk4::FileDialog::builder()
                .title("Select ISO")
                .filters(&filters)
                .build();
            let iso_path = Rc::clone(&iso_path);
            let iso_name_lbl = iso_name_lbl.clone();
            let win = dialog_ref.root().and_downcast::<gtk4::Window>();
            fd.open(win.as_ref(), gtk4::gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        let name = path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        iso_name_lbl.set_label(&name);
                        iso_name_lbl.remove_css_class("dim-label");
                        *iso_path.borrow_mut() = Some(path);
                    }
                }
            });
        });
    }

    // ══════════════════════════════════════════════════════════════════════
    // Page 4 — Display & Network
    // ══════════════════════════════════════════════════════════════════════
    let display_list = StringList::new(DISPLAYS);
    let display_row = ComboRow::new();
    display_row.set_title("Display Backend");
    display_row.set_model(Some(&display_list));

    let net_model_list = StringList::new(NET_MODELS);
    let net_model_row = ComboRow::new();
    net_model_row.set_title("Network Adapter");
    net_model_row.set_model(Some(&net_model_list));

    let net_backend_list = StringList::new(NET_BACKENDS);
    let net_backend_row = ComboRow::new();
    net_backend_row.set_title("Network Backend");
    net_backend_row.set_model(Some(&net_backend_list));

    let display_group = PreferencesGroup::new();
    display_group.set_title("Display");
    display_group.add(&display_row);

    let net_group = PreferencesGroup::new();
    net_group.set_title("Network");
    net_group.add(&net_model_row);
    net_group.add(&net_backend_row);

    let page4_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .build();
    let page4 = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    page4.append(&display_group);
    page4.append(&net_group);
    page4_scroll.set_child(Some(&page4));

    // ══════════════════════════════════════════════════════════════════════
    // Page 5 — Review
    // ══════════════════════════════════════════════════════════════════════
    let summary_label = Label::builder()
        .label("")
        .halign(gtk4::Align::Start)
        .wrap(true)
        .selectable(true)
        .build();
    summary_label.add_css_class("monospace");

    let review_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    let rev_hdr = Label::builder()
        .label("Review your settings")
        .halign(gtk4::Align::Start)
        .build();
    rev_hdr.add_css_class("heading");
    review_box.append(&rev_hdr);
    review_box.append(&summary_label);

    // ══════════════════════════════════════════════════════════════════════
    // Stack assembly
    // ══════════════════════════════════════════════════════════════════════
    let stack = Stack::new();
    stack.add_named(&page1, Some("p1"));
    stack.add_named(&page2_scroll, Some("p2"));
    stack.add_named(&page3, Some("p3"));
    stack.add_named(&page4_scroll, Some("p4"));
    stack.add_named(&review_box, Some("p5"));

    let header = HeaderBar::new();
    header.set_title_widget(Some(&step_label));

    let toast_overlay = ToastOverlay::new();
    toast_overlay.set_child(Some(&stack));

    let main_box = GtkBox::new(Orientation::Vertical, 0);
    main_box.append(&toast_overlay);
    main_box.append(&nav_bar);

    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&main_box));
    dialog.set_child(Some(&toolbar_view));

    // ── OS row-selected: populate profile defaults into step-2/4 widgets ─
    {
        let store = Rc::clone(&store);
        let profile_ids = Rc::clone(&profile_ids);
        let state = Rc::clone(&state);
        let mem_row = mem_row.clone();
        let cpu_row = cpu_row.clone();
        let disk_row = disk_row.clone();
        let kvm_row = kvm_row.clone();
        let uefi_row = uefi_row.clone();
        let tpm_row = tpm_row.clone();
        let display_row = display_row.clone();
        let net_model_row = net_model_row.clone();

        os_list_box.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let idx = row.index() as usize;
            let Some(id) = profile_ids.get(idx) else { return };
            let Some(profile) = store.get(id) else { return };

            mem_row.set_text(&profile.memory_mb.to_string());
            cpu_row.set_text(&profile.cpu_cores.to_string());
            disk_row.set_text(&profile.disk_size_gb.to_string());
            kvm_row.set_active(profile.enable_kvm);
            uefi_row.set_active(profile.uefi);
            tpm_row.set_active(profile.tpm);
            display_row.set_selected(
                DISPLAYS.iter().position(|&d| d == profile.display).unwrap_or(0) as u32,
            );
            net_model_row.set_selected(
                NET_MODELS.iter().position(|&m| m == profile.network_model).unwrap_or(0) as u32,
            );

            state.borrow_mut().selected_os = Some(id.clone());
        });
    }

    // ── Navigation ─────────────────────────────────────────────────────────
    let pages = ["p1", "p2", "p3", "p4", "p5"];

    {
        let current_step = Rc::clone(&current_step);
        let stack = stack.clone();
        let step_label = step_label.clone();
        let next_btn = next_btn.clone();
        let back_btn = back_btn.clone();

        back_btn.clone().connect_clicked(move |_| {
            let step = current_step.get();
            if step == 0 {
                return;
            }
            let new_step = step - 1;
            current_step.set(new_step);
            stack.set_visible_child_name(pages[new_step]);
            step_label.set_label(&format!("Step {} of 5", new_step + 1));
            back_btn.set_sensitive(new_step > 0);
            next_btn.set_label(if new_step == LAST { "Create" } else { "Next" });
            next_btn.add_css_class("suggested-action");
        });
    }

    {
        let current_step = Rc::clone(&current_step);
        let stack = stack.clone();
        let step_label = step_label.clone();
        let back_btn = back_btn.clone();
        let next_btn_ref = next_btn.clone();
        let state = Rc::clone(&state);
        let toast_overlay = toast_overlay.clone();
        let dialog_ref = dialog.clone();
        let on_created = Rc::new(on_created);
        let library_path = config.vm_library_path.clone();

        // Clones for review page
        let name_row_r = name_row.clone();
        let mem_row_r = mem_row.clone();
        let cpu_row_r = cpu_row.clone();
        let disk_row_r = disk_row.clone();
        let kvm_row_r = kvm_row.clone();
        let uefi_row_r = uefi_row.clone();
        let tpm_row_r = tpm_row.clone();
        let iso_path_r = Rc::clone(&iso_path);
        let iso_btn_r = iso_btn.clone();
        let store_r = Rc::clone(&store);
        let display_row_r = display_row.clone();
        let net_model_row_r = net_model_row.clone();
        let net_backend_row_r = net_backend_row.clone();

        next_btn.connect_clicked(move |_| {
            let step = current_step.get();

            // Validate current step
            match step {
                0 => {
                    let name = name_row_r.text();
                    if name.trim().is_empty() {
                        toast_overlay.add_toast(Toast::new("Enter a VM name to continue."));
                        return;
                    }
                    if state.borrow().selected_os.is_none() {
                        toast_overlay.add_toast(Toast::new("Select an OS to continue."));
                        return;
                    }
                    state.borrow_mut().vm_name = name.to_string();
                }
                _ => {}
            }

            if step == LAST {
                // Build and submit
                let mut ws = CreateWizardState::default();
                ws.vm_name = state.borrow().vm_name.clone();
                ws.selected_os = state.borrow().selected_os.clone();

                if let Some(ref os_id) = ws.selected_os.clone() {
                    if let Some(profile) = store_r.get(os_id) {
                        ws.apply_profile(profile);
                    }
                }

                ws.qemu_config.memory_mb =
                    mem_row_r.text().parse().unwrap_or(ws.qemu_config.memory_mb);
                ws.qemu_config.cpu_cores =
                    cpu_row_r.text().parse().unwrap_or(ws.qemu_config.cpu_cores);
                ws.disk_size_gb = disk_row_r.text().parse().unwrap_or(ws.disk_size_gb);
                ws.qemu_config.enable_kvm = kvm_row_r.is_active();
                ws.qemu_config.uefi = uefi_row_r.is_active();
                ws.qemu_config.tpm = tpm_row_r.is_active();
                ws.qemu_config.display = DISPLAYS
                    .get(display_row_r.selected() as usize)
                    .copied()
                    .unwrap_or("gtk")
                    .to_string();
                ws.qemu_config.network_model = NET_MODELS
                    .get(net_model_row_r.selected() as usize)
                    .copied()
                    .unwrap_or("virtio-net-pci")
                    .to_string();
                ws.qemu_config.network_backend = NET_BACKENDS
                    .get(net_backend_row_r.selected() as usize)
                    .copied()
                    .unwrap_or("user")
                    .to_string();

                if iso_btn_r.is_active() {
                    ws.iso_path = iso_path_r.borrow().clone();
                }

                ws.update_folder_name(&library_path);

                let (tx, rx) = mpsc::channel::<Result<(), String>>();
                let lib = library_path.clone();
                std::thread::spawn(move || {
                    let result = create_vm(&lib, &ws).map(|_| ()).map_err(|e| e.to_string());
                    tx.send(result).ok();
                });

                let dialog_ref = dialog_ref.clone();
                let toast_overlay = toast_overlay.clone();
                let on_created = Rc::clone(&on_created);
                let rx = Rc::new(RefCell::new(rx));
                next_btn_ref.set_sensitive(false);
                next_btn_ref.set_label("Creating…");
                let next_btn_ref = next_btn_ref.clone();
                gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
                    match rx.borrow().try_recv() {
                        Ok(Ok(())) => {
                            on_created();
                            dialog_ref.close();
                            gtk4::glib::ControlFlow::Break
                        }
                        Ok(Err(e)) => {
                            toast_overlay.add_toast(
                                Toast::builder()
                                    .title(&format!("Creation failed: {e}"))
                                    .timeout(0)
                                    .build(),
                            );
                            next_btn_ref.set_sensitive(true);
                            next_btn_ref.set_label("Create");
                            gtk4::glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => gtk4::glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => gtk4::glib::ControlFlow::Break,
                    }
                });
                return;
            }

            // Advance step
            let new_step = step + 1;
            current_step.set(new_step);
            stack.set_visible_child_name(pages[new_step]);
            step_label.set_label(&format!("Step {} of 5", new_step + 1));
            back_btn.set_sensitive(true);

            if new_step == LAST {
                // Populate review summary
                next_btn_ref.set_label("Create");
                let s = state.borrow();
                let os_name = s
                    .selected_os
                    .as_deref()
                    .map(|id| {
                        store_r
                            .get(id)
                            .map(|p| p.display_name.as_str())
                            .unwrap_or(id)
                    })
                    .unwrap_or("None");
                let summary = format!(
                    "Name:      {}\nOS:        {}\nMemory:    {} MB\nCPU:       {} cores\nDisk:      {} GB\nKVM:       {}\nUEFI:      {}\nTPM:       {}\nDisplay:   {}\nNetwork:   {} / {}\n",
                    s.vm_name,
                    os_name,
                    mem_row_r.text(),
                    cpu_row_r.text(),
                    disk_row_r.text(),
                    if kvm_row_r.is_active() { "on" } else { "off" },
                    if uefi_row_r.is_active() { "on" } else { "off" },
                    if tpm_row_r.is_active() { "on" } else { "off" },
                    DISPLAYS.get(display_row_r.selected() as usize).copied().unwrap_or("gtk"),
                    NET_BACKENDS.get(net_backend_row_r.selected() as usize).copied().unwrap_or("user"),
                    NET_MODELS.get(net_model_row_r.selected() as usize).copied().unwrap_or("virtio-net-pci"),
                );
                summary_label.set_label(&summary);
            }
        });
    }

    dialog.present(Some(parent));
}
