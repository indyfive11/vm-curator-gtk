use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{
    glib, Box as GtkBox, Button, Label, Orientation, Paned, ScrolledWindow, SelectionMode,
};
use libadwaita::{ActionRow, ApplicationWindow, HeaderBar, ToolbarView};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::config::Config;
use vm_curator::vm::{discover_vms, launch_vm_with_error_check, DiscoveredVm, LaunchOptions, BootMode};

pub fn build_and_show(app: &libadwaita::Application) {
    let config = Config::load().unwrap_or_default();

    let vms: Rc<Vec<DiscoveredVm>> =
        Rc::new(discover_vms(&config.vm_library_path).unwrap_or_default());

    let selected: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));

    // --- Left panel: VM list ---
    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(SelectionMode::Single);
    list_box.add_css_class("navigation-sidebar");

    for vm in vms.iter() {
        let row = ActionRow::new();
        row.set_title(&vm.display_name());
        row.set_subtitle(&vm.id);
        list_box.append(&row);
    }

    if vms.is_empty() {
        let placeholder = Label::new(Some("No VMs found.\nConfigure vm-curator first."));
        placeholder.set_justify(gtk4::Justification::Center);
        placeholder.add_css_class("dim-label");
        placeholder.set_margin_top(24);
        placeholder.set_margin_bottom(24);
        list_box.append(&placeholder);
    }

    let left_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .child(&list_box)
        .width_request(240)
        .build();

    // --- Right panel: VM detail + launch ---
    let detail_label = Label::builder()
        .label("Select a VM from the list.")
        .halign(gtk4::Align::Start)
        .valign(gtk4::Align::Start)
        .wrap(true)
        .margin_start(16)
        .margin_top(16)
        .build();
    detail_label.add_css_class("dim-label");

    let launch_button = Button::builder()
        .label("Launch VM")
        .sensitive(false)
        .margin_start(16)
        .margin_top(8)
        .halign(gtk4::Align::Start)
        .build();
    launch_button.add_css_class("suggested-action");
    launch_button.add_css_class("pill");

    let right_panel = GtkBox::new(Orientation::Vertical, 0);
    right_panel.set_hexpand(true);
    right_panel.set_vexpand(true);
    right_panel.append(&detail_label);
    right_panel.append(&launch_button);

    // --- Wire list selection to detail panel ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        let detail_label = detail_label.clone();
        let launch_button = launch_button.clone();

        list_box.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let idx = row.index() as usize;
                if idx >= vms.len() {
                    return;
                }
                *selected.borrow_mut() = Some(idx);
                let vm = &vms[idx];
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
                launch_button.set_sensitive(true);
            } else {
                *selected.borrow_mut() = None;
                detail_label.set_label("Select a VM from the list.");
                detail_label.set_use_markup(false);
                detail_label.add_css_class("dim-label");
                launch_button.set_sensitive(false);
            }
        });
    }

    // --- Wire launch button ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);

        launch_button.connect_clicked(move |btn| {
            let idx = match *selected.borrow() {
                Some(i) => i,
                None => return,
            };
            if idx >= vms.len() {
                return;
            }
            let vm = vms[idx].clone();
            btn.set_sensitive(false);
            btn.set_label("Launching…");

            // mpsc channel: thread sends result, main-thread timeout polls for it
            let (tx, rx) = mpsc::channel::<(String, bool)>();
            let rx = Rc::new(RefCell::new(rx));

            std::thread::spawn(move || {
                let options = LaunchOptions {
                    boot_mode: BootMode::Normal,
                    extra_args: Vec::new(),
                    usb_devices: Vec::new(),
                };
                let result = launch_vm_with_error_check(&vm, &options);
                tx.send((result.vm_name, result.success)).ok();
            });

            // Poll every 200ms; stop polling once the result arrives
            let launch_button_poll = btn.clone();
            glib::timeout_add_local(Duration::from_millis(200), move || {
                match rx.borrow().try_recv() {
                    Ok((vm_name, success)) => {
                        launch_button_poll.set_sensitive(true);
                        launch_button_poll.set_label("Launch VM");
                        if !success {
                            eprintln!("Launch failed for {vm_name}");
                        }
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        launch_button_poll.set_sensitive(true);
                        launch_button_poll.set_label("Launch VM");
                        glib::ControlFlow::Break
                    }
                }
            });
        });
    }

    // --- Main layout ---
    let paned = Paned::new(Orientation::Horizontal);
    paned.set_start_child(Some(&left_scroll));
    paned.set_end_child(Some(&right_panel));
    paned.set_position(260);
    paned.set_shrink_start_child(false);
    paned.set_shrink_end_child(false);

    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&HeaderBar::new());
    toolbar_view.set_content(Some(&paned));

    let window = ApplicationWindow::builder()
        .application(app)
        .title("VM Curator")
        .default_width(960)
        .default_height(640)
        .content(&toolbar_view)
        .build();

    window.present();
}
