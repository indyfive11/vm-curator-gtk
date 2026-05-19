use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{Box as GtkBox, Button, Label, Orientation, ScrolledWindow, StringList};
use libadwaita::{ComboRow, HeaderBar, PreferencesGroup, SwitchRow, Toast, ToastOverlay,
                 ToolbarView};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use vm_curator::vm::DiscoveredVm;

fn parse_display(raw: &str) -> (String, bool, bool) {
    // SPICE socket mode: detected by presence of the Unix socket arg
    if raw.contains("-spice unix,addr=$VM_DIR/spice.sock") {
        return ("spice".to_string(), false, false);
    }
    for line in raw.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("-display ") {
            let rest = &trimmed[9..];
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '\\')
                .unwrap_or(rest.len());
            let parts: Vec<&str> = rest[..end].split(',').collect();
            let backend = parts[0].to_string();
            let has_gl = parts.iter().any(|&p| p == "gl=on");
            let has_fullscreen = parts.iter().any(|&p| p == "full-screen=on");
            if !backend.is_empty() {
                return (backend, has_gl, has_fullscreen);
            }
        }
    }
    ("gtk".to_string(), false, false)
}

fn apply_display(content: &str, backend: &str, gl: bool, fullscreen: bool) -> String {
    if backend == "spice" {
        return apply_spice(content);
    }
    if content.contains("-spice unix,addr=$VM_DIR/spice.sock") {
        return remove_spice_add_display(content, backend, gl, fullscreen);
    }
    let mut opts = vec![backend.to_string()];
    if gl { opts.push("gl=on".to_string()); }
    if fullscreen { opts.push("full-screen=on".to_string()); }
    let new_arg = format!("-display {}", opts.join(","));
    let search = "-display ";
    let mut result = String::with_capacity(content.len() + 32);
    let mut pos = 0;
    while let Some(rel) = content[pos..].find(search) {
        let abs = pos + rel;
        result.push_str(&content[pos..abs]);
        result.push_str(&new_arg);
        let after = abs + search.len();
        let rest = &content[after..];
        let skip = rest
            .find(|c: char| c.is_whitespace() || c == '\\')
            .unwrap_or(rest.len());
        pos = after + skip;
    }
    result.push_str(&content[pos..]);
    result
}

fn apply_spice(content: &str) -> String {
    if content.contains("-spice unix,addr=$VM_DIR/spice.sock") {
        return content.to_string();
    }
    let mut result: Vec<String> = Vec::new();
    for line in content.lines() {
        if line.trim_start().starts_with("-display ") {
            result.push("        -spice unix,addr=$VM_DIR/spice.sock,disable-ticketing \\".to_string());
            result.push("        -device virtio-serial \\".to_string());
            result.push("        -chardev spicevmc,id=vdagent,name=vdagent \\".to_string());
            result.push("        -device virtserialport,chardev=vdagent,name=com.redhat.spice.0 \\".to_string());
        } else {
            result.push(line.to_string());
        }
    }
    let mut out = result.join("\n");
    if content.ends_with('\n') { out.push('\n'); }
    out
}

fn remove_spice_add_display(content: &str, backend: &str, gl: bool, fullscreen: bool) -> String {
    let mut opts = vec![backend.to_string()];
    if gl { opts.push("gl=on".to_string()); }
    if fullscreen { opts.push("full-screen=on".to_string()); }
    let new_display = format!("        -display {} \\", opts.join(","));
    let spice_prefixes = [
        "-spice ",
        "-device virtio-serial",
        "-chardev spicevmc,",
        "-device virtserialport,chardev=vdagent",
    ];
    let mut result: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if spice_prefixes.iter().any(|p| trimmed.starts_with(p)) {
            if !replaced {
                result.push(new_display.clone());
                replaced = true;
            }
        } else {
            result.push(line.to_string());
        }
    }
    let mut out = result.join("\n");
    if content.ends_with('\n') { out.push('\n'); }
    out
}

const BACKENDS: &[&str] = &["gtk", "sdl", "spice-app", "vnc", "none", "spice"];

pub fn show(parent: &impl IsA<gtk4::Widget>, vm: DiscoveredVm) {
    let dialog = libadwaita::Dialog::new();
    dialog.set_title(&format!("Display — {}", vm.display_name()));
    dialog.set_content_width(420);
    dialog.set_content_height(480);

    let (current_backend, current_gl, current_fullscreen) = parse_display(&vm.config.raw_script);

    let display_group = PreferencesGroup::new();
    display_group.set_title("Display Settings");

    let backend_list = StringList::new(BACKENDS);
    let backend_row = ComboRow::new();
    backend_row.set_title("Display Backend");
    backend_row.set_model(Some(&backend_list));
    backend_row.set_selected(
        BACKENDS
            .iter()
            .position(|&b| b == current_backend)
            .unwrap_or(0) as u32,
    );
    display_group.add(&backend_row);

    let gl_row = SwitchRow::new();
    gl_row.set_title("3D Acceleration (GL)");
    gl_row.set_subtitle("Requires virtio-gpu or compatible display backend");
    gl_row.set_active(current_gl);
    display_group.add(&gl_row);

    let fullscreen_row = SwitchRow::new();
    fullscreen_row.set_title("Launch Fullscreen");
    fullscreen_row.set_subtitle("Start QEMU in full-screen mode");
    fullscreen_row.set_active(current_fullscreen);
    display_group.add(&fullscreen_row);

    let is_spice_initial = current_backend == "spice";
    gl_row.set_sensitive(!is_spice_initial);
    fullscreen_row.set_sensitive(!is_spice_initial);

    let spice_note = Label::builder()
        .label("Requires spice-guest-tools installed in the guest and virt-viewer on the host (pacman -S virt-viewer). Fullscreen and resize are handled by the viewer.")
        .wrap(true)
        .halign(gtk4::Align::Start)
        .visible(is_spice_initial)
        .build();
    spice_note.add_css_class("caption");
    spice_note.add_css_class("dim-label");

    let content_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(16)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    content_box.append(&display_group);
    content_box.append(&spice_note);

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

    // Update row sensitivity and SPICE note when backend selection changes
    {
        let gl_row = gl_row.clone();
        let fullscreen_row = fullscreen_row.clone();
        let spice_note = spice_note.clone();

        backend_row.connect_selected_notify(move |row| {
            let is_spice = BACKENDS.get(row.selected() as usize).copied() == Some("spice");
            gl_row.set_sensitive(!is_spice);
            fullscreen_row.set_sensitive(!is_spice);
            spice_note.set_visible(is_spice);
        });
    }

    {
        let dialog_ref = dialog.clone();
        let toast_overlay = toast_overlay.clone();
        let launch_script = vm.launch_script.clone();

        save_btn.connect_clicked(move |_| {
            let backend = BACKENDS
                .get(backend_row.selected() as usize)
                .copied()
                .unwrap_or("gtk")
                .to_string();
            let gl = gl_row.is_active();
            let fullscreen = fullscreen_row.is_active();

            let launch_script = launch_script.clone();
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let result = (|| -> Result<(), String> {
                    let content = std::fs::read_to_string(&launch_script)
                        .map_err(|e| e.to_string())?;
                    let updated = apply_display(&content, &backend, gl, fullscreen);
                    std::fs::write(&launch_script, updated).map_err(|e| e.to_string())?;
                    Ok(())
                })();
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
