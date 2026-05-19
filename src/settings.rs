use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{Box as GtkBox, Button, Label, Orientation, ScrolledWindow, StringList};
use libadwaita::{ActionRow, ComboRow, EntryRow, HeaderBar, PreferencesGroup, SwitchRow,
                 ToolbarView};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use vm_curator::config::Config;

pub fn show(
    parent: &impl IsA<gtk4::Widget>,
    config: Config,
    on_save: impl Fn(Config) + 'static,
) {
    let dialog = libadwaita::Dialog::new();
    dialog.set_title("Settings");
    dialog.set_content_width(500);
    dialog.set_content_height(560);

    // Current library path (updated by folder chooser)
    let lib_path: Rc<RefCell<PathBuf>> =
        Rc::new(RefCell::new(config.vm_library_path.clone()));

    // --- Library group ---
    let lib_group = PreferencesGroup::new();
    lib_group.set_title("VM Library");

    let lib_row = ActionRow::new();
    lib_row.set_title("Library Path");
    lib_row.set_subtitle(&config.vm_library_path.display().to_string());

    let choose_btn = Button::builder()
        .label("Choose…")
        .valign(gtk4::Align::Center)
        .build();
    lib_row.add_suffix(&choose_btn);
    lib_row.set_activatable_widget(Some(&choose_btn));
    lib_group.add(&lib_row);

    // --- Defaults group ---
    let defaults_group = PreferencesGroup::new();
    defaults_group.set_title("New VM Defaults");

    let memory_row = EntryRow::new();
    memory_row.set_title("Memory (MB)");
    memory_row.set_text(&config.default_memory_mb.to_string());
    defaults_group.add(&memory_row);

    let cores_row = EntryRow::new();
    cores_row.set_title("CPU Cores");
    cores_row.set_text(&config.default_cpu_cores.to_string());
    defaults_group.add(&cores_row);

    let disk_row = EntryRow::new();
    disk_row.set_title("Disk Size (GB)");
    disk_row.set_text(&config.default_disk_size_gb.to_string());
    defaults_group.add(&disk_row);

    let display_list = StringList::new(&["gtk", "sdl", "spice-app", "vnc", "none"]);
    let display_row = ComboRow::new();
    display_row.set_title("Default Display");
    display_row.set_model(Some(&display_list));
    display_row.set_selected(match config.default_display.as_str() {
        "gtk" => 0u32,
        "sdl" => 1,
        "spice-app" => 2,
        "vnc" => 3,
        _ => 4,
    });
    defaults_group.add(&display_row);

    // --- Behavior group ---
    let behavior_group = PreferencesGroup::new();
    behavior_group.set_title("Behavior");

    let kvm_row = SwitchRow::new();
    kvm_row.set_title("Enable KVM by Default");
    kvm_row.set_active(config.default_enable_kvm);
    behavior_group.add(&kvm_row);

    let confirm_row = SwitchRow::new();
    confirm_row.set_title("Confirm Before Launch");
    confirm_row.set_active(config.confirm_before_launch);
    behavior_group.add(&confirm_row);

    // --- Passthrough group ---
    let pt_group = PreferencesGroup::new();
    pt_group.set_title("Hardware Passthrough");

    let pt_list = StringList::new(&["Disabled", "Multi-GPU", "Single GPU"]);
    let pt_row = ComboRow::new();
    pt_row.set_title("Passthrough Mode");
    pt_row.set_model(Some(&pt_list));
    pt_row.set_selected(if config.enable_multi_gpu_passthrough {
        1u32
    } else if config.single_gpu_enabled {
        2
    } else {
        0
    });
    pt_group.add(&pt_row);

    // --- Layout ---
    let content_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(16)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    let validation_label = Label::builder()
        .halign(gtk4::Align::Start)
        .wrap(true)
        .visible(false)
        .build();
    validation_label.add_css_class("error");

    content_box.append(&lib_group);
    content_box.append(&defaults_group);
    content_box.append(&validation_label);
    content_box.append(&behavior_group);
    content_box.append(&pt_group);

    // Highlight invalid numeric fields as the user types
    for row in [&memory_row, &cores_row, &disk_row] {
        row.connect_changed(|r| {
            let text = r.text();
            if text.is_empty() || text.parse::<u32>().is_ok() {
                r.remove_css_class("error");
            } else {
                r.add_css_class("error");
            }
        });
    }

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

    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&scroll));
    dialog.set_child(Some(&toolbar_view));

    // --- Folder chooser ---
    {
        let lib_path = Rc::clone(&lib_path);
        let lib_row = lib_row.clone();
        let dialog_ref = dialog.clone();

        choose_btn.connect_clicked(move |_| {
            let file_dialog = gtk4::FileDialog::builder()
                .title("Choose VM Library Folder")
                .accept_label("Select")
                .build();

            let lib_path = Rc::clone(&lib_path);
            let lib_row = lib_row.clone();
            let parent_win = dialog_ref.root().and_downcast::<gtk4::Window>();
            file_dialog.select_folder(
                parent_win.as_ref(),
                gtk4::gio::Cancellable::NONE,
                move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            lib_row.set_subtitle(&path.display().to_string());
                            *lib_path.borrow_mut() = path;
                        }
                    }
                },
            );
        });
    }

    // --- Save ---
    {
        let lib_path = Rc::clone(&lib_path);
        let dialog_ref = dialog.clone();

        save_btn.connect_clicked(move |_| {
            let mem: Result<u32, _> = memory_row.text().parse();
            let cpu: Result<u32, _> = cores_row.text().parse();
            let disk: Result<u32, _> = disk_row.text().parse();
            if mem.is_err() || cpu.is_err() || disk.is_err() {
                validation_label.set_label("Memory, CPU cores, and disk size must be positive whole numbers.");
                validation_label.set_visible(true);
                return;
            }
            validation_label.set_visible(false);

            let new_config = Config {
                vm_library_path: lib_path.borrow().clone(),
                default_memory_mb: mem.unwrap(),
                default_cpu_cores: cpu.unwrap(),
                default_disk_size_gb: disk.unwrap(),
                default_display: match display_row.selected() {
                    0 => "gtk".to_string(),
                    1 => "sdl".to_string(),
                    2 => "spice-app".to_string(),
                    3 => "vnc".to_string(),
                    _ => "none".to_string(),
                },
                default_enable_kvm: kvm_row.is_active(),
                confirm_before_launch: confirm_row.is_active(),
                enable_multi_gpu_passthrough: pt_row.selected() == 1,
                single_gpu_enabled: pt_row.selected() == 2,
                // Preserve fields not exposed in this dialog
                ..config.clone()
            };
            let _ = new_config.save();
            on_save(new_config);
            dialog_ref.close();
        });
    }

    dialog.present(Some(parent));
}
