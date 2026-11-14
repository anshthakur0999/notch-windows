//! Shelf previews: the picture Explorer shows for a file (a real thumbnail for images, PDFs, videos
//! and the like, else the file's icon), asked of the shell on a short-lived worker thread so a slow
//! file never stalls the pill.

use std::sync::Mutex;
use windows::Win32::Foundation::{HWND, LPARAM, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP, HGDIOBJ,
    ReleaseDC,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED, IBindCtx};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY, SIIGBF_THUMBNAILONLY,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::HSTRING;

/// Big enough to fill a tile (118 x 80 DIP) at 200% scale.
const THUMB: i32 = 256;
const ICON: i32 = 96;

/// One finished preview: premultiplied BGRA pixels, top row first.
pub struct Thumb {
    pub path: String,
    pub w: u32,
    pub h: u32,
    pub icon: bool,
    pub px: Vec<u8>,
}

/// Previews waiting for the UI thread. The message that announces them carries no data, so a stray
/// `WM_APP_THUMB` from another program just finds the queue empty.
static DONE: Mutex<Vec<Thumb>> = Mutex::new(Vec::new());

pub fn take() -> Vec<Thumb> {
    std::mem::take(&mut *DONE.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Asks for a preview of each path, in order; each one is announced with `msg` as it is ready.
pub fn request(hwnd: HWND, msg: u32, paths: Vec<String>) {
    let h = hwnd.0 as isize;
    let _ = std::thread::Builder::new().name("notch-thumbs".into()).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        for path in paths {
            if let Some(t) = fetch(&path) {
                DONE.lock().unwrap_or_else(|e| e.into_inner()).push(t);
                let _ = PostMessageW(Some(HWND(h as *mut core::ffi::c_void)), msg, WPARAM(0), LPARAM(0));
            }
        }
        CoUninitialize();
    });
}

/// A thumbnail if the shell can make one, else the file's icon (what Explorer would show).
fn fetch(path: &str) -> Option<Thumb> {
    unsafe {
        let item: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path), None::<&IBindCtx>).ok()?;
        let (bmp, icon) = match item.GetImage(SIZE { cx: THUMB, cy: THUMB }, SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK) {
            Ok(b) => (b, false),
            Err(_) => (item.GetImage(SIZE { cx: ICON, cy: ICON }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK).ok()?, true),
        };
        let t = pixels(bmp).map(|(w, h, px)| Thumb { path: path.to_string(), w, h, icon, px });
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        t
    }
}

/// Reads a shell bitmap as 32-bit BGRA, top row first.
fn pixels(bmp: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let mut info = BITMAP::default();
        if GetObjectW(HGDIOBJ(bmp.0), size_of::<BITMAP>() as i32, Some((&mut info as *mut BITMAP).cast())) == 0 {
            return None;
        }
        let (w, h) = (info.bmWidth, info.bmHeight);
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative: top-down
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let got = GetDIBits(dc, bmp, 0, h as u32, Some(px.as_mut_ptr().cast()), &mut bi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        if got == 0 {
            return None;
        }
        opaque_if_flat(&mut px);
        Some((w as u32, h as u32, px))
    }
}

/// Some thumbnails carry no alpha channel (all zero): treat them as opaque, not as invisible.
fn opaque_if_flat(px: &mut [u8]) {
    if px.chunks_exact(4).all(|p| p[3] == 0) {
        px.chunks_exact_mut(4).for_each(|p| p[3] = 0xFF);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_without_alpha_becomes_opaque_but_one_with_alpha_is_kept() {
        let mut flat = vec![10, 20, 30, 0, 1, 2, 3, 0];
        opaque_if_flat(&mut flat);
        assert_eq!(flat, [10, 20, 30, 255, 1, 2, 3, 255]);
        let mut real = vec![10, 20, 30, 0, 1, 2, 3, 200];
        opaque_if_flat(&mut real);
        assert_eq!(real[3], 0);
    }
}
