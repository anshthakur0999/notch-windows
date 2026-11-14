//! User settings (`%APPDATA%\Notch\settings.json`). Missing file / fields fall back to defaults.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;

/// A setting that is one of a few named choices, stored in JSON as a lowercase word. An
/// unknown or mistyped word falls back to the default instead of invalidating the whole file.
macro_rules! choice {
    ($(#[$m:meta])* $name:ident { $($var:ident => $text:literal),+ $(,)? } default $def:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name {
            $($var),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];

            pub fn name(self) -> &'static str {
                match self {
                    $($name::$var => $text),+
                }
            }

            pub fn parse(s: &str) -> $name {
                let s = s.trim();
                $(if s.eq_ignore_ascii_case($text) {
                    return $name::$var;
                })+
                $name::$def
            }
        }

        impl Default for $name {
            fn default() -> Self {
                $name::$def
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.name())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let v = serde_json::Value::deserialize(d)?;
                Ok($name::parse(v.as_str().unwrap_or("")))
            }
        }
    };
}

choice! {
    /// Light or dark pill. `System` follows Windows' app colour mode and switches with it.
    Theme { System => "system", Dark => "dark", Light => "light" } default Dark
}

choice! {
    /// Highlight colour. `Classic` keeps the logo's two tones (amber notepad, cyan clipboard);
    /// the others use one colour for both.
    Accent {
        Classic => "classic",
        Sky => "sky",
        Amber => "amber",
        Mint => "mint",
        Violet => "violet",
        Rose => "rose",
    } default Classic
}

choice! {
    /// Size of the text you read: the notes and the clipboard entries.
    TextSize { Small => "small", Medium => "medium", Large => "large" } default Medium
}

impl TextSize {
    pub fn scale(self) -> f32 {
        match self {
            TextSize::Small => 0.9,
            TextSize::Medium => 1.0,
            TextSize::Large => 1.15,
        }
    }
}

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
    /// Light or dark pill (also in the tray menu, applied immediately).
    pub theme: Theme,
    /// Highlight colour (tray menu).
    pub accent: Accent,
    /// Size of notes and clipboard text (tray menu).
    pub text_size: TextSize,
    /// Which monitor the pill sits on: the display's device name (`\\.\DISPLAY2`), or empty for
    /// the primary one. If that display is not connected the primary one is used (tray menu).
    pub monitor: String,
    /// Files on the shelf clear themselves after this many hours (0 = keep until cleared).
    pub shelf_hours: u32,
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
            theme: Theme::default(),
            accent: Accent::default(),
            text_size: TextSize::default(),
            monitor: String::new(),
            shelf_hours: 24,
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
        s.monitor = s.monitor.trim().to_string();
        s
    }

    /// Writes the settings back (the tray menu changes them). A file that `load` could not read
    /// (broken JSON, or a value of the wrong type such as `"max_items": "300"`) is kept as
    /// `settings.json.bad` instead of being overwritten.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Ok(text) = std::fs::read_to_string(path) {
            if serde_json::from_str::<Settings>(text.trim_start_matches('\u{feff}')).is_err() {
                let _ = std::fs::rename(path, path.with_extension("json.bad"));
            }
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        crate::store::write_atomic(path, json.as_bytes())
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
    fn choices_parse_leniently_and_bad_words_do_not_reset_the_file() {
        let dir = std::env::temp_dir().join(format!("notch-settings-choice-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.json");
        std::fs::write(
            &p,
            r#"{ "theme": "Light", "accent": "chartreuse", "text_size": 7, "monitor": "  \\\\.\\DISPLAY2 ", "max_items": 77 }"#,
        )
        .unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.theme, Theme::Light); // case-insensitive
        assert_eq!(s.accent, Accent::Classic); // unknown word: the default
        assert_eq!(s.text_size, TextSize::Medium); // wrong type: the default
        assert_eq!(s.monitor, r"\\.\DISPLAY2"); // trimmed
        assert_eq!(s.max_items, 77, "one bad choice must not reset the other settings");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_round_trips_and_keeps_an_unreadable_file() {
        let dir = std::env::temp_dir().join(format!("notch-settings-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.json");
        let mut s = Settings::default();
        s.theme = Theme::System;
        s.accent = Accent::Violet;
        s.text_size = TextSize::Large;
        s.monitor = r"\\.\DISPLAY2".into();
        s.max_items = 321;
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
        // a broken file is moved aside, not silently destroyed
        std::fs::write(&p, "{ my careful edits").unwrap();
        s.save(&p).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("settings.json.bad")).unwrap(), "{ my careful edits");
        assert_eq!(Settings::load(&p), s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wrongly_typed_value_is_kept_in_a_bad_copy_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("notch-settings-type-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.json");
        let mine = r#"{ "hotkey_toggle": "Ctrl+Alt+Q", "max_items": "300" }"#; // valid JSON, wrong type
        std::fs::write(&p, mine).unwrap();
        assert_eq!(Settings::load(&p), Settings::default(), "the whole file is unreadable as settings");
        Settings::default().save(&p).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("settings.json.bad")).unwrap(), mine);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_readable_file_is_not_copied_to_bad() {
        let dir = std::env::temp_dir().join(format!("notch-settings-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.json");
        std::fs::write(&p, r#"{ "max_items": 77, "theme": "chartreuse" }"#).unwrap();
        Settings::load(&p).save(&p).unwrap();
        assert!(!dir.join("settings.json.bad").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_sizes_grow_and_every_choice_round_trips_by_name() {
        assert!(TextSize::Small.scale() < TextSize::Medium.scale() && TextSize::Medium.scale() < TextSize::Large.scale());
        assert_eq!(TextSize::Medium.scale(), 1.0);
        for t in Theme::ALL {
            assert_eq!(Theme::parse(t.name()), *t);
        }
        for a in Accent::ALL {
            assert_eq!(Accent::parse(a.name()), *a);
        }
        for t in TextSize::ALL {
            assert_eq!(TextSize::parse(t.name()), *t);
        }
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
