use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{Box as GtkBox, Button, Label, Orientation, ScrolledWindow};
use libadwaita::{ActionRow, HeaderBar, Toast, ToastOverlay, ToolbarView};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::vm::{save_shared_folders, DiscoveredVm, SharedFolder};

type TaggedFolders = Vec<(u32, SharedFolder)>;

fn make_folder_row(
    id: u32,
    folder: &SharedFolder,
    folders: Rc<RefCell<TaggedFolders>>,
    list_box: gtk4::ListBox,
) -> ActionRow {
    let row = ActionRow::new();
    row.set_title(&folder.host_path);
    row.set_subtitle(&folder.mount_tag);

    let remove_btn = Button::builder()
        .icon_name("list-remove-symbolic")
        .valign(gtk4::Align::Center)
        .build();
    remove_btn.add_css_class("flat");
    row.add_suffix(&remove_btn);

    let row_rm = row.clone();
    remove_btn.connect_clicked(move |_| {
        folders.borrow_mut().retain(|(rid, _)| *rid != id);
        list_box.remove(&row_rm);
    });

    row
}

pub fn show(parent: &impl IsA<gtk4::Widget>, vm: DiscoveredVm) {
    let dialog = libadwaita::Dialog::new();
    dialog.set_title(&format!("Shared Folders — {}", vm.display_name()));
    dialog.set_content_width(480);
    dialog.set_content_height(480);

    let next_id: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    let initial: TaggedFolders = vm_curator::vm::load_shared_folders(&vm)
        .into_iter()
        .map(|f| {
            let id = next_id.get();
            next_id.set(id + 1);
            (id, f)
        })
        .collect();
    let folders: Rc<RefCell<TaggedFolders>> = Rc::new(RefCell::new(initial));

    // --- Folders list ---
    let pf_header = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .margin_bottom(4)
        .build();
    let pf_label = Label::builder()
        .label("Shared Folders")
        .halign(gtk4::Align::Start)
        .hexpand(true)
        .build();
    pf_label.add_css_class("heading");

    let add_btn = Button::builder().label("Add Folder…").build();
    add_btn.add_css_class("flat");
    pf_header.append(&pf_label);
    pf_header.append(&add_btn);

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::None);

    let empty_label = Label::builder()
        .label("No shared folders")
        .halign(gtk4::Align::Center)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    empty_label.add_css_class("dim-label");
    list_box.set_placeholder(Some(&empty_label));

    for (id, folder) in folders.borrow().iter() {
        list_box.append(&make_folder_row(
            *id,
            folder,
            Rc::clone(&folders),
            list_box.clone(),
        ));
    }

    // --- Add folder ---
    {
        let folders = Rc::clone(&folders);
        let list_box = list_box.clone();
        let next_id = Rc::clone(&next_id);
        let dialog_ref = dialog.clone();

        add_btn.connect_clicked(move |_| {
            let file_dialog = gtk4::FileDialog::builder()
                .title("Choose Folder to Share")
                .accept_label("Select")
                .build();

            let folders = Rc::clone(&folders);
            let list_box = list_box.clone();
            let next_id = Rc::clone(&next_id);
            let parent_win = dialog_ref.root().and_downcast::<gtk4::Window>();
            file_dialog.select_folder(
                parent_win.as_ref(),
                gtk4::gio::Cancellable::NONE,
                move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            let host_path = path.display().to_string();
                            let mount_tag = path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| "share".to_string());
                            let id = next_id.get();
                            next_id.set(id + 1);
                            let folder =
                                SharedFolder { host_path, mount_tag };
                            list_box.append(&make_folder_row(
                                id,
                                &folder,
                                Rc::clone(&folders),
                                list_box.clone(),
                            ));
                            folders.borrow_mut().push((id, folder));
                        }
                    }
                },
            );
        });
    }

    // --- Layout ---
    let content_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    content_box.append(&pf_header);
    content_box.append(&list_box);

    let note = Label::builder()
        .label("Folders are mounted via VirtIO 9p. Guest must have virtio-9p driver loaded.")
        .wrap(true)
        .halign(gtk4::Align::Start)
        .build();
    note.add_css_class("caption");
    note.add_css_class("dim-label");
    content_box.append(&note);

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

        save_btn.connect_clicked(move |_| {
            let folder_list: Vec<SharedFolder> =
                folders.borrow().iter().map(|(_, f)| f.clone()).collect();
            let vm = vm.clone();
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let result =
                    save_shared_folders(&vm, &folder_list).map_err(|e| e.to_string());
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
