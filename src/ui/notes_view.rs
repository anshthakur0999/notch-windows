//! Notepad tab chrome: the note tabs row, divider and footer. (The text itself is a native
//! RichEdit window placed over `notes_rect()`.)

use super::*;
use crate::render::{F, Renderer, color};

/// Hit rectangles for the note tabs and the "+" button (window coordinates, DIPs).
pub struct NoteTabs {
    pub tabs: Vec<RectF>,
    pub add: Option<RectF>,
}

impl NoteTabs {
    pub fn empty() -> NoteTabs {
        NoteTabs { tabs: Vec::new(), add: None }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if let Some(i) = self.tabs.iter().position(|t| t.contains(x, y)) {
            return Some(Hit::NoteTab(i));
        }
        match self.add {
            Some(a) if a.contains(x, y) => Some(Hit::NoteAdd),
            _ => None,
        }
    }
}

/// Lays out the tab row for the given note names (widths come from real text measurement).
pub fn layout(r: &Renderer, names: &[String]) -> NoteTabs {
    let p = exp();
    let (top, bot) = (p.t + 54.0, p.t + 88.0);
    let mut x = p.l + 12.7;
    let mut tabs = Vec::with_capacity(names.len());
    for n in names {
        let w = r.text_w(F::TabOn, n).min(110.0) + 21.0;
        tabs.push(RectF { l: x, t: top, r: x + w, b: bot });
        x += w + 4.0;
    }
    let add = (x + 34.0 <= p.r - 8.0 && names.len() < crate::notes::MAX_NOTES)
        .then_some(RectF { l: x, t: top, r: x + 34.0, b: bot });
    NoteTabs { tabs, add }
}

pub fn draw(r: &Renderer, v: &View, alpha: f32) {
    let p = exp();
    pill::hline(r, p.t + 88.0, alpha);

    for (i, rect) in v.note_tabs.tabs.iter().enumerate() {
        let active = i == v.note_active;
        let hover = v.hover == Hit::NoteTab(i);
        if v.renaming != Some(i) {
            let text = RectF { l: rect.l + 10.5, t: rect.t, r: rect.r - 10.5, b: p.t + 82.0 };
            let c = if active {
                0xF2F3F5
            } else if hover {
                0xC9CDD3
            } else {
                0x9BA1AB
            };
            let name = v.note_names.get(i).map(String::as_str).unwrap_or("");
            r.text(name, if active { F::TabOn } else { F::Tab }, text.d2d(), color(c, alpha));
        }
        if active {
            let u = RectF { l: rect.l, t: p.t + 86.6, r: rect.r, b: p.t + 88.6 };
            r.fill_rrect(u.d2d(), 1.0, color(pill::AMBER, alpha));
        }
    }
    if let Some(a) = v.note_tabs.add {
        let c = if v.hover == Hit::NoteAdd { 0xE6E8EB } else { 0x9BA1AB };
        r.glyph('\u{E710}', F::Icon14, a.cx(), p.t + 68.0, color(c, alpha));
    }

    // ---- footer ----
    pill::hline(r, p.t + FOOTER_SEP, alpha);
    let cy = p.t + FOOTER_CY;
    r.glyph('\u{E73E}', F::Icon14, p.l + 26.0, cy, color(pill::AMBER, alpha));
    r.text(
        "Saved automatically",
        F::Hint,
        RectF { l: p.l + 40.0, t: cy - 9.0, r: p.l + 220.0, b: cy + 9.0 }.d2d(),
        color(0x9AA0A8, alpha),
    );

    let label = "to collapse";
    let lw = r.text_w(F::Hint, label);
    let label_l = p.r - 20.0 - lw;
    r.text(
        label,
        F::Hint,
        RectF { l: label_l, t: cy - 9.0, r: label_l + lw + 4.0, b: cy + 9.0 }.d2d(),
        color(0x9AA0A8, alpha),
    );
    let key_w = pill::keycap_width(r, "Esc");
    pill::keycap(r, "Esc", label_l - 7.0 - key_w, cy, alpha);
}
