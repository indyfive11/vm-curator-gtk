use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{Box as GtkBox, Button, Orientation, ScrolledWindow, TextView, WrapMode};
use libadwaita::{HeaderBar, Toast, ToastOverlay, ToolbarView};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::vm::{save_notes, DiscoveredVm};

pub fn show(
    parent: &impl IsA<gtk4::Widget>,
    vm: &DiscoveredVm,
    on_saved: impl Fn(Option<String>) + 'static,
) {
    let dialog = libadwaita::Dialog::new();
    dialog.set_title(&format!("Notes — {}", vm.display_name()));
    dialog.set_content_width(480);
    dialog.set_content_height(420);

    let text_view = TextView::builder()
        .wrap_mode(WrapMode::Word)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .build();

    if let Some(ref notes) = vm.notes {
        text_view.buffer().set_text(notes);
    }

    let scroll = ScrolledWindow::builder()
        .child(&text_view)
        .hexpand(true)
        .vexpand(true)
        .min_content_height(160)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(4)
        .build();

    let save_btn = Button::builder().label("Save").hexpand(true).build();
    save_btn.add_css_class("suggested-action");

    let clear_btn = Button::builder().label("Clear Notes").hexpand(true).build();

    let btn_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(12)
        .margin_top(4)
        .build();
    btn_row.append(&save_btn);
    btn_row.append(&clear_btn);

    let toast_overlay = ToastOverlay::new();
    let content = GtkBox::new(Orientation::Vertical, 0);
    content.append(&scroll);
    content.append(&btn_row);
    toast_overlay.set_child(Some(&content));

    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&HeaderBar::new());
    toolbar_view.set_content(Some(&toast_overlay));
    dialog.set_child(Some(&toolbar_view));

    // Clear button: empty the text view
    {
        let buffer = text_view.buffer();
        clear_btn.connect_clicked(move |_| {
            buffer.set_text("");
        });
    }

    // Save button
    {
        let text_view = text_view.clone();
        let toast_overlay = toast_overlay.clone();
        let dialog_ref = dialog.clone();
        let vm = vm.clone();
        let on_saved = Rc::new(on_saved);

        save_btn.connect_clicked(move |_| {
            let buffer = text_view.buffer();
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            let notes_opt: Option<String> = if text.trim().is_empty() {
                None
            } else {
                Some(text.trim().to_string())
            };

            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            let vm = vm.clone();
            let notes_for_thread = notes_opt.clone();
            std::thread::spawn(move || {
                let result =
                    save_notes(&vm, notes_for_thread.as_deref()).map_err(|e| e.to_string());
                tx.send(result).ok();
            });

            let toast_overlay = toast_overlay.clone();
            let dialog_ref = dialog_ref.clone();
            let on_saved = Rc::clone(&on_saved);
            let rx = Rc::new(RefCell::new(rx));
            gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
                match rx.borrow().try_recv() {
                    Ok(Ok(())) => {
                        on_saved(notes_opt.clone());
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
