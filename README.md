<p align="center">
  <img src="assets/logo.png" width="96" alt="Notch logo: a pill split into an amber notepad half and a cyan clipboard half">
</p>

# Notch

[![Latest release](https://img.shields.io/github/v/release/anshthakur0999/notch-windows)](https://github.com/anshthakur0999/notch-windows/releases/latest)

A tiny floating pill at the top-centre of your screen (think Dynamic Island) that holds four small
tools: a **notepad**, a **clipboard history**, a **timer** and a **file shelf**. Hover it and it
opens, move away and it closes. Written in Rust against raw Win32 + DirectComposition/Direct2D, and
built to cost almost nothing while idle: one 670 KB exe, no installer, no background polling.

* **Notepad**: several auto-saved notes as tabs.
* **Clipboard**: searchable history of text, links, code and images; click an entry to paste it.
* **Timer**: type `25m`, it keeps counting on the collapsed pill and says so when time is up.
* **Shelf**: park files on the pill while you work, drag them out into any app later.
* **Copy peek**: every time you copy, the pill widens to say **Copied** and shows what it was.

## Install

1. Download `notch.exe` from the [latest release](https://github.com/anshthakur0999/notch-windows/releases/latest)
   and run it. The pill appears at the top of your screen and a tray icon is added.
2. Optional: right-click the tray icon and tick **Start with Windows**.

Windows 10 or 11, 64-bit. The exe is not code-signed, so Windows SmartScreen may ask you to confirm
the first time you run it. To check the download, compare
`certutil -hashfile notch.exe SHA256` with the `notch.exe.sha256` file next to it on the release page.

To remove it: untick **Start with Windows**, choose **Quit** from the tray menu, then delete
`notch.exe` and the folder `%APPDATA%\Notch` (your notes, clipboard history and settings).

## Quick start

| Action | How |
| --- | --- |
| Open | Hover the pill (it expands after a short dwell), click it, or press a hotkey |
| Open on the last tab | `Win+Alt+N`: opens the pill (on the notepad at first, then the tab you used last); press again or `Esc` to close |
| Open the clipboard | `Win+Alt+V`: opens the clipboard list with the search box ready, just start typing |
| Start a timer | Click **Timer** in the header, type a time such as `25m`, press `Enter` |
| Add files to the shelf | Drop them on the pill, then open the **Shelf** tab |
| Switch tab | Click **Notepad** / **Clipboard** / **Timer** / **Shelf** in the header, or press `Tab` |
| Keep open | The pin in the header: the pill stays open until you unpin it, press `Esc`, or click the chevron |
| Close | Move the mouse away, `Esc`, the chevron, or click another app |
| Hide / show the pill | `Win+Alt+H`, the tray menu, or the [commands](#command-line) |

If a hotkey is already owned by another app, Notch falls back to `Ctrl+Alt+N` / `Ctrl+Alt+V` /
`Ctrl+Alt+H`. The tray tooltip always shows which hotkeys are active.

## Notepad

* Several notes, shown as tabs under the header. **`+`** (or `Ctrl+N`) adds one, `Ctrl+Tab` /
  `Ctrl+Shift+Tab` cycles through them. Up to 30 notes.
* When the tabs no longer fit, the row scrolls (mouse wheel or a sideways touchpad swipe over the
  header; the active tab is always brought into view) and the **`+`** stays pinned at the right end.
* **Delete** a note with the **x** on its tab (shown on the active tab and on whichever tab you point
  at), by middle-clicking it, with `Ctrl+W`, or from the right-click menu. A note with text asks
  first. The **Quick note** (the first note) is permanent: it has no x and Delete does nothing for
  it, though you can rename it and clear its text yourself. If a notes list ever lacks it, it comes
  back, empty, as the first tab.
* **Double-click** a tab to rename it (`Enter` saves, `Esc` cancels). **Right-click** a tab for
  Rename / Delete.
* Everything is saved automatically, one second after you stop typing.

## Clipboard

* Type to **search** (the box is focused when opened with the hotkey; click it otherwise). The first
  `Esc` clears the search, the second closes the pill.
* Entries are grouped into **PINNED** and **RECENT**, labelled Text / Link / Code / Image with how
  long ago they were copied.
* The list is driven with the mouse or trackpad. Click an entry to paste it into the app you were in
  (hold `Shift` while clicking to only copy it); hover it for the pin and ✕ buttons; scroll with the
  wheel or a two-finger swipe.
* Pinned entries never expire. Anything a password manager flags as secret
  (`ExcludeClipboardContentFromMonitorProcessing` / `CanIncludeInClipboardHistory = 0`) is never
  recorded, and Notch ignores changes it makes itself. You can also pause capture from the tray menu.

## Timer

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

## Shelf

A parking spot for files while you work: drop them on the pill now, drag them out into any app later.

* **Add files** by dropping files or folders on the pill from Explorer, the desktop or any app. While
  you drag, the pill widens into a drop zone and glows when you are over it; on release a small
  **Added** toast says how many. Dropping on the open panel works too, and so does the **+** slot
  after the last tile, which opens a file picker (files only; drop a folder to add it). The shelf
  holds up to 48 files (the oldest go first).
* **Drag out**: press a tile and drag. Ticked tiles go together (click a tile to tick it); a tile
  that is not ticked goes alone. The drop is always a copy, so the originals stay where they are.
  `Esc` cancels a drag.
* **Remove**: hover a tile and click its **x** to take that one file off the shelf. `Del` removes
  the ticked tiles, **Clear** empties the shelf. Nothing is ever deleted from disk.
* The shelf holds **links, not copies**: nothing is duplicated and nothing is moved. If a file is
  moved or deleted, its tile says **Missing** (UNC network paths are not checked, so they are never
  marked missing).
* The resting pill shows a tray and the file count (beside the timer when one runs); hovering shows a
  shelf button with the count.
* Files leave the shelf after **24 hours**; set `"shelf_hours"` to change that (0 = keep until
  removed).
* Each tile shows the same preview Explorer does: a thumbnail for images, PDFs, videos and the like,
  otherwise the file's icon. Previews load in the background while the panel is open.
* Limits: real files only (not images dragged out of a browser), and Windows blocks dragging from an
  elevated (admin) app onto a normal one.

## Copy peek

Each time you copy, the collapsed pill widens into a capsule that says **Copied** and shows what it
was, then shrinks back. It holds for about 2 s, then eases back.

* Short text shows in full; long text is cut with an ellipsis. An image shows a small thumbnail and
  its size (`Image · 1280 × 720`).
* Copying again while it is up cross-fades to the new content and holds again. It stays up while the
  cursor is on it. Click it to open the pill.
* No peek when it would only be noise: the panel is already open, the pill is hidden or a full-screen
  app is in front, capture is paused, or the copy is one Notch ignores (password managers, its own
  pastes).
* With Windows animation effects off (Settings > Accessibility > Visual effects) the capsule
  cross-fades instead of widening.
* Privacy: `"copy_peek_preview": false` shows only "Copied", never the content. `"copy_peek": false`
  turns the peek off altogether.

## Tray menu and looks

Right-click the tray icon for: Open · Hide/Show pill · Theme · Accent colour · Text size · Monitor ·
Pause clipboard capture · Start with Windows · Quit. The look settings apply immediately and are
saved to `settings.json`, so there is nothing to edit and no restart.

* **Theme**: Dark (default), Light, or Match Windows (follows Windows' app light/dark mode and
  switches with it).
* **Accent colour**: Classic keeps the logo's two tones (amber notepad, cyan clipboard) and gives the
  timer its own violet; Sky, Amber, Mint, Violet and Rose use one colour for all the tabs, the
  selected entry and the copy peek's check.
* **Text size**: Small, Default or Large for the text you read: the notes and the clipboard entries.
  The header, tabs and hints keep their size.
* **Monitor**: which display the pill sits on. It follows the primary display unless you pick another
  one, and goes back to the primary one if the chosen display is unplugged.

The tray icon uses bright colours on a dark taskbar and deeper tones on a light one, and switches by
itself when you change Windows' light/dark mode.

## Command line

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
(`Win+Alt+N` / `Win+Alt+V`) do nothing until you show the pill again. Hiding is not remembered across
restarts. If Notch isn't running, the commands just tell you so.

## Settings

The tray menu covers the look. Everything else is in an optional file,
`%APPDATA%\Notch\settings.json`. All keys are optional, and you need to restart Notch to apply edits.

| Key | Default | What it does |
| --- | --- | --- |
| `hotkey_toggle` | `"Win+Alt+N"` | Opens / closes the pill |
| `hotkey_clipboard` | `"Win+Alt+V"` | Opens the pill on the clipboard tab |
| `hotkey_visibility` | `"Win+Alt+H"` | Hides / shows the pill |
| `hover_dwell_ms` | `120` | How long the cursor rests on the pill before it opens |
| `collapse_delay_ms` | `100` | How long after the cursor leaves before it closes |
| `max_items` | `200` | Unpinned clipboard entries kept |
| `max_image_mb` | `200` | Disk space for unpinned clipboard images |
| `paste_on_click` | `true` | Clicking a clipboard entry also pastes it into the previous app |
| `copy_peek` | `true` | The "Copied" capsule |
| `copy_peek_preview` | `true` | Show what was copied in the capsule |
| `copy_peek_ms` | `2000` | How long the capsule stays up |
| `shelf_hours` | `24` | Files leave the shelf after this long (`0` = never) |
| `theme` | `"dark"` | `dark`, `light` or `system` |
| `accent` | `"classic"` | `classic`, `sky`, `amber`, `mint`, `violet` or `rose` |
| `text_size` | `"medium"` | `small`, `medium` or `large` |
| `monitor` | `""` | A display's device name such as `\\.\DISPLAY2` (empty = the primary display) |
| `use_gpu` | `false` | Draw on the GPU instead of the software rasteriser |

Hotkeys are written like `Win+Alt+N` or `Ctrl+Alt+F9`. `theme`, `accent`, `text_size` and `monitor`
are what the tray menu changes; Notch rewrites the file when you use it, and a value it does not
know falls back to the default.

`use_gpu` is off by default: drawing a pill is trivial work, and using the GPU makes Windows load the
graphics vendor's user-mode driver, which cost about 55 MB more RAM in testing (Intel). The software
rasteriser (WARP) renders each animation frame in about 1 ms.

## Where your data lives

Everything is in `%APPDATA%\Notch`: `notes.json` and `notes\<id>.txt` (the notepad), `clips.json`
(index) and `clips\` (payloads), `shelf.json` (the paths on the shelf) and `settings.json`. A single
`notes.txt` from an earlier version is migrated automatically. If `notes.json` is ever unreadable it
is kept as `notes.json.bad` rather than overwritten.

## Why it is light

* **Event-driven.** The UI thread blocks in `GetMessageW`; there are no polling timers. The 60 fps
  frame loop exists only while the pill is animating, then the thread goes back to sleep. The copy
  peek rides on the clipboard event and one one-shot timer, so it costs nothing between copies.
* **Clipboard capture uses `WM_CLIPBOARDUPDATE`**, not polling. Image encoding (DIB → PNG +
  thumbnail through WIC) runs on a short-lived worker thread.
* **Small resident set.** The index only holds short text previews; full text and images stay on
  disk and are read only when pasted. Thumbnails (clipboard and shelf) are decoded while the pill is
  open and dropped when it closes. The notepad control is created the first time it is shown.
* **The timer ticks only while it runs.** One one-shot Windows timer, set for the exact moment the
  displayed second changes, redraws the digits and the ring once a second; paused, stopped or idle,
  nothing wakes up.
* **Click-through.** The window region is clipped to the visible pill, so clicks around it reach the
  windows underneath.
* **Hides itself** while a full-screen app or game is in front (event-driven, no polling).

Measured on v0.1.4 (Windows 11): 0.0 ms of CPU over 15 s idle, about 11 MB private working set after
normal use (notes, clipboard and shelf all in use), a 670 KB executable, and a steady handle count.

## Build from source

Requires the Rust toolchain for Windows (MSVC, edition 2024, so Rust 1.85 or newer) and the Visual
Studio C++ Build Tools.

```
cargo test
cargo build --release      # target\release\notch.exe
```

Set the environment variable `NOTCH_LOG=1` before launching to write a debug log to
`%TEMP%\notch.log`. `NOTCH_REDUCED_MOTION=1` forces the copy peek's cross-fade (for testing it
without changing the Windows setting).
