use gtk4::prelude::*;
use gtk4::{
    gdk, glib, Button, EventControllerKey,
    EventControllerMotion, GestureClick, HeaderBar, Label, Picture,
};
use libadwaita::{AlertDialog, ApplicationWindow, ResponseAppearance, ToolbarView};
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_trait::async_trait;
use qemu_display::zbus;
use vm_curator::vm::{
    detect_qemu_processes, force_stop_vm, is_vm_paused, pause_vm, resume_vm,
    stop_vm_by_pid, DiscoveredVm,
};

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum FrameMsg {
    Scanout { data: Vec<u8>, width: u32, height: u32, stride: u32, format: u32 },
    Update { x: i32, y: i32, w: i32, h: i32, data: Vec<u8>, stride: u32 },
}

// ---------------------------------------------------------------------------
// ConsoleListenerHandler
// ---------------------------------------------------------------------------

struct DisplayHandler {
    frame_tx: async_channel::Sender<FrameMsg>,
}

#[async_trait]
impl qemu_display::ConsoleListenerHandler for DisplayHandler {
    async fn scanout(&mut self, s: qemu_display::Scanout) {
        let _ = self.frame_tx.try_send(FrameMsg::Scanout {
            data: s.data,
            width: s.width,
            height: s.height,
            stride: s.stride,
            format: s.format,
        });
    }

    async fn update(&mut self, u: qemu_display::Update) {
        let _ = self.frame_tx.try_send(FrameMsg::Update {
            x: u.x,
            y: u.y,
            w: u.w,
            h: u.h,
            data: u.data,
            stride: u.stride,
        });
    }

    #[cfg(unix)]
    async fn scanout_dmabuf(&mut self, _: qemu_display::ScanoutDMABUF) {}
    #[cfg(unix)]
    async fn update_dmabuf(&mut self, _: qemu_display::UpdateDMABUF) {}
    async fn disable(&mut self) {}
    async fn mouse_set(&mut self, _: qemu_display::MouseSet) {}
    async fn cursor_define(&mut self, _: qemu_display::Cursor) {}
    fn disconnected(&mut self) {}
    fn interfaces(&self) -> Vec<String> { vec![] }
}

// ---------------------------------------------------------------------------
// Frame buffer helpers
// ---------------------------------------------------------------------------

struct FrameState {
    data: Vec<u8>,
    width: u32,
    height: u32,
    stride: u32,
    format: u32,
}

fn gdk_mem_format(pixman: u32) -> gdk::MemoryFormat {
    match pixman {
        0x20024 => gdk::MemoryFormat::B8g8r8x8, // PIXMAN_x8r8g8b8
        0x20208 => gdk::MemoryFormat::B8g8r8a8, // PIXMAN_a8r8g8b8
        _ => gdk::MemoryFormat::B8g8r8x8,
    }
}

fn apply_update(fs: &mut FrameState, x: i32, y: i32, w: i32, h: i32, data: &[u8], ustride: u32) {
    let bpp: usize = 4;
    for row in 0..h as usize {
        let src = row * ustride as usize;
        let dst = (y as usize + row) * fs.stride as usize + x as usize * bpp;
        let len = w as usize * bpp;
        if src + len <= data.len() && dst + len <= fs.data.len() {
            fs.data[dst..dst + len].copy_from_slice(&data[src..src + len]);
        }
    }
}

fn render_frame(picture: &Picture, fs: &FrameState) {
    let bytes = glib::Bytes::from(&fs.data[..]);
    let tex = gdk::MemoryTexture::new(
        fs.width as i32,
        fs.height as i32,
        gdk_mem_format(fs.format),
        &bytes,
        fs.stride as usize,
    );
    picture.set_paintable(Some(tex.upcast_ref::<gdk::Paintable>()));
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn show(app: &libadwaita::Application, vm: DiscoveredVm) {
    let vm_name = vm.display_name();
    let vm_path = vm.path.clone();
    let disk_path = vm.config.disks.first().map(|d| d.path.clone());
    drop(vm);

    // Resolved to the actual QEMU process PID once it appears in /proc
    let qemu_pid: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));

    // --- Window ---
    let window = ApplicationWindow::builder()
        .application(app)
        .title(&vm_name)
        .default_width(1024)
        .default_height(768)
        .build();

    // --- Header bar ---
    let header = HeaderBar::new();
    let name_label = Label::new(Some(&vm_name));
    name_label.add_css_class("heading");
    header.set_title_widget(Some(&name_label));

    let pause_btn = Button::with_label("Pause");
    let stop_btn = Button::with_label("Stop");
    let force_stop_btn = Button::with_label("Force Stop");
    force_stop_btn.add_css_class("destructive-action");
    let fs_btn = Button::with_label("Fullscreen");

    header.pack_end(&fs_btn);
    header.pack_end(&force_stop_btn);
    header.pack_end(&stop_btn);
    header.pack_end(&pause_btn);

    if let Some(ref dp) = disk_path {
        let snapshot_btn = Button::with_label("Snapshot");
        header.pack_end(&snapshot_btn);
        let dp = dp.clone();
        let vn = vm_name.clone();
        let window_weak = window.downgrade();
        snapshot_btn.connect_clicked(move |_| {
            if let Some(w) = window_weak.upgrade() {
                crate::snapshot::show(&w, &vn, dp.clone());
            }
        });
    }

    // --- Display area ---
    let picture = Picture::new();
    picture.set_content_fit(gtk4::ContentFit::Contain);
    picture.set_can_shrink(true);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_focusable(true);
    picture.set_css_classes(&["vm-display"]);

    // Black background
    let css = gtk4::CssProvider::new();
    css.load_from_string(".vm-display { background-color: black; }");
    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display, &css, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    // AdwApplicationWindow requires ToolbarView — set_titlebar() causes ABRT
    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&picture));
    window.set_content(Some(&toolbar_view));

    // --- Frame buffer state (main thread) ---
    let frame_state: Rc<RefCell<Option<FrameState>>> = Rc::new(RefCell::new(None));

    // --- async channel: D-Bus → main thread (frames) ---
    let (frame_tx, frame_rx) = async_channel::bounded::<FrameMsg>(128);
    {
        let frame_state = Rc::clone(&frame_state);
        let picture = picture.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(msg) = frame_rx.recv().await {
                match msg {
                    FrameMsg::Scanout { data, width, height, stride, format } => {
                        let fs = FrameState { data, width, height, stride, format };
                        render_frame(&picture, &fs);
                        *frame_state.borrow_mut() = Some(fs);
                    }
                    FrameMsg::Update { x, y, w, h, data, stride } => {
                        let mut borrow = frame_state.borrow_mut();
                        if let Some(ref mut fs) = *borrow {
                            apply_update(fs, x, y, w, h, &data, stride);
                            render_frame(&picture, fs);
                        }
                    }
                }
            }
        });
    }

    // --- Console holder: shared between D-Bus setup and GTK input callbacks ---
    let console_cell: Rc<RefCell<Option<qemu_display::Console>>> =
        Rc::new(RefCell::new(None));

    // --- Session bus D-Bus setup (runs on GTK main context) ---
    {
        let console_cell = Rc::clone(&console_cell);
        glib::MainContext::default().spawn_local(async move {
            let builder = match zbus::connection::Builder::session() {
                Ok(b) => b.internal_executor(false),
                Err(e) => {
                    log::error!("vm_window: session bus builder failed: {e}");
                    return;
                }
            };
            let conn = match builder.build().await {
                Ok(c) => c,
                Err(e) => {
                    log::error!("vm_window: session bus connect failed: {e}");
                    return;
                }
            };

            // Drive the D-Bus executor on the GTK main context (required by qemu-display)
            let conn_tick = conn.clone();
            glib::MainContext::default().spawn_local(async move {
                loop { conn_tick.executor().tick().await; }
            });

            // Wait for QEMU to register as org.qemu on the session bus
            let console = {
                let mut found = None;
                for attempt in 0..30 {
                    match qemu_display::Console::new(&conn, 0).await {
                        Ok(c) => {
                            log::info!(
                                "vm_window: found QEMU on session bus (attempt {})",
                                attempt + 1
                            );
                            found = Some(c);
                            break;
                        }
                        Err(e) => {
                            log::debug!("vm_window: console attempt {attempt}: {e}");
                            async_std::task::sleep(
                                std::time::Duration::from_millis(500)
                            ).await;
                        }
                    }
                }
                match found {
                    Some(c) => c,
                    None => {
                        log::error!(
                            "vm_window: QEMU did not appear on session bus after 15s"
                        );
                        return;
                    }
                }
            };

            if let Err(e) = console.register_listener(DisplayHandler { frame_tx }).await {
                log::error!("vm_window: register_listener failed: {e}");
                return;
            }
            log::info!("vm_window: display listener registered, waiting for frames");

            // Keep console alive for input callbacks
            *console_cell.borrow_mut() = Some(console);
        });
    }

    // --- Keyboard map ---
    let keymap: Option<&'static [u16]> = match gdk::Display::default() {
        Some(d) => match d.backend() {
            gdk::Backend::Wayland | gdk::Backend::X11 => Some(keycodemap::KEYMAP_XORGEVDEV2QNUM),
            _ => None,
        },
        None => Some(keycodemap::KEYMAP_XORGEVDEV2QNUM),
    };

    // --- Keyboard events ---
    let key_ctrl = EventControllerKey::new();
    {
        let console_cell = Rc::clone(&console_cell);
        key_ctrl.connect_key_pressed(move |_, _keyval, keycode, _state| {
            if let Some(kb) = console_cell.borrow().as_ref().map(|c| c.keyboard.clone()) {
                if let Some(map) = keymap {
                    if let Some(&qnum) = map.get(keycode as usize) {
                        if qnum != 0 {
                            glib::MainContext::default().spawn_local(async move {
                                let _ = kb.press(qnum as u32).await;
                            });
                        }
                    }
                }
            }
            glib::Propagation::Stop
        });
    }
    {
        let console_cell = Rc::clone(&console_cell);
        key_ctrl.connect_key_released(move |_, _keyval, keycode, _state| {
            if let Some(kb) = console_cell.borrow().as_ref().map(|c| c.keyboard.clone()) {
                if let Some(map) = keymap {
                    if let Some(&qnum) = map.get(keycode as usize) {
                        if qnum != 0 {
                            glib::MainContext::default().spawn_local(async move {
                                let _ = kb.release(qnum as u32).await;
                            });
                        }
                    }
                }
            }
        });
    }
    picture.add_controller(key_ctrl);

    // --- Mouse motion (absolute) ---
    {
        let console_cell = Rc::clone(&console_cell);
        let frame_state = Rc::clone(&frame_state);
        let motion = EventControllerMotion::new();
        let picture_motion = picture.clone();
        motion.connect_motion(move |_, x, y| {
            let mouse = match console_cell.borrow().as_ref().map(|c| c.mouse.clone()) {
                Some(m) => m,
                None => return,
            };
            let (fw, fh) = frame_state.borrow().as_ref()
                .map(|f| (f.width, f.height))
                .unwrap_or((1024, 768));
            let pw = picture_motion.width().max(1) as f64;
            let ph = picture_motion.height().max(1) as f64;
            // ContentFit::Contain scales uniformly and centers — compute the
            // actual rendered rect so coordinates map to the content, not the black bars.
            let scale = (pw / fw as f64).min(ph / fh as f64);
            let rendered_w = fw as f64 * scale;
            let rendered_h = fh as f64 * scale;
            let x_off = (pw - rendered_w) / 2.0;
            let y_off = (ph - rendered_h) / 2.0;
            let gx = ((x - x_off) / rendered_w * fw as f64)
                .clamp(0.0, fw as f64 - 1.0) as u32;
            let gy = ((y - y_off) / rendered_h * fh as f64)
                .clamp(0.0, fh as f64 - 1.0) as u32;
            glib::MainContext::default().spawn_local(async move {
                let _ = mouse.set_abs_position(gx, gy).await;
            });
        });
        picture.add_controller(motion);
    }

    // --- Mouse buttons ---
    {
        let click = GestureClick::new();
        click.set_button(0); // any button
        click.connect_pressed({
            let console_cell = Rc::clone(&console_cell);
            move |gesture, _, _, _| {
                let mouse = match console_cell.borrow().as_ref().map(|c| c.mouse.clone()) {
                    Some(m) => m,
                    None => return,
                };
                let btn = match gesture.current_button() {
                    1 => Some(qemu_display::MouseButton::Left),
                    2 => Some(qemu_display::MouseButton::Middle),
                    3 => Some(qemu_display::MouseButton::Right),
                    _ => None,
                };
                if let Some(b) = btn {
                    glib::MainContext::default().spawn_local(async move {
                        let _ = mouse.press(b).await;
                    });
                }
            }
        });
        click.connect_released({
            let console_cell = Rc::clone(&console_cell);
            move |gesture, _, _, _| {
                let mouse = match console_cell.borrow().as_ref().map(|c| c.mouse.clone()) {
                    Some(m) => m,
                    None => return,
                };
                let btn = match gesture.current_button() {
                    1 => Some(qemu_display::MouseButton::Left),
                    2 => Some(qemu_display::MouseButton::Middle),
                    3 => Some(qemu_display::MouseButton::Right),
                    _ => None,
                };
                if let Some(b) = btn {
                    glib::MainContext::default().spawn_local(async move {
                        let _ = mouse.release(b).await;
                    });
                }
            }
        });
        picture.add_controller(click);
    }

    // --- Fullscreen hover-reveal state ---
    let is_fs: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let bars_visible: Rc<Cell<bool>> = Rc::new(Cell::new(true));
    let pending_hide: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

    // --- Fullscreen button + F11 ---
    {
        let win = window.downgrade();
        let tv = toolbar_view.clone();
        let is_fs = Rc::clone(&is_fs);
        let bars_visible = Rc::clone(&bars_visible);
        let pending_hide = Rc::clone(&pending_hide);
        fs_btn.connect_clicked(move |btn| {
            let Some(w) = win.upgrade() else { return };
            if w.is_fullscreen() {
                w.unfullscreen();
                btn.set_label("Fullscreen");
                is_fs.set(false);
                if let Some(id) = pending_hide.borrow_mut().take() { id.remove(); }
                tv.set_reveal_top_bars(true);
                bars_visible.set(true);
            } else {
                w.fullscreen();
                btn.set_label("Exit Fullscreen");
                is_fs.set(true);
                // Auto-hide after 2s; motion handler cancels this if mouse stays at top
                let tv2 = tv.clone();
                let bv = Rc::clone(&bars_visible);
                let ph = Rc::clone(&pending_hide);
                let id = glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
                    tv2.set_reveal_top_bars(false);
                    bv.set(false);
                    ph.borrow_mut().take();
                    glib::ControlFlow::Break
                });
                *pending_hide.borrow_mut() = Some(id);
            }
        });
    }
    {
        let win = window.downgrade();
        let fs_btn_w = fs_btn.downgrade();
        let tv = toolbar_view.clone();
        let is_fs = Rc::clone(&is_fs);
        let bars_visible = Rc::clone(&bars_visible);
        let pending_hide = Rc::clone(&pending_hide);
        let key_fs = EventControllerKey::new();
        key_fs.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::F11 {
                let Some(w) = win.upgrade() else { return glib::Propagation::Stop };
                if w.is_fullscreen() {
                    w.unfullscreen();
                    fs_btn_w.upgrade().map(|b| b.set_label("Fullscreen"));
                    is_fs.set(false);
                    if let Some(id) = pending_hide.borrow_mut().take() { id.remove(); }
                    tv.set_reveal_top_bars(true);
                    bars_visible.set(true);
                } else {
                    w.fullscreen();
                    fs_btn_w.upgrade().map(|b| b.set_label("Exit Fullscreen"));
                    is_fs.set(true);
                    let tv2 = tv.clone();
                    let bv = Rc::clone(&bars_visible);
                    let ph = Rc::clone(&pending_hide);
                    let id = glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
                        tv2.set_reveal_top_bars(false);
                        bv.set(false);
                        ph.borrow_mut().take();
                        glib::ControlFlow::Break
                    });
                    *pending_hide.borrow_mut() = Some(id);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(key_fs);
    }

    // --- Hover-to-reveal header in fullscreen ---
    {
        let tv = toolbar_view.clone();
        let is_fs = Rc::clone(&is_fs);
        let bars_visible = Rc::clone(&bars_visible);
        let pending_hide = Rc::clone(&pending_hide);
        let fs_hover = EventControllerMotion::new();
        fs_hover.connect_motion(move |_, _x, y| {
            if !is_fs.get() { return; }
            if y < 10.0 {
                // Near top edge: reveal and cancel any pending hide
                if let Some(id) = pending_hide.borrow_mut().take() { id.remove(); }
                if !bars_visible.get() {
                    tv.set_reveal_top_bars(true);
                    bars_visible.set(true);
                }
            } else if y > 60.0 && bars_visible.get() && pending_hide.borrow().is_none() {
                // Below header area: schedule auto-hide after 2s
                let tv2 = tv.clone();
                let bv = Rc::clone(&bars_visible);
                let ph = Rc::clone(&pending_hide);
                let id = glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
                    tv2.set_reveal_top_bars(false);
                    bv.set(false);
                    ph.borrow_mut().take();
                    glib::ControlFlow::Break
                });
                *pending_hide.borrow_mut() = Some(id);
            }
        });
        window.add_controller(fs_hover);
    }

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
        let qemu_pid_stop = Rc::clone(&qemu_pid);
        stop_btn.connect_clicked(move |_| {
            let pid = match qemu_pid_stop.get() { Some(p) => p, None => return };
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
        let qemu_pid_force = Rc::clone(&qemu_pid);
        force_stop_btn.connect_clicked(move |_| {
            let pid = match qemu_pid_force.get() { Some(p) => p, None => return };
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

    // --- 2s poll: find QEMU by cwd, update PID, close when VM exits ---
    let window_weak = window.downgrade();
    let pause_btn_weak = pause_btn.downgrade();
    glib::timeout_add_seconds_local(2, move || {
        let procs = detect_qemu_processes();
        match procs.iter().find(|p| p.cwd.as_deref() == Some(vm_path.as_path())) {
            None => {
                if let Some(w) = window_weak.upgrade() { w.close(); }
                return glib::ControlFlow::Break;
            }
            Some(p) => qemu_pid.set(Some(p.pid)),
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
