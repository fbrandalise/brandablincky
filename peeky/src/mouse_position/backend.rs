/// Contract every mouse-position backend must satisfy.
///
/// Implementors are zero-sized types (`pub struct Backend;`). The active
/// backend is selected at compile time via a `type Active = …` alias in
/// `mod.rs`, and the public free functions delegate to `Active::*`. This
/// gives zero-cost static dispatch with no vtable and no runtime branching.
///
/// # Implementing a new backend
///
/// ```ignore
/// pub struct Backend;
/// impl MousePositionBackend for Backend {
///     fn mouse_movement() -> Result<(i64, i64), Box<dyn std::error::Error + Send + Sync>> {
///         // ...
///     }
/// }
/// ```
pub trait MousePositionBackend {
    /// Returns the cursor's absolute screen position in pixels as `(x, y)`.
    ///
    /// # Errors
    ///
    /// Returns an error if the compositor or OS cannot be queried.
    fn mouse_movement() -> Result<(i64, i64), Box<dyn std::error::Error + Send + Sync>>;

    /// Tell the backend the cursor was just synthetically moved to `(x, y)`
    /// (e.g. by a ydotool relative-move click), so a backend that integrates
    /// its own position estimate can stay in sync instead of drifting.
    /// Default no-op: backends that query live compositor/OS state (Hyprland
    /// IPC, X11) don't need this — only `evdev`'s integrated tracker does,
    /// since it can't see motion injected by ydotool's own virtual device
    /// (deliberately excluded from the devices it listens to).
    ///
    /// `allow(dead_code)`: only the evdev backend's build calls this for
    /// real.
    #[allow(dead_code)]
    fn report_synthetic_move(_x: i64, _y: i64) {}
}
