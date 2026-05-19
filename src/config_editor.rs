use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{Button, ScrolledWindow, TextView, WrapMode};
use libadwaita::{HeaderBar, Toast, ToastOverlay, ToolbarView};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::vm::DiscoveredVm;

pub fn show(parent: &impl IsA<gtk4::Widget>, vm: DiscoveredVm) {
    let dialog = libadwaita::Dialog::new();
    dialog.set_title(&format!("Raw Config — {}", vm.display_name()));
    dialog.set_content_width(700);
    dialog.set_content_height(520);

    let text_view = TextView::builder()
        .wrap_mode(WrapMode::None)
        .monospace(true)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .build();
    text_view.buffer().set_text(&vm.config.raw_script);

    let dirty: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    {
        let dirty = Rc::clone(&dirty);
        text_view.buffer().connect_changed(move |_| {
            dirty.set(true);
        });
    }

    let scroll = ScrolledWindow::builder()
        .child(&text_view)
        .hexpand(true)
        .vexpand(true)
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

    // Intercept close to warn about unsaved changes
    dialog.set_can_close(false);
    {
        let dirty = Rc::clone(&dirty);
        dialog.connect_close_attempt(move |d| {
            if !dirty.get() {
                d.force_close();
                return;
            }
            let alert = libadwaita::AlertDialog::new(
                Some("Discard Changes?"),
                Some("Your edits to the launch script will be lost."),
            );
            alert.add_response("cancel", "Cancel");
            alert.add_response("discard", "Discard");
            alert.set_response_appearance("discard", libadwaita::ResponseAppearance::Destructive);
            let d_for_close = d.clone();
            let d_for_present = d.clone();
            let dirty = Rc::clone(&dirty);
            alert.connect_response(None, move |_, response| {
                if response == "discard" {
                    dirty.set(false);
                    d_for_close.force_close();
                }
            });
            alert.present(Some(&d_for_present));
        });
    }

    // --- Save ---
    {
        let dialog_ref = dialog.clone();
        let toast_overlay = toast_overlay.clone();
        let dirty = Rc::clone(&dirty);
        let launch_script = vm.launch_script.clone();

        save_btn.connect_clicked(move |_| {
            let buffer = text_view.buffer();
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();

            let launch_script = launch_script.clone();
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let result =
                    std::fs::write(&launch_script, text).map_err(|e| e.to_string());
                tx.send(result).ok();
            });

            let dialog_ref = dialog_ref.clone();
            let toast_overlay = toast_overlay.clone();
            let dirty = Rc::clone(&dirty);
            let rx = Rc::new(RefCell::new(rx));
            gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
                match rx.borrow().try_recv() {
                    Ok(Ok(())) => {
                        dirty.set(false);
                        dialog_ref.force_close();
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
