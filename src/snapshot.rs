use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{
    glib, Box as GtkBox, Button, Entry, Orientation, ScrolledWindow, SelectionMode, Separator,
};
use libadwaita::{ActionRow, AlertDialog, HeaderBar, ResponseAppearance, Toast, ToastOverlay,
                 ToolbarView};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::vm::{
    create_snapshot, delete_snapshot, list_snapshots, restore_snapshot, Snapshot,
};

/// Open the snapshot management dialog for a VM.
/// `disk_path` must be the first qcow2 disk (`vm.config.disks[0].path`).
pub fn show(parent: &impl IsA<gtk4::Widget>, vm_name: &str, disk_path: PathBuf) {
    // --- State ---
    let snapshots: Rc<RefCell<Vec<Snapshot>>> = Rc::new(RefCell::new(Vec::new()));
    let selected_idx: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));

    // --- Dialog shell ---
    let dialog = libadwaita::Dialog::new();
    dialog.set_title(&format!("Snapshots — {vm_name}"));
    dialog.set_content_width(520);
    dialog.set_content_height(480);

    // --- Entry row for new snapshot name ---
    let name_entry = Entry::builder()
        .placeholder_text("New snapshot name")
        .hexpand(true)
        .activates_default(true)
        .build();

    let create_btn = Button::builder().label("Create").build();
    create_btn.add_css_class("suggested-action");

    let entry_box = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(8)
        .build();
    entry_box.append(&name_entry);
    entry_box.append(&create_btn);

    // --- Snapshot list ---
    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(SelectionMode::Single);
    list_box.add_css_class("boxed-list");

    let list_scroll = ScrolledWindow::builder()
        .child(&list_box)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .min_content_height(200)
        .margin_start(12)
        .margin_end(12)
        .margin_top(4)
        .margin_bottom(8)
        .build();

    // --- Restore / Delete buttons ---
    let restore_btn = Button::builder()
        .label("Restore")
        .sensitive(false)
        .hexpand(true)
        .build();

    let delete_btn = Button::builder()
        .label("Delete")
        .sensitive(false)
        .hexpand(true)
        .build();
    delete_btn.add_css_class("destructive-action");

    let action_box = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(12)
        .build();
    action_box.append(&restore_btn);
    action_box.append(&delete_btn);

    // --- Toast overlay wrapping the whole content area ---
    let toast_overlay = ToastOverlay::new();

    let content = GtkBox::new(Orientation::Vertical, 0);
    content.append(&entry_box);
    content.append(&Separator::new(Orientation::Horizontal));
    content.append(&list_scroll);
    content.append(&action_box);
    toast_overlay.set_child(Some(&content));

    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&HeaderBar::new());
    toolbar_view.set_content(Some(&toast_overlay));
    dialog.set_child(Some(&toolbar_view));

    // --- Refresh helper: reload the list from disk ---
    let refresh = {
        let list_box = list_box.clone();
        let snapshots = Rc::clone(&snapshots);
        let selected_idx = Rc::clone(&selected_idx);
        let restore_btn = restore_btn.clone();
        let delete_btn = delete_btn.clone();
        let toast_overlay = toast_overlay.clone();
        let disk_path = disk_path.clone();

        move || {
            while let Some(child) = list_box.first_child() {
                list_box.remove(&child);
            }
            *selected_idx.borrow_mut() = None;
            restore_btn.set_sensitive(false);
            delete_btn.set_sensitive(false);

            match list_snapshots(&disk_path) {
                Ok(snaps) => {
                    if snaps.is_empty() {
                        let row = ActionRow::new();
                        row.set_title("No snapshots yet");
                        row.set_sensitive(false);
                        list_box.append(&row);
                    } else {
                        for snap in &snaps {
                            let row = ActionRow::new();
                            row.set_title(&snap.name);
                            row.set_subtitle(&format!("{}  ·  {}", snap.date, snap.size));
                            row.set_activatable(true);
                            list_box.append(&row);
                        }
                    }
                    *snapshots.borrow_mut() = snaps;
                }
                Err(e) => {
                    toast_overlay.add_toast(
                        Toast::builder()
                            .title(&format!("Failed to load snapshots: {e}"))
                            .timeout(0)
                            .build(),
                    );
                }
            }
        }
    };

    // --- Row selection ---
    {
        let selected_idx = Rc::clone(&selected_idx);
        let snapshots = Rc::clone(&snapshots);
        let restore_btn = restore_btn.clone();
        let delete_btn = delete_btn.clone();

        list_box.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let idx = row.index() as usize;
                let is_real = idx < snapshots.borrow().len();
                *selected_idx.borrow_mut() = if is_real { Some(idx) } else { None };
                restore_btn.set_sensitive(is_real);
                delete_btn.set_sensitive(is_real);
            } else {
                *selected_idx.borrow_mut() = None;
                restore_btn.set_sensitive(false);
                delete_btn.set_sensitive(false);
            }
        });
    }

    // --- Create ---
    {
        let name_entry = name_entry.clone();
        let toast_overlay = toast_overlay.clone();
        let disk_path = disk_path.clone();
        let refresh = refresh.clone();
        let create_btn = create_btn.clone();

        create_btn.clone().connect_clicked(move |_| {
            let name = name_entry.text().trim().to_string();
            if name.is_empty() {
                toast_overlay.add_toast(Toast::new("Enter a snapshot name first."));
                return;
            }
            name_entry.set_sensitive(false);
            create_btn.set_sensitive(false);

            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            let dp = disk_path.clone();
            std::thread::spawn(move || {
                let result = create_snapshot(&dp, &name).map_err(|e| e.to_string());
                tx.send(result).ok();
            });

            let name_entry = name_entry.clone();
            let create_btn = create_btn.clone();
            let toast_overlay = toast_overlay.clone();
            let refresh = refresh.clone();
            let rx = Rc::new(RefCell::new(rx));
            glib::timeout_add_local(Duration::from_millis(200), move || {
                match rx.borrow().try_recv() {
                    Ok(Ok(())) => {
                        name_entry.set_text("");
                        name_entry.set_sensitive(true);
                        create_btn.set_sensitive(true);
                        toast_overlay.add_toast(Toast::new("Snapshot created."));
                        refresh();
                        glib::ControlFlow::Break
                    }
                    Ok(Err(e)) => {
                        name_entry.set_sensitive(true);
                        create_btn.set_sensitive(true);
                        toast_overlay.add_toast(
                            Toast::builder()
                                .title(&format!("Create failed: {e}"))
                                .timeout(0)
                                .build(),
                        );
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        name_entry.set_sensitive(true);
                        create_btn.set_sensitive(true);
                        glib::ControlFlow::Break
                    }
                }
            });
        });
    }

    // --- Restore (with confirmation) ---
    {
        let selected_idx = Rc::clone(&selected_idx);
        let snapshots = Rc::clone(&snapshots);
        let toast_overlay = toast_overlay.clone();
        let disk_path = disk_path.clone();
        let refresh = refresh.clone();
        let dialog_ref = dialog.clone();

        restore_btn.connect_clicked(move |_| {
            let idx = match *selected_idx.borrow() {
                Some(i) => i,
                None => return,
            };
            let snap_name = match snapshots.borrow().get(idx) {
                Some(s) => s.name.clone(),
                None => return,
            };

            let alert = AlertDialog::builder()
                .heading("Restore Snapshot?")
                .body(&format!(
                    "Restore \"{snap_name}\"? All changes since this snapshot was taken will be lost."
                ))
                .build();
            alert.add_response("cancel", "Cancel");
            alert.add_response("restore", "Restore");
            alert.set_response_appearance("restore", ResponseAppearance::Destructive);
            alert.set_default_response(Some("cancel"));
            alert.set_close_response("cancel");

            let toast_overlay = toast_overlay.clone();
            let disk_path = disk_path.clone();
            let refresh = refresh.clone();
            alert.connect_response(None, move |_, response| {
                if response != "restore" {
                    return;
                }
                let name = snap_name.clone();
                let (tx, rx) = mpsc::channel::<Result<(), String>>();
                let dp = disk_path.clone();
                std::thread::spawn(move || {
                    let result = restore_snapshot(&dp, &name).map_err(|e| e.to_string());
                    tx.send(result).ok();
                });

                let toast_overlay = toast_overlay.clone();
                let refresh = refresh.clone();
                let rx = Rc::new(RefCell::new(rx));
                glib::timeout_add_local(Duration::from_millis(200), move || {
                    match rx.borrow().try_recv() {
                        Ok(Ok(())) => {
                            toast_overlay.add_toast(Toast::new("Snapshot restored."));
                            refresh();
                            glib::ControlFlow::Break
                        }
                        Ok(Err(e)) => {
                            toast_overlay.add_toast(
                                Toast::builder()
                                    .title(&format!("Restore failed: {e}"))
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

            alert.present(Some(&dialog_ref));
        });
    }

    // --- Delete (with confirmation) ---
    {
        let selected_idx = Rc::clone(&selected_idx);
        let snapshots = Rc::clone(&snapshots);
        let toast_overlay = toast_overlay.clone();
        let disk_path = disk_path.clone();
        let dialog_ref = dialog.clone();
        let refresh = refresh.clone();

        delete_btn.connect_clicked(move |_| {
            let idx = match *selected_idx.borrow() {
                Some(i) => i,
                None => return,
            };
            let snap_name = match snapshots.borrow().get(idx) {
                Some(s) => s.name.clone(),
                None => return,
            };

            let alert = AlertDialog::builder()
                .heading("Delete Snapshot?")
                .body(&format!("Delete \"{snap_name}\"? This cannot be undone."))
                .build();
            alert.add_response("cancel", "Cancel");
            alert.add_response("delete", "Delete");
            alert.set_response_appearance("delete", ResponseAppearance::Destructive);
            alert.set_default_response(Some("cancel"));
            alert.set_close_response("cancel");

            let toast_overlay = toast_overlay.clone();
            let disk_path = disk_path.clone();
            let refresh = refresh.clone();
            alert.connect_response(None, move |_, response| {
                if response != "delete" {
                    return;
                }
                let name = snap_name.clone();
                let (tx, rx) = mpsc::channel::<Result<(), String>>();
                let dp = disk_path.clone();
                std::thread::spawn(move || {
                    let result = delete_snapshot(&dp, &name).map_err(|e| e.to_string());
                    tx.send(result).ok();
                });

                let toast_overlay = toast_overlay.clone();
                let refresh = refresh.clone();
                let rx = Rc::new(RefCell::new(rx));
                glib::timeout_add_local(Duration::from_millis(200), move || {
                    match rx.borrow().try_recv() {
                        Ok(Ok(())) => {
                            toast_overlay.add_toast(Toast::new("Snapshot deleted."));
                            refresh();
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

            alert.present(Some(&dialog_ref));
        });
    }

    // Initial snapshot load
    refresh();

    dialog.present(Some(parent));
}
