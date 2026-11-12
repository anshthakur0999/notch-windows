<p align="center">
  <img src="assets/logo.png" width="96" alt="Notch logo: a pill split into an amber notepad half and a cyan clipboard half">
</p>

# Notch

A tiny floating pill at the top-centre of your screen (think Dynamic Island) with a **notepad** and
**clipboard history**. Written in Rust against raw Win32 + DirectComposition/Direct2D, built to cost
almost nothing while idle.

## Use it

| Action | How |
| --- | --- |
| Open | Hover the pill (it expands after a short dwell), click it, or press a hotkey |
| Notepad (toggle) | `Win+Alt+N` — opens the notepad with the cursor in it; press again or `Esc` to close |
| Clipboard | `Win+Alt+V` — newest entry pre-selected |
| Switch tab | Click **Notepad** / **Clipboard** in the header, or press `Tab` |
| Keep open | The pin in the header: the pill stays open until you unpin it, press `Esc`, or click the chevron |
| Close | Move the mouse away, `Esc`, the chevron, or click another app |
| Hide / show the pill | `Win+Alt+H`, the tray menu, or the commands below |
| Tray icon | Open · Hide/Show pill · Pause clipboard capture · Start with Windows · Quit |

If a hotkey is already owned by another app, Notch falls back to `Ctrl+Alt+N` / `Ctrl+Alt+V` /
`Ctrl+Alt+H`. The tray tooltip always shows which hotkeys are active.

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
  asks first; the last remaining note is only cleared, never removed.
* **Double-click** a tab to rename it (Enter saves, Esc cancels). **Right-click** a tab for
  Rename / Delete.
* Everything is saved automatically, one second after you stop typing.

### Clipboard

* Type to **search** (the box is focused when opened with the hotkey; click it otherwise). The first
  `Esc` clears the search, the second closes the pill.
* Entries are grouped into **PINNED** and **RECENT**, labelled Text / Link / Code / Image with how
  long ago they were copied.
* `↑` `↓` move, `Enter` pastes into the app you were in, `Shift+Enter` only copies, `Ctrl+P` pins,
  `Del` removes. With the mouse: click a row to paste; hover it for the pin and ✕ buttons.
* Pinned entries never expire. Anything a password manager flags as secret
  (`ExcludeClipboardContentFromMonitorProcessing` / `CanIncludeInClipboardHistory = 0`) is never
  recorded, and Notch ignores changes it makes itself.

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
  "use_gpu": false
}
```

`use_gpu` is off by default: drawing a pill is trivial work, and using the GPU makes Windows load the
graphics vendor's user-mode driver, which cost ~55 MB more RAM in testing (Intel). The software
rasteriser (WARP) renders each animation frame in about 1 ms.

Data lives in `%APPDATA%\Notch`: `notes.json` + `notes\<id>.txt` (the notepad), `clips.json` (index)
and `clips\` (payloads). A single `notes.txt` from an earlier version is migrated automatically.
If `notes.json` is ever unreadable it is kept as `notes.json.bad` rather than overwritten.

## Why it is light

* **Event-driven.** The UI thread blocks in `GetMessageW`; there are no polling timers. The 60 fps
  frame loop exists only while the pill is animating, then the thread goes back to sleep.
* **Clipboard capture uses `WM_CLIPBOARDUPDATE`**, not polling. Image encoding (DIB → PNG +
  thumbnail through WIC) runs on a short-lived worker thread.
* **Small resident set.** The index only holds short text previews; full text and images stay on
  disk and are read only when pasted. Thumbnails are decoded while the pill is open and dropped when
  it closes. The notepad control is created the first time it is shown.
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
`%TEMP%\notch.log`.
