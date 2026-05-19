mod boot_dialog;
mod config_editor;
mod create_wizard;
mod display_dialog;
mod folders_dialog;
mod import_wizard;
mod network_dialog;
mod notes_dialog;
mod multi_gpu;
mod pci_dialog;
mod settings;
mod single_gpu;
mod overlay;
mod snapshot;
mod vm_window;
mod usb_dialog;
mod window;

use libadwaita::prelude::*;
use log::{LevelFilter, Log, Metadata, Record, SetLoggerError};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::Mutex;

struct DualLogger {
    file: Mutex<BufWriter<File>>,
    level: LevelFilter,
}

impl Log for DualLogger {
    fn enabled(&self, meta: &Metadata) -> bool {
        meta.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // Skip noisy GTK/GLib internal log spam
        let target = record.target();
        if target.starts_with("gtk4") || target.starts_with("glib") || target.starts_with("gio") {
            return;
        }
        let msg = format!("[{:<5} {}] {}\n", record.level(), record.target(), record.args());
        eprint!("{}", msg);
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(msg.as_bytes());
            let _ = f.flush();
        }
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

fn init_logger() -> Result<(), SetLoggerError> {
    let log_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("vm-curator-gtk");

    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("vm-curator-gtk.log");

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .unwrap_or_else(|_| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/vm-curator-gtk.log")
                .expect("failed to open fallback log file")
        });

    let level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(LevelFilter::Info);

    log::set_boxed_logger(Box::new(DualLogger {
        file: Mutex::new(BufWriter::new(file)),
        level,
    }))?;
    log::set_max_level(level);

    log::info!("vm-curator-gtk starting — log: {}", log_dir.join("vm-curator-gtk.log").display());
    Ok(())
}

fn main() -> gtk4::glib::ExitCode {
    let _ = init_logger();

    let app = libadwaita::Application::builder()
        .application_id("com.github.indyfive11.vm-curator-gtk")
        .build();
    app.connect_activate(|app| window::build_and_show(app));
    app.run()
}
