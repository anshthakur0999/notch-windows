//! Colours for the light and dark pill and the accent choices. Everything that draws asks the
//! renderer's current `Palette` instead of hard-coding colours, so the tray menu can switch
//! the look immediately.
//!
//! The dark palette is the original look. Greys that used to differ by a few levels are folded
//! into a handful of text tones.

use crate::render::mix;
use crate::settings::Accent;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub light: bool,
    /// The collapsed pill (it reads as part of the bezel on dark, a white pill on light) and the
    /// open panel; the body eases from one to the other as it opens.
    pub rest: u32,
    pub body: u32,
    /// Base colour of translucent overlays (hairline border, hover fills, the collapsed dash):
    /// white on dark, black on light. `edge` is the hairline's opacity.
    pub wash: u32,
    pub edge: f32,
    /// Header track, search field, tiles (rename box, thumbnail slot), key caps, rules.
    pub seg: u32,
    pub field: u32,
    pub tile: u32,
    pub cap: u32,
    pub rule: u32,
    /// Text tones, strongest first. `faint` is for placeholders and idle controls.
    pub text: u32,
    pub text2: u32,
    pub muted: u32,
    pub subtle: u32,
    pub faint: u32,
    pub danger: u32,
    /// The pin / "paused" amber: a fixed meaning, not part of the accent.
    pub pin: u32,
    /// Accent for the notepad side, the clipboard side and the timer (the same colour unless
    /// `Classic`, which gives the timer its own violet).
    pub notes: u32,
    pub clip: u32,
    pub timer: u32,
    /// Ink on top of an accent fill (the check mark in the copy peek's badge).
    pub ink: u32,
    /// Notepad editor text.
    pub editor_text: u32,
}

/// (notepad, clipboard, timer) accent for the dark and light pill.
fn accent_pair(accent: Accent, light: bool) -> (u32, u32, u32) {
    // dark = bright on a dark panel; light = deeper so it reads on a pale one
    let one = |dark: u32, deep: u32| if light { (deep, deep, deep) } else { (dark, dark, dark) };
    match accent {
        Accent::Classic => {
            if light {
                (0xB97B0C, 0x0A86C4, 0x6D4FD8)
            } else {
                (0xE9B44C, 0x4FC3F7, 0xBFA6FF)
            }
        }
        Accent::Sky => one(0x4FC3F7, 0x0A86C4),
        Accent::Amber => one(0xE9B44C, 0xB97B0C),
        Accent::Mint => one(0x5FD6A3, 0x12936A),
        Accent::Violet => one(0xA78BFA, 0x6D4FD8),
        Accent::Rose => one(0xF472B6, 0xD6347E),
    }
}

/// The colours a menu swatch shows for an accent (left, right); the same for every accent but
/// `Classic`, which is two-toned.
pub fn swatch(accent: Accent) -> (u32, u32) {
    let (notes, clip, _) = accent_pair(accent, false);
    (notes, clip)
}

impl Palette {
    pub fn new(light: bool, accent: Accent) -> Palette {
        let (notes, clip, timer) = accent_pair(accent, light);
        if light {
            Palette {
                light,
                rest: 0xFFFFFF,
                body: 0xF7F8FA,
                wash: 0x000000,
                edge: 0.12,
                seg: 0xE9EBEF,
                field: 0xECEEF2,
                tile: 0xE2E5EA,
                cap: 0xE1E4E9,
                rule: 0xDADDE3,
                text: 0x14171C,
                text2: 0x3A404A,
                muted: 0x5E6571,
                subtle: 0x6B7280,
                faint: 0x9AA1AC,
                danger: 0xD93636,
                pin: 0xD48806,
                notes,
                clip,
                timer,
                ink: 0xFFFFFF,
                editor_text: 0x1A1D22,
            }
        } else {
            Palette {
                light,
                rest: 0x000000,
                body: 0x0B0D11,
                wash: 0xFFFFFF,
                edge: 0.09,
                seg: 0x15171C,
                field: 0x16181D,
                tile: 0x1B1E24,
                cap: 0x20232A,
                rule: 0x22252B,
                text: 0xF1F3F5,
                text2: 0xC9CDD3,
                muted: 0x9BA1AB,
                subtle: 0x8E949D,
                faint: 0x6E747D,
                danger: 0xFF6B6B,
                pin: 0xF2B84B,
                notes,
                clip,
                timer,
                ink: 0x06222E,
                editor_text: 0xE8EAED,
            }
        }
    }

    /// Fill of the active header segment: the accent washed into the header track.
    pub fn seg_active(&self, accent: u32) -> u32 {
        mix(self.seg, accent, if self.light { 0.22 } else { 0.18 })
    }

    /// Selected clipboard row: a faint fill and a clearer outline in the clipboard accent.
    pub fn sel_fill(&self) -> u32 {
        mix(self.body, self.clip, if self.light { 0.10 } else { 0.13 })
    }

    pub fn sel_edge(&self) -> u32 {
        mix(self.body, self.clip, if self.light { 0.45 } else { 0.47 })
    }
}

impl Default for Palette {
    fn default() -> Self {
        Palette::new(false, Accent::Classic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: u32) -> f32 {
        let ch = |s: u32| {
            let v = ((c >> s) & 0xFF) as f32 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
    }

    /// WCAG contrast ratio.
    fn contrast(a: u32, b: u32) -> f32 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn dark_is_the_original_look() {
        let p = Palette::new(false, Accent::Classic);
        assert_eq!((p.rest, p.body, p.notes, p.clip), (0x000000, 0x0B0D11, 0xE9B44C, 0x4FC3F7));
        // the old hand-picked tints are close to what the formulas give
        let close = |a: u32, b: u32| (0..3).all(|i| (((a >> (i * 8)) & 0xFF) as i32 - ((b >> (i * 8)) & 0xFF) as i32).abs() <= 12);
        assert!(close(p.seg_active(p.notes), 0x3B2F1B), "{:06X}", p.seg_active(p.notes));
        assert!(close(p.seg_active(p.clip), 0x1E3A47), "{:06X}", p.seg_active(p.clip));
        assert!(close(p.sel_fill(), 0x142229), "{:06X}", p.sel_fill());
        assert!(close(p.sel_edge(), 0x2B6577), "{:06X}", p.sel_edge());
    }

    #[test]
    fn text_is_readable_in_both_themes() {
        for light in [false, true] {
            let p = Palette::new(light, Accent::Classic);
            assert!(contrast(p.text, p.body) >= 12.0, "light={light}");
            assert!(contrast(p.text2, p.body) >= 7.0, "light={light}");
            assert!(contrast(p.muted, p.body) >= 4.5, "light={light}");
            assert!(contrast(p.subtle, p.body) >= 4.0, "light={light}");
            assert!(contrast(p.editor_text, p.body) >= 12.0, "light={light}");
            assert!(contrast(p.text, p.seg) >= 10.0 && contrast(p.text, p.field) >= 10.0, "light={light}");
            assert!(contrast(p.danger, p.body) >= 3.5, "light={light}");
            assert!(contrast(p.pin, p.body) >= 2.5, "light={light}");
        }
    }

    #[test]
    fn every_accent_is_visible_and_its_badge_ink_is_readable() {
        for light in [false, true] {
            for &a in Accent::ALL {
                let p = Palette::new(light, a);
                for c in [p.notes, p.clip, p.timer] {
                    assert!(contrast(c, p.body) >= 2.6, "{a:?} light={light} {c:06X} on the panel");
                    assert!(contrast(p.ink, c) >= 3.0, "{a:?} light={light} ink on {c:06X}");
                }
            }
        }
    }

    #[test]
    fn classic_is_two_toned_and_the_rest_are_one_colour() {
        for light in [false, true] {
            let c = Palette::new(light, Accent::Classic);
            assert_ne!(c.notes, c.clip);
            assert!(c.timer != c.notes && c.timer != c.clip, "the timer has its own violet");
            for &a in Accent::ALL.iter().filter(|a| **a != Accent::Classic) {
                let p = Palette::new(light, a);
                assert_eq!((p.notes, p.timer), (p.clip, p.clip), "{a:?}");
            }
        }
        assert_ne!(swatch(Accent::Classic).0, swatch(Accent::Classic).1);
        assert_eq!(swatch(Accent::Mint).0, swatch(Accent::Mint).1);
    }

    #[test]
    fn light_and_dark_differ_where_it_matters() {
        let (d, l) = (Palette::new(false, Accent::Sky), Palette::new(true, Accent::Sky));
        assert!(lum(l.body) > 0.8 && lum(d.body) < 0.02);
        assert!(lum(l.rest) > lum(d.rest));
        assert_eq!((d.wash, l.wash), (0xFFFFFF, 0x000000));
        assert!(lum(l.text) < lum(l.body) && lum(d.text) > lum(d.body));
    }
}
