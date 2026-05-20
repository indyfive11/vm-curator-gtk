# vm-curator-gtk

A GTK4/libadwaita graphical frontend for [vm-curator](https://github.com/mroboff/vm-curator), a QEMU/KVM virtual machine manager. Provides the same VM lifecycle management as the original TUI — create, import, launch, snapshot, configure — through a native desktop GUI with an embedded live display.

---

## Background

[vm-curator](https://github.com/mroboff/vm-curator) is a terminal-based QEMU/KVM manager with a 5-step creation wizard, automatic VM discovery, snapshot management, USB/PCI passthrough, and shared folders. It is well-suited to power users comfortable in the terminal but lacks a persistent graphical interface for day-to-day use.

This project adds a GTK4/libadwaita frontend that consumes vm-curator as a Rust library. The TUI remains fully functional alongside the GUI — both share the same VM library, launch scripts, and configuration on disk. The GUI adds one capability the TUI cannot easily provide: embedding the QEMU display directly inside the application window using QEMU's D-Bus display backend and the `qemu-display` protocol.

Development has proceeded in three milestones:

- **v0.1 — Core GUI shell**: main window with VM list and detail panel, launch/stop/pause controls, running process detection, toast notifications, app icon.
- **v0.2 — Full feature parity**: create wizard, import wizard, snapshot dialog, all configuration dialogs (network, display, boot, USB passthrough, PCI passthrough, shared folders, notes, raw config editor), single-GPU and multi-GPU passthrough setup, settings, layer-shell overlay for externally-launched VMs.
- **v0.2 (current) — Embedded display**: QEMU display embedded directly in the application window via the D-Bus session bus. Live keyboard and mouse input forwarding. Fullscreen with auto-hiding header.

---

## Features

### Main Window
- Searchable sidebar list of all VMs in the configured library directory
- Detail panel showing CPU cores, RAM, KVM support, disk paths, and per-VM notes
- Launch, Stop, Force Stop, and Pause/Resume controls
- 3-second background poll keeps running state and PID in sync
- Separate launch path for VMs started outside the app (detected automatically)

### VM Creation Wizard (5 steps)
1. OS profile selection with live search (profiles include CPU, memory, disk, and display defaults)
2. Resource configuration — CPU cores, RAM, disk size, KVM, UEFI/TPM
3. Boot media — no ISO, configured install media, or custom ISO via file browser
4. Display backend and network backend selection
5. Review summary before creation

### VM Import Wizard (3 steps)
1. Source selection — libvirt XML, quickemu `.conf` files, or any directory
2. Discovery results with per-VM warnings (unreadable disks, network translation notes)
3. Confirmation with name editing and disk handling choice (symlink / copy / move)

### Embedded VM Display
When you launch a VM from the GUI, the QEMU display opens inside a dedicated application window rather than a separate QEMU window. This uses QEMU's `-display dbus` backend — QEMU registers on the session D-Bus as `org.qemu` and the GUI connects via the `qemu-display` protocol.

- Live scanout and incremental frame updates rendered to a GTK `Picture` widget
- Absolute mouse positioning with correct coordinate mapping for letterboxed/pillarboxed content
- Full keyboard forwarding using the evdev-to-QNUM keymap
- **Fullscreen** via the Fullscreen button or `F11` — header auto-hides after 2 seconds, reappears when the mouse reaches the top edge of the screen

### Overlay Bar (externally-launched VMs)
For VMs not started through the GUI, a floating layer-shell control bar appears at the top of the screen when the VM is detected running. Hover to reveal Pause, Stop, Force Stop, Fullscreen, and Snapshot controls without switching away from the VM window.

### Configuration Dialogs
Each is accessible from the main window's detail panel:

| Dialog | What it configures |
|---|---|
| Network | Backend (user/passt/bridge/none), model, MAC, port forwards |
| Display | Backend (gtk/sdl/spice/vnc/none), 3D acceleration, fullscreen launch |
| Boot | Normal / Install media / Custom ISO |
| Snapshots | Create, restore, delete qcow2 snapshots with timestamps and sizes |
| USB Passthrough | Per-device selection with udev rule installer |
| PCI Passthrough | IOMMU-group-aware device selection, GPU+audio companion auto-detection |
| Shared Folders | VirtIO 9p mounts with auto-generated mount tags |
| Notes | Free-text per-VM notes, synced to main window |
| Raw Config | Direct editor for the QEMU launch script |
| Single GPU | Prerequisites check, script generation for single-GPU passthrough |
| Multi GPU | IOMMU/VFIO status, GPU selector, Looking Glass detection |

### Settings
Global configuration: VM library path, new-VM defaults (memory, CPU, disk, display), KVM default toggle, confirm-before-launch option, passthrough mode (disabled / multi-GPU / single GPU).

---

## Requirements

**Runtime**
- QEMU with KVM support (`qemu-system-x86_64` or equivalent)
- GTK 4.18+
- libadwaita 1.9+
- gtk4-layer-shell 1.3+ (for the overlay bar — Wayland only)
- A session D-Bus (standard on any modern desktop session)

**Build**
- Rust stable toolchain (1.75+)
- Development headers: `gtk4`, `libadwaita`, `gtk4-layer-shell`
- [vm-curator](https://github.com/indyfive11/vm-curator) checked out as a sibling directory (`../vm-curator`)

---

## Building from Source

```
# Clone both repos as siblings
git clone https://github.com/indyfive11/vm-curator.git
git clone https://github.com/indyfive11/vm-curator-gtk.git

cd vm-curator-gtk
cargo build --release
```

The binary is at `target/release/vm-curator-gtk`.

On Arch Linux, build dependencies can be installed with:
```
sudo pacman -S gtk4 libadwaita gtk4-layer-shell qemu-desktop
```

---

## Usage

Point the app at your VM library directory in **Settings** (defaults to `~/VMs` or wherever vm-curator is configured). VMs are discovered automatically on startup and whenever the library path is changed.

**Launching a VM** — select it in the sidebar and click Launch. The QEMU display opens in a new window. Stop/pause controls appear in both the main window and the embedded display window.

**Creating a VM** — click the + button in the header. Work through the 5-step wizard; the final step shows a full summary before writing anything to disk.

**Importing a VM** — click the import button. Choose libvirt, quickemu, or a custom directory. Review any warnings on the discovered VMs (unreadable disk permissions are flagged with the exact `chmod` command needed). Confirm to copy the VM into your library.

**Fullscreen** — inside the embedded display window, press `F11` or click the Fullscreen button. Move the mouse to the very top of the screen to reveal the control header; it hides again automatically after 2 seconds of inactivity.

**Snapshots** — open a VM's detail panel and click Snapshots. Type a name and click Create. Restore and Delete are available per snapshot with confirmation dialogs.

---

## Architecture

```
vm-curator-gtk  (this repo)
    └── depends on vm-curator as a Rust library
            └── src/lib.rs exposes vm/, config/, hardware/, wizard_types/
                The TUI binary (cargo run) continues to work independently.
```

QEMU display embedding uses the [qemu-display](https://gitlab.com/marcandre.lureau/qemu-display) crate. When a VM is launched, the QEMU `-display dbus` flag causes QEMU to register on the session D-Bus as `org.qemu`. The GUI connects via `zbus`, drives the D-Bus executor on the GLib main context, and forwards frames to a GTK `Picture` widget.

---

## License

MIT — same as the upstream [vm-curator](https://github.com/mroboff/vm-curator) project.
