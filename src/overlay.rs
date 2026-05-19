use gtk4::prelude::*;
use gtk4::{
    glib, Align, Box as GtkBox, Button, Label, Orientation, Revealer,
    RevealerTransitionType, Separator,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use libadwaita::{AlertDialog, ResponseAppearance};
use libadwaita::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use vm_curator::vm::{
    detect_qemu_processes, force_stop_vm, is_vm_paused, pause_vm, resume_vm,
    stop_vm_by_pid, DiscoveredVm,
};

const OVERLAY_CSS: &str = "
.vm-overlay-trigger {
    /* Invisible hover zone — no background, just occupies space at top of screen */
    background-color: transparent;
    min-height: 10px;
}
.vm-overlay-bar {
    background-color: alpha(@window_bg_color, 0.92);
    border-radius: 0 0 10px 10px;
    border: 1px solid alpha(@borders, 0.6);
}
.vm-overlay-name {
    font-weight: bold;
}
";

fn toggle_fullscreen_by_pid(pid: u32) {
    // Step 1: use KWin scripting to give QEMU keyboard focus. QEMU's GTK display
    // manages its own fullscreen state and ignores compositor-driven fullscreen,
    // so we can't set window.fullScreen directly. Instead we activate the window
    // first so wtype's subsequent key injection lands on QEMU.
    let script = format!(
        "var ws = workspace.windowList(); \
         for (var i = 0; i < ws.length; i++) {{ \
             if (ws[i].pid === {pid}) {{ \
                 workspace.activeWindow = ws[i]; break; \
             }} \
         }}"
    );
    let path = format!("/tmp/.vm-curator-fs-{pid}.js");
    if std::fs::write(&path, &script).is_ok() {
        let id_out = std::process::Command::new("qdbus6")
            .args(["org.kde.KWin", "/Scripting",
                   "org.kde.kwin.Scripting.loadScript", &path])
            .output();
        let _ = std::fs::remove_file(&path);
        if let Ok(out) = id_out {
            let id_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !id_str.is_empty() {
                let script_path = format!("/Scripting/Script{id_str}");
                let _ = std::process::Command::new("qdbus6")
                    .args(["org.kde.KWin", &script_path, "org.kde.kwin.Script.run"])
                    .status();
                // Brief pause for the compositor to process the focus change.
                std::thread::sleep(std::time::Duration::from_millis(80));
            }
        }
    }

    // Step 2: send Ctrl+Alt+F to the now-focused QEMU window.
    let status = std::process::Command::new("wtype")
        .args(["-M", "ctrl", "-M", "alt", "-k", "f"])
        .status();
    if status.map(|s| !s.success()).unwrap_or(true) {
        log::warn!("wtype failed; install with: sudo pacman -S wtype");
    }
}

pub fn show(vm: DiscoveredVm, pid: u32) {
    // Extract everything needed before consuming vm
    let vm_name = vm.display_name();
    let vm_path = vm.path.clone();
    let disk_path = vm.config.disks.first().map(|d| d.path.clone());
    drop(vm);

    // CSS — loaded fresh per overlay; GObject keeps the provider alive in the
    // display's style cascade after this function returns.
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(OVERLAY_CSS);
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    // --- Window ---
    let window = gtk4::Window::new();
    window.set_decorated(false);
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, false);
    window.set_anchor(Edge::Right, false);
    window.set_anchor(Edge::Bottom, false);
    window.set_exclusive_zone(0);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_namespace(Some("vm-curator-overlay"));

    // --- Trigger strip (invisible 10 px hover zone at top of screen) ---
    let trigger = GtkBox::new(Orientation::Horizontal, 0);
    trigger.set_height_request(10);
    trigger.add_css_class("vm-overlay-trigger");

    // --- Control bar ---
    let bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::Center)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(16)
        .margin_end(16)
        .build();
    bar.add_css_class("vm-overlay-bar");

    let name_label = Label::new(Some(&vm_name));
    name_label.add_css_class("vm-overlay-name");
    bar.append(&name_label);
    bar.append(&Separator::new(Orientation::Vertical));

    let pause_btn = Button::with_label("Pause");
    let stop_btn = Button::with_label("Stop");
    let force_stop_btn = Button::with_label("Force Stop");
    force_stop_btn.add_css_class("destructive-action");
    bar.append(&pause_btn);
    bar.append(&stop_btn);
    bar.append(&force_stop_btn);

    let fs_btn = Button::with_label("Fullscreen");
    bar.append(&fs_btn);
    fs_btn.connect_clicked(move |_| {
        std::thread::spawn(move || toggle_fullscreen_by_pid(pid));
    });

    if let Some(ref dp) = disk_path {
        let snapshot_btn = Button::with_label("Snapshot");
        bar.append(&snapshot_btn);
        let dp = dp.clone();
        let vn = vm_name.clone();
        let window_weak = window.downgrade();
        snapshot_btn.connect_clicked(move |_| {
            if let Some(w) = window_weak.upgrade() {
                crate::snapshot::show(&w, &vn, dp.clone());
            }
        });
    }

    // --- Revealer wrapping the bar ---
    let revealer = Revealer::builder()
        .transition_type(RevealerTransitionType::SlideDown)
        .transition_duration(150)
        .reveal_child(false)
        .child(&bar)
        .build();

    let root = GtkBox::new(Orientation::Vertical, 0);
    root.set_size_request(520, -1);
    root.append(&trigger);
    root.append(&revealer);
    window.set_child(Some(&root));

    // --- Hover: expand on enter, collapse after 600 ms leave ---
    // Use a cancel flag rather than SourceId::remove(), which panics if the
    // one-shot timer already auto-removed itself before we call remove().
    let should_collapse: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let motion = gtk4::EventControllerMotion::new();
    {
        let revealer = revealer.clone();
        let should_collapse = Rc::clone(&should_collapse);
        motion.connect_enter(move |_, _, _| {
            should_collapse.set(false);
            revealer.set_reveal_child(true);
        });
    }
    {
        let revealer = revealer.clone();
        let should_collapse = Rc::clone(&should_collapse);
        motion.connect_leave(move |_| {
            should_collapse.set(true);
            let revealer = revealer.clone();
            let flag = Rc::clone(&should_collapse);
            glib::timeout_add_local(Duration::from_millis(600), move || {
                if flag.get() {
                    revealer.set_reveal_child(false);
                }
                glib::ControlFlow::Break
            });
        });
    }
    window.add_controller(motion);

    // --- Pause / Resume ---
    let paused_state: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    {
        let vm_path = vm_path.clone();
        let paused_state = Rc::clone(&paused_state);
        pause_btn.connect_clicked(move |_| {
            let paused = paused_state.get();
            let vp = vm_path.clone();
            std::thread::spawn(move || {
                let result = if paused { resume_vm(&vp) } else { pause_vm(&vp) };
                if let Err(e) = result {
                    log::warn!("{} failed: {e}", if paused { "resume_vm" } else { "pause_vm" });
                }
            });
        });
    }

    // --- Stop ---
    {
        let vm_name = vm_name.clone();
        let window_weak = window.downgrade();
        stop_btn.connect_clicked(move |_| {
            let alert = AlertDialog::builder()
                .heading("Stop VM?")
                .body(&format!(
                    "Send shutdown signal to \"{vm_name}\"? Unsaved guest work may be lost."
                ))
                .build();
            alert.add_response("cancel", "Cancel");
            alert.add_response("stop", "Stop");
            alert.set_response_appearance("stop", ResponseAppearance::Suggested);
            alert.set_default_response(Some("cancel"));
            alert.set_close_response("cancel");
            alert.connect_response(None, move |_, response| {
                if response == "stop" {
                    if let Err(e) = stop_vm_by_pid(pid) {
                        log::warn!("stop_vm_by_pid({pid}) failed: {e}");
                    }
                }
            });
            if let Some(w) = window_weak.upgrade() {
                alert.present(Some(&w));
            }
        });
    }

    // --- Force Stop ---
    {
        let vm_name = vm_name.clone();
        let window_weak = window.downgrade();
        force_stop_btn.connect_clicked(move |_| {
            let alert = AlertDialog::builder()
                .heading("Force Stop VM?")
                .body(&format!(
                    "Force kill \"{vm_name}\"? The guest OS will not shut down cleanly."
                ))
                .build();
            alert.add_response("cancel", "Cancel");
            alert.add_response("force", "Force Stop");
            alert.set_response_appearance("force", ResponseAppearance::Destructive);
            alert.set_default_response(Some("cancel"));
            alert.set_close_response("cancel");
            alert.connect_response(None, move |_, response| {
                if response == "force" {
                    if let Err(e) = force_stop_vm(pid) {
                        log::warn!("force_stop_vm({pid}) failed: {e}");
                    }
                }
            });
            if let Some(w) = window_weak.upgrade() {
                alert.present(Some(&w));
            }
        });
    }

    // --- 2 s self-managing poll: update pause label; close when VM exits ---
    let window_weak = window.downgrade();
    let pause_btn_weak = pause_btn.downgrade();
    glib::timeout_add_seconds_local(2, move || {
        let processes = detect_qemu_processes();
        if !processes.iter().any(|p| p.pid == pid) {
            if let Some(w) = window_weak.upgrade() {
                w.close();
            }
            return glib::ControlFlow::Break;
        }

        let paused = is_vm_paused(&vm_path);
        paused_state.set(paused);
        if let Some(btn) = pause_btn_weak.upgrade() {
            btn.set_label(if paused { "Resume" } else { "Pause" });
        }

        glib::ControlFlow::Continue
    });

    window.present();
}
