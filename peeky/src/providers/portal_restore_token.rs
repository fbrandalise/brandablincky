//! xdg-desktop-portal ScreenCast restore token, used by the GNOME/Wayland
//! screenshot fallback (`peeky/src/screenshot/portal.rs`) to skip the "Share
//! your screen" picker dialog on every run after the first.
//!
//! Written after a successful `Screencast::start()` with whatever
//! `Streams::restore_token()` the portal returns, to:
//!   Linux: $XDG_CONFIG_HOME/peeky/portal_restore_token (or ~/.config/peeky/portal_restore_token)
//!
//! Absent or empty file means no prior grant; the next capture attempt shows
//! the picker again. The portal treats an invalid or revoked token the same
//! as no token (falls back to prompting), so no validation happens here.

use std::fs;
use std::path::PathBuf;

/// Returns the stored restore token, or None if there isn't one yet.
pub fn load() -> Option<String> {
    let path = portal_restore_token_path()?;
    load_from_path(&path)
}

/// Write the token returned by the portal's `start()` response (0600 on
/// Unix), so the next process run can skip the picker dialog.
pub fn store(token: &str) -> std::io::Result<()> {
    let path = portal_restore_token_path()
        .ok_or_else(|| std::io::Error::other("no config dir on this platform"))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Same logic as `load()` but against an explicit path, for tests.
fn load_from_path(path: &PathBuf) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn portal_restore_token_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("peeky").join("portal_restore_token"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "peeky-portal-restore-token-test-{}-{}.txt",
            label,
            std::process::id()
        ))
    }

    #[test]
    fn missing_file_returns_none() {
        let p = tmp_path("missing");
        let _ = std::fs::remove_file(&p);
        assert!(load_from_path(&p).is_none());
    }

    #[test]
    fn empty_file_returns_none() {
        let p = tmp_path("empty");
        std::fs::write(&p, "").unwrap();
        assert!(load_from_path(&p).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn whitespace_only_returns_none() {
        let p = tmp_path("whitespace");
        std::fs::write(&p, "  \n\t \n").unwrap();
        assert!(load_from_path(&p).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn token_is_trimmed_and_returned() {
        let p = tmp_path("valid");
        std::fs::write(&p, "  restore-token-abc123\n").unwrap();
        assert_eq!(
            load_from_path(&p),
            Some("restore-token-abc123".to_string())
        );
        let _ = std::fs::remove_file(&p);
    }
}
