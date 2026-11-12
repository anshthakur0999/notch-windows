//! The notepad: several named notes, edited in a native RichEdit (plain-text mode) that sits
//! over the pill body.
//!
//! Using the system edit control gives IME input, undo/redo, selection and large-document
//! handling for free. It is created lazily the first time the Notes tab is shown, so a pill
//! that is never opened never pays for it.
//!
//! Storage: `notes.json` (list + active note) and one `notes\<id>.txt` per note. A single
//! `notes.txt` from an earlier version is migrated into the first note.

use crate::store::write_atomic;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows::Win32::UI::Controls::RichEdit::*;
use windows::Win32::UI::Controls::SetWindowTheme;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, GetKeyState, SetFocus};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

const EM_SETMARGINS: u32 = 0x00D3;
const EM_SETSEL_MSG: u32 = 0x00B1;
const EM_EMPTYUNDOBUFFER: u32 = 0x00CD;
const EM_SETPARAFORMAT_MSG: u32 = 0x0447;

/// COLORREF (0x00BBGGRR) values matching the pill body (#0B0D11) and text (#E8EAED).
const BG: u32 = 0x0011_0D0B;
const FG: u32 = 0x00ED_EAE8;
const EDIT_BG: u32 = 0x0024_1E1B; // #1B1E24, rename box

/// The tab row scrolls, so this is only a sanity limit (each note is one small file).
pub const MAX_NOTES: usize = 30;
const DEFAULT_NAME: &str = "Quick note";

// ------------------------------------------------------------ note list (pure)

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteMeta {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Meta {
    pub next_id: u64,
    pub active: usize,
    pub notes: Vec<NoteMeta>,
}

pub fn note_path(dir: &Path, id: u64) -> PathBuf {
    dir.join("notes").join(format!("{id}.txt"))
}

impl Meta {
    fn fresh() -> Meta {
        // id 1 is taken by the first note, so the next free id is 2
        Meta { next_id: 2, active: 0, notes: vec![NoteMeta { id: 1, name: DEFAULT_NAME.into() }] }
    }

    /// Loads the list from `dir`, creating it (and migrating a legacy `notes.txt`) if needed.
    pub fn load(dir: &Path) -> Meta {
        let _ = std::fs::create_dir_all(dir.join("notes"));
        let raw = std::fs::read_to_string(dir.join("notes.json")).ok();
        // tolerate a UTF-8 BOM (Notepad and PowerShell add one)
        let mut meta = raw
            .as_deref()
            .and_then(|t| serde_json::from_str::<Meta>(t.trim_start_matches('\u{feff}')).ok())
            .filter(|m| !m.notes.is_empty());
        if meta.is_none() {
            if raw.is_some() {
                // never overwrite something we could not read: keep it for recovery
                let _ = std::fs::rename(dir.join("notes.json"), dir.join("notes.json.bad"));
            }
            let m = Meta::fresh();
            let legacy = dir.join("notes.txt");
            if legacy.exists() && !note_path(dir, 1).exists() {
                let _ = std::fs::rename(&legacy, note_path(dir, 1));
            }
            let _ = m.save(dir);
            meta = Some(m);
        }
        let mut m = meta.unwrap();
        m.active = m.active.min(m.notes.len() - 1);
        m.next_id = m.next_id.max(m.notes.iter().map(|n| n.id).max().unwrap_or(0) + 1);
        m
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        write_atomic(&dir.join("notes.json"), &json)
    }

    /// Adds a note named "Note N" and makes it active. Returns its index.
    pub fn add(&mut self) -> usize {
        let mut n = self.notes.len() + 1;
        while self.notes.iter().any(|m| m.name == format!("Note {n}")) {
            n += 1;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.notes.push(NoteMeta { id, name: format!("Note {n}") });
        self.active = self.notes.len() - 1;
        self.active
    }

    pub fn rename(&mut self, i: usize, name: &str) {
        let name: String = name.trim().chars().take(40).collect();
        if let Some(n) = self.notes.get_mut(i) {
            if !name.is_empty() {
                n.name = name;
            }
        }
    }

    /// Removes note `i` (never the last one). Returns the removed id and whether the active
    /// note changed (so its text must be reloaded).
    pub fn delete(&mut self, i: usize) -> Option<(u64, bool)> {
        if self.notes.len() <= 1 || i >= self.notes.len() {
            return None;
        }
        let removed = self.notes.remove(i).id;
        let was_active = i == self.active;
        if i < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.notes.len() - 1);
        Some((removed, was_active))
    }
}

// ---------------------------------------------------------------- the editor

pub struct Notes {
    hwnd: Cell<HWND>,
    edit: Cell<HWND>,
    dir: PathBuf,
    meta: RefCell<Meta>,
    dirty: Cell<bool>,
}

impl Notes {
    pub fn new(dir: PathBuf) -> Notes {
        let meta = Meta::load(&dir);
        Notes {
            hwnd: Cell::new(HWND::default()),
            edit: Cell::new(HWND::default()),
            dir,
            meta: RefCell::new(meta),
            dirty: Cell::new(false),
        }
    }

    // ---- list accessors

    pub fn names(&self) -> Vec<String> {
        self.meta.borrow().notes.iter().map(|n| n.name.clone()).collect()
    }
    pub fn count(&self) -> usize {
        self.meta.borrow().notes.len()
    }
    pub fn active(&self) -> usize {
        self.meta.borrow().active
    }
    pub fn name(&self, i: usize) -> String {
        self.meta.borrow().notes.get(i).map(|n| n.name.clone()).unwrap_or_default()
    }

    /// Size in bytes of note `i` on disk (call `flush` first for the active note).
    pub fn len_on_disk(&self, i: usize) -> u64 {
        let id = self.meta.borrow().notes.get(i).map(|n| n.id);
        id.and_then(|id| std::fs::metadata(note_path(&self.dir, id)).ok()).map(|m| m.len()).unwrap_or(0)
    }

    fn active_path(&self) -> PathBuf {
        let m = self.meta.borrow();
        note_path(&self.dir, m.notes[m.active].id)
    }

    // ---- list mutations (each persists)

    pub fn select(&self, i: usize) {
        {
            let m = self.meta.borrow();
            if i >= m.notes.len() || i == m.active {
                return;
            }
        }
        self.flush();
        {
            let mut m = self.meta.borrow_mut();
            m.active = i;
            let _ = m.save(&self.dir);
        }
        self.reload();
    }

    /// Creates and switches to a new empty note. Returns false if the limit is reached.
    pub fn add(&self) -> bool {
        if self.count() >= MAX_NOTES {
            return false;
        }
        self.flush();
        {
            let mut m = self.meta.borrow_mut();
            m.add();
            let _ = write_atomic(&note_path(&self.dir, m.notes[m.active].id), b"");
            let _ = m.save(&self.dir);
        }
        self.reload();
        true
    }

    pub fn rename(&self, i: usize, name: &str) {
        let mut m = self.meta.borrow_mut();
        m.rename(i, name);
        let _ = m.save(&self.dir);
    }

    /// Deletes note `i`; the last remaining note is cleared instead.
    pub fn delete(&self, i: usize) {
        self.flush();
        let removed = self.meta.borrow_mut().delete(i);
        match removed {
            Some((id, was_active)) => {
                let _ = std::fs::remove_file(note_path(&self.dir, id));
                let _ = self.meta.borrow().save(&self.dir);
                if was_active {
                    self.reload();
                }
            }
            None => {
                let _ = write_atomic(&self.active_path(), b"");
                self.reload();
            }
        }
    }

    // ---- the editor window

    pub fn hwnd(&self) -> HWND {
        self.hwnd.get()
    }

    pub fn edit_hwnd(&self) -> HWND {
        self.edit.get()
    }

    fn created(&self) -> bool {
        !self.hwnd.get().0.is_null()
    }

    pub fn is_visible(&self) -> bool {
        self.created() && unsafe { IsWindowVisible(self.hwnd.get()).as_bool() }
    }

    pub fn has_focus(&self) -> bool {
        self.created() && unsafe { GetFocus() == self.hwnd.get() }
    }

    /// Creates the edit control on first use and loads the active note.
    fn ensure(&self, owner: HWND) -> bool {
        if self.created() {
            return true;
        }
        unsafe {
            let Some(h) = make_richedit(owner, WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | ES_NOHIDESEL) as u32)) else {
                return false;
            };
            self.hwnd.set(h);
            send(h, EM_SETTEXTMODE, TM_PLAINTEXT.0 as usize, 0);
            send(h, EM_SETBKGNDCOLOR, 0, BG as isize);
            let _ = SetWindowTheme(h, w!("DarkMode_Explorer"), PCWSTR::null());
            apply_font(h, 210, "Segoe UI");
            apply_line_spacing(h);
            self.load_text_into(h);
            // Only now start listening for edits, so the initial load is not "a change".
            send(h, EM_SETEVENTMASK, 0, ENM_CHANGE as isize);
            let _ = SetWindowSubclass(h, Some(subclass), 1, 0);
            true
        }
    }

    unsafe fn load_text_into(&self, h: HWND) {
        unsafe {
            let text = std::fs::read_to_string(self.active_path()).unwrap_or_default();
            let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\n', "\r\n");
            let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            let mask = send(h, EM_GETEVENTMASK, 0, 0).0;
            send(h, EM_SETEVENTMASK, 0, 0); // loading is not an edit
            let _ = SetWindowTextW(h, PCWSTR(wide.as_ptr()));
            send(h, EM_EMPTYUNDOBUFFER, 0, 0);
            send(h, EM_SETEVENTMASK, 0, mask);
            self.dirty.set(false);
        }
    }

    /// Replaces the editor contents with the active note.
    fn reload(&self) {
        if self.created() {
            unsafe { self.load_text_into(self.hwnd.get()) };
        }
    }

    /// Shows the notepad at the given screen rectangle (px).
    pub fn show(&self, owner: HWND, x: i32, y: i32, w: i32, h: i32, dpi: u32) {
        if !self.ensure(owner) {
            return;
        }
        let hwnd = self.hwnd.get();
        unsafe {
            let m = (9.5 * dpi as f32 / 96.0) as isize;
            send(hwnd, EM_SETMARGINS, (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize, m | (m << 16));
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    pub fn hide(&self) {
        if self.created() {
            self.flush();
            unsafe {
                let _ = ShowWindow(self.hwnd.get(), SW_HIDE);
            }
        }
    }

    pub fn focus(&self) {
        if self.is_visible() {
            unsafe {
                let h = self.hwnd.get();
                let _ = SetForegroundWindow(h);
                let _ = SetFocus(Some(h));
            }
        }
    }

    pub fn mark_dirty(&self) {
        self.dirty.set(true);
    }

    /// Writes the active note to disk if it changed (atomic replace).
    pub fn flush(&self) {
        if !self.dirty.get() || !self.created() {
            return;
        }
        unsafe {
            let h = self.hwnd.get();
            let len = GetWindowTextLengthW(h);
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(h, &mut buf);
            let text = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
            let text = text.replace("\r\n", "\n").replace('\r', "\n").replace('\n', "\r\n");
            if write_atomic(&self.active_path(), text.as_bytes()).is_ok() {
                self.dirty.set(false);
            }
        }
    }

    // ---- inline rename box

    /// Shows a one-line editor over a tab's label. Returns false if it could not be created.
    ///
    /// The rectangle is just the text strip, one line tall; the rounded box around it is drawn by
    /// the pill itself (`notes_view`) in the same colour, so the label is vertically centred in a
    /// proper pill instead of hugging the top of a plain rectangle.
    pub fn begin_rename(&self, owner: HWND, x: i32, y: i32, w: i32, h: i32, text: &str) -> bool {
        unsafe {
            if self.edit.get().0.is_null() {
                let Some(e) = make_richedit(owner, WINDOW_STYLE(ES_AUTOHSCROLL as u32)) else { return false };
                send(e, EM_SETTEXTMODE, TM_PLAINTEXT.0 as usize, 0);
                send(e, EM_SETBKGNDCOLOR, 0, EDIT_BG as isize);
                // same face and size as the tab label, so the text does not change look or jump
                apply_font(e, 195, "Segoe UI Semibold");
                let _ = SetWindowSubclass(e, Some(edit_subclass), 2, 0);
                self.edit.set(e);
            }
            let e = self.edit.get();
            let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            let _ = SetWindowTextW(e, PCWSTR(wide.as_ptr()));
            send(e, EM_SETMARGINS, (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize, 0);
            let _ = SetWindowPos(e, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            let _ = SetForegroundWindow(e);
            let _ = SetFocus(Some(e));
            send(e, EM_SETSEL_MSG, 0, -1); // select all
            true
        }
    }

    pub fn edit_text(&self) -> String {
        let e = self.edit.get();
        if e.0.is_null() {
            return String::new();
        }
        unsafe {
            let len = GetWindowTextLengthW(e);
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(e, &mut buf);
            String::from_utf16_lossy(&buf[..n.max(0) as usize])
        }
    }

    pub fn hide_edit(&self) {
        let e = self.edit.get();
        if !e.0.is_null() {
            unsafe {
                let _ = ShowWindow(e, SW_HIDE);
            }
        }
    }
}

unsafe fn make_richedit(owner: HWND, extra: WINDOW_STYLE) -> Option<HWND> {
    unsafe {
        let _ = LoadLibraryW(w!("Msftedit.dll"));
        let hmod = GetModuleHandleW(None).ok()?;
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("RICHEDIT50W"),
            w!(""),
            WS_POPUP | extra,
            0,
            0,
            10,
            10,
            Some(owner),
            None,
            Some(HINSTANCE(hmod.0)),
            None,
        )
        .ok()
    }
}

/// Sets the face, size and (light-on-dark) colour of everything in the control. Weight comes from
/// the face itself ("Segoe UI Semibold"), never from the bold flag.
unsafe fn apply_font(h: HWND, height_twips: i32, face: &str) {
    unsafe {
        let mut cf = CHARFORMAT2W {
            Base: CHARFORMATW {
                cbSize: size_of::<CHARFORMAT2W>() as u32,
                dwMask: CFM_COLOR | CFM_FACE | CFM_SIZE | CFM_BOLD,
                dwEffects: CFE_EFFECTS(0),
                yHeight: height_twips,
                crTextColor: COLORREF(FG),
                ..Default::default()
            },
            ..Default::default()
        };
        for (i, c) in face.encode_utf16().take(31).enumerate() {
            cf.Base.szFaceName[i] = c;
        }
        let p = &cf as *const CHARFORMAT2W as isize;
        send(h, EM_SETCHARFORMAT, SCF_DEFAULT as usize, p);
        send(h, EM_SETCHARFORMAT, SCF_ALL as usize, p);
    }
}

/// 1.5x line spacing, matching the airy look of the design.
unsafe fn apply_line_spacing(h: HWND) {
    unsafe {
        let pf = PARAFORMAT2 {
            Base: PARAFORMAT {
                cbSize: size_of::<PARAFORMAT2>() as u32,
                dwMask: PFM_LINESPACING,
                ..Default::default()
            },
            bLineSpacingRule: 1, // one and a half lines
            ..Default::default()
        };
        send(h, EM_SETPARAFORMAT_MSG, 0, &pf as *const PARAFORMAT2 as isize);
    }
}

unsafe fn send(h: HWND, msg: u32, wp: usize, lp: isize) -> LRESULT {
    unsafe { SendMessageW(h, msg, Some(WPARAM(wp)), Some(LPARAM(lp))) }
}

fn key_down(vk: i32) -> bool {
    unsafe { GetKeyState(vk) < 0 }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_KEYDOWN => {
            let ctrl = key_down(0x11);
            match wp.0 as u16 {
                0x1B => {
                    // Esc closes the pill and hands focus back to the previous window.
                    crate::app::with_app(|a| a.collapse());
                    return LRESULT(0);
                }
                0x09 if ctrl => {
                    let dir = if key_down(0x10) { -1 } else { 1 };
                    crate::app::with_app(|a| a.cycle_note(dir));
                    return LRESULT(0);
                }
                0x4E if ctrl && !key_down(0x12) => {
                    crate::app::with_app(|a| a.add_note());
                    return LRESULT(0);
                }
                0x57 if ctrl && !key_down(0x12) => {
                    // Ctrl+W closes (deletes) the note being edited; asks first if it has text
                    crate::app::with_app(|a| a.close_note());
                    return LRESULT(0);
                }
                _ => {}
            }
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            // Depending on Windows' "scroll inactive windows" setting, wheel messages go to the
            // focused window rather than the one under the pointer. When the pointer is over the
            // pill's own chrome (the note tabs), not over this editor, hand the scroll over to it.
            let x = (lp.0 & 0xFFFF) as i16 as i32;
            let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut rc = RECT::default();
            unsafe {
                let _ = GetWindowRect(hwnd, &mut rc);
            }
            if x < rc.left || x >= rc.right || y < rc.top || y >= rc.bottom {
                let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16;
                let horizontal = msg == WM_MOUSEHWHEEL;
                crate::app::with_app(|a| if horizontal { a.on_hwheel(delta) } else { a.on_wheel(delta) });
                return LRESULT(0);
            }
        }
        WM_SETFOCUS => {
            crate::app::with_app(|a| a.on_notes_focus());
        }
        WM_KILLFOCUS => {
            crate::app::with_app(|a| a.post_focus_check());
        }
        WM_NCDESTROY => unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(subclass), id);
        },
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn edit_subclass(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_KEYDOWN if wp.0 as u16 == 0x0D => {
            crate::app::with_app(|a| a.finish_rename(true));
            return LRESULT(0);
        }
        WM_KEYDOWN if wp.0 as u16 == 0x1B => {
            crate::app::with_app(|a| a.finish_rename(false));
            return LRESULT(0);
        }
        // swallow the characters Enter/Esc would also produce (avoids the error beep)
        WM_CHAR if wp.0 == 0x0D || wp.0 == 0x1B => return LRESULT(0),
        WM_KILLFOCUS => {
            crate::app::with_app(|a| a.finish_rename(true));
        }
        WM_NCDESTROY => unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(edit_subclass), id);
        },
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("notch-notes-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn fresh_install_has_one_quick_note() {
        let d = tmp("fresh");
        let m = Meta::load(&d);
        assert_eq!(m.notes.len(), 1);
        assert_eq!(m.notes[0].name, "Quick note");
        assert_eq!(m.active, 0);
        assert!(d.join("notes.json").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn legacy_notes_txt_is_migrated_into_first_note() {
        let d = tmp("legacy");
        std::fs::write(d.join("notes.txt"), "my old note").unwrap();
        let m = Meta::load(&d);
        let moved = std::fs::read_to_string(note_path(&d, m.notes[0].id)).unwrap();
        assert_eq!(moved, "my old note");
        assert!(!d.join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn add_picks_unique_names_and_becomes_active() {
        let mut m = Meta::fresh();
        assert_eq!(m.add(), 1);
        assert_eq!(m.notes[1].name, "Note 2");
        m.rename(1, "Today");
        m.add();
        assert_eq!(m.notes[2].name, "Note 3");
        assert_eq!(m.active, 2);
        // ids never repeat
        let ids: Vec<u64> = m.notes.iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn rename_trims_ignores_empty_and_caps_length() {
        let mut m = Meta::fresh();
        m.rename(0, "  Standup  ");
        assert_eq!(m.notes[0].name, "Standup");
        m.rename(0, "   ");
        assert_eq!(m.notes[0].name, "Standup");
        m.rename(0, &"x".repeat(100));
        assert_eq!(m.notes[0].name.chars().count(), 40);
        m.rename(9, "nope"); // out of range is a no-op
    }

    #[test]
    fn delete_keeps_active_pointing_at_the_same_note() {
        let mut m = Meta::fresh();
        m.add();
        m.add(); // [1,2,3], active = 2 (id 3)
        let (id, was_active) = m.delete(0).unwrap();
        assert_eq!(id, 1);
        assert!(!was_active);
        assert_eq!(m.notes[m.active].id, 3);

        let (id, was_active) = m.delete(m.active).unwrap(); // delete the active one
        assert_eq!(id, 3);
        assert!(was_active);
        assert!(m.active < m.notes.len());
    }

    #[test]
    fn the_last_note_cannot_be_removed() {
        let mut m = Meta::fresh();
        assert!(m.delete(0).is_none());
        assert_eq!(m.notes.len(), 1);
    }

    #[test]
    fn corrupt_or_empty_metadata_falls_back_to_a_fresh_list() {
        let d = tmp("corrupt");
        std::fs::write(d.join("notes.json"), "not json").unwrap();
        assert_eq!(Meta::load(&d).notes.len(), 1);
        std::fs::write(d.join("notes.json"), r#"{"next_id":5,"active":0,"notes":[]}"#).unwrap();
        assert_eq!(Meta::load(&d).notes.len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn utf8_bom_is_accepted() {
        let d = tmp("bom");
        let json = "\u{feff}{\"next_id\":4,\"active\":1,\"notes\":[{\"id\":1,\"name\":\"Quick note\"},{\"id\":2,\"name\":\"Today\"},{\"id\":3,\"name\":\"Snippets\"}]}";
        std::fs::write(d.join("notes.json"), json).unwrap();
        let m = Meta::load(&d);
        assert_eq!(m.notes.len(), 3);
        assert_eq!(m.notes[2].name, "Snippets");
        assert_eq!(m.active, 1);
        assert!(!d.join("notes.json.bad").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unreadable_metadata_is_kept_for_recovery_not_overwritten() {
        let d = tmp("keepbad");
        std::fs::write(d.join("notes.json"), "{ half written").unwrap();
        let m = Meta::load(&d);
        assert_eq!(m.notes.len(), 1); // started fresh...
        assert_eq!(std::fs::read_to_string(d.join("notes.json.bad")).unwrap(), "{ half written"); // ...but kept the old file
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn state_round_trips_and_active_is_clamped() {
        let d = tmp("roundtrip");
        let mut m = Meta::load(&d);
        m.add();
        m.rename(0, "Today");
        m.save(&d).unwrap();
        let m2 = Meta::load(&d);
        assert_eq!(m2.notes.len(), 2);
        assert_eq!(m2.notes[0].name, "Today");
        assert_eq!(m2.active, 1);
        assert!(m2.next_id > 2);
        std::fs::write(d.join("notes.json"), r#"{"next_id":3,"active":99,"notes":[{"id":1,"name":"A"}]}"#).unwrap();
        assert_eq!(Meta::load(&d).active, 0);
        let _ = std::fs::remove_dir_all(&d);
    }
}
