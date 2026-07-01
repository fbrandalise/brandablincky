//! Raw evdev mouse-position backend for GNOME/Mutter, where the X11 path
//! (`crossplatform.rs`, `XQueryPointer` under the hood) only reflects real
//! cursor motion while the pointer is over an XWayland-backed window — the
//! same limitation that already forced `hotkey/evdev.rs` off X11 for the
//! push-to-talk key. Integrates raw `REL_X`/`REL_Y` deltas into a running
//! absolute position, seeded from an X11 query and periodically nudged back
//! toward one to bound drift.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use evdev::{Device, EventSummary, RelativeAxisCode};

use super::backend::MousePositionBackend;
use super::crossplatform;
use crate::tuning::{MOUSE_EVDEV_RESYNC_BLEND, MOUSE_EVDEV_RESYNC_MS};

/// Evdev mouse-position backend. Zero-sized; never instantiated.
pub struct Backend;

/// Running absolute position, updated by every listener thread and read by
/// `mouse_movement()`. Seeded and clamped by `init()`.
static POSITION: Mutex<(f64, f64)> = Mutex::new((0.0, 0.0));
/// Active screen bounds, used to clamp `POSITION` so integrated drift can't
/// wander fully off-screen. Set once by `init()`.
static BOUNDS: OnceLock<(i32, i32, i32, i32)> = OnceLock::new();
static INIT: OnceLock<()> = OnceLock::new();

impl MousePositionBackend for Backend {
    fn mouse_movement() -> Result<(i64, i64), Box<dyn std::error::Error + Send + Sync>> {
        INIT.get_or_init(init);
        let (x, y) = *POSITION
            .lock()
            .map_err(|_| "evdev mouse position lock poisoned".to_string())?;
        Ok((x.round() as i64, y.round() as i64))
    }

    /// Ydotool clicks compute their own relative delta from this backend's
    /// tracked position (see `input/hyprland.rs::exec_click`), then fire it
    /// through ydotoold's virtual device — which `matching_devices` below
    /// deliberately excludes from the devices this backend listens to. Left
    /// unreported, every synthetic click would drift the tracked position
    /// further from reality with each one.
    fn report_synthetic_move(x: i64, y: i64) {
        let (bx, by, bw, bh) = BOUNDS.get().copied().unwrap_or((0, 0, 1920, 1080));
        if let Ok(mut pos) = POSITION.lock() {
            pos.0 = (x as f64).clamp(bx as f64, (bx + bw) as f64);
            pos.1 = (y as f64).clamp(by as f64, (by + bh) as f64);
        }
    }
}

/// Seeds `POSITION` from an X11 query (falling back to screen center),
/// starts one listener thread per matching device, and starts the periodic
/// drift-correcting resync thread. Runs once, lazily, on first call.
fn init() {
    let bounds = crate::screenshot::active_workspace_geometry()
        .map(|(x, y, w, h)| (x, y, w as i32, h as i32))
        .unwrap_or((0, 0, 1920, 1080));
    let _ = BOUNDS.set(bounds);

    let seed = crossplatform::Backend::mouse_movement()
        .map(|(x, y)| (x as f64, y as f64))
        .unwrap_or((
            bounds.0 as f64 + bounds.2 as f64 / 2.0,
            bounds.1 as f64 + bounds.3 as f64 / 2.0,
        ));
    if let Ok(mut pos) = POSITION.lock() {
        *pos = seed;
    }

    let devices = matching_devices();
    if devices.is_empty() {
        eprintln!(
            "[mouse_position] no device exposes REL_X/REL_Y under /dev/input (or none \
             readable); position will stay fixed at the seeded value. Add your user to \
             the 'input' group and log back in: sudo usermod -aG input $USER"
        );
    }
    for (path, device) in devices {
        eprintln!(
            "[mouse_position] watching {:?} ({})",
            path,
            device.name().unwrap_or("unnamed device")
        );
        spawn_listener(device);
    }

    spawn_resync_thread();
}

/// Devices that report relative motion and look like a real mouse. Excludes
/// ydotoold's own virtual uinput device, same as `hotkey/evdev.rs`'s filter.
fn matching_devices() -> Vec<(PathBuf, Device)> {
    evdev::enumerate()
        .filter(|(_, device)| {
            let is_synthetic = device
                .name()
                .is_some_and(|n| n.to_lowercase().contains("ydotool"));
            !is_synthetic
                && device.supported_relative_axes().is_some_and(|axes| {
                    axes.contains(RelativeAxisCode::REL_X) && axes.contains(RelativeAxisCode::REL_Y)
                })
        })
        .collect()
}

/// Reads one device's event queue forever, integrating REL_X/REL_Y into
/// `POSITION`. `fetch_events` blocks until events are available, so this
/// parks the thread rather than spinning.
fn spawn_listener(mut device: Device) {
    thread::spawn(move || {
        loop {
            let events = match device.fetch_events() {
                Ok(events) => events,
                Err(e) => {
                    eprintln!("[mouse_position] evdev read failed, stopping listener: {e}");
                    return;
                }
            };
            let (mut dx, mut dy) = (0.0, 0.0);
            for event in events {
                let EventSummary::RelativeAxis(_, code, value) = event.destructure() else {
                    continue;
                };
                match code {
                    RelativeAxisCode::REL_X => dx += value as f64,
                    RelativeAxisCode::REL_Y => dy += value as f64,
                    _ => {}
                }
            }
            if dx != 0.0 || dy != 0.0 {
                apply_delta(dx, dy);
            }
        }
    });
}

/// Applies one integrated motion step, clamped to the active screen bounds.
fn apply_delta(dx: f64, dy: f64) {
    let (bx, by, bw, bh) = BOUNDS.get().copied().unwrap_or((0, 0, 1920, 1080));
    let Ok(mut pos) = POSITION.lock() else {
        return;
    };
    pos.0 = (pos.0 + dx).clamp(bx as f64, (bx + bw) as f64);
    pos.1 = (pos.1 + dy).clamp(by as f64, (by + bh) as f64);
}

/// Periodically nudges `POSITION` a fraction of the way toward an X11
/// reading, bounding the evdev integration's drift. A blend (not a snap)
/// because the X11 reading itself may be stale — see module docs.
///
/// Confirmed on a real GNOME/Mutter session that XQueryPointer can go
/// further than "stale": it can report the exact same frozen coordinate
/// forever, never once reflecting real motion, for the entire session. A
/// blend toward a value like that isn't drift correction, it's actively
/// dragging the tracked position toward a wrong, immovable anchor. So an
/// X11 reading identical to the last one is treated as untrustworthy and
/// skipped rather than blended; a reading that actually changes is treated
/// as live and gets blended in as before.
fn spawn_resync_thread() {
    thread::spawn(|| {
        let mut last_seen: Option<(i64, i64)> = None;
        loop {
            thread::sleep(Duration::from_millis(MOUSE_EVDEV_RESYNC_MS));
            let Ok(reading) = crossplatform::Backend::mouse_movement() else {
                continue;
            };
            if last_seen == Some(reading) {
                continue;
            }
            last_seen = Some(reading);
            let (rx, ry) = reading;
            let Ok(mut pos) = POSITION.lock() else {
                continue;
            };
            pos.0 += (rx as f64 - pos.0) * MOUSE_EVDEV_RESYNC_BLEND;
            pos.1 += (ry as f64 - pos.1) * MOUSE_EVDEV_RESYNC_BLEND;
        }
    });
}
