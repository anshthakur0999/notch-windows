//! The Devices tab: the audio outputs as chips (one click makes one the default), the connected
//! devices with Disconnect, the paired ones that are not connected with Connect, the "Add a
//! device" screen, and the two empty states (nothing paired, Bluetooth off).
//!
//! Plain data in, so the layout can be tested: `Shape` (what is listed and how each row is doing,
//! enough to lay out and hit-test) and `DevicesView` (the same plus what to write). Geometry is in
//! DIPs as offsets from the expanded pill; the rows scroll by `scroll`.

use super::devices_view::{BAR_W, battery_bar, battery_icon, heading, kind_glyph, out_glyph, right_text};
use super::timer_view::{centred, lit};
use super::*;
use crate::audio::Output;
use crate::devices::Device;
use crate::render::{F, Renderer, color, mix};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- what a row is doing

/// How one device row is doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Row {
    #[default]
    Idle,
    /// Connecting, disconnecting or pairing: a ring where the button was.
    Busy,
    /// It did not work: the row says so and offers "Try again" until the tab is left.
    Failed,
    /// Windows looks after this device (Bluetooth LE): nothing to press.
    Manual,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Screen {
    #[default]
    List,
    /// "Add a device": searching for, and pairing with, nearby devices.
    Add,
}

/// What the tab lists, as sizes and states.
#[derive(Clone, Copy, Default)]
pub struct Shape<'a> {
    pub screen: Screen,
    /// The Bluetooth radio is off.
    pub off: bool,
    /// The width of each audio output's chip.
    pub chips: &'a [f32],
    pub conn: &'a [Row],
    pub avail: &'a [Row],
    /// The Add screen's rows: how each is doing, and whether a code is on show under it.
    pub near: &'a [(Row, bool)],
}

/// `Shape`'s data, owned by whoever builds a frame or a hit-test (`shape()` borrows it).
#[derive(Default)]
pub struct Rows {
    pub screen: Screen,
    pub off: bool,
    pub chips: Vec<f32>,
    pub conn: Vec<Row>,
    pub avail: Vec<Row>,
    pub near: Vec<(Row, bool)>,
}

impl Rows {
    pub fn shape(&self) -> Shape<'_> {
        Shape { screen: self.screen, off: self.off, chips: &self.chips, conn: &self.conn, avail: &self.avail, near: &self.near }
    }
}

// ---------------------------------------------------------------- the connects and disconnects

/// A connect or disconnect long enough to show a ring has been started from the tab.
const SETTLE: Duration = Duration::from_secs(3);
/// A device that connects within this long of the click is the click's doing, not news.
const OURS: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug)]
enum State {
    Busy,
    /// The worker saw it work; Windows' own report that it did is still on its way.
    Settling(Instant),
    Failed,
}

#[derive(Clone, Copy, Debug)]
struct Link {
    addr: u64,
    connect: bool,
    state: State,
}

/// The connects and disconnects the user started from the tab, and how each is going.
#[derive(Default)]
pub struct Links {
    rows: Vec<Link>,
}

impl Links {
    /// A click (or a "Try again"): replaces whatever was said about the device before.
    pub fn start(&mut self, addr: u64, connect: bool) {
        self.rows.retain(|l| l.addr != addr);
        self.rows.push(Link { addr, connect, state: State::Busy });
    }

    /// The worker is done.
    pub fn finish(&mut self, addr: u64, connect: bool, ok: bool, now: Instant) {
        if let Some(l) = self.rows.iter_mut().find(|l| l.addr == addr && l.connect == connect) {
            l.state = if ok { State::Settling(now) } else { State::Failed };
        }
    }

    /// The row of a device in the connected list (`connect`: false: what a click would do there is
    /// disconnect) or the available one. `None` address: Windows connects it itself.
    pub fn row(&self, addr: Option<u64>, connect: bool, now: Instant) -> Row {
        let Some(addr) = addr else { return Row::Manual };
        match self.rows.iter().find(|l| l.addr == addr && l.connect == connect).map(|l| l.state) {
            Some(State::Busy) => Row::Busy,
            Some(State::Settling(t)) if now.duration_since(t) < SETTLE => Row::Busy,
            Some(State::Failed) => Row::Failed,
            _ => Row::Idle,
        }
    }

    /// Whether `addr` connected because the user asked it to (so it is not announced as news).
    pub fn ours(&self, addr: Option<u64>, now: Instant) -> bool {
        addr.is_some_and(|a| {
            self.rows.iter().any(|l| {
                l.addr == a
                    && l.connect
                    && match l.state {
                        State::Busy => true,
                        State::Settling(t) => now.duration_since(t) < OURS,
                        State::Failed => false,
                    }
            })
        })
    }

    /// Something is going on that needs a ring turning.
    pub fn busy(&self, now: Instant) -> bool {
        self.rows.iter().any(|l| match l.state {
            State::Busy => true,
            State::Settling(t) => now.duration_since(t) < SETTLE,
            State::Failed => false,
        })
    }

    /// The tab was left: failures do not follow it.
    pub fn clear_failed(&mut self) {
        self.rows.retain(|l| !matches!(l.state, State::Failed));
    }

    pub fn prune(&mut self, now: Instant) {
        self.rows.retain(|l| !matches!(l.state, State::Settling(t) if now.duration_since(t) >= OURS));
    }
}

// ---------------------------------------------------------------- the list's geometry

const ROW_X: f32 = 16.0;
const ROW_W: f32 = 408.0;
const HEAD_H: f32 = 22.0;
const OUT_HEAD: f32 = 56.0;
const CHIPS_TOP: f32 = 80.0;
const CHIP_H: f32 = 32.0;
const CHIP_PITCH: f32 = 38.0;
const CHIP_GAP: f32 = 8.0;
const ROW_H: f32 = 50.0;
const ROW_GAP: f32 = 4.0;
/// The "Open Bluetooth settings" line under a row that failed.
const LINK_H: f32 = 24.0;
const NOTE_H: f32 = 28.0;
const BTN_W: f32 = 108.0;
const BTN_H: f32 = 34.0;
const CARD_H: f32 = 212.0;
const CODE_H: f32 = 86.0;
/// Where the Add screen's rows start (its header above does not scroll).
const NEAR_TOP: f32 = 132.0;

/// A chip as wide as its label (the icon and the padding come with it).
pub fn chip_width(label_w: f32) -> f32 {
    (38.0 + label_w + 16.0).clamp(72.0, ROW_W)
}

/// What a chip says: for earbuds and headphones the device behind them, otherwise the output's own name.
pub fn chip_label(o: &Output) -> &str {
    if o.kind == crate::audio::OutKind::Headphones && !o.sub.is_empty() { &o.sub } else { &o.name }
}

fn row_h(s: Row) -> f32 {
    if s == Row::Failed { ROW_H + LINK_H } else { ROW_H }
}

fn near_h(s: Row, code: bool) -> f32 {
    if code && s == Row::Busy { ROW_H + 6.0 + CODE_H + 6.0 } else { ROW_H }
}

/// Where the lists scroll, cut off just above the footer rule and under the header.
pub fn viewport() -> RectF {
    at(0.0, 52.0, EXPANDED_W, FOOTER_SEP - 4.0 - 52.0)
}

fn near_viewport() -> RectF {
    at(0.0, NEAR_TOP - 2.0, EXPANDED_W, FOOTER_SEP - 4.0 - (NEAR_TOP - 2.0))
}

pub struct Layout {
    pub out_head: RectF,
    pub chips: Vec<RectF>,
    pub conn_head: Option<RectF>,
    pub conn: Vec<RectF>,
    /// "Nothing connected", when something is paired but not connected.
    pub none: Option<RectF>,
    pub avail_head: Option<RectF>,
    pub avail: Vec<RectF>,
    /// "Every paired device is connected".
    pub all_on: Option<RectF>,
    /// The heading above the empty-state card.
    pub card_head: Option<RectF>,
    /// The card for "nothing paired" or "Bluetooth is off".
    pub card: Option<RectF>,
    /// Where the content ends, as an offset from the pill's top, before scrolling.
    pub bottom: f32,
}

pub fn layout(s: &Shape, scroll: f32) -> Layout {
    let y = |dy: f32| dy - scroll;
    let strip = |cur: f32, h: f32| at(ROW_X, y(cur), ROW_W, h);
    let (mut x, mut top) = (ROW_X, CHIPS_TOP);
    let mut chips = Vec::new();
    for &w in s.chips {
        if x > ROW_X && x + w > ROW_X + ROW_W {
            (x, top) = (ROW_X, top + CHIP_PITCH);
        }
        chips.push(at(x, y(top), w, CHIP_H));
        x += w + CHIP_GAP;
    }
    let mut cur = top + if s.chips.is_empty() { 0.0 } else { CHIP_H } + 12.0;
    let mut l = Layout {
        out_head: strip(OUT_HEAD, HEAD_H),
        chips,
        conn_head: None,
        conn: Vec::new(),
        none: None,
        avail_head: None,
        avail: Vec::new(),
        all_on: None,
        card_head: None,
        card: None,
        bottom: 0.0,
    };
    if s.off || (s.conn.is_empty() && s.avail.is_empty()) {
        l.card_head = Some(strip(cur, HEAD_H));
        cur += HEAD_H + 4.0;
        l.card = Some(strip(cur, CARD_H));
        cur += CARD_H;
    } else {
        l.conn_head = Some(strip(cur, HEAD_H));
        cur += HEAD_H + 2.0;
        if s.conn.is_empty() {
            l.none = Some(strip(cur, NOTE_H));
            cur += NOTE_H + ROW_GAP;
        }
        for &st in s.conn {
            l.conn.push(strip(cur, row_h(st)));
            cur += row_h(st) + ROW_GAP;
        }
        cur += 6.0;
        l.avail_head = Some(strip(cur, HEAD_H));
        cur += HEAD_H + 2.0;
        if s.avail.is_empty() {
            l.all_on = Some(strip(cur, NOTE_H));
            cur += NOTE_H + ROW_GAP;
        }
        for &st in s.avail {
            l.avail.push(strip(cur, row_h(st)));
            cur += row_h(st) + ROW_GAP;
        }
    }
    l.bottom = cur + 4.0;
    l
}

pub struct AddLayout {
    pub back: RectF,
    pub title: RectF,
    pub stop: RectF,
    pub status: RectF,
    pub rows: Vec<RectF>,
    pub bottom: f32,
}

pub fn add_layout(s: &Shape, scroll: f32) -> AddLayout {
    let mut cur = NEAR_TOP;
    let rows = s
        .near
        .iter()
        .map(|&(st, code)| {
            let r = at(ROW_X, cur - scroll, ROW_W, near_h(st, code));
            cur += near_h(st, code) + ROW_GAP;
            r
        })
        .collect();
    AddLayout {
        back: at(ROW_X, 58.0, 36.0, 36.0),
        title: at(ROW_X + 46.0, 58.0, 230.0, 36.0),
        stop: at(ROW_X + ROW_W - BTN_W, 59.0, BTN_W, BTN_H),
        status: at(ROW_X + 4.0, 100.0, ROW_W - 8.0, 24.0),
        rows,
        bottom: cur + 4.0,
    }
}

/// How far the tab can scroll.
pub fn max_scroll(s: &Shape) -> f32 {
    match s.screen {
        Screen::List => (layout(s, 0.0).bottom - (FOOTER_SEP - 4.0)).max(0.0),
        Screen::Add => (add_layout(s, 0.0).bottom - (FOOTER_SEP - 4.0)).max(0.0),
    }
}

/// A row's button (Connect, Disconnect, Pair, Try again, Cancel) at its right end.
fn row_btn(row: RectF) -> RectF {
    RectF { l: row.r - 4.0 - BTN_W, t: row.t + (ROW_H - BTN_H) / 2.0, r: row.r - 4.0, b: row.t + (ROW_H + BTN_H) / 2.0 }
}

/// "Open Bluetooth settings" under a failed row.
fn row_link(row: RectF) -> RectF {
    RectF { l: row.r - 4.0 - 190.0, t: row.t + ROW_H, r: row.r - 4.0, b: row.t + ROW_H + LINK_H }
}

fn tile_rect(row: RectF) -> RectF {
    RectF { l: row.l + 4.0, t: row.t + (ROW_H - 40.0) / 2.0, r: row.l + 44.0, b: row.t + (ROW_H + 40.0) / 2.0 }
}

/// "+ Add device" at the right of the Available heading.
fn add_link(head: RectF) -> RectF {
    RectF { l: head.r - 8.0 - 106.0, t: head.t - 4.0, r: head.r, b: head.b + 4.0 }
}

/// The two buttons of an empty-state card, side by side and centred: the first is `wa` wide.
fn card_btns(card: RectF, wa: f32) -> (RectF, RectF) {
    const WB: f32 = 150.0;
    let l = card.cx() - (wa + 12.0 + WB) / 2.0;
    let (t, b) = (card.t + 160.0, card.t + 198.0);
    (RectF { l, t, r: l + wa, b }, RectF { l: l + wa + 12.0, t, r: l + wa + 12.0 + WB, b })
}
const ADD_BTN_W: f32 = 128.0;
const ON_BTN_W: f32 = 168.0;

fn row_hit(row: RectF, st: Row, x: f32, y: f32, act: Hit) -> Option<Hit> {
    match st {
        Row::Idle if row_btn(row).contains(x, y) => Some(act),
        Row::Failed if row_btn(row).contains(x, y) => Some(act),
        Row::Failed if row_link(row).contains(x, y) => Some(Hit::BtSettings),
        _ => None,
    }
}

/// Hit-test inside the Devices tab (`None`: not on a control).
pub fn hit(x: f32, y: f32, scroll: f32, s: &Shape) -> Option<Hit> {
    if s.screen == Screen::Add {
        return add_hit(x, y, scroll, s);
    }
    if !viewport().contains(x, y) {
        return None;
    }
    let l = layout(s, scroll);
    if let Some(i) = l.chips.iter().position(|r| r.contains(x, y)) {
        return Some(Hit::OutputRow(i));
    }
    for (i, (row, st)) in l.conn.iter().zip(s.conn).enumerate() {
        if let Some(h) = row_hit(*row, *st, x, y, Hit::DeviceDisconnect(i)) {
            return Some(h);
        }
    }
    for (i, (row, st)) in l.avail.iter().zip(s.avail).enumerate() {
        if let Some(h) = row_hit(*row, *st, x, y, Hit::DeviceConnect(i)) {
            return Some(h);
        }
    }
    if l.avail_head.is_some_and(|h| add_link(h).contains(x, y)) {
        return Some(Hit::AddOpen);
    }
    let card = l.card?;
    let (a, b) = card_btns(card, if s.off { ON_BTN_W } else { ADD_BTN_W });
    if a.contains(x, y) {
        return Some(if s.off { Hit::BtOn } else { Hit::AddOpen });
    }
    b.contains(x, y).then_some(Hit::BtSettings)
}

fn add_hit(x: f32, y: f32, scroll: f32, s: &Shape) -> Option<Hit> {
    let l = add_layout(s, scroll);
    if l.back.contains(x, y) {
        return Some(Hit::AddBack);
    }
    if l.stop.contains(x, y) {
        return Some(Hit::AddStop);
    }
    if !near_viewport().contains(x, y) {
        return None;
    }
    for (i, (row, &(st, _))) in l.rows.iter().zip(s.near).enumerate() {
        if row_btn(*row).contains(x, y) {
            return match st {
                Row::Busy => Some(Hit::AddCancel),
                _ => Some(Hit::AddPair(i)),
            };
        }
    }
    None
}

// ---------------------------------------------------------------- drawing

fn tile(r: &Renderer, rect: RectF, glyph: char, bg: u32, fg: u32, alpha: f32) {
    r.fill_rrect(rect.d2d(), 12.0, color(bg, alpha));
    r.glyph(glyph, F::Icon16, rect.cx(), rect.cy(), color(fg, alpha));
}

fn button(r: &Renderer, rect: RectF, label: &str, fill: u32, ink: u32, hovered: bool, alpha: f32) {
    r.fill_rrect(rect.d2d(), rect.h() / 2.0, color(lit(r, fill, hovered), alpha));
    centred(r, label, F::Seg, rect, color(ink, alpha));
}

/// A turning ring: a faint circle with a short bright arc running round it.
pub fn spinner(r: &Renderer, cx: f32, cy: f32, rad: f32, clock: f32, alpha: f32) {
    let pal = r.pal;
    r.ring(cx, cy, rad, 2.0, color(mix(pal.rest, pal.wash, 0.2), alpha));
    r.arc_from(cx, cy, rad, 2.0, color(pal.device, alpha), (clock * 330.0) % 360.0, 90.0);
}

/// The accent button of the design: a deep fill, accent text.
fn accent_fill(r: &Renderer) -> u32 {
    mix(r.pal.body, r.pal.device, 0.2)
}

/// A button's place on a row, or the ring that stands in for it while it works.
fn row_action(r: &Renderer, row: RectF, st: Row, label: &str, accent: bool, hovered: bool, clock: f32, alpha: f32) {
    let pal = r.pal;
    let b = row_btn(row);
    match st {
        Row::Manual => {}
        Row::Busy => {
            r.fill_rrect(b.d2d(), b.h() / 2.0, color(pal.tile, alpha));
            spinner(r, b.cx(), b.cy(), 8.0, clock, alpha);
        }
        Row::Failed => button(r, b, "Try again", accent_fill(r), pal.device, hovered, alpha),
        Row::Idle if accent => button(r, b, label, accent_fill(r), pal.device, hovered, alpha),
        Row::Idle => button(r, b, label, pal.tile, pal.text, hovered, alpha),
    }
    if st == Row::Failed {
        let k = row_link(row);
        right_text(r, "Open Bluetooth settings", F::Hint, k.r - 2.0, k.t + 2.0, k.h() - 4.0, color(pal.device, alpha));
    }
}

/// The line under a device's name: a status glyph and words, from `x`.
fn status(r: &Renderer, st: Row, x: f32, row: RectF, busy: (&str, u32), fail_text: &str, alpha: f32) -> Option<f32> {
    let pal = r.pal;
    let (t, rt) = (row.t + 26.0, row_btn(row).l - 8.0);
    let line = |l: f32| RectF { l, t, r: rt.max(l + 1.0), b: t + 16.0 }.d2d();
    match st {
        Row::Busy => {
            r.text(busy.0, F::Hint, line(x), color(busy.1, alpha));
            None
        }
        Row::Failed => {
            r.glyph('\u{E7BA}', F::Icon12, x + 6.0, t + 8.0, color(pal.danger, alpha));
            r.text(fail_text, F::Hint, line(x + 17.0), color(pal.danger, alpha));
            None
        }
        _ => Some(x),
    }
}

pub fn draw(r: &Renderer, v: &View, alpha: f32) {
    let page = &v.devices;
    if page.shape.screen == Screen::Add {
        draw_add(r, v, alpha);
    } else {
        draw_list(r, v, alpha);
    }
    let p = exp();
    let cy = p.t + FOOTER_CY;
    let note = match (page.shape.screen, page.shape.off) {
        (Screen::Add, _) => "Nothing is searched outside this screen",
        (_, true) => "No Bluetooth searching while it is off",
        _ => "Searches only while Add device is open",
    };
    pill::hline(r, p.t + FOOTER_SEP, alpha);
    r.text(note, F::Hint, RectF { l: p.l + 20.0, t: cy - 9.0, r: p.l + 300.0, b: cy + 9.0 }.d2d(), color(r.pal.muted, alpha));
    pill::collapse_hint(r, cy, alpha);
}

fn draw_list(r: &Renderer, v: &View, alpha: f32) {
    let pal = r.pal;
    let page = &v.devices;
    let l = layout(&page.shape, v.scroll);
    r.push_clip(viewport().d2d());

    heading(r, "Audio output", "One click to switch", l.out_head, alpha);
    for (i, (o, chip)) in page.outputs.iter().zip(&l.chips).enumerate() {
        let (on, hov) = (o.default, v.hover == Hit::OutputRow(i));
        r.fill_rrect(chip.d2d(), CHIP_H / 2.0, color(if on { pal.device } else { lit(r, pal.tile, hov) }, alpha));
        let ink = if on { pal.ink } else { pal.text };
        r.glyph(out_glyph(o.kind), F::Icon16, chip.l + 22.0, chip.cy(), color(ink, alpha));
        r.text(chip_label(o), F::Seg, RectF { l: chip.l + 38.0, r: chip.r - 10.0, ..*chip }.d2d(), color(ink, alpha));
    }

    if let (Some(h), Some(card)) = (l.card_head, l.card) {
        heading(r, if page.shape.off { "Devices" } else { "Your devices" }, "", h, alpha);
        if page.shape.off {
            bt_off_card(r, v, card, alpha);
        } else {
            no_devices_card(r, v, card, alpha);
        }
    }
    if let Some(h) = l.conn_head {
        heading(r, "Connected", &page.conn.len().to_string(), h, alpha);
    }
    if let Some(n) = l.none {
        r.text("Nothing connected", F::Hint, RectF { l: n.l + 8.0, ..n }.d2d(), color(pal.muted, alpha));
    }
    for (i, ((d, row), st)) in page.conn.iter().zip(&l.conn).zip(page.shape.conn).enumerate() {
        let hov = v.hover == Hit::DeviceDisconnect(i);
        let tint = if *st == Row::Failed || d.low() { pal.danger } else { pal.device };
        tile(r, tile_rect(*row), kind_glyph(d.kind), mix(pal.body, tint, 0.2), tint, alpha);
        let x = row.l + 54.0;
        let name_r = if *st == Row::Manual { row.r - 12.0 } else { row_btn(*row).l - 8.0 };
        r.text(&d.name, F::Seg, RectF { l: x, t: row.t + 6.0, r: name_r, b: row.t + 24.0 }.d2d(), color(pal.text, alpha));
        if let Some(x) = status(r, *st, x, *row, ("Disconnecting\u{2026}", pal.muted), "Didn't disconnect.", alpha) {
            battery_line(r, d, x, row.t + 26.0, alpha);
        }
        row_action(r, *row, *st, "Disconnect", false, hov, page.clock, alpha);
    }
    if let Some(h) = l.avail_head {
        let hov = v.hover == Hit::AddOpen;
        r.text("Available", F::Seg, RectF { l: h.l + 8.0, ..h }.d2d(), color(pal.muted, alpha));
        let a = add_link(h);
        let c = color(lit(r, pal.device, hov), alpha);
        r.glyph('\u{E710}', F::Icon14, a.l + 14.0, a.cy(), c);
        r.text("Add device", F::Seg, RectF { l: a.l + 28.0, ..a }.d2d(), c);
    }
    if let Some(n) = l.all_on {
        r.text("Every paired device is connected", F::Hint, RectF { l: n.l + 8.0, ..n }.d2d(), color(pal.muted, alpha));
    }
    for (i, ((d, row), st)) in page.avail.iter().zip(&l.avail).zip(page.shape.avail).enumerate() {
        let hov = v.hover == Hit::DeviceConnect(i);
        let tint = if *st == Row::Failed { pal.danger } else { pal.device };
        let (bg, fg) = match st {
            Row::Failed => (mix(pal.body, tint, 0.2), tint),
            Row::Busy => (mix(pal.body, tint, 0.2), tint),
            _ => (pal.tile, pal.text2),
        };
        tile(r, tile_rect(*row), kind_glyph(d.kind), bg, fg, alpha);
        let x = row.l + 54.0;
        let name_r = if *st == Row::Manual { row.r - 12.0 } else { row_btn(*row).l - 8.0 };
        r.text(&d.name, F::Seg, RectF { l: x, t: row.t + 6.0, r: name_r, b: row.t + 24.0 }.d2d(), color(pal.text, alpha));
        if let Some(x) = status(r, *st, x, *row, ("Connecting\u{2026}", pal.device), "Didn't connect. Is it on and nearby?", alpha) {
            let sub = if *st == Row::Manual { "Paired \u{B7} connects on its own" } else { "Paired" };
            r.text(sub, F::Hint, RectF { l: x, t: row.t + 26.0, r: name_r, b: row.t + 42.0 }.d2d(), color(pal.muted, alpha));
        }
        row_action(r, *row, *st, "Connect", true, hov, page.clock, alpha);
    }
    r.pop_clip();
}

/// "Connected", the battery and its bar: the second line of a connected device.
fn battery_line(r: &Renderer, d: &Device, x: f32, top: f32, alpha: f32) {
    let pal = r.pal;
    let (low, cy) = (d.low(), top + 8.0);
    let tint = if low { pal.danger } else { pal.device };
    let line = |l: f32, w: f32| RectF { l, t: top, r: l + w, b: top + 16.0 }.d2d();
    let mut x = x;
    let word = if low { "Battery low" } else { "Connected" };
    if low {
        battery_icon(r, x + 7.0, cy, color(pal.danger, alpha));
        x += 19.0;
    } else {
        r.fill_circle(x + 3.0, cy, 3.0, color(pal.device, alpha));
        x += 12.0;
    }
    let w = r.measure(F::Hint, word);
    r.text(word, F::Hint, line(x, w + 4.0), color(if low { pal.danger } else { pal.muted }, alpha));
    x += w + 8.0;
    match d.battery {
        Some(pct) => {
            let s = format!("{pct}%");
            let w = r.measure(F::Seg, &s);
            r.text(&s, F::Seg, line(x, w + 4.0), color(if low { pal.danger } else { pal.text }, alpha));
            x += w + 6.0;
            battery_bar(r, RectF { l: x, t: cy - 2.5, r: x + BAR_W, b: cy + 2.5 }, pct, tint, alpha);
        }
        None => r.text("Battery unknown", F::Hint, line(x, 110.0), color(pal.muted, alpha)),
    }
}

fn card_top(r: &Renderer, card: RectF, glyph: char, off: bool, alpha: f32) {
    let pal = r.pal;
    r.fill_rrect(card.d2d(), 22.0, color(pal.field, alpha));
    let c = (card.cx(), card.t + 46.0);
    r.fill_circle(c.0, c.1, 24.0, color(if off { pal.tile } else { mix(pal.field, pal.device, 0.2) }, alpha));
    let ink = if off { pal.muted } else { pal.device };
    r.glyph(glyph, F::Icon24, c.0, c.1, color(ink, alpha));
    if off {
        r.line(c.0 - 11.0, c.1 - 11.0, c.0 + 11.0, c.1 + 11.0, color(pal.field, alpha), 4.5);
        r.line(c.0 - 11.0, c.1 - 11.0, c.0 + 11.0, c.1 + 11.0, color(ink, alpha), 1.8);
    }
}

fn card_lines(r: &Renderer, card: RectF, title: &str, lines: &[&str], alpha: f32) {
    let pal = r.pal;
    let line = |s: &str, f: F, cy: f32, col: u32| {
        r.text(s, f, RectF { l: card.l, t: card.t + cy - 16.0, r: card.r, b: card.t + cy + 16.0 }.d2d(), color(col, alpha));
    };
    line(title, F::Head, 90.0, pal.text);
    for (i, s) in lines.iter().enumerate() {
        line(s, F::Center, 120.0 + 18.0 * i as f32, pal.muted);
    }
}

fn no_devices_card(r: &Renderer, v: &View, card: RectF, alpha: f32) {
    let pal = r.pal;
    card_top(r, card, '\u{E702}', false, alpha);
    card_lines(r, card, "No paired devices", &["Pair earbuds, a controller or a mouse once. After that", "you can connect and disconnect them here."], alpha);
    let (a, b) = card_btns(card, ADD_BTN_W);
    r.fill_rrect(a.d2d(), a.h() / 2.0, color(lit(r, pal.device, v.hover == Hit::AddOpen), alpha));
    r.glyph('\u{E710}', F::Icon14, a.l + 26.0, a.cy(), color(pal.ink, alpha));
    r.text("Add device", F::Seg, RectF { l: a.l + 42.0, ..a }.d2d(), color(pal.ink, alpha));
    button(r, b, "Bluetooth settings", pal.tile, pal.text, v.hover == Hit::BtSettings, alpha);
}

fn bt_off_card(r: &Renderer, v: &View, card: RectF, alpha: f32) {
    let pal = r.pal;
    card_top(r, card, '\u{E702}', true, alpha);
    card_lines(r, card, "Bluetooth is off", &["Turn it on to connect earbuds, controllers and mice."], alpha);
    let (a, b) = card_btns(card, ON_BTN_W);
    button(r, a, "Turn on Bluetooth", pal.device, pal.ink, v.hover == Hit::BtOn, alpha);
    button(r, b, "Bluetooth settings", pal.tile, pal.text, v.hover == Hit::BtSettings, alpha);
}

fn draw_add(r: &Renderer, v: &View, alpha: f32) {
    let pal = r.pal;
    let page = &v.devices;
    let l = add_layout(&page.shape, v.scroll);
    r.fill_circle(l.back.cx(), l.back.cy(), l.back.w() / 2.0, color(lit(r, pal.tile, v.hover == Hit::AddBack), alpha));
    r.glyph('\u{E76B}', F::Icon16, l.back.cx(), l.back.cy(), color(pal.text, alpha));
    r.text("Add a device", F::Big, l.title.d2d(), color(pal.text, alpha));
    let hov = v.hover == Hit::AddStop;
    if page.searching {
        button(r, l.stop, "Stop", pal.tile, pal.text, hov, alpha);
        spinner(r, l.status.l + 9.0, l.status.cy(), 6.5, page.clock, alpha);
        let w = r.measure(F::Seg, "Searching nearby");
        r.text("Searching nearby", F::Seg, RectF { l: l.status.l + 24.0, r: l.status.l + 30.0 + w, ..l.status }.d2d(), color(pal.text, alpha));
        r.text("Stops by itself after 30 seconds", F::Hint, RectF { l: l.status.l + 34.0 + w, ..l.status }.d2d(), color(pal.muted, alpha));
    } else {
        button(r, l.stop, "Search again", accent_fill(r), pal.device, hov, alpha);
        r.text("Search stopped", F::Hint, RectF { l: l.status.l + 4.0, ..l.status }.d2d(), color(pal.muted, alpha));
    }

    r.push_clip(near_viewport().d2d());
    if page.near.is_empty() {
        let msg = if page.searching { "Put the device in pairing mode. It shows up here." } else { "Nothing found." };
        r.text(msg, F::Hint, at(ROW_X + 8.0, NEAR_TOP + 4.0, ROW_W - 16.0, 22.0).d2d(), color(pal.muted, alpha));
    }
    for (i, ((n, row), &(st, _))) in page.near.iter().zip(&l.rows).zip(page.shape.near).enumerate() {
        let busy = st == Row::Busy;
        let tint = if st == Row::Failed { pal.danger } else { pal.device };
        if busy {
            r.fill_rrect(RectF { t: row.t - 2.0, b: row.b, l: row.l - 4.0, r: row.r + 4.0 }.d2d(), 20.0, color(mix(pal.body, pal.device, 0.09), alpha));
        }
        let (bg, fg) = if busy || st == Row::Failed { (mix(pal.body, tint, 0.22), tint) } else { (pal.tile, pal.text2) };
        let glyph = if n.name.is_empty() { '\u{E702}' } else { kind_glyph(n.kind) };
        tile(r, tile_rect(*row), glyph, bg, fg, alpha);
        let x = row.l + 54.0;
        let name_r = row_btn(*row).l - 8.0;
        let name = if n.name.is_empty() { "Unnamed device" } else { &n.name };
        r.text(name, F::Seg, RectF { l: x, t: row.t + 6.0, r: name_r, b: row.t + 24.0 }.d2d(), color(pal.text, alpha));
        let sub = if n.name.is_empty() { "Nearby, no name shared" } else { "Nearby" };
        if let Some(x) = status(r, st, x, *row, ("Pairing\u{2026}", pal.device), "Couldn't pair. Is it in pairing mode?", alpha) {
            r.text(sub, F::Hint, RectF { l: x, t: row.t + 26.0, r: name_r, b: row.t + 42.0 }.d2d(), color(pal.muted, alpha));
        }
        let b = row_btn(*row);
        match st {
            Row::Busy => button(r, b, "Cancel", pal.tile, pal.text, v.hover == Hit::AddCancel, alpha),
            Row::Failed => button(r, b, "Try again", accent_fill(r), pal.device, v.hover == Hit::AddPair(i), alpha),
            _ => button(r, b, "Pair", accent_fill(r), pal.device, v.hover == Hit::AddPair(i), alpha),
        }
        if let (true, Some(code)) = (busy, page.code) {
            let box_ = RectF { l: row.l + 4.0, t: row.t + ROW_H + 6.0, r: row.r - 4.0, b: row.t + ROW_H + 6.0 + CODE_H };
            r.fill_rrect(box_.d2d(), 16.0, color(pal.field, alpha));
            let what = if n.kind == crate::devices::Kind::Keyboard { "the keyboard" } else { "the device" };
            centred(r, &format!("Type this code on {what}, then press Enter"), F::Center, RectF { t: box_.t + 8.0, b: box_.t + 32.0, ..box_ }, color(pal.muted, alpha));
            centred(r, code, F::Field, RectF { t: box_.t + 34.0, b: box_.b - 6.0, ..box_ }, color(pal.text, alpha));
        }
    }
    r.pop_clip();
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: [Row; 2] = [Row::Idle, Row::Idle];

    fn shape<'a>(chips: &'a [f32], conn: &'a [Row], avail: &'a [Row]) -> Shape<'a> {
        Shape { chips, conn, avail, ..Shape::default() }
    }

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn a_connect_shows_a_ring_until_windows_reports_it_and_a_failure_stays_until_the_tab_is_left() {
        let (mut l, t) = (Links::default(), t0());
        assert_eq!(l.row(Some(1), true, t), Row::Idle);
        assert_eq!(l.row(None, true, t), Row::Manual, "Windows looks after an LE device");
        l.start(1, true);
        assert_eq!(l.row(Some(1), true, t), Row::Busy);
        assert_eq!(l.row(Some(1), false, t), Row::Idle, "a connect says nothing about the connected list's row");
        assert!(l.ours(Some(1), t) && l.busy(t), "so the connect card stays down, and the ring turns");
        l.finish(1, true, true, t);
        assert_eq!(l.row(Some(1), true, t + Duration::from_secs(1)), Row::Busy, "worked: the ring stays till Windows says so");
        assert_eq!(l.row(Some(1), true, t + SETTLE), Row::Idle, "and no longer than that");
        assert!(l.ours(Some(1), t + Duration::from_secs(5)), "a late report is still the click's doing");
        assert!(!l.ours(Some(1), t + OURS));
        // a failure stays on the row, with no timer to clear it
        l.start(2, false);
        l.finish(2, false, false, t);
        assert_eq!(l.row(Some(2), false, t + Duration::from_secs(600)), Row::Failed);
        assert!(!l.busy(t + Duration::from_secs(600)) && !l.ours(Some(2), t));
        l.start(2, false); // Try again
        assert_eq!(l.row(Some(2), false, t), Row::Busy);
        l.finish(2, false, false, t);
        l.clear_failed();
        assert_eq!(l.row(Some(2), false, t), Row::Idle, "leaving the tab clears it");
        l.prune(t + OURS);
        assert!(!l.ours(Some(1), t + OURS));
    }

    #[test]
    fn chips_are_as_wide_as_their_label_and_wrap_to_a_second_line() {
        assert_eq!(chip_width(10.0), 72.0, "never a sliver");
        assert_eq!(chip_width(900.0), ROW_W, "never wider than the panel");
        assert_eq!(chip_width(60.0), 38.0 + 60.0 + 16.0);
        let one = layout(&shape(&[117.0, 134.0, 106.0], &[], &[]), 0.0);
        assert!(one.chips.windows(2).all(|c| c[0].r + CHIP_GAP <= c[1].l + 0.01 && c[0].t == c[1].t), "the design's three on one line");
        let two = layout(&shape(&[150.0, 150.0, 150.0], &[], &[]), 0.0);
        assert_eq!(two.chips[2].l, ROW_X + exp().l, "the third starts a new line");
        assert!(two.chips[2].t >= two.chips[0].b, "under the first");
        assert!(two.chips.iter().all(|c| c.r <= exp().l + ROW_X + ROW_W + 0.01), "none sticks out of the panel");
    }

    #[test]
    fn the_design_fits_without_scrolling_and_the_sections_do_not_touch() {
        let s = shape(&[117.0, 134.0, 106.0], &IDLE, &IDLE);
        assert_eq!(max_scroll(&s), 0.0, "two connected and two available, as designed");
        let e = exp();
        for (chips, conn, avail) in [
            (&[117.0, 134.0][..], &IDLE[..], &IDLE[..]),
            (&[117.0][..], &[][..], &IDLE[..]),
            (&[117.0][..], &IDLE[..], &[][..]),
            (&[117.0, 134.0, 100.0, 120.0][..], &[Row::Failed, Row::Busy][..], &[Row::Manual, Row::Failed][..]),
        ] {
            let l = layout(&shape(chips, conn, avail), 0.0);
            let mut rows: Vec<RectF> = vec![l.out_head];
            rows.extend(&l.chips);
            rows.extend(l.conn_head);
            rows.extend(l.none);
            rows.extend(&l.conn);
            rows.extend(l.avail_head);
            rows.extend(&l.avail);
            rows.extend(l.all_on);
            assert!(rows.windows(2).all(|w| w[0].b <= w[1].t + 0.01 || w[0].t == w[1].t), "{chips:?}: in order, not touching");
            assert!(rows.iter().all(|r| r.l >= e.l && r.r <= e.r), "inside the panel");
            assert_eq!((l.conn.len(), l.avail.len()), (conn.len(), avail.len()));
            assert!(l.bottom > rows.last().unwrap().b - e.t);
        }
        // a failed row has room for its Bluetooth settings line
        let l = layout(&shape(&[100.0], &[Row::Failed], &[]), 0.0);
        assert_eq!(l.conn[0].h(), ROW_H + LINK_H);
        assert!(max_scroll(&shape(&[100.0], &[Row::Idle; 8], &[Row::Idle; 4])) > 0.0, "a long list scrolls");
    }

    #[test]
    fn the_empty_states_replace_the_lists_only_when_nothing_is_paired_or_bluetooth_is_off() {
        let none = layout(&shape(&[100.0], &[], &[]), 0.0);
        assert!(none.card.is_some() && none.conn_head.is_none(), "nothing paired at all");
        let some = layout(&shape(&[100.0], &[], &[Row::Idle]), 0.0);
        assert!(some.card.is_none() && some.none.is_some() && some.avail.len() == 1, "paired but not connected: Nothing connected, then Available");
        let off = layout(&Shape { off: true, chips: &[100.0], conn: &IDLE, avail: &IDLE, ..Shape::default() }, 0.0);
        assert!(off.card.is_some() && off.conn.is_empty(), "off wins over whatever was listed");
        let all = layout(&shape(&[100.0], &[Row::Idle], &[]), 0.0);
        assert!(all.all_on.is_some() && all.avail_head.is_some(), "the Add device link stays reachable");
    }

    #[test]
    fn the_buttons_sit_inside_their_rows_and_clear_of_the_text() {
        let row = layout(&shape(&[100.0], &[Row::Failed], &[]), 0.0).conn[0];
        let (b, k) = (row_btn(row), row_link(row));
        assert!(b.l >= row.l && b.r <= row.r && b.t >= row.t && b.b <= row.t + ROW_H, "the button is in the row's first line");
        assert_eq!(b.w(), BTN_W);
        assert!(b.l - tile_rect(row).r > 150.0, "the name keeps room");
        assert!(k.t >= b.b && k.b <= row.b && k.r <= row.r, "the link is under the button, inside the row");
    }

    #[test]
    fn a_click_finds_what_it_is_on() {
        let s = shape(&[100.0, 120.0], &[Row::Idle, Row::Failed], &[Row::Idle, Row::Busy, Row::Manual]);
        let l = layout(&s, 0.0);
        let at_ = |r: RectF| hit(r.cx(), r.cy(), 0.0, &s);
        assert_eq!(at_(l.chips[1]), Some(Hit::OutputRow(1)));
        assert_eq!(at_(row_btn(l.conn[0])), Some(Hit::DeviceDisconnect(0)));
        assert_eq!(at_(row_btn(l.conn[1])), Some(Hit::DeviceDisconnect(1)), "Try again does the same");
        assert_eq!(at_(row_link(l.conn[1])), Some(Hit::BtSettings));
        assert_eq!(at_(row_btn(l.avail[0])), Some(Hit::DeviceConnect(0)));
        assert_eq!(at_(row_btn(l.avail[1])), None, "a ring is not a button");
        assert_eq!(at_(row_btn(l.avail[2])), None, "nothing to press on a device Windows looks after");
        assert_eq!(at_(l.conn[0]), None, "the rest of a row is only information");
        assert_eq!(at_(add_link(l.avail_head.unwrap())), Some(Hit::AddOpen));
        // a row scrolled under the header cannot be clicked
        let long = shape(&[100.0], &[Row::Idle; 6], &[Row::Idle; 3]);
        let s = layout(&long, 140.0);
        assert!(s.conn[0].t < viewport().t);
        assert_eq!(hit(row_btn(s.conn[0]).cx(), row_btn(s.conn[0]).cy(), 140.0, &long), None);
    }

    #[test]
    fn the_empty_cards_have_their_buttons() {
        let e = shape(&[100.0], &[], &[]);
        let card = layout(&e, 0.0).card.unwrap();
        let (a, b) = card_btns(card, ADD_BTN_W);
        assert!(a.l >= card.l && b.r <= card.r && b.b <= card.b && a.r < b.l, "inside the card, apart");
        assert_eq!(hit(a.cx(), a.cy(), 0.0, &e), Some(Hit::AddOpen));
        assert_eq!(hit(b.cx(), b.cy(), 0.0, &e), Some(Hit::BtSettings));
        assert_eq!(hit(card.cx(), card.t + 20.0, 0.0, &e), None);
        let off = Shape { off: true, ..e };
        let (a, b) = card_btns(layout(&off, 0.0).card.unwrap(), ON_BTN_W);
        assert!(a.r < b.l && b.r <= card.r);
        assert_eq!(hit(a.cx(), a.cy(), 0.0, &off), Some(Hit::BtOn));
        assert_eq!(hit(b.cx(), b.cy(), 0.0, &off), Some(Hit::BtSettings));
    }

    #[test]
    fn the_add_screen_has_back_stop_pair_and_cancel() {
        let near = [(Row::Idle, false), (Row::Busy, true), (Row::Failed, false)];
        let s = Shape { screen: Screen::Add, near: &near, ..Shape::default() };
        let l = add_layout(&s, 0.0);
        let at_ = |r: RectF| hit(r.cx(), r.cy(), 0.0, &s);
        assert_eq!(at_(l.back), Some(Hit::AddBack));
        assert_eq!(at_(l.stop), Some(Hit::AddStop));
        assert_eq!(at_(row_btn(l.rows[0])), Some(Hit::AddPair(0)));
        assert_eq!(at_(row_btn(l.rows[1])), Some(Hit::AddCancel), "the row that is pairing has Cancel");
        assert_eq!(at_(row_btn(l.rows[2])), Some(Hit::AddPair(2)), "Try again");
        assert!(l.rows[1].h() > l.rows[0].h() + CODE_H, "room for the code");
        assert!(l.rows.windows(2).all(|w| w[0].b <= w[1].t), "rows do not touch");
        assert!(l.rows[0].t >= l.status.b, "the list is under the header");
        assert!(l.stop.r <= exp().l + ROW_X + ROW_W && l.title.r < l.stop.l, "the title does not run into Stop");
        // the header stays put and the rows scroll
        let many = [(Row::Idle, false); 9];
        let s = Shape { screen: Screen::Add, near: &many, ..Shape::default() };
        assert!(max_scroll(&s) > 0.0);
        let far = max_scroll(&s);
        assert!(add_layout(&s, far).rows.last().unwrap().b <= exp().t + FOOTER_SEP - 3.9);
        assert_eq!(add_layout(&s, far).back.t, l.back.t);
    }
}
