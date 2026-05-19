use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{Box as GtkBox, Button, CheckButton, Label, Orientation, ScrolledWindow, Separator,
           Spinner, Stack, StringList};
use libadwaita::{ActionRow, ComboRow, EntryRow, HeaderBar, PreferencesGroup, Toast,
                 ToastOverlay, ToolbarView};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use log::{info, warn};
use vm_curator::config::Config;
use vm_curator::vm::{discover_libvirt_vms, discover_quickemu_vms, discover_vms_in_dir,
                     execute_import};
use vm_curator::wizard_types::{ImportDiskAction, ImportableVm, ImportSource};

enum DiscoverResult {
    Ok(Vec<ImportableVm>),
    Err(String),
}

pub fn show(
    parent: &impl IsA<gtk4::Widget>,
    config: Config,
    on_imported: impl Fn() + 'static,
) {
    let dialog = libadwaita::Dialog::new();
    dialog.set_title("Import VM");
    dialog.set_content_width(560);
    dialog.set_content_height(500);

    let current_step: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    const LAST: usize = 2;

    let step_label = Label::new(Some("Step 1 of 3"));
    step_label.add_css_class("caption");
    step_label.add_css_class("dim-label");

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
    // Page 1 — Source selection
    // ══════════════════════════════════════════════════════════════════════
    let libvirt_btn = CheckButton::builder()
        .label("libvirt / KVM  (scans ~/.config/libvirt and /etc/libvirt)")
        .active(true)
        .build();
    let quickemu_btn = CheckButton::builder()
        .label("quickemu  (scans ~/quickemu, ~/.quickemu, ~/VMs)")
        .group(&libvirt_btn)
        .build();
    let dir_btn = CheckButton::builder()
        .label("Browse for a directory…")
        .group(&libvirt_btn)
        .build();

    // Folder chooser row — shown only when dir_btn is active
    let dir_path_label = Label::builder()
        .label("No directory selected")
        .halign(gtk4::Align::Start)
        .hexpand(true)
        .ellipsize(gtk4::pango::EllipsizeMode::Start)
        .build();
    dir_path_label.add_css_class("dim-label");

    let dir_browse_btn = Button::builder().label("Choose…").build();

    let dir_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(28)
        .margin_top(2)
        .visible(false)
        .build();
    dir_row.append(&dir_path_label);
    dir_row.append(&dir_browse_btn);

    let custom_dir: Rc<RefCell<Option<std::path::PathBuf>>> = Rc::new(RefCell::new(None));

    let source_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(16)
        .margin_bottom(8)
        .build();
    let src_hdr = Label::builder()
        .label("Import Source")
        .halign(gtk4::Align::Start)
        .build();
    src_hdr.add_css_class("heading");
    source_box.append(&src_hdr);
    source_box.append(&libvirt_btn);
    source_box.append(&quickemu_btn);
    source_box.append(&dir_btn);
    source_box.append(&dir_row);
    source_box.append(&Separator::new(Orientation::Horizontal));

    let note = Label::builder()
        .label("vm-curator will scan the selected location for VMs and let you choose which to import.")
        .wrap(true)
        .halign(gtk4::Align::Start)
        .build();
    note.add_css_class("caption");
    note.add_css_class("dim-label");
    source_box.append(&note);

    // Show/hide the folder row when Browse is toggled
    {
        let dir_row = dir_row.clone();
        dir_btn.connect_toggled(move |btn| {
            dir_row.set_visible(btn.is_active());
        });
    }

    // Folder chooser
    {
        let custom_dir = Rc::clone(&custom_dir);
        let dir_path_label = dir_path_label.clone();
        let dialog_ref = dialog.clone();
        dir_browse_btn.connect_clicked(move |_| {
            let file_dialog = gtk4::FileDialog::builder()
                .title("Choose directory to scan")
                .accept_label("Select")
                .build();
            let custom_dir = Rc::clone(&custom_dir);
            let dir_path_label = dir_path_label.clone();
            let parent_win = dialog_ref.root().and_downcast::<gtk4::Window>();
            file_dialog.select_folder(
                parent_win.as_ref(),
                gtk4::gio::Cancellable::NONE,
                move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            dir_path_label.set_label(&path.to_string_lossy());
                            dir_path_label.remove_css_class("dim-label");
                            *custom_dir.borrow_mut() = Some(path);
                        }
                    }
                },
            );
        });
    }

    // ══════════════════════════════════════════════════════════════════════
    // Page 2 — VM list (populated after discovery)
    // ══════════════════════════════════════════════════════════════════════
    let spinner = Spinner::builder().spinning(true).halign(gtk4::Align::Center).build();
    let spinner_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .valign(gtk4::Align::Center)
        .spacing(8)
        .margin_top(32)
        .margin_bottom(32)
        .build();
    spinner_box.append(&spinner);
    let scan_lbl = Label::new(Some("Scanning for VMs…"));
    scan_lbl.add_css_class("dim-label");
    spinner_box.append(&scan_lbl);

    let vm_list_box = gtk4::ListBox::new();
    vm_list_box.add_css_class("boxed-list");
    vm_list_box.set_selection_mode(gtk4::SelectionMode::Single);
    vm_list_box.set_visible(false);

    let discovered_vms: Rc<RefCell<Vec<ImportableVm>>> = Rc::new(RefCell::new(Vec::new()));
    let selected_vm_idx: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));

    {
        let discovered_vms = Rc::clone(&discovered_vms);
        let selected_vm_idx = Rc::clone(&selected_vm_idx);
        vm_list_box.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let idx = row.index() as usize;
                if idx < discovered_vms.borrow().len() {
                    selected_vm_idx.set(Some(idx));
                }
            }
        });
    }

    let page2 = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    let vm_hdr = Label::builder()
        .label("Select a VM to import")
        .halign(gtk4::Align::Start)
        .build();
    vm_hdr.add_css_class("heading");
    let vm_scroll = ScrolledWindow::builder()
        .child(&vm_list_box)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .min_content_height(200)
        .build();
    page2.append(&vm_hdr);
    page2.append(&spinner_box);
    page2.append(&vm_scroll);

    // ══════════════════════════════════════════════════════════════════════
    // Page 3 — Configure & import
    // ══════════════════════════════════════════════════════════════════════
    let vm_name_row = EntryRow::new();
    vm_name_row.set_title("VM Name");

    let disk_action_list = StringList::new(&["Link (symlink)", "Copy", "Move"]);
    let disk_action_row = ComboRow::new();
    disk_action_row.set_title("Disk Handling");
    disk_action_row.set_model(Some(&disk_action_list));

    let cfg_group = PreferencesGroup::new();
    cfg_group.set_title("Import Settings");
    cfg_group.add(&vm_name_row);
    cfg_group.add(&disk_action_row);

    let warnings_label = Label::builder()
        .label("")
        .wrap(true)
        .halign(gtk4::Align::Start)
        .build();
    warnings_label.add_css_class("caption");

    let page3_scroll = ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();
    let page3 = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    page3.append(&cfg_group);
    page3.append(&warnings_label);
    page3_scroll.set_child(Some(&page3));

    // ══════════════════════════════════════════════════════════════════════
    // Stack assembly
    // ══════════════════════════════════════════════════════════════════════
    let stack = Stack::new();
    stack.add_named(&source_box, Some("p1"));
    stack.add_named(&page2, Some("p2"));
    stack.add_named(&page3_scroll, Some("p3"));

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

    let pages = ["p1", "p2", "p3"];

    // ── Back ───────────────────────────────────────────────────────────────
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
            let new = step - 1;
            current_step.set(new);
            stack.set_visible_child_name(pages[new]);
            step_label.set_label(&format!("Step {} of 3", new + 1));
            back_btn.set_sensitive(new > 0);
            next_btn.set_label(if new == LAST { "Import" } else { "Next" });
        });
    }

    // ── Next / Import ──────────────────────────────────────────────────────
    {
        let current_step = Rc::clone(&current_step);
        let stack = stack.clone();
        let step_label = step_label.clone();
        let back_btn = back_btn.clone();
        let next_btn_ref = next_btn.clone();
        let discovered_vms = Rc::clone(&discovered_vms);
        let selected_vm_idx = Rc::clone(&selected_vm_idx);
        let toast_overlay = toast_overlay.clone();
        let dialog_ref = dialog.clone();
        let on_imported = Rc::new(on_imported);
        let library_path = config.vm_library_path.clone();

        // Clones for use in closure
        let libvirt_btn_c = libvirt_btn.clone();
        let quickemu_btn_c = quickemu_btn.clone();
        let dir_btn_c = dir_btn.clone();
        let custom_dir_c = Rc::clone(&custom_dir);
        let spinner_box_c = spinner_box.clone();
        let vm_list_box_c = vm_list_box.clone();
        let vm_name_row_c = vm_name_row.clone();
        let warnings_label_c = warnings_label.clone();
        let disk_action_row_c = disk_action_row.clone();

        next_btn.connect_clicked(move |_| {
            let step = current_step.get();

            if step == 0 {
                // Validate directory selection before spawning
                if dir_btn_c.is_active() && custom_dir_c.borrow().is_none() {
                    toast_overlay.add_toast(Toast::new("Choose a directory to scan first."));
                    return;
                }

                // Start discovery in background
                let source = if libvirt_btn_c.is_active() {
                    0u8 // libvirt
                } else if quickemu_btn_c.is_active() {
                    1   // quickemu
                } else {
                    2   // custom dir
                };
                let custom_path = custom_dir_c.borrow().clone();
                let (tx, rx) = mpsc::channel::<DiscoverResult>();
                std::thread::spawn(move || {
                    let vms = match source {
                        0 => discover_libvirt_vms(),
                        1 => discover_quickemu_vms(),
                        _ => custom_path
                            .as_deref()
                            .map(discover_vms_in_dir)
                            .unwrap_or_default(),
                    };
                    tx.send(if vms.is_empty() {
                        DiscoverResult::Err("No VMs found in the selected location.".to_string())
                    } else {
                        DiscoverResult::Ok(vms)
                    })
                    .ok();
                });

                let discovered_vms = Rc::clone(&discovered_vms);
                let vm_list_box = vm_list_box_c.clone();
                let spinner_box = spinner_box_c.clone();
                let toast_overlay = toast_overlay.clone();
                let rx = Rc::new(RefCell::new(rx));
                gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
                    match rx.borrow().try_recv() {
                        Ok(DiscoverResult::Ok(vms)) => {
                            spinner_box.set_visible(false);
                            let mut stored = discovered_vms.borrow_mut();
                            for vm in &vms {
                                let row = ActionRow::new();
                                row.set_title(&vm.name);
                                let src = match vm.source {
                                    ImportSource::Libvirt => "libvirt",
                                    ImportSource::Quickemu => "quickemu",
                                };
                                row.set_subtitle(&format!("{src} · {}", vm.config_path.display()));
                                vm_list_box.append(&row);
                                stored.push(vm.clone());
                            }
                            vm_list_box.set_visible(true);
                            gtk4::glib::ControlFlow::Break
                        }
                        Ok(DiscoverResult::Err(e)) => {
                            spinner_box.set_visible(false);
                            toast_overlay.add_toast(
                                Toast::builder().title(&e).timeout(0).build(),
                            );
                            gtk4::glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => gtk4::glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => gtk4::glib::ControlFlow::Break,
                    }
                });
            } else if step == 1 {
                // Validate selection, populate step 3
                let Some(idx) = selected_vm_idx.get() else {
                    toast_overlay.add_toast(Toast::new("Select a VM to continue."));
                    return;
                };
                let vms = discovered_vms.borrow();
                let Some(vm) = vms.get(idx) else { return };
                vm_name_row_c.set_text(&vm.name);
                if vm.import_notes.is_empty() {
                    info!("Import: {} — no warnings", vm.name);
                    warnings_label_c.set_label("");
                } else {
                    for note in &vm.import_notes {
                        warn!("Import warning for {}: {}", vm.name, note);
                    }
                    let w = vm.import_notes.join("\n• ");
                    warnings_label_c.set_label(&format!("⚠ Warnings:\n• {}", w));
                    warnings_label_c.add_css_class("warning");
                }
                next_btn_ref.set_label("Import");
            } else if step == LAST {
                // Execute import
                let Some(idx) = selected_vm_idx.get() else { return };
                let vms = discovered_vms.borrow();
                let Some(vm) = vms.get(idx) else { return };
                let vm_name = vm_name_row_c.text().to_string();
                if vm_name.trim().is_empty() {
                    toast_overlay.add_toast(Toast::new("Enter a VM name."));
                    return;
                }
                let folder =
                    vm_curator::wizard_types::CreateWizardState::generate_folder_name(&vm_name);
                let disk_action = match disk_action_row_c.selected() {
                    1 => ImportDiskAction::Copy,
                    2 => ImportDiskAction::Move,
                    _ => ImportDiskAction::Symlink,
                };
                let vm_clone = vm.clone();
                let lib = library_path.clone();
                let (tx, rx) = mpsc::channel::<Result<(), String>>();
                std::thread::spawn(move || {
                    let result = execute_import(&lib, &vm_clone, &vm_name, &folder, disk_action);
                    let mapped = match &result {
                        Ok(path) => { info!("Import succeeded: {}", path.display()); Ok(()) }
                        Err(e) => { warn!("Import failed for {}: {}", vm_name, e); Err(e.to_string()) }
                    };
                    tx.send(mapped).ok();
                });

                next_btn_ref.set_sensitive(false);
                next_btn_ref.set_label("Importing…");
                let dialog_ref = dialog_ref.clone();
                let toast_overlay = toast_overlay.clone();
                let on_imported = Rc::clone(&on_imported);
                let next_btn_ref2 = next_btn_ref.clone();
                let rx = Rc::new(RefCell::new(rx));
                gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
                    match rx.borrow().try_recv() {
                        Ok(Ok(())) => {
                            on_imported();
                            dialog_ref.close();
                            gtk4::glib::ControlFlow::Break
                        }
                        Ok(Err(e)) => {
                            toast_overlay.add_toast(
                                Toast::builder()
                                    .title(&format!("Import failed: {e}"))
                                    .timeout(0)
                                    .build(),
                            );
                            next_btn_ref2.set_sensitive(true);
                            next_btn_ref2.set_label("Import");
                            gtk4::glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => gtk4::glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => gtk4::glib::ControlFlow::Break,
                    }
                });
                return;
            }

            // Advance to next page
            let new = step + 1;
            current_step.set(new);
            stack.set_visible_child_name(pages[new]);
            step_label.set_label(&format!("Step {} of 3", new + 1));
            back_btn.set_sensitive(true);
        });
    }

    dialog.present(Some(parent));
}
