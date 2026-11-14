//! Persistent clipboard history: index (`clips.json`) plus per-item payload files.
//!
//! The index only keeps short previews in memory. Full text lives in `clips\<id>.txt`,
//! images in `clips\<id>.png` with a small `clips\<id>_t.png` thumbnail. That keeps the
//! resident footprint tiny no matter how much has been copied.
//!
//! `items` is kept ordered for display: pinned items first, then unpinned, each group
//! newest-first.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Text,
    Image,
}

/// What a text entry looks like; drives the row icon and label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Sub {
    #[default]
    Plain,
    Link,
    Code,
}

impl Sub {
    /// Label shown under the entry ("Text", "Link", "Code", "Image").
    pub fn label(self, kind: Kind) -> &'static str {
        match (kind, self) {
            (Kind::Image, _) => "Image",
            (_, Sub::Plain) => "Text",
            (_, Sub::Link) => "Link",
            (_, Sub::Code) => "Code",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipItem {
    pub id: u64,
    pub kind: Kind,
    #[serde(default)]
    pub sub: Sub,
    pub hash: u64,
    /// Single-line, searchable preview for text items (first few hundred characters).
    pub preview: String,
    pub chars: u32,
    pub width: u32,
    pub height: u32,
    /// Bytes on disk used by an image item (png + thumbnail).
    pub bytes: u64,
    pub pinned: bool,
    /// Unix seconds of the last time this item was copied.
    pub ts: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    next_id: u64,
    items: Vec<ClipItem>,
}

pub struct Store {
    dir: PathBuf,
    idx: Index,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AddResult {
    Added,
    /// An identical item already existed; it was moved to the front of its group.
    Touched(u64),
}

/// FNV-1a 64-bit. Good enough for de-duplication and has no dependencies.
pub fn hash64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Collapses whitespace and truncates to `max` chars for list display.
pub fn make_preview(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    let mut last_space = true;
    for c in text.chars() {
        let c = if c.is_whitespace() || c.is_control() { ' ' } else { c };
        if c == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        out.push(c);
        count += 1;
        if count >= max {
            break;
        }
    }
    out.trim_end().to_string()
}

/// Guesses whether copied text is a link, source code, or plain text.
pub fn classify(text: &str) -> Sub {
    let t = text.trim();
    if t.is_empty() {
        return Sub::Plain;
    }
    let lower = t.to_lowercase();
    if !t.chars().any(char::is_whitespace)
        && ["http://", "https://", "ftp://", "www."].iter().any(|p| lower.starts_with(p))
    {
        return Sub::Link;
    }
    if looks_like_code(t, &lower) {
        Sub::Code
    } else {
        Sub::Plain
    }
}

fn looks_like_code(t: &str, lower: &str) -> bool {
    const SQL: [&str; 6] = ["select ", "insert into ", "update ", "delete from ", "create table ", "alter table "];
    if SQL.iter().any(|k| lower.starts_with(k))
        && (lower.contains(" from ")
            || lower.contains(" set ")
            || lower.contains(" values")
            || lower.contains(" table ")
            || t.ends_with(';'))
    {
        return true;
    }
    let mut score = 0;
    if t.ends_with(';') || t.ends_with('{') || t.ends_with('}') {
        score += 1;
    }
    if ["=>", "->", "::", "==", "!=", "&&", "||", "();", "){", "() {", "</", "/>"].iter().any(|s| t.contains(s)) {
        score += 1;
    }
    const KEYWORDS: [&str; 13] = [
        "function ", "fn ", "def ", "class ", "import ", "#include", "const ", "let ", "var ", "return ",
        "public ", "private ", "console.log",
    ];
    if KEYWORDS.iter().any(|k| lower.starts_with(k) || lower.contains(&format!("\n{k}"))) {
        score += 2;
    }
    let lines: Vec<&str> = t.lines().collect();
    if lines.len() > 1
        && lines.iter().filter(|l| l.starts_with("    ") || l.starts_with('\t')).count() * 2 >= lines.len()
    {
        score += 1;
    }
    score >= 2
}

/// Human-friendly age: "just now", "2 min ago", "3 hr ago", "yesterday", "5 days ago", "2 wk ago".
pub fn ago(now: u64, ts: u64) -> String {
    let secs = now.saturating_sub(ts);
    if secs < 45 {
        return "just now".into();
    }
    if secs < 3600 {
        // rounded to the nearest minute, but 59 min 45 s is not yet "60 min" (it would fall
        // through to "0 hr ago")
        return format!("{} min ago", ((secs + 30) / 60).min(59));
    }
    let hours = secs / 3600;
    if hours < 24 {
        return format!("{hours} hr ago");
    }
    let days = secs / 86_400;
    if days == 1 {
        return "yesterday".into();
    }
    if days < 14 {
        return format!("{days} days ago");
    }
    let weeks = days / 7;
    if weeks < 52 {
        return format!("{weeks} wk ago");
    }
    format!("{} yr ago", (days / 365).max(1)) // day 364 is already past 51 weeks
}

/// Writes `bytes` to `path` atomically (temp file in the same folder, then rename).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Store {
    /// Opens (or creates) the store rooted at `dir`. A corrupt index starts a fresh one.
    pub fn open(dir: &Path) -> Store {
        let _ = std::fs::create_dir_all(dir.join("clips"));
        let idx = std::fs::read_to_string(dir.join("clips.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Index>(&t).ok())
            .unwrap_or_default();
        let mut s = Store { dir: dir.to_path_buf(), idx };
        // Drop index entries whose payload vanished.
        let missing: Vec<u64> = s
            .idx
            .items
            .iter()
            .filter(|i| !s.payload_path(i).exists())
            .map(|i| i.id)
            .collect();
        s.idx.items.retain(|i| !missing.contains(&i.id));
        s
    }

    pub fn items(&self) -> &[ClipItem] {
        &self.idx.items
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.idx.items.len()
    }

    #[cfg(test)]
    pub fn get(&self, id: u64) -> Option<&ClipItem> {
        self.idx.items.iter().find(|i| i.id == id)
    }

    pub fn alloc_id(&mut self) -> u64 {
        self.idx.next_id += 1;
        self.idx.next_id
    }

    pub fn text_path(&self, id: u64) -> PathBuf {
        self.dir.join("clips").join(format!("{id}.txt"))
    }
    pub fn image_path(&self, id: u64) -> PathBuf {
        self.dir.join("clips").join(format!("{id}.png"))
    }
    pub fn thumb_path(&self, id: u64) -> PathBuf {
        self.dir.join("clips").join(format!("{id}_t.png"))
    }
    fn payload_path(&self, item: &ClipItem) -> PathBuf {
        match item.kind {
            Kind::Text => self.text_path(item.id),
            Kind::Image => self.image_path(item.id),
        }
    }

    fn pinned_count(&self) -> usize {
        self.idx.items.iter().take_while(|i| i.pinned).count()
    }

    pub fn find_hash(&self, kind: Kind, hash: u64) -> Option<u64> {
        self.idx
            .items
            .iter()
            .find(|i| i.kind == kind && i.hash == hash)
            .map(|i| i.id)
    }

    /// Adds `item`, or just refreshes the existing identical entry.
    pub fn add_or_touch(&mut self, item: ClipItem) -> AddResult {
        if let Some(existing) = self.find_hash(item.kind, item.hash) {
            self.touch(existing);
            return AddResult::Touched(existing);
        }
        let at = if item.pinned { 0 } else { self.pinned_count() };
        self.idx.items.insert(at, item);
        AddResult::Added
    }

    /// Moves an item to the front of its own (pinned / unpinned) group.
    pub fn touch(&mut self, id: u64) {
        if let Some(pos) = self.idx.items.iter().position(|i| i.id == id) {
            let mut it = self.idx.items.remove(pos);
            it.ts = now_secs();
            let at = if it.pinned { 0 } else { self.pinned_count() };
            self.idx.items.insert(at, it);
        }
    }

    pub fn toggle_pin(&mut self, id: u64) {
        if let Some(pos) = self.idx.items.iter().position(|i| i.id == id) {
            let mut it = self.idx.items.remove(pos);
            it.pinned = !it.pinned;
            let at = if it.pinned { 0 } else { self.pinned_count() };
            self.idx.items.insert(at, it);
        }
    }

    pub fn remove(&mut self, id: u64) -> Option<ClipItem> {
        let pos = self.idx.items.iter().position(|i| i.id == id)?;
        Some(self.idx.items.remove(pos))
    }

    /// Removes the oldest unpinned items until both limits hold. Pinned items are exempt.
    /// Returns the removed items so the caller can delete their files.
    pub fn evict(&mut self, max_items: usize, max_image_bytes: u64) -> Vec<ClipItem> {
        let mut removed = Vec::new();
        loop {
            let unpinned = self.idx.items.iter().filter(|i| !i.pinned).count();
            let image_bytes: u64 = self
                .idx
                .items
                .iter()
                .filter(|i| !i.pinned && i.kind == Kind::Image)
                .map(|i| i.bytes)
                .sum();
            if unpinned <= max_items && image_bytes <= max_image_bytes {
                break;
            }
            // Oldest unpinned == last unpinned in display order. If only the image budget is
            // exceeded, evict the oldest *image* so text history is not wasted.
            let over_items = unpinned > max_items;
            let victim = self
                .idx
                .items
                .iter()
                .rposition(|i| !i.pinned && (over_items || i.kind == Kind::Image));
            match victim {
                Some(p) => removed.push(self.idx.items.remove(p)),
                None => break,
            }
        }
        removed
    }

    pub fn delete_files(&self, item: &ClipItem) {
        match item.kind {
            Kind::Text => {
                let _ = std::fs::remove_file(self.text_path(item.id));
            }
            Kind::Image => {
                let _ = std::fs::remove_file(self.image_path(item.id));
                let _ = std::fs::remove_file(self.thumb_path(item.id));
            }
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let json = serde_json::to_vec(&self.idx).map_err(std::io::Error::other)?;
        write_atomic(&self.dir.join("clips.json"), &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("notch-store-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn text_item(s: &mut Store, text: &str) -> ClipItem {
        let id = s.alloc_id();
        std::fs::write(s.text_path(id), text).unwrap();
        ClipItem {
            id,
            kind: Kind::Text,
            sub: classify(text),
            hash: hash64(text.as_bytes()),
            preview: make_preview(text, 100),
            chars: text.chars().count() as u32,
            width: 0,
            height: 0,
            bytes: 0,
            pinned: false,
            ts: now_secs(),
        }
    }

    fn image_item(s: &mut Store, tag: u8, bytes: u64) -> ClipItem {
        let id = s.alloc_id();
        std::fs::write(s.image_path(id), [tag]).unwrap();
        ClipItem {
            id,
            kind: Kind::Image,
            sub: Sub::Plain,
            hash: hash64(&[tag, 99]),
            preview: String::new(),
            chars: 0,
            width: 10,
            height: 10,
            bytes,
            pinned: false,
            ts: now_secs(),
        }
    }

    #[test]
    fn newest_first_and_dedup_moves_to_front() {
        let d = tmp("dedup");
        let mut s = Store::open(&d);
        let a = text_item(&mut s, "alpha");
        let b = text_item(&mut s, "beta");
        let a_id = a.id;
        assert_eq!(s.add_or_touch(a), AddResult::Added);
        assert_eq!(s.add_or_touch(b), AddResult::Added);
        assert_eq!(s.items()[0].preview, "beta");
        // copying "alpha" again must not duplicate it, just bring it to the top
        let again = text_item(&mut s, "alpha");
        assert_eq!(s.add_or_touch(again), AddResult::Touched(a_id));
        assert_eq!(s.len(), 2);
        assert_eq!(s.items()[0].preview, "alpha");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn eviction_skips_pinned_and_removes_oldest() {
        let d = tmp("evict");
        let mut s = Store::open(&d);
        let mut ids = vec![];
        for i in 0..6 {
            let it = text_item(&mut s, &format!("item {i}"));
            ids.push(it.id);
            s.add_or_touch(it);
        }
        // pin the very oldest one: it must survive eviction
        s.toggle_pin(ids[0]);
        let removed = s.evict(3, u64::MAX);
        let removed_ids: Vec<u64> = removed.iter().map(|i| i.id).collect();
        assert_eq!(removed.len(), 2); // 5 unpinned -> 3
        assert!(!removed_ids.contains(&ids[0]));
        assert_eq!(removed_ids, vec![ids[1], ids[2]]); // oldest unpinned first
        assert!(s.get(ids[0]).is_some());
        assert_eq!(s.len(), 4);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn image_budget_evicts_old_images_not_text() {
        let d = tmp("imgcap");
        let mut s = Store::open(&d);
        let t = text_item(&mut s, "keep me");
        let t_id = t.id;
        s.add_or_touch(t);
        let i1 = image_item(&mut s, 1, 600);
        let i1_id = i1.id;
        s.add_or_touch(i1);
        let i2 = image_item(&mut s, 2, 600);
        let i2_id = i2.id;
        s.add_or_touch(i2);
        let removed = s.evict(100, 1000);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, i1_id); // the older image
        assert!(s.get(t_id).is_some());
        assert!(s.get(i2_id).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pinned_group_sorts_first_and_unpin_returns() {
        let d = tmp("pin");
        let mut s = Store::open(&d);
        let mut ids = vec![];
        for i in 0..3 {
            let it = text_item(&mut s, &format!("n{i}"));
            ids.push(it.id);
            s.add_or_touch(it);
        }
        // display order: n2, n1, n0 ; pin n0 -> n0 first
        s.toggle_pin(ids[0]);
        assert_eq!(s.items()[0].id, ids[0]);
        assert!(s.items()[0].pinned);
        // new copies land below the pinned group
        let it = text_item(&mut s, "fresh");
        let fresh = it.id;
        s.add_or_touch(it);
        assert_eq!(s.items()[1].id, fresh);
        s.toggle_pin(ids[0]);
        assert!(!s.get(ids[0]).unwrap().pinned);
        assert_eq!(s.items()[0].id, ids[0]); // newest unpinned position (front of unpinned group)
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn json_round_trip_and_missing_payload_pruned() {
        let d = tmp("roundtrip");
        let (keep, gone);
        {
            let mut s = Store::open(&d);
            let a = text_item(&mut s, "persist me");
            keep = a.id;
            s.add_or_touch(a);
            let b = text_item(&mut s, "my file will vanish");
            gone = b.id;
            s.add_or_touch(b);
            s.toggle_pin(keep);
            s.save().unwrap();
        }
        std::fs::remove_file(d.join("clips").join(format!("{gone}.txt"))).unwrap();
        let mut s = Store::open(&d);
        assert_eq!(s.len(), 1);
        let it = s.get(keep).unwrap();
        assert_eq!(it.preview, "persist me");
        assert!(it.pinned);
        // ids keep increasing across restarts
        assert!(s.alloc_id() > gone);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_index_starts_empty() {
        let d = tmp("corrupt");
        std::fs::create_dir_all(d.join("clips")).unwrap();
        std::fs::write(d.join("clips.json"), "garbage{{").unwrap();
        let s = Store::open(&d);
        assert_eq!(s.len(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_tmp() {
        let d = tmp("atomic");
        let p = d.join("f.txt");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert!(!p.with_extension("tmp").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn preview_collapses_whitespace_and_truncates() {
        assert_eq!(make_preview("  a \n\n  b\tc  ", 100), "a b c");
        assert_eq!(make_preview("abcdefghij", 4), "abcd");
        assert_eq!(make_preview("", 10), "");
    }

    #[test]
    fn classifies_links_code_and_text() {
        assert_eq!(classify("https://example.com/release-notes"), Sub::Link);
        assert_eq!(classify("  www.example.com  "), Sub::Link);
        assert_eq!(classify("see https://example.com for details"), Sub::Plain); // not just a link
        assert_eq!(classify("SELECT id, status FROM orders WHERE status = 'failed';"), Sub::Code);
        assert_eq!(classify("const total = items.length;"), Sub::Code);
        assert_eq!(classify("fn main() {"), Sub::Code);
        assert_eq!(classify("if (a == b) { return x; }"), Sub::Code);
        assert_eq!(classify("Meeting moved to 4 pm"), Sub::Plain);
        assert_eq!(classify("hello@example.com"), Sub::Plain);
        assert_eq!(classify("Thanks, talk soon;"), Sub::Plain); // one weak signal is not enough
        assert_eq!(classify("   "), Sub::Plain);
    }

    #[test]
    fn sub_labels() {
        assert_eq!(Sub::Plain.label(Kind::Text), "Text");
        assert_eq!(Sub::Link.label(Kind::Text), "Link");
        assert_eq!(Sub::Code.label(Kind::Text), "Code");
        assert_eq!(Sub::Plain.label(Kind::Image), "Image");
    }

    #[test]
    fn relative_time_labels() {
        let now = 100_000_000;
        assert_eq!(ago(now, now), "just now");
        assert_eq!(ago(now, now - 44), "just now");
        assert_eq!(ago(now, now - 60), "1 min ago");
        assert_eq!(ago(now, now - 120), "2 min ago");
        assert_eq!(ago(now, now - 11 * 60), "11 min ago");
        assert_eq!(ago(now, now - 3 * 3600), "3 hr ago");
        assert_eq!(ago(now, now - 30 * 3600), "yesterday");
        assert_eq!(ago(now, now - 5 * 86_400), "5 days ago");
        assert_eq!(ago(now, now - 21 * 86_400), "3 wk ago");
        assert_eq!(ago(now, now + 500), "just now"); // clock skew never underflows
        // the edges between two units never read "0 hr ago" / "0 yr ago"
        assert_eq!(ago(now, now - 3569), "59 min ago");
        assert_eq!(ago(now, now - 3570), "59 min ago");
        assert_eq!(ago(now, now - 3599), "59 min ago");
        assert_eq!(ago(now, now - 3600), "1 hr ago");
        assert_eq!(ago(now, now - 364 * 86_400), "1 yr ago");
        assert_eq!(ago(now, now - 800 * 86_400), "2 yr ago");
    }

    #[test]
    fn old_index_without_sub_field_still_loads() {
        let d = tmp("legacy");
        std::fs::create_dir_all(d.join("clips")).unwrap();
        std::fs::write(d.join("clips").join("1.txt"), "x").unwrap();
        std::fs::write(
            d.join("clips.json"),
            r#"{"next_id":1,"items":[{"id":1,"kind":"Text","hash":5,"preview":"x","chars":1,"width":0,"height":0,"bytes":0,"pinned":false,"ts":1}]}"#,
        )
        .unwrap();
        let s = Store::open(&d);
        assert_eq!(s.len(), 1);
        assert_eq!(s.items()[0].sub, Sub::Plain);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hash_differs_and_is_stable() {
        assert_eq!(hash64(b"hello"), hash64(b"hello"));
        assert_ne!(hash64(b"hello"), hash64(b"hellp"));
    }
}
