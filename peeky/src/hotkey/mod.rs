//! Push-to-talk hotkey wiring. Backend selected at compile time by target OS.
//!
//! Callers use the free functions directly; the backend is an implementation
//! detail. Adding a backend requires only a file implementing `HotkeyBackend`
//! plus a `mod` + type-alias arm below; no call site changes.

mod backend;
pub use backend::HotkeyBackend;

#[cfg(not(target_os = "linux"))]
mod crossplatform;
#[cfg(all(target_os = "linux", feature = "hyprland"))]
mod hyprland;
#[cfg(all(target_os = "linux", not(feature = "hyprland")))]
mod evdev;

// Hyprland's bind/bindr signal mechanism on Linux with the feature on;
// otherwise raw evdev on Linux (GNOME/X11/anything else - the crossplatform
// backend's compositor-level grab only reaches XWayland-focused windows under
// GNOME/Mutter, so it can't be a real global hotkey there); the portable
// global-hotkey backend everywhere else (macOS, Windows). The three cfgs are
// mutually exclusive and exhaustive, so exactly one Active is always defined.
#[cfg(all(target_os = "linux", feature = "hyprland"))]
type Active = hyprland::Backend;
#[cfg(all(target_os = "linux", not(feature = "hyprland")))]
type Active = evdev::Backend;
#[cfg(not(target_os = "linux"))]
type Active = crossplatform::Backend;

/// Start listening for the push-to-talk key.
///
/// # Errors
///
/// Propagates any setup error from the active backend.
pub fn init() -> std::io::Result<()> {
    Active::init()
}

/// Drain pending hotkey events into the recording state. No-op on backends
/// with an independent listener thread. Call once per main-loop iteration.
///
/// `allow(dead_code)`: only the polling backend's build calls this.
#[allow(dead_code)]
pub fn poll() {
    Active::poll();
}

/// True while the hotkey is held.
pub fn is_recording() -> bool {
    Active::is_recording()
}

/// Block the calling thread until the hotkey is pressed.
pub fn wait_for_press() {
    Active::wait_for_press();
}

/// Register a callback fired on press. Call before [`init`].
///
/// `allow(dead_code)`: only the signal backend's build calls this.
#[allow(dead_code)]
pub fn on_press(f: impl Fn() + Send + Sync + 'static) {
    Active::on_press(Box::new(f));
}

/// Register a callback fired on release. Call before [`init`].
#[allow(dead_code)]
pub fn on_release(f: impl Fn() + Send + Sync + 'static) {
    Active::on_release(Box::new(f));
}

/// Register a callback fired when the region-analysis key (Alt) is pressed.
/// Call before [`init`]. Only the evdev backend fires this for real; other
/// backends accept the registration but never call it.
#[allow(dead_code)]
pub fn on_analyze_press(f: impl Fn() + Send + Sync + 'static) {
    Active::on_analyze_press(Box::new(f));
}

/// Register a callback fired when the mouse-tracking recalibration key
/// (Home) is pressed. Call before [`init`]. Only the evdev backend fires
/// this for real; other backends accept the registration but never call it.
#[allow(dead_code)]
pub fn on_recalibrate_press(f: impl Fn() + Send + Sync + 'static) {
    Active::on_recalibrate_press(Box::new(f));
}
