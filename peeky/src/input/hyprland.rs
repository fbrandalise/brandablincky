//! Linux input injection via ydotool (uinput). Used on Hyprland/Wayland,
//! which has no userspace "click/type at point" primitive.

use std::process::Command;
use std::thread;
use std::time::Duration;

use super::backend::InputInjector;

/// ydotool backend. Zero-sized; never instantiated.
pub struct Backend;

impl InputInjector for Backend {
    fn exec_click(x: i64, y: i64) {
        match run_ydotool(&[
            "mousemove",
            "--absolute",
            "-x",
            &x.to_string(),
            "-y",
            &y.to_string(),
        ]) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("[action:click] mousemove failed: {}", e);
                return;
            }
        }
        // Above the ~10ms pointer-debounce floor so the click registers at
        // the new position, not the previous one.
        thread::sleep(Duration::from_millis(30));
        // 0xC0 = BTN_LEFT down+up combined in ydotool's encoding.
        if let Err(e) = run_ydotool(&["click", "0xC0"]) {
            eprintln!("[action:click] click failed: {}", e);
        }
    }

    fn exec_type(text: &str) {
        // Covers GTK/Qt focus-handling delay after a preceding click;
        // without it the first few keystrokes get dropped.
        thread::sleep(Duration::from_millis(80));
        // `--` so a `text` starting with `-` isn't parsed as a flag.
        if let Err(e) = run_ydotool(&["type", "--", text]) {
            eprintln!("[action:type] type failed: {}", e);
        }
    }

    fn exec_key(combo: &str) {
        let scancodes: Vec<u16> = combo
            .split('+')
            .filter_map(|part| key_name_to_scancode(part.trim()))
            .collect();
        if scancodes.is_empty() {
            eprintln!("[action:key] no recognized keys in '{}'", combo);
            return;
        }

        // Build the ydotool args: press all keys in order, then release in
        // reverse. ydotool's syntax: `key 28:1 28:0` = press+release Enter.
        // Modifier combos: `key 29:1 30:1 30:0 29:0` = Ctrl+A.
        let mut args: Vec<String> = vec!["key".to_string()];
        for sc in &scancodes {
            args.push(format!("{}:1", sc));
        }
        for sc in scancodes.iter().rev() {
            args.push(format!("{}:0", sc));
        }

        // Shorter than exec_type because combos typically hit
        // already-focused windows (hotkeys, form submit).
        thread::sleep(Duration::from_millis(50));
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        if let Err(e) = run_ydotool(&arg_refs) {
            eprintln!("[action:key] ydotool key '{}' failed: {}", combo, e);
        }
    }

    fn exec_scroll(direction: &str, amount: u32) {
        let scancode: u16 = match direction.to_lowercase().as_str() {
            "down" => 108,
            "up" => 103,
            "left" => 105,
            "right" => 106,
            other => {
                eprintln!(
                    "[action:scroll] unknown direction '{}', defaulting to down",
                    other
                );
                108
            }
        };
        // 3 presses/wheel-click matches GTK's default scroll-line count.
        // Cap at 30 because Claude sometimes emits amount=99 meaning "scroll
        // to the end". Uncapped that hangs the UI firing arrow keys.
        let presses = (amount.saturating_mul(3)).clamp(1, 30);

        let mut args: Vec<String> = vec!["key".to_string()];
        for _ in 0..presses {
            args.push(format!("{}:1", scancode));
            args.push(format!("{}:0", scancode));
        }
        thread::sleep(Duration::from_millis(30));
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        if let Err(e) = run_ydotool(&arg_refs) {
            eprintln!("[action:scroll] ydotool failed: {}", e);
        }
    }

    fn check_available() {
        // `help` (not `--version`) since some ydotool builds don't implement
        // a version subcommand and would otherwise report a false negative.
        match Command::new("ydotool").arg("help").output() {
            Ok(o) if o.status.success() => {
                eprintln!("[startup] ydotool available. click actions will fire real input");
            }
            _ => {
                eprintln!(
                    "[startup] WARNING: ydotool not found on PATH. Click actions will move the\n\
                     \toverlay but NOT inject a real click. To enable:\n\
                     \t  sudo pacman -S ydotool   # (or apt/dnf equivalent)\n\
                     \t  sudo usermod -aG input $USER\n\
                     \t  systemctl --user enable --now ydotool.service"
                );
            }
        }
    }
}

/// Runs `ydotool` with the given args and turns a nonzero exit into an
/// error. `Command::status()` alone only reports spawn failures, so a
/// process that runs but fails at runtime (e.g. can't reach ydotoold's
/// socket) would otherwise be silently treated as success.
fn run_ydotool(args: &[&str]) -> Result<(), String> {
    let status = Command::new("ydotool")
        .args(args)
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("ydotool exited with {}", status))
    }
}

/// Map a key name (Claude's wording) to a Linux input-event scancode.
/// Covers the keys peeky-style voice commands actually emit: navigation,
/// modifiers, letters, digits. Anything not in this table returns None
/// and gets logged as unrecognized.
fn key_name_to_scancode(name: &str) -> Option<u16> {
    let lower = name.to_lowercase();
    match lower.as_str() {
        "esc" | "escape" => Some(1),
        "1" => Some(2),
        "2" => Some(3),
        "3" => Some(4),
        "4" => Some(5),
        "5" => Some(6),
        "6" => Some(7),
        "7" => Some(8),
        "8" => Some(9),
        "9" => Some(10),
        "0" => Some(11),
        "minus" | "-" => Some(12),
        "equal" | "=" => Some(13),
        "backspace" => Some(14),
        "tab" => Some(15),
        "enter" | "return" | "kp_enter" => Some(28),
        "ctrl" | "control" | "leftctrl" => Some(29),
        "shift" | "leftshift" => Some(42),
        "rightshift" => Some(54),
        "alt" | "leftalt" => Some(56),
        "rightalt" | "altgr" => Some(100),
        "space" => Some(57),
        "capslock" => Some(58),
        "f1" => Some(59),
        "f2" => Some(60),
        "f3" => Some(61),
        "f4" => Some(62),
        "f5" => Some(63),
        "f6" => Some(64),
        "f7" => Some(65),
        "f8" => Some(66),
        "f9" => Some(67),
        "f10" => Some(68),
        "f11" => Some(87),
        "f12" => Some(88),
        "home" => Some(102),
        "up" | "arrowup" => Some(103),
        "pageup" | "page_up" => Some(104),
        "left" | "arrowleft" => Some(105),
        "right" | "arrowright" => Some(106),
        "end" => Some(107),
        "down" | "arrowdown" => Some(108),
        "pagedown" | "page_down" => Some(109),
        "insert" => Some(110),
        "delete" | "del" => Some(111),
        "super" | "meta" | "win" | "leftmeta" => Some(125),
        // Letters a-z map to KEY_A=30 through KEY_Z=44 in keyboard layout order
        // (not alphabetical: QWERTY row order).
        s if s.len() == 1 => {
            let c = s.chars().next()?;
            const QWERTY: &[(char, u16)] = &[
                ('a', 30),
                ('b', 48),
                ('c', 46),
                ('d', 32),
                ('e', 18),
                ('f', 33),
                ('g', 34),
                ('h', 35),
                ('i', 23),
                ('j', 36),
                ('k', 37),
                ('l', 38),
                ('m', 50),
                ('n', 49),
                ('o', 24),
                ('p', 25),
                ('q', 16),
                ('r', 19),
                ('s', 31),
                ('t', 20),
                ('u', 22),
                ('v', 47),
                ('w', 17),
                ('x', 45),
                ('y', 21),
                ('z', 44),
            ];
            QWERTY
                .iter()
                .find(|(ch, _)| *ch == c)
                .map(|(_, code)| *code)
        }
        _ => None,
    }
}
