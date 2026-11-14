//! Clipboard history: capture (text + images), privacy filtering, and paste-back.
//!
//! Capture is driven by `WM_CLIPBOARDUPDATE` (no polling). Text is handled inline; image
//! encoding (DIB -> PNG + thumbnail through WIC) runs on a short-lived worker thread so the
//! UI never stalls, and the thread exits as soon as it is done.

use crate::app::{App, WM_APP_CLIPDONE};
use crate::peek::Copied;
use crate::store::{ClipItem, Kind, Sub, classify, hash64, make_preview, now_secs, write_atomic};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::*;

const MAX_TEXT_CHARS: usize = 1_000_000;
const MAX_IMAGE_BYTES: usize = 256 * 1024 * 1024;
const MAX_DIMENSION: i32 = 16384;

/// Clipboard formats that password managers and well-behaved apps use to opt out of history.
pub struct Formats {
    exclude: u32,
    can_include: u32,
    png: u32,
}

pub fn register_formats() -> Formats {
    unsafe {
        Formats {
            exclude: RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")),
            can_include: RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")),
            png: RegisterClipboardFormatW(w!("PNG")),
        }
    }
}

enum Captured {
    Text(String),
    Png(Vec<u8>),
    Dib(Vec<u8>),
}

fn open_clipboard(hwnd: HWND) -> bool {
    for _ in 0..5 {
        if unsafe { OpenClipboard(Some(hwnd)).is_ok() } {
            return true;
        }
        unsafe { windows::Win32::System::Threading::Sleep(10) };
    }
    false
}

unsafe fn read_global(format: u32) -> Option<Vec<u8>> {
    unsafe {
        let h = GetClipboardData(format).ok()?;
        let hg = HGLOBAL(h.0);
        let size = GlobalSize(hg);
        if size == 0 || size > MAX_IMAGE_BYTES {
            return None;
        }
        let p = GlobalLock(hg);
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p as *const u8, size).to_vec();
        let _ = GlobalUnlock(hg);
        Some(v)
    }
}

unsafe fn read_clipboard(f: &Formats) -> Option<Captured> {
    unsafe {
        // Honour opt-out flags (password managers etc.).
        if IsClipboardFormatAvailable(f.exclude).is_ok() {
            return None;
        }
        if IsClipboardFormatAvailable(f.can_include).is_ok() {
            if let Some(b) = read_global(f.can_include) {
                if b.len() >= 4 && u32::from_le_bytes([b[0], b[1], b[2], b[3]]) == 0 {
                    return None;
                }
            }
        }

        if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok() {
            let bytes = read_global(CF_UNICODETEXT.0 as u32)?;
            let wide: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len()).min(MAX_TEXT_CHARS);
            return Some(Captured::Text(String::from_utf16_lossy(&wide[..end])));
        }
        if f.png != 0 && IsClipboardFormatAvailable(f.png).is_ok() {
            if let Some(b) = read_global(f.png) {
                return Some(Captured::Png(b));
            }
        }
        if IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok() {
            if let Some(b) = read_global(CF_DIB.0 as u32) {
                return Some(Captured::Dib(b));
            }
        }
        None
    }
}

/// `WM_CLIPBOARDUPDATE`.
pub fn on_update(app: &App) {
    if app.paused.get() {
        return;
    }
    unsafe {
        // Ignore changes we made ourselves (pasting an entry back).
        if GetClipboardOwner().unwrap_or_default() == app.hwnd {
            return;
        }
        if !open_clipboard(app.hwnd) {
            return;
        }
        let captured = read_clipboard(&app.clip_formats);
        let _ = CloseClipboard();
        match captured {
            Some(Captured::Text(t)) => add_text(app, t),
            Some(Captured::Png(b)) => spawn_image(app, b, true),
            Some(Captured::Dib(b)) => spawn_image(app, b, false),
            None => {}
        }
    }
}

fn add_text(app: &App, text: String) {
    if text.trim().is_empty() {
        return;
    }
    let hash = hash64(text.as_bytes());
    {
        let mut store = app.store.borrow_mut();
        if let Some(id) = store.find_hash(Kind::Text, hash) {
            store.touch(id);
        } else {
            let id = store.alloc_id();
            if write_atomic(&store.text_path(id), text.as_bytes()).is_err() {
                return;
            }
            store.add_or_touch(ClipItem {
                id,
                kind: Kind::Text,
                sub: classify(&text),
                hash,
                preview: make_preview(&text, 400),
                chars: text.chars().count() as u32,
                width: 0,
                height: 0,
                bytes: 0,
                pinned: false,
                ts: now_secs(),
            });
        }
    }
    app.after_store_change();
    app.show_copy_peek(Copied::Text(&text));
}

// ---------------------------------------------------------------- images

struct ImageJob {
    hwnd: isize,
    id: u64,
    img: PathBuf,
    thumb: PathBuf,
    data: Vec<u8>,
    is_png: bool,
}

pub struct ImageDone {
    pub id: u64,
    pub hash: u64,
    pub w: u32,
    pub h: u32,
    pub bytes: u64,
    pub ok: bool,
}

fn spawn_image(app: &App, data: Vec<u8>, is_png: bool) {
    if !is_png && dib_pixel_offset(&data).is_none() {
        return;
    }
    let (id, img, thumb) = {
        let mut s = app.store.borrow_mut();
        let id = s.alloc_id();
        (id, s.image_path(id), s.thumb_path(id))
    };
    let job = ImageJob { hwnd: app.hwnd.0 as isize, id, img, thumb, data, is_png };
    let _ = std::thread::Builder::new().name("notch-image".into()).spawn(move || {
        let done = run_job(&job);
        DONE.lock().unwrap_or_else(|e| e.into_inner()).push(done);
        unsafe {
            let h = HWND(job.hwnd as *mut core::ffi::c_void);
            let _ = PostMessageW(Some(h), WM_APP_CLIPDONE, WPARAM(0), LPARAM(0));
        }
    });
}

/// Finished image jobs waiting for the UI thread. The message that announces them carries no
/// data, so a stray `WM_APP_CLIPDONE` from another program just finds the queue empty.
static DONE: Mutex<Vec<ImageDone>> = Mutex::new(Vec::new());

fn run_job(job: &ImageJob) -> ImageDone {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let hash = hash64(&job.data);
        let res = encode_job(job);
        CoUninitialize();
        match res {
            Ok((w, h)) => {
                let bytes = [&job.img, &job.thumb]
                    .iter()
                    .filter_map(|p| std::fs::metadata(p).ok())
                    .map(|m| m.len())
                    .sum();
                ImageDone { id: job.id, hash, w, h, bytes, ok: true }
            }
            Err(_) => ImageDone { id: job.id, hash, w: 0, h: 0, bytes: 0, ok: false },
        }
    }
}

/// Byte offset of the pixel data inside a packed DIB (BITMAPINFOHEADER-based), or `None` if
/// the header looks wrong or unreasonably large.
fn dib_pixel_offset(d: &[u8]) -> Option<usize> {
    if d.len() < 40 {
        return None;
    }
    let u32_at = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    let i32_at = |o: usize| u32_at(o) as i32;
    let size = u32_at(0) as usize;
    if size < 40 || size > d.len() {
        return None;
    }
    let (w, h) = (i32_at(4), i32_at(8));
    if w <= 0 || w > MAX_DIMENSION || h == 0 || h.abs() > MAX_DIMENSION {
        return None;
    }
    let bit_count = u16::from_le_bytes([d[14], d[15]]) as u32;
    let compression = u32_at(16);
    let clr_used = u32_at(32);
    let palette = if clr_used > 0 {
        clr_used as usize
    } else if bit_count <= 8 {
        1usize << bit_count
    } else {
        0
    };
    let masks = if size == 40 {
        match compression {
            3 => 12,
            6 => 16,
            _ => 0,
        }
    } else {
        0
    };
    let off = size + masks + palette * 4;
    (off < d.len()).then_some(off)
}

unsafe fn encode_job(job: &ImageJob) -> Result<(u32, u32)> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;

        // DIBs are wrapped in a BMP file header so the stock WIC BMP decoder can read them.
        let bmp;
        let src: &[u8] = if job.is_png {
            &job.data
        } else {
            let off = dib_pixel_offset(&job.data).ok_or_else(|| Error::from(E_INVALIDARG))?;
            let mut v = Vec::with_capacity(14 + job.data.len());
            v.extend_from_slice(b"BM");
            v.extend_from_slice(&((14 + job.data.len()) as u32).to_le_bytes());
            v.extend_from_slice(&0u32.to_le_bytes());
            v.extend_from_slice(&((14 + off) as u32).to_le_bytes());
            v.extend_from_slice(&job.data);
            bmp = v;
            &bmp
        };

        let stream = wic.CreateStream()?;
        stream.InitializeFromMemory(src)?;
        let decoder = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let conv = wic.CreateFormatConverter()?;
        conv.Initialize(&frame, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h)?;
        if w == 0 || h == 0 || w > MAX_DIMENSION as u32 || h > MAX_DIMENSION as u32 {
            return Err(Error::from(E_INVALIDARG));
        }

        let full: IWICBitmapSource = conv.cast()?;
        save_png(&wic, &full, &job.img)?;

        // thumbnail: at most 68 px tall and 160 px wide, never upscaled
        let mut th = h.min(68);
        let mut tw = ((w as f32 * th as f32 / h as f32).round() as u32).max(1);
        if tw > 160 {
            tw = 160;
            th = ((h as f32 * tw as f32 / w as f32).round() as u32).max(1);
        }
        let scaler = wic.CreateBitmapScaler()?;
        scaler.Initialize(&conv, tw, th, WICBitmapInterpolationModeFant)?;
        let small: IWICBitmapSource = scaler.cast()?;
        save_png(&wic, &small, &job.thumb)?;
        Ok((w, h))
    }
}

unsafe fn save_png(wic: &IWICImagingFactory, src: &IWICBitmapSource, path: &Path) -> Result<()> {
    unsafe {
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame: Option<IWICBitmapFrameEncode> = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.ok_or_else(|| Error::from(E_FAIL))?;
        frame.Initialize(None)?;
        let (mut w, mut h) = (0u32, 0u32);
        src.GetSize(&mut w, &mut h)?;
        frame.SetSize(w, h)?;
        let mut fmt = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut fmt)?;
        frame.WriteSource(src, std::ptr::null())?;
        frame.Commit()?;
        enc.Commit()?;
        Ok(())
    }
}

/// `WM_APP_CLIPDONE`: a worker finished; handle every result that is waiting.
pub fn on_image_done(app: &App) {
    let finished = std::mem::take(&mut *DONE.lock().unwrap_or_else(|e| e.into_inner()));
    for done in finished {
        finish_image(app, done);
    }
}

fn finish_image(app: &App, done: ImageDone) {
    let shown_id;
    {
        let mut store = app.store.borrow_mut();
        let cleanup = |s: &crate::store::Store| {
            let _ = std::fs::remove_file(s.image_path(done.id));
            let _ = std::fs::remove_file(s.thumb_path(done.id));
        };
        if !done.ok {
            cleanup(&store);
            return;
        }
        if let Some(existing) = store.find_hash(Kind::Image, done.hash) {
            store.touch(existing);
            cleanup(&store);
            shown_id = existing;
        } else {
            shown_id = done.id;
            store.add_or_touch(ClipItem {
                id: done.id,
                kind: Kind::Image,
                sub: Sub::Plain,
                hash: done.hash,
                preview: String::new(),
                chars: 0,
                width: done.w,
                height: done.h,
                bytes: done.bytes,
                pinned: false,
                ts: now_secs(),
            });
        }
    }
    app.after_store_change();
    app.show_copy_peek(Copied::Image { id: shown_id, w: done.w, h: done.h });
}

// ---------------------------------------------------------------- paste-back

unsafe fn alloc_global(bytes: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).ok()?;
        let p = GlobalLock(h);
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
        let _ = GlobalUnlock(h);
        Some(h)
    }
}

unsafe fn set_format(format: u32, bytes: &[u8]) -> bool {
    unsafe {
        let Some(h) = alloc_global(bytes) else { return false };
        if SetClipboardData(format, Some(HANDLE(h.0))).is_err() {
            let _ = GlobalFree(Some(h));
            return false;
        }
        true // the clipboard owns the memory now
    }
}

pub fn set_text(hwnd: HWND, text: &str) -> bool {
    if !open_clipboard(hwnd) {
        return false;
    }
    unsafe {
        let _ = EmptyClipboard();
        let wide: Vec<u8> = text.encode_utf16().chain(Some(0)).flat_map(|c| c.to_le_bytes()).collect();
        let ok = set_format(CF_UNICODETEXT.0 as u32, &wide);
        let _ = CloseClipboard();
        ok
    }
}

/// Puts a stored PNG back on the clipboard as both a DIBV5 (for classic apps) and "PNG".
pub fn set_image(hwnd: HWND, png_path: &Path, png_format: u32) -> bool {
    let Some((w, h, bgra)) = (unsafe { decode_bgra(png_path) }) else { return false };
    let png_bytes = std::fs::read(png_path).unwrap_or_default();
    let dib = build_dibv5(w, h, &bgra);
    if !open_clipboard(hwnd) {
        return false;
    }
    unsafe {
        let _ = EmptyClipboard();
        let mut ok = set_format(windows::Win32::System::Ole::CF_DIBV5.0 as u32, &dib);
        if !png_bytes.is_empty() && png_format != 0 {
            ok |= set_format(png_format, &png_bytes);
        }
        let _ = CloseClipboard();
        ok
    }
}

pub fn png_format(f: &Formats) -> u32 {
    f.png
}

unsafe fn decode_bgra(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let decoder = wic
            .CreateDecoderFromFilename(
                &HSTRING::from(path.as_os_str()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&frame, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)
            .ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h).ok()?;
        let stride = w * 4;
        let mut buf = vec![0u8; (stride * h) as usize];
        conv.CopyPixels(std::ptr::null(), stride, &mut buf).ok()?;
        Some((w, h, buf))
    }
}

/// Builds a 32-bit bottom-up BITMAPV5HEADER DIB with an alpha channel.
fn build_dibv5(w: u32, h: u32, bgra: &[u8]) -> Vec<u8> {
    let mut v: Vec<u8> = Vec::with_capacity(124 + bgra.len());
    let p32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    p32(&mut v, 124); // bV5Size
    p32(&mut v, w);
    p32(&mut v, h); // positive = bottom-up
    v.extend_from_slice(&1u16.to_le_bytes()); // planes
    v.extend_from_slice(&32u16.to_le_bytes()); // bit count
    p32(&mut v, 3); // BI_BITFIELDS
    p32(&mut v, bgra.len() as u32);
    p32(&mut v, 2835);
    p32(&mut v, 2835);
    p32(&mut v, 0);
    p32(&mut v, 0);
    p32(&mut v, 0x00FF0000); // R
    p32(&mut v, 0x0000FF00); // G
    p32(&mut v, 0x000000FF); // B
    p32(&mut v, 0xFF000000); // A
    p32(&mut v, 0x73524742); // LCS_sRGB
    v.extend_from_slice(&[0u8; 36]); // CIEXYZTRIPLE endpoints
    p32(&mut v, 0);
    p32(&mut v, 0);
    p32(&mut v, 0); // gamma
    p32(&mut v, 4); // LCS_GM_IMAGES
    p32(&mut v, 0);
    p32(&mut v, 0);
    p32(&mut v, 0);
    debug_assert_eq!(v.len(), 124);
    let stride = (w * 4) as usize;
    for row in (0..h as usize).rev() {
        v.extend_from_slice(&bgra[row * stride..(row + 1) * stride]);
    }
    v
}

/// Sends Ctrl+V to whichever window currently has focus.
pub fn send_ctrl_v() {
    fn key(vk: u16, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }
    let inputs = [key(0x11, false), key(0x56, false), key(0x56, true), key(0x11, true)];
    unsafe {
        let n = SendInput(&inputs, size_of::<INPUT>() as i32);
        crate::log(&format!("SendInput returned {n} of {}", inputs.len()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(size: u32, w: i32, h: i32, bits: u16, compression: u32, clr_used: u32) -> Vec<u8> {
        let mut v = vec![0u8; 40];
        v[0..4].copy_from_slice(&size.to_le_bytes());
        v[4..8].copy_from_slice(&w.to_le_bytes());
        v[8..12].copy_from_slice(&h.to_le_bytes());
        v[12..14].copy_from_slice(&1u16.to_le_bytes());
        v[14..16].copy_from_slice(&bits.to_le_bytes());
        v[16..20].copy_from_slice(&compression.to_le_bytes());
        v[32..36].copy_from_slice(&clr_used.to_le_bytes());
        v
    }

    #[test]
    fn dib_offset_plain_32bit() {
        let mut d = header(40, 2, 2, 32, 0, 0);
        d.extend_from_slice(&[0u8; 16]);
        assert_eq!(dib_pixel_offset(&d), Some(40));
    }

    #[test]
    fn dib_offset_with_bitfield_masks() {
        let mut d = header(40, 2, 2, 32, 3, 0);
        d.extend_from_slice(&[0u8; 12 + 16]);
        assert_eq!(dib_pixel_offset(&d), Some(52));
    }

    #[test]
    fn dib_offset_with_palette() {
        let mut d = header(40, 4, -4, 8, 0, 0);
        d.extend_from_slice(&[0u8; 256 * 4 + 16]);
        assert_eq!(dib_pixel_offset(&d), Some(40 + 256 * 4));
    }

    #[test]
    fn dib_rejects_garbage() {
        assert_eq!(dib_pixel_offset(&[0u8; 10]), None);
        assert_eq!(dib_pixel_offset(&header(40, 0, 2, 32, 0, 0)), None); // zero width
        assert_eq!(dib_pixel_offset(&header(40, 99999, 2, 32, 0, 0)), None); // absurd width
        assert_eq!(dib_pixel_offset(&header(40, 2, 2, 32, 0, 0)), None); // no pixel data
    }

    #[test]
    fn dibv5_layout_and_row_flip() {
        // 1x2 image: top pixel red, bottom pixel blue (BGRA)
        let bgra = [0, 0, 255, 255, 255, 0, 0, 255];
        let d = build_dibv5(1, 2, &bgra);
        assert_eq!(d.len(), 124 + 8);
        assert_eq!(u32::from_le_bytes([d[0], d[1], d[2], d[3]]), 124);
        // bottom-up: first stored row is the bottom pixel (blue)
        assert_eq!(&d[124..128], &[255, 0, 0, 255]);
        assert_eq!(&d[128..132], &[0, 0, 255, 255]);
    }
}
