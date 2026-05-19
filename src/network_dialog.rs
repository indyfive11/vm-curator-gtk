use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{Box as GtkBox, Button, Label, Orientation, ScrolledWindow, StringList};
use libadwaita::{ActionRow, ComboRow, EntryRow, HeaderBar, PreferencesGroup, Toast,
                 ToastOverlay, ToolbarView};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::vm::{update_network_in_script, DiscoveredVm, NetworkBackend, PortForward,
                     PortProtocol};

// Rows keyed by a stable u32 ID so remove buttons can operate by ID
// without needing to rebuild the whole list.
type TaggedForwards = Vec<(u32, PortForward)>;

fn make_pf_row(
    id: u32,
    fwd: &PortForward,
    forwards: Rc<RefCell<TaggedForwards>>,
    list_box: gtk4::ListBox,
) -> ActionRow {
    let proto_str = match fwd.protocol {
        PortProtocol::Tcp => "TCP",
        PortProtocol::Udp => "UDP",
    };
    let row = ActionRow::new();
    row.set_title(&format!("{} {}→{}", proto_str, fwd.host_port, fwd.guest_port));

    let remove_btn = Button::builder()
        .icon_name("list-remove-symbolic")
        .valign(gtk4::Align::Center)
        .build();
    remove_btn.add_css_class("flat");
    row.add_suffix(&remove_btn);

    let row_rm = row.clone();
    remove_btn.connect_clicked(move |_| {
        forwards.borrow_mut().retain(|(rid, _)| *rid != id);
        list_box.remove(&row_rm);
    });

    row
}

pub fn show(parent: &impl IsA<gtk4::Widget>, vm: DiscoveredVm) {
    let dialog = libadwaita::Dialog::new();
    dialog.set_title(&format!("Network — {}", vm.display_name()));
    dialog.set_content_width(480);
    dialog.set_content_height(540);

    let net = vm.config.network.clone();

    let next_id: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    let initial: TaggedForwards = net
        .as_ref()
        .map(|n| {
            n.port_forwards
                .iter()
                .map(|f| {
                    let id = next_id.get();
                    next_id.set(id + 1);
                    (id, f.clone())
                })
                .collect()
        })
        .unwrap_or_default();
    let forwards: Rc<RefCell<TaggedForwards>> = Rc::new(RefCell::new(initial));

    // --- Network Config group ---
    let net_group = PreferencesGroup::new();
    net_group.set_title("Network Configuration");

    let backend_list = StringList::new(&["User (NAT)", "Passt", "Bridge", "None"]);
    let backend_row = ComboRow::new();
    backend_row.set_title("Backend");
    backend_row.set_model(Some(&backend_list));
    backend_row.set_selected(match net.as_ref().map(|n| &n.backend) {
        Some(NetworkBackend::User) | None => 0u32,
        Some(NetworkBackend::Passt) => 1,
        Some(NetworkBackend::Bridge(_)) => 2,
        Some(NetworkBackend::None) => 3,
    });
    net_group.add(&backend_row);

    let bridge_row = EntryRow::new();
    bridge_row.set_title("Bridge Name");
    bridge_row.set_text(
        net.as_ref()
            .and_then(|n| {
                if let NetworkBackend::Bridge(ref b) = n.backend {
                    Some(b.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("br0"),
    );
    bridge_row.set_visible(backend_row.selected() == 2);
    net_group.add(&bridge_row);

    let mac_row = EntryRow::new();
    mac_row.set_title("MAC Address");
    mac_row.set_text(net.as_ref().and_then(|n| n.mac_address.as_deref()).unwrap_or(""));
    net_group.add(&mac_row);

    let model_list = StringList::new(&["virtio-net-pci", "e1000", "rtl8139"]);
    let model_row = ComboRow::new();
    model_row.set_title("Adapter Model");
    model_row.set_model(Some(&model_list));
    model_row.set_selected(match net.as_ref().map(|n| n.model.as_str()) {
        Some("e1000") => 1u32,
        Some("rtl8139") => 2,
        _ => 0,
    });
    net_group.add(&model_row);

    {
        let bridge_row = bridge_row.clone();
        backend_row.connect_selected_notify(move |row| {
            bridge_row.set_visible(row.selected() == 2);
        });
    }

    // --- Port Forwards group ---
    let pf_header = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .margin_top(16)
        .margin_bottom(4)
        .build();
    let pf_label = Label::builder()
        .label("Port Forwards")
        .halign(gtk4::Align::Start)
        .hexpand(true)
        .build();
    pf_label.add_css_class("heading");

    let add_pf_btn = Button::builder().label("Add…").build();
    add_pf_btn.add_css_class("flat");
    pf_header.append(&pf_label);
    pf_header.append(&add_pf_btn);

    let pf_list_box = gtk4::ListBox::new();
    pf_list_box.add_css_class("boxed-list");
    pf_list_box.set_selection_mode(gtk4::SelectionMode::None);

    // Populate initial rows
    for (id, fwd) in forwards.borrow().iter() {
        pf_list_box.append(&make_pf_row(
            *id,
            fwd,
            Rc::clone(&forwards),
            pf_list_box.clone(),
        ));
    }

    // --- Add port forward dialog ---
    {
        let forwards = Rc::clone(&forwards);
        let pf_list_box = pf_list_box.clone();
        let next_id = Rc::clone(&next_id);

        add_pf_btn.connect_clicked(move |btn| {
            let alert = libadwaita::AlertDialog::new(Some("Add Port Forward"), None);
            alert.add_response("cancel", "Cancel");
            alert.add_response("add", "Add");
            alert.set_response_appearance("add", libadwaita::ResponseAppearance::Suggested);
            alert.set_response_enabled("add", false);

            let proto_list = StringList::new(&["TCP", "UDP"]);
            let proto_row = ComboRow::new();
            proto_row.set_title("Protocol");
            proto_row.set_model(Some(&proto_list));

            let host_entry = EntryRow::new();
            host_entry.set_title("Host Port (1–65535)");

            let guest_entry = EntryRow::new();
            guest_entry.set_title("Guest Port (1–65535)");

            let form = GtkBox::new(Orientation::Vertical, 0);
            form.add_css_class("boxed-list");
            form.append(&proto_row);
            form.append(&host_entry);
            form.append(&guest_entry);
            alert.set_extra_child(Some(&form));

            fn port_valid(s: &str) -> bool {
                s.parse::<u16>().map_or(false, |p| p > 0)
            }

            // Enable Add only when both ports are valid u16 > 0
            {
                let alert_c = alert.clone();
                let host_c = host_entry.clone();
                let guest_c = guest_entry.clone();
                host_entry.connect_changed(move |_| {
                    alert_c.set_response_enabled("add", port_valid(&host_c.text()) && port_valid(&guest_c.text()));
                });
            }
            {
                let alert_c = alert.clone();
                let host_c = host_entry.clone();
                let guest_c = guest_entry.clone();
                guest_entry.connect_changed(move |_| {
                    alert_c.set_response_enabled("add", port_valid(&host_c.text()) && port_valid(&guest_c.text()));
                });
            }

            let forwards = Rc::clone(&forwards);
            let pf_list_box = pf_list_box.clone();
            let next_id = Rc::clone(&next_id);
            alert.connect_response(None, move |_, response| {
                if response != "add" {
                    return;
                }
                let Ok(host) = host_entry.text().parse::<u16>() else { return };
                let Ok(guest) = guest_entry.text().parse::<u16>() else { return };
                let protocol = if proto_row.selected() == 0 {
                    PortProtocol::Tcp
                } else {
                    PortProtocol::Udp
                };
                let id = next_id.get();
                next_id.set(id + 1);
                let fwd = PortForward { protocol, host_port: host, guest_port: guest };
                pf_list_box.append(&make_pf_row(
                    id,
                    &fwd,
                    Rc::clone(&forwards),
                    pf_list_box.clone(),
                ));
                forwards.borrow_mut().push((id, fwd));
            });

            alert.present(Some(btn));
        });
    }

    // --- Layout ---
    let content_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(0)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    content_box.append(&net_group);
    content_box.append(&pf_header);
    content_box.append(&pf_list_box);

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

    // --- Save ---
    {
        let dialog_ref = dialog.clone();
        let toast_overlay = toast_overlay.clone();
        let vm_path = vm.path.clone();

        save_btn.connect_clicked(move |_| {
            let model = match model_row.selected() {
                1 => "e1000",
                2 => "rtl8139",
                _ => "virtio-net-pci",
            }
            .to_string();
            let backend = match backend_row.selected() {
                1 => "passt",
                2 => "bridge",
                3 => "none",
                _ => "user",
            }
            .to_string();
            let bridge = bridge_row.text().to_string();
            let bridge_opt: Option<String> =
                if backend == "bridge" && !bridge.is_empty() { Some(bridge) } else { None };
            let mac_text = mac_row.text().to_string();
            let mac_opt: Option<String> = if mac_text.is_empty() { None } else { Some(mac_text) };
            let pf_vec: Vec<PortForward> =
                forwards.borrow().iter().map(|(_, f)| f.clone()).collect();

            let vm_path = vm_path.clone();
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let result = update_network_in_script(
                    &vm_path,
                    &model,
                    &backend,
                    bridge_opt.as_deref(),
                    &pf_vec,
                    mac_opt.as_deref(),
                )
                .map_err(|e| e.to_string());
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
