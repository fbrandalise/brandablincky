//! Mouse position polling. Backend selected at compile time by target OS.
//!
//! Callers use the free functions directly; the backend is an implementation
//! detail. Adding a new backend requires only:
//!   1. A new file implementing `MousePositionBackend`.
//!   2. A `#[cfg(...)] mod new_backend;` + type alias arm below.
//!   3. No changes to any call site.

mod backend;
pub use backend::MousePositionBackend;

#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
mod crossplatform;
#[cfg(all(target_os = "linux", feature = "hyprland"))]
mod hyprland;
#[cfg(all(target_os = "linux", not(feature = "hyprland")))]
mod evdev;

// Hyprland uses its native IPC. Linux without the hyprland feature (GNOME,
// X11, anything else) uses raw evdev: X11 querying via `crossplatform`'s
// XQueryPointer only reflects live motion while the cursor is over an
// XWayland-backed window under Mutter, the same limitation `hotkey/`
// already works around for the keyboard (see `evdev.rs`, which itself uses
// `crossplatform` internally to seed and resync). Everything else (macOS,
// Windows) uses the portable `crossplatform` backend directly. The three
// cfgs are mutually exclusive and exhaustive, so exactly one Active is
// always defined.
#[cfg(all(target_os = "linux", feature = "hyprland"))]
type Active = hyprland::Backend;
#[cfg(all(target_os = "linux", not(feature = "hyprland")))]
type Active = evdev::Backend;
#[cfg(not(target_os = "linux"))]
type Active = crossplatform::Backend;

/// Returns the cursor's absolute screen position as `(x, y)` in pixels.
///
/// # Errors
///
/// Propagates any error from the active backend.
///
/// # Example
///
/// ```no_run
/// let (x, y) = peeky::mouse_position::mouse_movement().unwrap();
/// println!("cursor at ({x}, {y})");
/// ```
pub fn mouse_movement() -> Result<(i64, i64), Box<dyn std::error::Error + Send + Sync>> {
    Active::mouse_movement()
}

/// Tell the active backend the cursor was just synthetically moved to
/// `(x, y)` (e.g. a ydotool relative-move click), so an integrating backend
/// can stay in sync. No-op on backends that query live state directly.
pub fn report_synthetic_move(x: i64, y: i64) {
    Active::report_synthetic_move(x, y);
}
