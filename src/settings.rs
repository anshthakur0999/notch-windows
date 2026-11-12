//! User settings (`%APPDATA%\Notch\settings.json`). Missing file / fields fall back to defaults.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Global hotkey that toggles the pill on the last used tab.
    pub hotkey_toggle: String,
    /// Global hotkey that opens the pill directly on the clipboard tab.
    pub hotkey_clipboard: String,
    /// Global hotkey that hides the pill from the screen / brings it back.
    pub hotkey_visibility: String,
    /// How long the cursor must rest on the pill before it expands.
    pub hover_dwell_ms: u32,
    /// How long after the cursor leaves before the pill collapses.
    pub collapse_delay_ms: u32,
    /// Maximum number of unpinned clipboard items kept.
    pub max_items: usize,
    /// Maximum disk space (MB) used by unpinned clipboard images.
    pub max_image_mb: u64,
    /// Clicking a clipboard entry also sends Ctrl+V to the previous window.
    pub paste_on_click: bool,
    /// On each copy the collapsed pill briefly widens to say "Copied".
    pub copy_peek: bool,
    /// Show what was copied in that capsule. Off shows only "Copied" (nothing about the content).
    pub copy_peek_preview: bool,
    /// How long the capsule stays up (longer while the cursor is on it).
    pub copy_peek_ms: u32,
    /// Draw on the GPU. Off (default) uses Microsoft's WARP software rasteriser, which avoids
    /// loading the vendor graphics driver (often 50+ MB of RAM) for what is a tiny drawing job.
    pub use_gpu: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey_toggle: "Win+Alt+N".into(),
            hotkey_clipboard: "Win+Alt+V".into(),
            hotkey_visibility: "Win+Alt+H".into(),
            hover_dwell_ms: 120,
            collapse_delay_ms: 100,
            max_items: 200,
            max_image_mb: 200,
            paste_on_click: true,
            copy_peek: true,
            copy_peek_preview: true,
            copy_peek_ms: 2000,
            use_gpu: false,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        let mut s: Settings = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default();
        s.hover_dwell_ms = s.hover_dwell_ms.clamp(0, 2000);
        s.collapse_delay_ms = s.collapse_delay_ms.clamp(50, 5000);
        s.max_items = s.max_items.clamp(10, 5000);
        s.max_image_mb = s.max_image_mb.clamp(10, 10_000);
        s.copy_peek_ms = s.copy_peek_ms.clamp(500, 10_000);
        s
    }
}

/// Parses strings such as `"Win+Shift+N"` or `"Ctrl+Alt+F9"` into `(modifiers, virtual-key)`.
pub fn parse_hotkey(spec: &str) -> Option<(u32, u32)> {
    let mut mods = 0u32;
    let mut vk = None;
    for part in spec.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "win" | "super" | "meta" => mods |= MOD_WIN,
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "shift" => mods |= MOD_SHIFT,
            "alt" => mods |= MOD_ALT,
            key => {
                if vk.is_some() {
                    return None; // two non-modifier keys
                }
                vk = Some(parse_key(key)?);
            }
        }
    }
    let vk = vk?;
    if mods == 0 {
        return None; // refuse bare keys: they would hijack normal typing
    }
    Some((mods, vk))
}

fn parse_key(key: &str) -> Option<u32> {
    let b = key.as_bytes();
    if b.len() == 1 {
        let c = b[0].to_ascii_uppercase();
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    match key {
        "space" => Some(0x20),
        "tab" => Some(0x09),
        "enter" | "return" => Some(0x0D),
        "backspace" => Some(0x08),
        "insert" => Some(0x2D),
        "delete" | "del" => Some(0x2E),
        "home" => Some(0x24),
        "end" => Some(0x23),
        "pageup" => Some(0x21),
        "pagedown" => Some(0x22),
        "`" | "grave" => Some(0xC0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let s = Settings::default();
        assert_eq!(s.max_items, 200);
        assert!(s.paste_on_click);
        assert!(s.copy_peek && s.copy_peek_preview);
        assert_eq!(s.copy_peek_ms, 2000);
        assert_eq!(parse_hotkey(&s.hotkey_toggle), Some((MOD_WIN | MOD_ALT, b'N' as u32)));
        assert_eq!(parse_hotkey(&s.hotkey_clipboard), Some((MOD_WIN | MOD_ALT, b'V' as u32)));
        assert_eq!(parse_hotkey(&s.hotkey_visibility), Some((MOD_WIN | MOD_ALT, b'H' as u32)));
    }

    #[test]
    fn parses_hotkeys() {
        assert_eq!(parse_hotkey("ctrl+alt+F9"), Some((MOD_CONTROL | MOD_ALT, 0x78)));
        assert_eq!(parse_hotkey("Ctrl + Space"), Some((MOD_CONTROL, 0x20)));
        assert_eq!(parse_hotkey("alt+1"), Some((MOD_ALT, b'1' as u32)));
    }

    #[test]
    fn rejects_bad_hotkeys() {
        assert_eq!(parse_hotkey("N"), None); // no modifier
        assert_eq!(parse_hotkey("Ctrl+"), None); // no key
        assert_eq!(parse_hotkey("Ctrl+A+B"), None); // two keys
        assert_eq!(parse_hotkey("Ctrl+Banana"), None);
        assert_eq!(parse_hotkey("Ctrl+F25"), None);
        assert_eq!(parse_hotkey(""), None);
    }

    #[test]
    fn partial_json_keeps_defaults_and_clamps() {
        let dir = std::env::temp_dir().join(format!("notch-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.json");
        std::fs::write(&p, r#"{ "max_items": 1, "hover_dwell_ms": 250, "copy_peek_ms": 5 }"#).unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.max_items, 10); // clamped up
        assert_eq!(s.hover_dwell_ms, 250);
        assert_eq!(s.collapse_delay_ms, 100); // default
        assert_eq!(s.copy_peek_ms, 500); // clamped up
        assert!(s.copy_peek); // default
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_corrupt_file_gives_defaults() {
        assert_eq!(Settings::load(Path::new("Z:\\definitely\\missing.json")), Settings::default());
        let dir = std::env::temp_dir().join(format!("notch-settings-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.json");
        std::fs::write(&p, "{ not json").unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
