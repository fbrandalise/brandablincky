//! Global push-to-talk via raw evdev, for Linux desktops with no compositor
//! grab available (GNOME/Mutter in particular). The `crossplatform` backend's
//! X11 hotkey grab only receives events while an XWayland client holds
//! Wayland-level focus, since Mutter routes physical input by Wayland focus
//! and only forwards it into the XWayland/X11 subsystem when that focus is
//! an XWayland surface. Reading `/dev/input` directly bypasses the
//! compositor, so the hotkey fires no matter what has focus.
//!
//! Requires the running user to be in the `input` group (same requirement as
//! ydotool's uinput access; see `init`'s error message on failure).

use evdev::{Device, EventSummary, KeyCode};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use super::backend::HotkeyBackend;

static RECORDING: AtomicBool = AtomicBool::new(false);
static ON_PRESS: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static ON_RELEASE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// The push-to-talk key. Matches the Hyprland config and the crossplatform
/// backend's non-macOS default (plain Insert).
const HOTKEY: KeyCode = KeyCode::KEY_INSERT;

/// Evdev signal backend. Zero-sized; never instantiated.
pub struct Backend;

impl HotkeyBackend for Backend {
    /// Spawns one listener thread per matching keyboard device. Multiple
    /// devices (e.g. a laptop keyboard plus a USB one) all feed the same
    /// `RECORDING` flag, so the hotkey works from whichever is pressed.
    fn init() -> std::io::Result<()> {
        let devices = matching_devices();
        if devices.is_empty() {
            return Err(std::io::Error::other(
                "no keyboard device exposes KEY_INSERT under /dev/input (or none \
                 readable). Add your user to the 'input' group and log back in: \
                 sudo usermod -aG input $USER",
            ));
        }
        for (path, device) in devices {
            eprintln!(
                "[hotkey] watching {:?} ({})",
                path,
                device.name().unwrap_or("unnamed device")
            );
            spawn_listener(device);
        }
        Ok(())
    }

    fn is_recording() -> bool {
        RECORDING.load(Ordering::Relaxed)
    }

    /// 1ms poll keeps latency well below human-perceptual without burning a
    /// measurable CPU slice.
    fn wait_for_press() {
        while !Self::is_recording() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Registers a callback fired immediately after a press event. Call
    /// before `init()`. At most one per process; later registrations are
    /// ignored.
    fn on_press(f: Box<dyn Fn() + Send + Sync + 'static>) {
        let _ = ON_PRESS.set(f);
    }

    /// Registers a callback fired immediately after a release event. See
    /// [`on_press`](Self::on_press).
    fn on_release(f: Box<dyn Fn() + Send + Sync + 'static>) {
        let _ = ON_RELEASE.set(f);
    }
}

/// Devices that can report the hotkey and look like a real keyboard. Excludes
/// ydotoold's own virtual uinput device, which advertises the full key range
/// (so it can synthesize any key) but only ever emits the keys peeky itself
/// asked it to inject, not real user input.
fn matching_devices() -> Vec<(PathBuf, Device)> {
    evdev::enumerate()
        .filter(|(_, device)| {
            let is_synthetic = device
                .name()
                .is_some_and(|n| n.to_lowercase().contains("ydotool"));
            !is_synthetic
                && device
                    .supported_keys()
                    .is_some_and(|keys| keys.contains(HOTKEY))
        })
        .collect()
}

/// Reads one device's event queue forever, translating Insert press/release
/// into `RECORDING` transitions. `fetch_events` blocks until events are
/// available, so this parks the thread rather than spinning.
fn spawn_listener(mut device: Device) {
    thread::spawn(move || {
        loop {
            let events = match device.fetch_events() {
                Ok(events) => events,
                Err(e) => {
                    eprintln!("[hotkey] evdev read failed, stopping listener: {e}");
                    return;
                }
            };
            for event in events {
                let EventSummary::Key(_, code, value) = event.destructure() else {
                    continue;
                };
                if code != HOTKEY {
                    continue;
                }
                match value {
                    1 => {
                        eprintln!("[hotkey] press");
                        RECORDING.store(true, Ordering::Relaxed);
                        if let Some(f) = ON_PRESS.get() {
                            f();
                        }
                    }
                    0 => {
                        eprintln!("[hotkey] release");
                        RECORDING.store(false, Ordering::Relaxed);
                        if let Some(f) = ON_RELEASE.get() {
                            f();
                        }
                    }
                    _ => {}
                }
            }
        }
    });
}
