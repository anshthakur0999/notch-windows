//! The file shelf: files held while you work, kept as links (paths), never copies, so a 4 GB video
//! costs nothing. Saved in `shelf.json`; an entry clears itself after the `shelf_hours` setting.
//!
//! shortcut: a file's size and whether it still exists are read when the panel opens (`refresh`),
//! not watched, and network paths are not looked at at all (a dead share would freeze the pill);
//! upgrade if people keep files on shares.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Most files held at once; the oldest make room. The grid is made for a handful, not an archive.
pub const MAX_FILES: usize = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    Folder,
    File,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub path: String,
    /// Unix seconds when it was put on the shelf.
    pub added: u64,
    /// Read by `refresh`, never saved.
    #[serde(skip)]
    pub missing: bool,
    #[serde(skip)]
    pub folder: bool,
    #[serde(skip)]
    pub size: u64,
    /// Ticked in the panel: a drag takes the ticked tiles.
    #[serde(skip)]
    pub selected: bool,
}

impl Item {
    fn new(path: String, added: u64) -> Item {
        Item { path, added, missing: false, folder: false, size: 0, selected: false }
    }

    pub fn name(&self) -> &str {
        let p = self.path.trim_end_matches(['\\', '/']);
        p.rsplit(['\\', '/']).next().filter(|n| !n.is_empty()).unwrap_or(p)
    }

    /// "PNG", "PDF"... for a file, "Folder" for a folder, "File" when there is no extension.
    pub fn type_label(&self) -> String {
        if self.folder {
            return "Folder".into();
        }
        match Path::new(self.name()).extension().and_then(|e| e.to_str()) {
            Some(e) if !e.is_empty() && e.len() <= 5 => e.to_ascii_uppercase(),
            _ => "File".into(),
        }
    }

    pub fn kind(&self) -> Kind {
        if self.folder {
            return Kind::Folder;
        }
        const IMAGES: [&str; 10] = ["PNG", "JPG", "JPEG", "GIF", "BMP", "WEBP", "TIFF", "ICO", "SVG", "HEIC"];
        if IMAGES.contains(&self.type_label().as_str()) { Kind::Image } else { Kind::File }
    }
}

/// "48 KB", "1.4 MB", "2.1 GB".
pub fn size_text(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Shelf {
    /// Newest first.
    items: Vec<Item>,
}

fn same_path(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl Shelf {
    /// Reads `shelf.json` from `dir`. A file that cannot be read is kept as `shelf.json.bad` and
    /// the shelf starts empty.
    pub fn load(dir: &Path) -> Shelf {
        let path = dir.join("shelf.json");
        let Ok(text) = std::fs::read_to_string(&path) else { return Shelf::default() };
        serde_json::from_str(&text).unwrap_or_else(|_| {
            let _ = std::fs::rename(&path, dir.join("shelf.json.bad"));
            Shelf::default()
        })
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        crate::store::write_atomic(&dir.join("shelf.json"), &bytes)
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[cfg(test)]
    pub fn missing(&self) -> usize {
        self.items.iter().filter(|i| i.missing).count()
    }

    #[cfg(test)]
    pub fn selected(&self) -> usize {
        self.items.iter().filter(|i| i.selected).count()
    }

    /// Puts `paths` at the front, newest first. A file already held moves to the front instead of
    /// doubling up. Returns how many were not on the shelf before.
    pub fn add(&mut self, paths: &[String], now: u64) -> usize {
        let mut added = 0;
        for p in paths.iter().rev() {
            let old = self.items.iter().position(|i| same_path(&i.path, p));
            if old.is_none() {
                added += 1;
            }
            let item = old.map(|k| self.items.remove(k)).unwrap_or_else(|| Item::new(p.clone(), now));
            self.items.insert(0, item);
        }
        self.items.truncate(MAX_FILES);
        self.refresh();
        added
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.items.len() {
            self.items.remove(i);
        }
    }

    pub fn remove_selected(&mut self) {
        self.items.retain(|i| !i.selected);
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn toggle(&mut self, i: usize) {
        if let Some(it) = self.items.get_mut(i) {
            it.selected = !it.selected;
        }
    }

    /// Drops what has been held longer than `hours` (0 = keep until cleared). True if any went.
    pub fn prune(&mut self, hours: u32, now: u64) -> bool {
        let n = self.items.len();
        if hours > 0 {
            let keep_from = now.saturating_sub(hours as u64 * 3600);
            self.items.retain(|i| i.added > keep_from);
        }
        self.items.len() != n
    }

    /// Reads each file's size and whether it is still there. Network paths are left alone.
    pub fn refresh(&mut self) {
        for it in &mut self.items {
            if it.path.starts_with("\\\\") {
                continue;
            }
            match std::fs::metadata(&it.path) {
                Ok(m) => (it.missing, it.folder, it.size) = (false, m.is_dir(), if m.is_dir() { 0 } else { m.len() }),
                Err(e) => it.missing = e.kind() == std::io::ErrorKind::NotFound,
            }
        }
    }

    /// The files a drag that starts on tile `i` takes: the ticked tiles if `i` is one of them,
    /// else just `i`. Files that are gone are left out.
    pub fn drag_set(&self, i: usize) -> Vec<String> {
        let Some(start) = self.items.get(i) else { return Vec::new() };
        let takes = |x: &Item| !x.missing && if start.selected { x.selected } else { std::ptr::eq(x, start) };
        self.items.iter().filter(|x| takes(x)).map(|x| x.path.clone()).collect()
    }

    /// Everything that is still there, for "Drag all".
    pub fn all_present(&self) -> Vec<String> {
        self.items.iter().filter(|i| !i.missing).map(|i| i.path.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(s: &Shelf) -> Vec<&str> {
        s.items().iter().map(|i| i.path.as_str()).collect()
    }
    fn v(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }
    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("notch-shelf-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn newest_files_come_first_and_a_repeat_moves_up_instead_of_doubling() {
        let mut s = Shelf::default();
        assert_eq!(s.add(&v(&["C:\\a.txt", "C:\\b.txt"]), 10), 2);
        assert_eq!(paths(&s), ["C:\\a.txt", "C:\\b.txt"], "a drop keeps its own order");
        assert_eq!(s.add(&v(&["C:\\c.txt", "c:\\B.TXT"]), 20), 1, "b was already there (any case)");
        assert_eq!(paths(&s), ["C:\\c.txt", "C:\\b.txt", "C:\\a.txt"], "b moves up and keeps its first spelling");
    }

    #[test]
    fn the_oldest_make_room_when_the_shelf_is_full() {
        let mut s = Shelf::default();
        for n in 0..MAX_FILES + 5 {
            s.add(&[format!("C:\\f{n}.txt")], n as u64);
        }
        assert_eq!(s.len(), MAX_FILES);
        assert_eq!(s.items()[0].path, format!("C:\\f{}.txt", MAX_FILES + 4));
        assert!(!paths(&s).contains(&"C:\\f0.txt"));
    }

    #[test]
    fn entries_clear_themselves_after_the_set_hours() {
        let mut s = Shelf::default();
        s.add(&v(&["C:\\old.txt"]), 1000);
        s.add(&v(&["C:\\new.txt"]), 1000 + 23 * 3600);
        let now = 1000 + 25 * 3600;
        assert!(!s.prune(0, now), "0 hours means keep until cleared");
        assert_eq!(s.len(), 2);
        assert!(s.prune(24, now));
        assert_eq!(paths(&s), ["C:\\new.txt"]);
        assert!(!s.prune(24, now), "nothing more to drop");
    }

    #[test]
    fn names_types_and_sizes_read_the_way_the_tiles_show_them() {
        let it = |p: &str| Item::new(p.into(), 0);
        assert_eq!(it("C:\\Users\\me\\Q4-report.pdf").name(), "Q4-report.pdf");
        assert_eq!(it("C:\\Users\\me\\Q4-report.pdf").type_label(), "PDF");
        assert_eq!(it("C:\\Users\\me\\notes").type_label(), "File");
        assert_eq!(it("D:\\").name(), "D:");
        assert_eq!(it("C:\\a\\shot.PNG").kind(), Kind::Image);
        assert_eq!(it("C:\\a\\assets.zip").kind(), Kind::File);
        let mut dir = it("C:\\a\\stuff\\");
        assert_eq!(dir.name(), "stuff", "a trailing slash does not hide the name");
        dir.folder = true;
        assert_eq!((dir.kind(), dir.type_label().as_str()), (Kind::Folder, "Folder"));
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(48 * 1024), "48 KB");
        assert_eq!(size_text(1_468_006), "1.4 MB");
        assert_eq!(size_text(11_900_000), "11.3 MB");
        assert_eq!(size_text(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn a_moved_or_deleted_file_reads_missing_and_stays_off_a_drag() {
        let d = temp("refresh");
        let (a, b) = (d.join("a.txt"), d.join("b.txt"));
        std::fs::write(&a, "hello").unwrap();
        std::fs::write(&b, "x").unwrap();
        let mut s = Shelf::default();
        s.add(&[a.to_string_lossy().into(), b.to_string_lossy().into(), d.to_string_lossy().into()], 1);
        assert_eq!((s.missing(), s.items()[0].size, s.items()[2].folder), (0, 5, true));
        std::fs::remove_file(&b).unwrap();
        s.refresh();
        assert_eq!(s.missing(), 1);
        assert!(s.items()[1].missing);
        // dragging the gone file takes nothing; "drag all" takes the rest
        assert!(s.drag_set(1).is_empty());
        assert_eq!(s.all_present().len(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_drag_takes_the_ticked_tiles_or_just_the_one_it_started_on() {
        let mut s = Shelf::default();
        s.add(&v(&["C:\\no1.txt", "C:\\no2.txt", "C:\\no3.txt"]), 1);
        s.items.iter_mut().for_each(|i| i.missing = false); // these paths need not exist
        assert_eq!(s.drag_set(1), ["C:\\no2.txt"], "nothing ticked: only that tile");
        s.toggle(0);
        s.toggle(2);
        assert_eq!(s.selected(), 2);
        assert_eq!(s.drag_set(0), ["C:\\no1.txt", "C:\\no3.txt"], "on a ticked tile: all ticked");
        assert_eq!(s.drag_set(1), ["C:\\no2.txt"], "on an unticked tile: just it");
        s.remove_selected();
        assert_eq!(paths(&s), ["C:\\no2.txt"]);
    }

    #[test]
    fn the_shelf_survives_a_restart_and_keeps_an_unreadable_file() {
        let d = temp("save");
        let mut s = Shelf::default();
        s.add(&v(&["C:\\kept.txt"]), 77);
        s.toggle(0);
        s.save(&d).unwrap();
        let back = Shelf::load(&d);
        assert_eq!((paths(&back), back.items()[0].added, back.selected()), (vec!["C:\\kept.txt"], 77, 0), "ticks are not saved");
        std::fs::write(d.join("shelf.json"), "{ broken").unwrap();
        assert_eq!(Shelf::load(&d).len(), 0);
        assert_eq!(std::fs::read_to_string(d.join("shelf.json.bad")).unwrap(), "{ broken");
        assert_eq!(Shelf::load(&temp("none")).len(), 0, "no file yet is an empty shelf");
        let _ = std::fs::remove_dir_all(&d);
    }
}
