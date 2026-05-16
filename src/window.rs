use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{
    glib, Box as GtkBox, Button, Label, Orientation, Paned, ScrolledWindow, SelectionMode,
};
use libadwaita::{ActionRow, ApplicationWindow, HeaderBar, Toast, ToastOverlay, ToolbarView};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::config::Config;
use vm_curator::vm::{
    detect_qemu_processes, discover_vms, launch_vm_with_error_check, BootMode, DiscoveredVm,
    LaunchOptions,
};

pub fn build_and_show(app: &libadwaita::Application) {
    let config = Config::load().unwrap_or_default();

    let vms: Rc<Vec<DiscoveredVm>> =
        Rc::new(discover_vms(&config.vm_library_path).unwrap_or_default());

    let selected: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));

    // PID of the running QEMU process for each VM, or None if not running.
    // Indexed in sync with `vms`.
    let running_pids: Rc<RefCell<Vec<Option<u32>>>> =
        Rc::new(RefCell::new(vec![None; vms.len()]));

    // --- Left panel: VM list ---
    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(SelectionMode::Single);
    list_box.add_css_class("navigation-sidebar");

    // Keep handles to every row so the detection timer can update their subtitles.
    let mut rows_vec: Vec<ActionRow> = Vec::with_capacity(vms.len());
    for vm in vms.iter() {
        let row = ActionRow::new();
        row.set_title(&vm.display_name());
        row.set_subtitle(&vm.id);
        list_box.append(&row);
        rows_vec.push(row);
    }
    let rows: Rc<Vec<ActionRow>> = Rc::new(rows_vec);

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

    // --- Wire list selection ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        let running_pids = Rc::clone(&running_pids);
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

                let is_running = running_pids.borrow()[idx].is_some();
                launch_button.set_sensitive(!is_running);
                launch_button.set_label(if is_running { "Running" } else { "Launch VM" });
            } else {
                *selected.borrow_mut() = None;
                detail_label.set_label("Select a VM from the list.");
                detail_label.set_use_markup(false);
                detail_label.add_css_class("dim-label");
                launch_button.set_sensitive(false);
                launch_button.set_label("Launch VM");
            }
        });
    }

    // --- Toast overlay ---
    let toast_overlay = ToastOverlay::new();

    // --- Wire launch button ---
    {
        let vms = Rc::clone(&vms);
        let selected = Rc::clone(&selected);
        let toast_overlay = toast_overlay.clone();

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

            let (tx, rx) = mpsc::channel::<(String, bool, Option<String>)>();
            let rx = Rc::new(RefCell::new(rx));

            std::thread::spawn(move || {
                let options = LaunchOptions {
                    boot_mode: BootMode::Normal,
                    extra_args: Vec::new(),
                    usb_devices: Vec::new(),
                };
                let result = launch_vm_with_error_check(&vm, &options);
                tx.send((result.vm_name, result.success, result.error)).ok();
            });

            let launch_button_poll = btn.clone();
            let toast_overlay_poll = toast_overlay.clone();
            glib::timeout_add_local(Duration::from_millis(200), move || {
                match rx.borrow().try_recv() {
                    Ok((vm_name, success, error)) => {
                        // Running-state badge will update on the next detection tick;
                        // only restore the button here if the launch failed.
                        if success {
                            toast_overlay_poll.add_toast(Toast::new(&format!("{vm_name} launched")));
                        } else {
                            launch_button_poll.set_sensitive(true);
                            launch_button_poll.set_label("Launch VM");
                            let msg = error.as_deref().unwrap_or("unknown error");
                            let toast = Toast::builder()
                                .title(&format!("Failed to launch {vm_name}: {msg}"))
                                .timeout(0)
                                .build();
                            toast_overlay_poll.add_toast(toast);
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

    // --- Running VM detection (every 3 s) ---
    {
        let vms = Rc::clone(&vms);
        let rows = Rc::clone(&rows);
        let running_pids = Rc::clone(&running_pids);
        let selected = Rc::clone(&selected);
        let launch_button = launch_button.clone();

        glib::timeout_add_seconds_local(3, move || {
            let processes = detect_qemu_processes();
            let mut pids = running_pids.borrow_mut();

            for (i, vm) in vms.iter().enumerate() {
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

                    // Keep the launch button consistent with the selected VM's new state.
                    if selected.borrow().as_ref() == Some(&i) {
                        launch_button.set_sensitive(new_pid.is_none());
                        launch_button.set_label(if new_pid.is_some() { "Running" } else { "Launch VM" });
                    }
                }
            }

            glib::ControlFlow::Continue
        });
    }

    // --- Main layout ---
    let paned = Paned::new(Orientation::Horizontal);
    paned.set_start_child(Some(&left_scroll));
    paned.set_end_child(Some(&right_panel));
    paned.set_position(260);
    paned.set_shrink_start_child(false);
    paned.set_shrink_end_child(false);

    toast_overlay.set_child(Some(&paned));

    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&HeaderBar::new());
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
