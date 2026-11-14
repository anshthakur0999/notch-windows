<p align="center">
  <img src="assets/logo.png" width="96" alt="Notch logo: a pill split into an amber notepad half and a cyan clipboard half">
</p>

# Notch

A tiny floating pill at the top-centre of your screen (think Dynamic Island) with a **notepad**,
**clipboard history**, a **timer** and a **file shelf**. Written in Rust against raw Win32 + DirectComposition/Direct2D, built to cost
almost nothing while idle.

## Use it

| Action | How |
| --- | --- |
| Open | Hover the pill (it expands after a short dwell), click it, or press a hotkey |
| Notepad (toggle) | `Win+Alt+N` — opens the notepad with the cursor in it; press again or `Esc` to close |
| Clipboard | `Win+Alt+V` — opens the clipboard list with the search box ready (just start typing) |
| Timer | Click **Timer** in the header, type a time such as `25m`, press `Enter` (details below) |
| Shelf | Drop files on the pill, then open the **Shelf** tab (details below) |
| Switch tab | Click **Notepad** / **Clipboard** / **Timer** / **Shelf** in the header, or press `Tab` |
| Keep open | The pin in the header: the pill stays open until you unpin it, press `Esc`, or click the chevron |
| Close | Move the mouse away, `Esc`, the chevron, or click another app |
| Hide / show the pill | `Win+Alt+H`, the tray menu, or the commands below |
| Tray icon | Open · Hide/Show pill · Theme · Accent colour · Text size · Monitor · Pause clipboard capture · Start with Windows · Quit |

If a hotkey is already owned by another app, Notch falls back to `Ctrl+Alt+N` / `Ctrl+Alt+V` /
`Ctrl+Alt+H`. The tray tooltip always shows which hotkeys are active.

### Look and monitor (tray menu)

Right-click the tray icon. These apply immediately and are saved to `settings.json`, so there is
nothing to edit and no restart:

* **Theme**: Dark (default), Light, or Match Windows (follows Windows' app light/dark mode and
  switches with it).
* **Accent colour**: Classic keeps the logo's two tones (amber notepad, cyan clipboard) and gives the
  timer its own violet; Sky, Amber, Mint, Violet and Rose use one colour for all three tabs, the
  selected entry and the copy peek's check.
* **Text size**: Small, Default or Large for the text you read: the notes and the clipboard
  entries. The header, tabs and hints keep their size.
* **Monitor**: which display the pill sits on. It follows the primary display unless you pick another
  one, and goes back to the primary one if the chosen display is unplugged.

### Copy peek

Each time you copy, the collapsed pill widens into a capsule that says **Copied** and shows what
it was, then shrinks back. The badge pops in and its check mark draws itself, then "Copied" and
the preview slide out from behind it. It holds for about 2 s, then eases back: the text fades
while the capsule is already closing, so there is no pause before it goes.

* Short text shows in full; long text is cut with an ellipsis (it never wraps). An image shows a
  small thumbnail and its size (`Image · 1280 × 720`).
* Copying again while it is up cross-fades to the new content (the old preview drops out, the new
  one fades in as the width adjusts) and holds again, without replaying the widen.
  It stays up while the cursor is on it. Click it to open the pill.
* No peek when it would only be noise: the panel is already open, the pill is hidden or a
  full-screen app is in front, capture is paused, or the copy is one Notch ignores (password
  managers, its own pastes).
* With Windows animation effects off (Settings > Accessibility > Visual effects) the capsule
  cross-fades instead of widening.
* Privacy: `"copy_peek_preview": false` shows only "Copied", never the content. `"copy_peek": false`
  turns the peek off altogether.

The logo is a pill split in two: amber for the notepad, cyan for the clipboard. The tray icon uses
the bright colours on a dark taskbar and deeper tones on a light one, and switches by itself when
you change Windows' light/dark mode.

## Commands

While Notch is running, run these from any terminal (or a shortcut, script or launcher) to control it:

```
notch.exe hide       hide the pill from the screen
notch.exe show       bring it back
notch.exe toggle     hide it if visible, show it if hidden
notch.exe quit       close Notch
notch.exe --hidden   start Notch with the pill hidden (handy for autostart)
notch.exe help       list the commands
```

A leading `-`, `--` or `/` is optional and case doesn't matter (`notch.exe --Hide` works). While
hidden, Notch keeps recording the clipboard and the hotkeys stay registered, but the open hotkeys
(`Win+Alt+N` / `Win+Alt+V`) do nothing until you show the pill again. Hiding is not remembered
across restarts. If Notch isn't running, the commands just tell you so.

### Notepad

* Several notes, shown as tabs under the header. **`+`** (or `Ctrl+N`) adds one, `Ctrl+Tab` /
  `Ctrl+Shift+Tab` cycles through them.
* When the tabs no longer fit, the row scrolls (mouse wheel or a sideways touchpad swipe over the
  header; the active tab is always brought into view) and the **`+`** stays pinned at the right
  end. Up to 30 notes.
* **Delete** a note with the **x** on its tab (shown on the active tab and on whichever tab you
  point at), by middle-clicking it, with `Ctrl+W`, or from the right-click menu. A note with text
  asks first. The **Quick note** (the first note) is permanent: it has no x and Delete does
  nothing for it, though you can rename it and clear its text yourself. If a notes list ever lacks
  it, it comes back, empty, as the first tab.
* **Double-click** a tab to rename it (Enter saves, Esc cancels). **Right-click** a tab for
  Rename / Delete.
* Everything is saved automatically, one second after you stop typing.

### Clipboard

* Type to **search** (the box is focused when opened with the hotkey; click it otherwise). The first
  `Esc` clears the search, the second closes the pill.
* Entries are grouped into **PINNED** and **RECENT**, labelled Text / Link / Code / Image with how
  long ago they were copied.
* The list is driven with the mouse or trackpad. Click an entry to paste it into the app you were
  in (hold `Shift` while clicking to only copy it); hover it for the pin and ✕ buttons; scroll with
  the wheel or a two-finger swipe.
* Pinned entries never expire. Anything a password manager flags as secret
  (`ExcludeClipboardContentFromMonitorProcessing` / `CanIncludeInClipboardHistory = 0`) is never
  recorded, and Notch ignores changes it makes itself.

### Timer

One timer at a time, on the third tab.

* **Set a time**: type `25m`, `1h 30m`, `90s`, `2 hours` or a bare number (minutes), or tap a chip
  (5 min, 10 min, 25 min, 45 min, 1 hour). The box shows what it means (`= 25:00`) and when the timer
  will end. Add an optional label, then press `Enter` or **Start**. Up to 24 hours. Starting a new
  timer replaces the current one.
* **While it runs** the tab shows a ring and the time left, with stop (x), pause / resume and
  **+1 min**. Collapse the pill and it carries on: the resting pill shows the ring and the digits (a
  play mark while paused), the header's Timer tab shows the time left whichever tab is open, and a
  copy peek keeps the timer on its right.
* **Hover** the pill while a timer runs and it widens into a capsule instead of opening the panel:
  notepad and clipboard buttons, the ring (click to pause or resume) and the time (click to open the
  Timer tab). Click anywhere else on it to open the panel.
* **Time's up**: the pill widens and glows with **+5 min** and **Done** until you press one (+5 min
  starts a fresh five minutes). If the pill is hidden (a full-screen app is in front, or you hid it)
  Windows shows a notification instead. There is no sound.
* The timer is not saved (quitting Notch ends it) and does not count time the PC spends asleep.

### Shelf

A parking spot for files while you work: drop them on the pill now, drag them out into any app later.

* **Drop** files or folders on the pill from Explorer, the desktop or any app. While you drag, the
  pill widens into a drop zone and glows when you are over it; on release a small **Added** toast
  says how many. Dropping on the open panel works too. It holds up to 48 files (the oldest go first).
* The shelf holds **links, not copies**: nothing is duplicated and nothing is moved. If a file is
  moved or deleted, its tile says **Missing** and the x removes it. UNC (network) paths are not
  checked, so they are never marked missing.
* The resting pill shows a tray and the file count (beside the timer when one runs); hovering shows a
  shelf button with the count.
* **Drag out**: press a tile and drag. Ticked tiles go together (click a tile to tick it); a tile
  that is not ticked goes alone; **Drag all** takes every file still there. The drop is always a
  copy, so the originals stay where they are. `Esc` cancels a drag.
* `Del` removes the ticked tiles from the shelf (never from disk); **Clear** empties it.
* Files leave the shelf after **24 hours**: set `"shelf_hours"` (0 = keep until removed).
* Each tile shows the same preview Explorer does: a thumbnail for images, PDFs, videos and the like,
  otherwise the file's icon. Previews load in the background while the panel is open.
* Limits: real files only (not images dragged out of a browser), and Windows blocks dragging from an
  elevated (admin) app onto a normal one.

## Settings

Optional file: `%APPDATA%\Notch\settings.json` (all keys optional; restart to apply).

```json
{
  "hotkey_toggle": "Win+Alt+N",
  "hotkey_clipboard": "Win+Alt+V",
  "hotkey_visibility": "Win+Alt+H",
  "hover_dwell_ms": 120,
  "collapse_delay_ms": 100,
  "max_items": 200,
  "max_image_mb": 200,
  "paste_on_click": true,
  "copy_peek": true,
  "copy_peek_preview": true,
  "copy_peek_ms": 2000,
  "theme": "dark",
  "accent": "classic",
  "text_size": "medium",
  "monitor": "",
  "shelf_hours": 24,
  "use_gpu": false
}
```

`theme` is `dark`, `light` or `system`; `accent` is `classic`, `sky`, `amber`, `mint`, `violet` or
`rose`; `text_size` is `small`, `medium` or `large`; `monitor` is a display's device name such as
`\\.\DISPLAY2` (empty = the primary display). The first four are what the tray menu changes; Notch
rewrites the file when you use it, and a word it does not know falls back to the default.

`use_gpu` is off by default: drawing a pill is trivial work, and using the GPU makes Windows load the
graphics vendor's user-mode driver, which cost ~55 MB more RAM in testing (Intel). The software
rasteriser (WARP) renders each animation frame in about 1 ms.

Data lives in `%APPDATA%\Notch`: `notes.json` + `notes\<id>.txt` (the notepad), `clips.json` (index)
and `clips\` (payloads), `shelf.json` (the paths on the shelf). A single `notes.txt` from an earlier version is migrated automatically.
If `notes.json` is ever unreadable it is kept as `notes.json.bad` rather than overwritten.

## Why it is light

* **Event-driven.** The UI thread blocks in `GetMessageW`; there are no polling timers. The 60 fps
  frame loop exists only while the pill is animating, then the thread goes back to sleep. The copy
  peek rides on the clipboard event and one one-shot timer, so it costs nothing between copies.
* **Clipboard capture uses `WM_CLIPBOARDUPDATE`**, not polling. Image encoding (DIB → PNG +
  thumbnail through WIC) runs on a short-lived worker thread.
* **Small resident set.** The index only holds short text previews; full text and images stay on
  disk and are read only when pasted. Thumbnails are decoded while the pill is open and dropped when
  it closes. The notepad control is created the first time it is shown.
* **The timer ticks only while it runs.** One one-shot Windows timer, set for the exact moment the
  displayed second changes, redraws the digits and the ring once a second; paused, stopped or idle,
  nothing wakes up.
* **Click-through.** The window region is clipped to the visible pill, so clicks around it reach the
  windows underneath.
* **Hides itself** while a full-screen app or game is in front (event-driven, no polling).

Measured on this machine (Intel Iris Xe, 150% DPI, Windows 11): 0.0 ms CPU over 10-20 s idle,
~4 MB private working set collapsed (~10 MB after opening both tabs), 430 KB executable, ≈1.4 ms per
animation frame at full size, no growth in memory or handles over 40+ open/close cycles.

## Build

Requires the Rust toolchain (MSVC) and the Visual Studio C++ Build Tools.

```
cargo test
cargo build --release      # target\release\notch.exe
```

Set the environment variable `NOTCH_LOG=1` before launching to write a debug log to
`%TEMP%\notch.log`. `NOTCH_REDUCED_MOTION=1` forces the copy peek's cross-fade (for testing it
without changing the Windows setting).
