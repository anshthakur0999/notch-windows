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
use crate::ui::theme::Palette;
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

/// Colours and text size of the two native editors (the note and the rename box), taken from the
/// pill's palette so they match whichever theme is on. Changed live from the tray menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteStyle {
    /// 0xRRGGBB: the note's background (the panel colour) and text, and the rename box's fill.
    pub bg: u32,
    pub fg: u32,
    pub tile: u32,
    /// Dark scroll bar (dark theme) or the normal one.
    pub dark: bool,
    /// Text size multiplier for the note (the rename box matches the tab labels and stays fixed).
    pub scale: f32,
}

impl NoteStyle {
    pub fn new(pal: &Palette, scale: f32) -> NoteStyle {
        NoteStyle { bg: pal.body, fg: pal.editor_text, tile: pal.tile, dark: !pal.light, scale }
    }
}

impl Default for NoteStyle {
    fn default() -> Self {
        NoteStyle::new(&Palette::default(), 1.0)
    }
}

/// 0xRRGGBB -> COLORREF (0x00BBGGRR).
fn cref(rgb: u32) -> u32 {
    ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)
}

/// Editor text height in twips (1/20 pt) for the note: 10.5 pt, scaled.
fn note_twips(scale: f32) -> i32 {
    (210.0 * scale).round() as i32
}

/// The tab row scrolls, so this is only a sanity limit (each note is one small file).
pub const MAX_NOTES: usize = 30;
const DEFAULT_NAME: &str = "Quick note";
/// The "Quick note" is permanent: the note with this id (the first one ever made) cannot be
/// deleted, even if it is renamed, and it is put back if a list is missing it.
pub const QUICK_NOTE_ID: u64 = 1;

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

/// Reads a note. A file that is not valid UTF-8 (edited elsewhere, saved as ANSI or UTF-16) is
/// copied to `<id>.bad` first and shown with the bad bytes replaced, so the original survives
/// the next save. A missing file is an empty note.
fn read_note(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else { return String::new() };
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => {
            let bad = path.with_extension("bad");
            if !bad.exists() {
                let _ = std::fs::copy(path, bad);
            }
            String::from_utf8_lossy(e.as_bytes()).into_owned()
        }
    }
}

impl Meta {
    fn fresh() -> Meta {
        // the Quick note takes id 1, so the next free id is 2
        Meta { next_id: 2, active: 0, notes: vec![NoteMeta { id: QUICK_NOTE_ID, name: DEFAULT_NAME.into() }] }
    }

    /// Position of the Quick note, if the list has it.
    pub fn quick(&self) -> Option<usize> {
        self.notes.iter().position(|n| n.id == QUICK_NOTE_ID)
    }

    /// Puts the Quick note back, empty and first, if the list has none (a list saved before it
    /// became permanent). `active` keeps pointing at the same note. Returns true if it was added.
    fn ensure_quick(&mut self) -> bool {
        if self.quick().is_some() {
            return false;
        }
        self.notes.insert(0, NoteMeta { id: QUICK_NOTE_ID, name: DEFAULT_NAME.into() });
        self.active += 1;
        true
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
        if m.ensure_quick() {
            let p = note_path(dir, QUICK_NOTE_ID);
            if !p.exists() {
                let _ = write_atomic(&p, b"");
            }
            let _ = m.save(dir);
        }
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

    /// Removes note `i` (never the last one, and never the Quick note). Returns the removed id
    /// and whether the active note changed (so its text must be reloaded).
    pub fn delete(&mut self, i: usize) -> Option<(u64, bool)> {
        if self.notes.len() <= 1 || i >= self.notes.len() || self.quick() == Some(i) {
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
    style: Cell<NoteStyle>,
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
            style: Cell::new(NoteStyle::default()),
        }
    }

    /// Sets the editors' colours and text size. Editors that do not exist yet pick them up when
    /// they are created; existing ones are restyled now.
    pub fn set_style(&self, style: NoteStyle) {
        self.style.set(style);
        unsafe {
            let h = self.hwnd.get();
            if !h.0.is_null() {
                style_note(h, &style);
            }
            let e = self.edit.get();
            if !e.0.is_null() {
                style_rename(e, &style);
            }
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
        // If the edits could not be written, stay on this note: reloading would throw them away.
        if !self.flush() {
            return;
        }
        {
            let mut m = self.meta.borrow_mut();
            m.active = i;
            let _ = m.save(&self.dir);
        }
        self.reload();
    }

    /// Creates and switches to a new empty note. Returns false if the limit is reached, or if
    /// the current note could not be saved first.
    pub fn add(&self) -> bool {
        if self.count() >= MAX_NOTES || !self.flush() {
            return false;
        }
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

    /// Position of the Quick note, the one note that cannot be deleted.
    pub fn quick(&self) -> Option<usize> {
        self.meta.borrow().quick()
    }

    /// Deletes note `i`. Does nothing for the Quick note.
    pub fn delete(&self, i: usize) {
        if self.quick() == Some(i) {
            return;
        }
        self.flush();
        let removed = self.meta.borrow_mut().delete(i);
        if let Some((id, was_active)) = removed {
            let _ = std::fs::remove_file(note_path(&self.dir, id));
            let _ = self.meta.borrow().save(&self.dir);
            if was_active {
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

    /// Creates the (hidden) edit control now if it does not exist yet, so the first time the
    /// notepad is shown nothing has to be built in the middle of the opening animation.
    pub fn prepare(&self, owner: HWND) {
        let _ = self.ensure(owner);
    }

    /// Creates the edit control on first use and loads the active note.
    fn ensure(&self, owner: HWND) -> bool {
        if self.created() {
            return true;
        }
        let started = std::time::Instant::now();
        let ok = self.create_editor(owner);
        crate::log(&format!("notepad editor created in {:.1} ms", started.elapsed().as_secs_f32() * 1000.0));
        ok
    }

    fn create_editor(&self, owner: HWND) -> bool {
        unsafe {
            let Some(h) = make_richedit(owner, WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | ES_NOHIDESEL) as u32)) else {
                return false;
            };
            self.hwnd.set(h);
            send(h, EM_SETTEXTMODE, TM_PLAINTEXT.0 as usize, 0);
            // The default limit is 32,767 characters: longer pastes and typing are dropped silently.
            send(h, EM_EXLIMITTEXT, 0, 0x7FFF_FFFE);
            style_note(h, &self.style.get());
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
            let text = read_note(&self.active_path());
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

    /// Writes the active note to disk if it changed (atomic replace). Returns false if the text
    /// is still unsaved because the write failed.
    pub fn flush(&self) -> bool {
        if !self.dirty.get() || !self.created() {
            return true;
        }
        unsafe {
            let h = self.hwnd.get();
            let len = GetWindowTextLengthW(h);
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(h, &mut buf);
            let text = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
            let text = text.replace("\r\n", "\n").replace('\r', "\n").replace('\n', "\r\n");
            let ok = write_atomic(&self.active_path(), text.as_bytes()).is_ok();
            if ok {
                self.dirty.set(false);
            } else {
                crate::log("could not save the note; keeping it in the editor");
            }
            ok
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
                style_rename(e, &self.style.get());
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

/// Colours, scroll bar and text of the note editor.
unsafe fn style_note(h: HWND, s: &NoteStyle) {
    unsafe {
        send(h, EM_SETBKGNDCOLOR, 0, cref(s.bg) as isize);
        let theme = if s.dark { w!("DarkMode_Explorer") } else { w!("Explorer") };
        let _ = SetWindowTheme(h, theme, PCWSTR::null());
        apply_font(h, note_twips(s.scale), "Segoe UI", s.fg);
    }
}

/// The rename box's fill and text. Same face and size as the tab label, so the text does not
/// change look or jump.
unsafe fn style_rename(e: HWND, s: &NoteStyle) {
    unsafe {
        send(e, EM_SETBKGNDCOLOR, 0, cref(s.tile) as isize);
        apply_font(e, 195, "Segoe UI Semibold", s.fg);
    }
}

/// Sets the face, size and colour (0xRRGGBB) of everything in the control. Weight comes from
/// the face itself ("Segoe UI Semibold"), never from the bold flag.
unsafe fn apply_font(h: HWND, height_twips: i32, face: &str, text_rgb: u32) {
    unsafe {
        let mut cf = CHARFORMAT2W {
            Base: CHARFORMATW {
                cbSize: size_of::<CHARFORMAT2W>() as u32,
                dwMask: CFM_COLOR | CFM_FACE | CFM_SIZE | CFM_BOLD,
                dwEffects: CFE_EFFECTS(0),
                yHeight: height_twips,
                crTextColor: COLORREF(cref(text_rgb)),
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
        let (id, was_active) = m.delete(1).unwrap(); // a note before the active one
        assert_eq!(id, 2);
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
    fn the_quick_note_cannot_be_removed_even_renamed_or_among_many() {
        let mut m = Meta::fresh();
        m.add();
        m.add(); // [1,2,3]
        m.rename(0, "Work"); // a new name does not make it deletable
        assert_eq!(m.quick(), Some(0));
        assert!(m.delete(0).is_none());
        assert_eq!(m.notes.len(), 3);
        assert!(m.delete(1).is_some(), "the other notes can still be deleted");
        assert!(m.delete(1).is_some());
        assert!(m.delete(0).is_none());
        assert_eq!(m.notes.len(), 1);
    }

    #[test]
    fn a_list_saved_without_the_quick_note_gets_it_back_first_and_empty() {
        let d = tmp("noquick");
        // what an older version left behind: the Quick note was deleted, only "Note 2" is left
        std::fs::write(d.join("notes.json"), r#"{"next_id":39,"active":0,"notes":[{"id":38,"name":"Note 2"}]}"#).unwrap();
        std::fs::create_dir_all(d.join("notes")).unwrap();
        std::fs::write(note_path(&d, 38), "keep me").unwrap();
        let m = Meta::load(&d);
        assert_eq!(m.notes.len(), 2);
        assert_eq!((m.notes[0].id, m.notes[0].name.as_str()), (QUICK_NOTE_ID, "Quick note"));
        assert_eq!(m.notes[m.active].id, 38, "the open note is still the open note");
        assert_eq!(m.next_id, 39);
        assert_eq!(std::fs::read_to_string(note_path(&d, QUICK_NOTE_ID)).unwrap(), "");
        assert_eq!(std::fs::read_to_string(note_path(&d, 38)).unwrap(), "keep me");
        // it was saved, so the next start adds nothing more
        assert_eq!(Meta::load(&d).notes.len(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn deleting_the_quick_note_leaves_its_text_and_the_list_alone() {
        let d = tmp("keepquick");
        let n = Notes::new(d.clone());
        std::fs::write(note_path(&d, QUICK_NOTE_ID), "my scratch text").unwrap();
        assert!(n.add()); // a second note, so a delete is not refused just for being the last
        n.delete(0);
        assert_eq!(n.count(), 2);
        assert_eq!(std::fs::read_to_string(note_path(&d, QUICK_NOTE_ID)).unwrap(), "my scratch text");
        n.delete(1); // another note is deleted as before
        assert_eq!(n.count(), 1);
        let _ = std::fs::remove_dir_all(&d);
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

    #[test]
    fn a_note_that_is_not_utf8_is_backed_up_before_it_can_be_overwritten() {
        let d = tmp("ansi");
        std::fs::create_dir_all(d.join("notes")).unwrap();
        let p = note_path(&d, 1);
        std::fs::write(&p, b"caf\xE9 au lait").unwrap(); // "café" saved as Windows-1252
        let text = read_note(&p);
        assert!(text.starts_with("caf") && text.ends_with(" au lait"), "{text:?}");
        assert_eq!(std::fs::read(p.with_extension("bad")).unwrap(), b"caf\xE9 au lait");
        std::fs::write(&p, "plain utf-8 é").unwrap();
        assert_eq!(read_note(&p), "plain utf-8 é");
        assert_eq!(read_note(&note_path(&d, 9)), "", "a missing file is an empty note");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// These two need a real RichEdit control, so they create one (never shown).
    #[test]
    fn the_editor_takes_long_notes_and_a_failed_save_keeps_the_edits() {
        let d = tmp("editor");
        let n = Notes::new(d.clone());
        assert!(n.ensure(HWND::default()));
        const EM_GETLIMITTEXT: u32 = 0x00D5;
        let limit = unsafe { send(n.hwnd(), EM_GETLIMITTEXT, 0, 0) }.0;
        assert!(limit > 1_000_000, "the default 32,767-character limit is still in place: {limit}");

        assert!(n.add()); // two notes; the new one (id 2) is active
        let blocker = d.join("notes").join("2.tmp");
        std::fs::create_dir(&blocker).unwrap(); // the temp file for note 2 can no longer be created
        n.mark_dirty();
        assert!(!n.flush());
        n.select(0);
        assert_eq!(n.active(), 1, "switching would have thrown the unsaved text away");
        assert!(!n.add());
        assert_eq!(n.count(), 2);

        std::fs::remove_dir(&blocker).unwrap();
        n.select(0);
        assert_eq!(n.active(), 0, "once saving works again the switch goes through");
        unsafe {
            let _ = DestroyWindow(n.hwnd());
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
