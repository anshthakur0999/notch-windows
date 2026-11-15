//! GPU pipeline: D3D11 device -> DXGI composition swap chain -> DirectComposition visual,
//! with Direct2D / DirectWrite drawing straight into the swap chain's back buffer.
//! This gives per-pixel transparency without a layered window and without a CPU copy.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::mem::ManuallyDrop;
use std::path::Path;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::ui::theme::Palette;

/// Text styles. The discriminant indexes `Fonts::v`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum F {
    /// Segmented header control labels.
    Seg,
    Tab,
    TabOn,
    Title,
    Section,
    Hint,
    Keycap,
    /// Centred body text (empty states).
    Center,
    Icon12,
    Icon14,
    Icon16,
    /// Clipboard entries: title, subtitle and code. These follow the "text size" setting; the
    /// header, tabs, hints and key caps keep a fixed size.
    Row,
    RowSub,
    RowMono,
    /// The timer: the big countdown, the small one (pill, header, capsules) and the time box.
    Clock,
    ClockSm,
    Field,
    /// The shelf: a heading, and the big file glyph on a tile.
    Head,
    Icon24,
    /// The track title on the Media tab (left-aligned, unlike `Head`).
    Big,
}

pub struct Fonts {
    v: Vec<IDWriteTextFormat>,
}

pub struct Renderer {
    swap: IDXGISwapChain1,
    _dcomp: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    d2d: ID2D1Factory1,
    pub dc: ID2D1DeviceContext,
    pub brush: ID2D1SolidColorBrush,
    dw: IDWriteFactory,
    fonts: Fonts,
    stroke: ID2D1StrokeStyle,
    /// Round-capped dashes for the shelf's drop targets.
    dash: ID2D1StrokeStyle,
    pin_geom: ID2D1PathGeometry,
    tray_geom: ID2D1PathGeometry,
    widths: RefCell<HashMap<(u8, String), f32>>,
    wic: Option<IWICImagingFactory>,
    /// Decoded thumbnails, only kept while the pill is open.
    pub thumbs: HashMap<u64, ID2D1Bitmap1>,
    /// Previews of the shelf's files by path (`true`: a file icon, not a thumbnail), and the paths
    /// already asked for. Only kept while the pill is open.
    pub shelf_tex: HashMap<String, (ID2D1Bitmap1, bool)>,
    pub shelf_asked: HashSet<String>,
    /// The cover of what is playing. Unlike the thumbnails it stays while the pill is closed: the
    /// resting pill and the banner show it.
    pub media_art: Option<ID2D1Bitmap1>,
    /// The same cover, small, for the pill's covers (see `media::ART_SMALL`).
    pub media_small: Option<ID2D1Bitmap1>,
    dpi: f32,
    /// The current colours and the scale of the clipboard-entry text (see `set_look`).
    pub pal: Palette,
    pub text_scale: f32,
}

pub fn color(hex: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a,
    }
}

/// Linear blend of two 0xRRGGBB colours.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |s: u32| {
        let x = ((a >> s) & 0xFF) as f32;
        let y = ((b >> s) & 0xFF) as f32;
        ((x + (y - x) * t).round() as u32) & 0xFF
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

pub fn is_device_lost(e: &Error) -> bool {
    let c = e.code();
    c == DXGI_ERROR_DEVICE_REMOVED || c == DXGI_ERROR_DEVICE_RESET || c == D2DERR_RECREATE_TARGET
}

impl Renderer {
    pub fn new(hwnd: HWND, w_px: u32, h_px: u32, dpi: f32, use_gpu: bool) -> Result<Renderer> {
        unsafe {
            // Try the preferred driver type first, then the other one.
            let order = if use_gpu {
                [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP]
            } else {
                [D3D_DRIVER_TYPE_WARP, D3D_DRIVER_TYPE_HARDWARE]
            };
            let mut device: Option<ID3D11Device> = None;
            let mut last = Ok(());
            for kind in order {
                last = D3D11CreateDevice(
                    None,
                    kind,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                );
                if last.is_ok() && device.is_some() {
                    break;
                }
            }
            last?;
            let device = device.ok_or_else(|| Error::from(E_FAIL))?;
            let dxgi_dev: IDXGIDevice = device.cast()?;
            let adapter = dxgi_dev.GetAdapter()?;
            let factory: IDXGIFactory2 = adapter.GetParent()?;

            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w_px,
                Height: h_px,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: FALSE,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                Flags: 0,
            };
            let swap = factory.CreateSwapChainForComposition(&device, &desc, None)?;

            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi_dev)?;
            let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = dcomp.CreateVisual()?;
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            dcomp.Commit()?;

            let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d_dev = d2d.CreateDevice(&dxgi_dev)?;
            let dc = d2d_dev.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            dc.SetDpi(dpi, dpi);
            let brush = dc.CreateSolidColorBrush(&color(0xFFFFFF, 1.0), None)?;

            let stroke1 = d2d.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES1 {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.0,
                    transformType: D2D1_STROKE_TRANSFORM_TYPE_NORMAL,
                },
                None,
            )?;
            let stroke: ID2D1StrokeStyle = stroke1.cast()?;
            let dash1 = d2d.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES1 {
                    startCap: D2D1_CAP_STYLE_FLAT,
                    endCap: D2D1_CAP_STYLE_FLAT,
                    dashCap: D2D1_CAP_STYLE_FLAT,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_CUSTOM,
                    dashOffset: 0.0,
                    transformType: D2D1_STROKE_TRANSFORM_TYPE_NORMAL,
                },
                Some(&[3.0, 2.2]), // in stroke widths
            )?;
            let dash: ID2D1StrokeStyle = dash1.cast()?;
            let pin_geom = make_pin_geometry(&d2d)?;
            let tray_geom = make_tray_geometry(&d2d)?;

            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fonts = make_fonts(&dw, 1.0)?;

            let r = Renderer {
                swap,
                _dcomp: dcomp,
                _target: target,
                _visual: visual,
                d2d,
                dc,
                brush,
                dw,
                fonts,
                stroke,
                dash,
                pin_geom,
                tray_geom,
                widths: RefCell::new(HashMap::new()),
                wic: None,
                thumbs: HashMap::new(),
                shelf_tex: HashMap::new(),
                shelf_asked: HashSet::new(),
                media_art: None,
                media_small: None,
                dpi,
                pal: Palette::default(),
                text_scale: 1.0,
            };
            r.attach_target()?;
            Ok(r)
        }
    }

    /// Switches colours and the size of the clipboard-entry text (tray menu, live). Fonts are only
    /// rebuilt when the size changed.
    pub fn set_look(&mut self, pal: Palette, text_scale: f32) {
        self.pal = pal;
        if (text_scale - self.text_scale).abs() > 0.001 {
            if let Ok(fonts) = make_fonts(&self.dw, text_scale) {
                self.fonts = fonts;
                self.text_scale = text_scale;
                self.widths.borrow_mut().clear();
            }
        }
    }

    unsafe fn attach_target(&self) -> Result<()> {
        unsafe {
            let surface: IDXGISurface = self.swap.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: self.dpi,
                dpiY: self.dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: ManuallyDrop::new(None),
            };
            let bmp = self.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.dc.SetTarget(&bmp);
            Ok(())
        }
    }

    /// Re-sizes the back buffers (monitor DPI change).
    pub fn resize(&mut self, w_px: u32, h_px: u32, dpi: f32) -> Result<()> {
        unsafe {
            self.dc.SetTarget(None);
            self.swap.ResizeBuffers(0, w_px, h_px, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
            self.dpi = dpi;
            self.dc.SetDpi(dpi, dpi);
            self.thumbs.clear();
            self.attach_target()
        }
    }

    pub fn begin(&self) {
        unsafe {
            self.dc.BeginDraw();
            self.dc.Clear(Some(&color(0, 0.0)));
        }
    }

    /// Ends drawing and presents. Returns the error if the device was lost.
    pub fn end(&self) -> Result<()> {
        unsafe {
            self.dc.EndDraw(None, None)?;
            self.swap.Present(1, DXGI_PRESENT(0)).ok()?;
            Ok(())
        }
    }

    // ---- small drawing helpers (DIP coordinates) ----

    pub fn fill_rect(&self, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(&c);
            self.dc.FillRectangle(&r, &self.brush);
        }
    }

    pub fn fill_rrect(&self, r: D2D_RECT_F, radius: f32, c: D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(&c);
            self.dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }, &self.brush);
        }
    }

    pub fn stroke_rrect(&self, r: D2D_RECT_F, radius: f32, c: D2D1_COLOR_F, width: f32) {
        unsafe {
            self.brush.SetColor(&c);
            self.dc.DrawRoundedRectangle(
                &D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius },
                &self.brush,
                width,
                None,
            );
        }
    }

    /// A dashed rounded outline (a place to drop things).
    pub fn dashed_rrect(&self, r: D2D_RECT_F, radius: f32, c: D2D1_COLOR_F, width: f32) {
        unsafe {
            self.brush.SetColor(&c);
            self.dc.DrawRoundedRectangle(
                &D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius },
                &self.brush,
                width,
                Some(&self.dash),
            );
        }
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, rad: f32, c: D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(&c);
            self.dc.FillEllipse(
                &D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: rad, radiusY: rad },
                &self.brush,
            );
        }
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: D2D1_COLOR_F, width: f32) {
        unsafe {
            self.brush.SetColor(&c);
            self.dc.DrawLine(
                Vector2 { X: x0, Y: y0 },
                Vector2 { X: x1, Y: y1 },
                &self.brush,
                width,
                Some(&self.stroke),
            );
        }
    }

    fn fmt(&self, f: F) -> &IDWriteTextFormat {
        &self.fonts.v[f as usize]
    }

    pub fn text(&self, s: &str, f: F, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        if s.is_empty() {
            return;
        }
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            self.brush.SetColor(&c);
            self.dc.DrawText(
                &wide,
                self.fmt(f),
                &r,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    /// Text with extra tracking between letters (section headers).
    pub fn text_spaced(&self, s: &str, f: F, r: D2D_RECT_F, c: D2D1_COLOR_F, spacing: f32) {
        if s.is_empty() {
            return;
        }
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dw.CreateTextLayout(&wide, self.fmt(f), r.right - r.left, r.bottom - r.top) else {
                return;
            };
            if let Ok(l1) = layout.cast::<IDWriteTextLayout1>() {
                let _ = l1.SetCharacterSpacing(
                    0.0,
                    spacing,
                    0.0,
                    DWRITE_TEXT_RANGE { startPosition: 0, length: wide.len() as u32 },
                );
            }
            self.brush.SetColor(&c);
            self.dc.DrawTextLayout(
                Vector2 { X: r.left, Y: r.top },
                &layout,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
            );
        }
    }

    /// Width in DIPs of `s` in style `f` (cached: only used for short UI labels).
    pub fn text_w(&self, f: F, s: &str) -> f32 {
        let key = (f as u8, s.to_string());
        if let Some(w) = self.widths.borrow().get(&key) {
            return *w;
        }
        let w = self.measure(f, s);
        self.widths.borrow_mut().insert(key, w);
        w
    }

    /// Width in DIPs of `s` in style `f`, not cached (for text that is different every time,
    /// such as a copied clip).
    pub fn measure(&self, f: F, s: &str) -> f32 {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            self.dw
                .CreateTextLayout(&wide, self.fmt(f), 4000.0, 100.0)
                .ok()
                .and_then(|l| {
                    let mut m = DWRITE_TEXT_METRICS::default();
                    l.GetMetrics(&mut m).ok().map(|_| m.widthIncludingTrailingWhitespace)
                })
                .unwrap_or(s.chars().count() as f32 * 7.0)
        }
    }

    /// Width of a clock text such as "14:32" drawn by `clock_text`.
    pub fn clock_width(&self, f: F, s: &str) -> f32 {
        s.chars().map(|c| self.clock_cell(f, c)).sum()
    }

    /// Every digit gets a cell as wide as a "0", so the text does not jitter as the digits change.
    fn clock_cell(&self, f: F, c: char) -> f32 {
        self.text_w(f, if c.is_ascii_digit() { "0" } else { ":" })
    }

    /// A clock such as "14:32" or "1:24:58" with its left edge at `x`, centred on `cy`.
    pub fn clock_text(&self, s: &str, f: F, x: f32, cy: f32, c: D2D1_COLOR_F) {
        let mut x = x;
        for ch in s.chars() {
            let w = self.clock_cell(f, ch);
            let rect = D2D_RECT_F { left: x, top: cy - 40.0, right: x + w, bottom: cy + 40.0 };
            self.text(ch.encode_utf8(&mut [0u8; 4]), f, rect, c);
            x += w;
        }
    }

    /// A circle outline.
    pub fn ring(&self, cx: f32, cy: f32, radius: f32, width: f32, c: D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(&c);
            self.dc.DrawEllipse(
                &D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius, radiusY: radius },
                &self.brush,
                width,
                None,
            );
        }
    }

    /// An arc of a circle with round ends, starting at 12 o'clock and running clockwise for
    /// `sweep` degrees (a full turn is drawn as a plain ring). One path, so a translucent colour
    /// does not double up where pieces would overlap. Built once a second at most: the timer ring.
    pub fn arc(&self, cx: f32, cy: f32, radius: f32, width: f32, c: D2D1_COLOR_F, sweep: f32) {
        if sweep <= 0.0 {
            return;
        }
        if sweep >= 359.9 {
            return self.ring(cx, cy, radius, width, c);
        }
        let a = sweep.to_radians();
        unsafe {
            let Ok(geom) = self.d2d.CreatePathGeometry() else { return };
            let Ok(sink) = geom.Open() else { return };
            sink.BeginFigure(Vector2 { X: cx, Y: cy - radius }, D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: Vector2 { X: cx + radius * a.sin(), Y: cy - radius * a.cos() },
                size: D2D_SIZE_F { width: radius, height: radius },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if sweep > 180.0 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            if sink.Close().is_err() {
                return;
            }
            self.brush.SetColor(&c);
            self.dc.DrawGeometry(&geom, &self.brush, width, Some(&self.stroke));
        }
    }

    /// One icon-font glyph centred on (cx, cy). `f` must be one of the `Icon*` styles.
    pub fn glyph(&self, code: char, f: F, cx: f32, cy: f32, c: D2D1_COLOR_F) {
        let mut buf = [0u16; 2];
        let wide = code.encode_utf16(&mut buf);
        unsafe {
            self.brush.SetColor(&c);
            self.dc.DrawText(
                wide,
                self.fmt(f),
                &D2D_RECT_F { left: cx - 14.0, top: cy - 14.0, right: cx + 14.0, bottom: cy + 14.0 },
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    /// Push-pin icon centred on (cx, cy), `size` DIPs square. `filled` for the pinned state.
    pub fn pin(&self, cx: f32, cy: f32, size: f32, c: D2D1_COLOR_F, filled: bool) {
        let s = size / 16.0;
        let m = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: cx - size / 2.0, M32: cy - size / 2.0 };
        unsafe {
            self.brush.SetColor(&c);
            self.dc.SetTransform(&m);
            if filled {
                self.dc.FillGeometry(&self.pin_geom, &self.brush, None);
            }
            self.dc.DrawGeometry(&self.pin_geom, &self.brush, 1.25, Some(&self.stroke));
            self.dc.DrawLine(
                Vector2 { X: 8.0, Y: 9.2 },
                Vector2 { X: 8.0, Y: 14.8 },
                &self.brush,
                1.25,
                Some(&self.stroke),
            );
            self.dc.SetTransform(&Matrix3x2::identity());
        }
    }

    /// The shelf's tray icon centred on (cx, cy), `size` DIPs square.
    pub fn tray(&self, cx: f32, cy: f32, size: f32, c: D2D1_COLOR_F) {
        let s = size / 16.0;
        let m = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: cx - size / 2.0, M32: cy - size / 2.0 };
        unsafe {
            self.brush.SetColor(&c);
            self.dc.SetTransform(&m);
            self.dc.DrawGeometry(&self.tray_geom, &self.brush, 1.3, Some(&self.stroke));
            self.dc.SetTransform(&Matrix3x2::identity());
        }
    }

    /// Upward chevron (collapse button).
    pub fn chevron_up(&self, cx: f32, cy: f32, half: f32, c: D2D1_COLOR_F) {
        let h = half * 0.55;
        self.line(cx - half, cy + h, cx, cy - h, c, 1.6);
        self.line(cx, cy - h, cx + half, cy + h, c, 1.6);
    }

    pub fn push_clip(&self, r: D2D_RECT_F) {
        unsafe { self.dc.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED) }
    }

    pub fn pop_clip(&self) {
        unsafe { self.dc.PopAxisAlignedClip() }
    }

    pub fn draw_bitmap(&self, bmp: &ID2D1Bitmap1, dest: D2D_RECT_F, opacity: f32) {
        unsafe {
            // cubic, which also filters properly when shrinking a big picture; plain linear aliases
            self.dc.DrawBitmap(bmp, Some(&dest), opacity, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None);
        }
    }

    /// Fills a rounded rectangle with `bmp`, scaled to cover it (centred, the overflow cropped),
    /// so the picture itself has the rounded corners.
    pub fn fill_rrect_cover(&self, bmp: &ID2D1Bitmap1, dest: D2D_RECT_F, radius: f32, opacity: f32) {
        self.cover_with(bmp, dest, radius, opacity, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC)
    }

    /// `mode` is the filter: the high-quality cubic one is several passes on the software rasteriser,
    /// too slow for what is redrawn many times a second, so the live cover uses linear on a copy
    /// already near the drawn size.
    fn cover_with(&self, bmp: &ID2D1Bitmap1, dest: D2D_RECT_F, radius: f32, opacity: f32, mode: D2D1_INTERPOLATION_MODE) {
        unsafe {
            let size = bmp.GetSize();
            if size.width <= 0.0 || size.height <= 0.0 {
                return;
            }
            let (dw, dh) = (dest.right - dest.left, dest.bottom - dest.top);
            let s = (dw / size.width).max(dh / size.height);
            let props = D2D1_BRUSH_PROPERTIES {
                opacity,
                transform: Matrix3x2 {
                    M11: s,
                    M12: 0.0,
                    M21: 0.0,
                    M22: s,
                    M31: dest.left + (dw - size.width * s) / 2.0,
                    M32: dest.top + (dh - size.height * s) / 2.0,
                },
            };
            let quality = D2D1_BITMAP_BRUSH_PROPERTIES1 {
                extendModeX: D2D1_EXTEND_MODE_CLAMP,
                extendModeY: D2D1_EXTEND_MODE_CLAMP,
                interpolationMode: mode,
            };
            let Ok(brush) = self.dc.CreateBitmapBrush(bmp, Some(&quality), Some(&props)) else { return };
            self.dc.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT { rect: dest, radiusX: radius, radiusY: radius },
                &brush,
            );
        }
    }

    /// `fill_rrect_cover`, except a small non-square picture that would be blown up past `MAX_ZOOM`
    /// (a 150x83 video thumbnail in a 109 DIP square) is shown whole instead, over a faint copy of
    /// itself: zooming into its middle is what made it blurry.
    pub fn fill_rrect_art(&self, bmp: &ID2D1Bitmap1, dest: D2D_RECT_F, radius: f32, opacity: f32) {
        const MAX_ZOOM: f32 = 1.15;
        let size = unsafe { bmp.GetSize() };
        let (dw, dh) = (dest.right - dest.left, dest.bottom - dest.top);
        let zoom = (dw / size.width).max(dh / size.height);
        let square = (size.width - size.height).abs() < size.width.max(size.height) * 0.05;
        const LINEAR: D2D1_INTERPOLATION_MODE = D2D1_INTERPOLATION_MODE_LINEAR;
        if size.width <= 0.0 || size.height <= 0.0 || zoom <= MAX_ZOOM || square {
            return self.cover_with(bmp, dest, radius, opacity, LINEAR);
        }
        self.cover_with(bmp, dest, radius, opacity * 0.3, LINEAR);
        let k = (dw / size.width).min(dh / size.height);
        let (w, h) = (size.width * k, size.height * k);
        let (l, t) = (dest.left + (dw - w) / 2.0, dest.top + (dh - h) / 2.0);
        self.cover_with(bmp, D2D_RECT_F { left: l, top: t, right: l + w, bottom: t + h }, 5.0, opacity, LINEAR);
    }

    /// Returns the (cached) GPU bitmap for a thumbnail PNG, decoding it on first use.
    pub fn thumb(&mut self, id: u64, path: &Path) -> Option<ID2D1Bitmap1> {
        if let Some(b) = self.thumbs.get(&id) {
            return Some(b.clone());
        }
        let bmp = self.load_bitmap(path).ok()?;
        self.thumbs.insert(id, bmp.clone());
        Some(bmp)
    }

    fn load_bitmap(&mut self, path: &Path) -> Result<ID2D1Bitmap1> {
        unsafe {
            if self.wic.is_none() {
                self.wic = Some(CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?);
            }
            let wic = self.wic.as_ref().unwrap();
            let decoder = wic.CreateDecoderFromFilename(
                &HSTRING::from(path.as_os_str()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )?;
            let frame = decoder.GetFrame(0)?;
            let conv = wic.CreateFormatConverter()?;
            conv.Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeMedianCut,
            )?;
            self.dc.CreateBitmapFromWicBitmap(&conv, None)
        }
    }

    /// Uploads a shelf preview the worker finished (ignored if the pill has closed since).
    pub fn add_shelf_thumb(&mut self, t: crate::thumbs::Thumb) {
        if !self.shelf_asked.contains(&t.path) {
            return;
        }
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            colorContext: ManuallyDrop::new(None),
        };
        let size = D2D_SIZE_U { width: t.w, height: t.h };
        if let Ok(bmp) = unsafe { self.dc.CreateBitmap(size, Some(t.px.as_ptr().cast()), t.w * 4, &props) } {
            self.shelf_tex.insert(t.path, (bmp, t.icon));
        }
    }

    /// Uploads the cover the media worker decoded (`None`: there is no cover now).
    pub fn set_media_art(&mut self, art: Option<crate::media::Cover>) {
        let upload = |a: &crate::media::Art| {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: ManuallyDrop::new(None),
            };
            let size = D2D_SIZE_U { width: a.w, height: a.h };
            unsafe { self.dc.CreateBitmap(size, Some(a.px.as_ptr().cast()), a.w * 4, &props) }.ok()
        };
        let (full, small) = art.map_or((None, None), |c| (upload(&c.full), upload(&c.small)));
        (self.media_art, self.media_small) = (full, small);
    }

    /// Drops GPU thumbnails and the WIC factory so a closed pill holds no extra memory.
    pub fn release_transient(&mut self) {
        self.thumbs.clear();
        self.shelf_tex.clear();
        self.shelf_asked.clear();
        self.wic = None;
    }
}

/// A push-pin in a 16x16 box: flat head, tapered body and a base plate (the needle is a line).
fn make_pin_geometry(d2d: &ID2D1Factory1) -> Result<ID2D1PathGeometry> {
    unsafe {
        let geom = d2d.CreatePathGeometry()?;
        let sink = geom.Open()?;
        let pts = [(5.0f32, 1.5f32), (11.0, 1.5), (10.2, 6.4), (12.2, 9.2), (3.8, 9.2), (5.8, 6.4)];
        sink.BeginFigure(Vector2 { X: pts[0].0, Y: pts[0].1 }, D2D1_FIGURE_BEGIN_FILLED);
        for p in &pts[1..] {
            sink.AddLine(Vector2 { X: p.0, Y: p.1 });
        }
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;
        geom.cast::<ID2D1PathGeometry>()
    }
}

/// An open tray (an inbox) on a 16 x 16 grid: the box, and the dip of its front edge.
fn make_tray_geometry(d2d: &ID2D1Factory1) -> Result<ID2D1PathGeometry> {
    unsafe {
        let geom = d2d.CreatePathGeometry()?;
        let sink = geom.Open()?;
        let figure = |pts: &[(f32, f32)], end: D2D1_FIGURE_END| {
            sink.BeginFigure(Vector2 { X: pts[0].0, Y: pts[0].1 }, D2D1_FIGURE_BEGIN_HOLLOW);
            for p in &pts[1..] {
                sink.AddLine(Vector2 { X: p.0, Y: p.1 });
            }
            sink.EndFigure(end);
        };
        figure(
            &[(1.6, 8.2), (3.8, 3.4), (5.0, 2.6), (11.0, 2.6), (12.2, 3.4), (14.4, 8.2), (14.4, 12.0), (13.0, 13.4), (3.0, 13.4), (1.6, 12.0)],
            D2D1_FIGURE_END_CLOSED,
        );
        figure(&[(1.6, 8.2), (5.2, 8.2), (6.4, 10.4), (9.6, 10.4), (10.8, 8.2), (14.4, 8.2)], D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        geom.cast::<ID2D1PathGeometry>()
    }
}

fn has_family(dw: &IDWriteFactory, name: PCWSTR) -> bool {
    unsafe {
        let mut coll: Option<IDWriteFontCollection> = None;
        if dw.GetSystemFontCollection(&mut coll, false).is_err() {
            return false;
        }
        let Some(c) = coll else { return false };
        let mut idx = 0u32;
        let mut exists = BOOL(0);
        c.FindFamilyName(name, &mut idx, &mut exists).is_ok() && exists.as_bool()
    }
}

fn make_format(
    dw: &IDWriteFactory,
    family: PCWSTR,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
    center: bool,
) -> Result<IDWriteTextFormat> {
    unsafe {
        let f = dw.CreateTextFormat(
            family,
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("en-us"),
        )?;
        f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        f.SetTextAlignment(if center { DWRITE_TEXT_ALIGNMENT_CENTER } else { DWRITE_TEXT_ALIGNMENT_LEADING })?;
        let sign = dw.CreateEllipsisTrimmingSign(&f)?;
        let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
        f.SetTrimming(&trim, &sign)?;
        Ok(f)
    }
}

fn make_fonts(dw: &IDWriteFactory, k: f32) -> Result<Fonts> {
    let ui = if has_family(dw, w!("Segoe UI Variable Text")) { w!("Segoe UI Variable Text") } else { w!("Segoe UI") };
    let mono = if has_family(dw, w!("Cascadia Mono")) { w!("Cascadia Mono") } else { w!("Consolas") };
    let icons =
        if has_family(dw, w!("Segoe Fluent Icons")) { w!("Segoe Fluent Icons") } else { w!("Segoe MDL2 Assets") };
    let n = DWRITE_FONT_WEIGHT_NORMAL;
    let sb = DWRITE_FONT_WEIGHT_SEMI_BOLD;
    // order must match the `F` enum
    let v = vec![
        make_format(dw, ui, 13.0, sb, false)?,   // Seg
        make_format(dw, ui, 13.0, n, false)?,    // Tab
        make_format(dw, ui, 13.0, sb, false)?,   // TabOn
        make_format(dw, ui, 14.0, n, false)?,    // Title
        make_format(dw, ui, 11.0, sb, false)?,   // Section
        make_format(dw, ui, 12.0, n, false)?,    // Hint
        make_format(dw, ui, 11.5, sb, true)?,    // Keycap
        make_format(dw, ui, 13.0, n, true)?,     // Center
        make_format(dw, icons, 12.0, n, true)?,  // Icon12
        make_format(dw, icons, 14.0, n, true)?,  // Icon14
        make_format(dw, icons, 16.0, n, true)?,  // Icon16
        make_format(dw, ui, 14.0 * k, n, false)?,   // Row
        make_format(dw, ui, 11.5 * k, n, false)?,   // RowSub
        make_format(dw, mono, 11.0 * k, n, false)?, // RowMono
        make_format(dw, ui, 46.0, sb, true)?,       // Clock
        make_format(dw, ui, 12.5, sb, true)?,       // ClockSm
        make_format(dw, ui, 32.0, sb, false)?,      // Field
        make_format(dw, ui, 22.0, sb, true)?,       // Head
        make_format(dw, icons, 24.0, n, true)?,     // Icon24
        make_format(dw, ui, 21.0, sb, false)?,      // Big
    ];
    Ok(Fonts { v })
}

/// Blocks until the next compositor frame (vsync). Used only while an animation runs.
pub fn wait_vsync() {
    unsafe {
        let r = DCompositionWaitForCompositorClock(None, 20);
        if r == u32::MAX {
            let _ = DwmFlush();
        }
    }
}
