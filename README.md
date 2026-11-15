<p align="center">
  <img src="assets/logo.png" width="96" alt="Notch logo: a pill split into an amber notepad half and a cyan clipboard half">
</p>

# Notch

[![Latest release](https://img.shields.io/github/v/release/anshthakur0999/notch-windows)](https://github.com/anshthakur0999/notch-windows/releases/latest)

A tiny floating pill at the top-centre of your screen (think Dynamic Island) that holds six small
tools: a **notepad**, a **clipboard history**, a **timer**, a **file shelf**, a **now-playing
readout with a visualizer** and a **Bluetooth device hub**. Hover it and it opens, move away and it
closes. Written in Rust against
raw Win32 + DirectComposition/Direct2D, and built to cost almost nothing while idle: one 956 KB exe,
no installer, no background polling.

* **Notepad**: several auto-saved notes as tabs.
* **Clipboard**: searchable history of text, links, code and images; click an entry to paste it.
* **Timer**: type `25m`, it keeps counting on the collapsed pill and says so when time is up.
* **Shelf**: park files on the pill while you work, drag them out into any app later.
* **Media**: what is playing in any browser or music app, with its cover, play / pause, skip, seek
  and a live visualizer, on the pill itself.
* **Devices**: when earbuds, a mouse, a keyboard or a controller connects, the pill opens a card with
  its battery percentage and, for earbuds, a one-click **Use for audio**; the tab lists every audio
  output (click one to switch), the connected devices with **Disconnect**, the paired ones that are
  not connected with **Connect**, and an **Add device** screen to pair new ones.
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
| Play / pause the music | Hover the pill while something plays and press the round button on the right |
| Switch tab | Click **Notepad** / **Clipboard** / **Timer** / **Shelf** / **Media** / **Devices** in the header, or press `Tab` |
| Switch the sound output | Click the **Devices** tab, then the output you want; or press **Use for audio** on the card that opens when earbuds connect |
| Connect, disconnect or pair a Bluetooth device | **Devices** tab: **Disconnect** on a connected device, **Connect** on a paired one, **Add device** to pair a new one |
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

## Media

Shows what is playing and lets you drive it, without opening the app that plays it. Notch reads
Windows' own media session, which browsers (Chrome, Edge, Firefox and the rest), Spotify and the
Windows media apps all report to, so there is nothing to install per app. It follows the session
Windows marks as current.

* **On the pill.** While something plays, the resting pill shows its cover and a tiny equaliser; paused,
  the equaliser becomes a pause mark. Next to a running timer the pill grows a little to hold both;
  with the shelf as well, the shelf shrinks to a dot.
* **New track banner.** When the track changes, the pill briefly widens to show the cover, title and
  artist, then eases back. Not shown while the panel is open, the pill is hidden, or for what was
  already playing when Notch started.
* **Hover.** With media on show, resting the pointer on the pill widens it into a capsule with the
  notepad and clipboard buttons (and the shelf and timer when they are in use), the cover and title,
  and a play / pause button. Click the cover or title to open the **Media** tab.
* **The Media tab.** The cover, where it is playing, title and artist, the visualizer, a seek bar
  with elapsed and remaining time, and previous / play-pause / next. Click the bar to seek. Its line
  is always there; the knob and the remaining time appear once the player reports the track's length,
  which most do at once, Zen a few seconds after a track change, and live streams never do. With
  nothing playing the tab says so.
* **Bars** switches the visualizer off (the button then reads **Off**). The choice is kept in
  `settings.json` as `"visualizer"`.
* Limits: only apps that report to Windows' media session show up (most do; some games and older
  players do not). It shows one session at a time, and clicking the bar seeks but cannot be dragged.
  Previous / next are requests to the player, which may ignore them (seen with some YouTube videos in
  Chrome).

**Privacy.** The visualizer listens to what the speakers are playing (the same loopback Windows uses
for "what you hear"; never the microphone), turns each 40 ms into bar heights in memory and throws the
samples away. Nothing is recorded, saved or sent. It listens **only while** music is playing, the
pill is visible and the bars are on screen (the closed pill, or the Media tab); paused, hidden, on
another tab or with **Bars** off, no audio is touched at all and no thread runs.

## Devices

Shows what is connected over Bluetooth and how full its battery is, and switches where the sound
goes in one click. Notch asks Windows to tell it when a paired device connects, disconnects or
reports a new battery value, so there is nothing to set up. What kind of device it is comes from the
class Windows records for it when it is paired.

* **The card.** When a paired earbud, headphone, mouse, keyboard or game controller connects, the
  resting pill widens a little while an azure ring sweeps once round it, then springs open into a
  card with the device's icon, name and battery percentage with a small bar. Earbuds and headsets
  also get **Use for audio**, which makes them the default playback device for apps, media and calls
  without the pill taking keyboard focus. The card stays about 4 seconds (longer while the pointer is
  on it), the **x** closes it, and clicking anywhere else on it opens the **Devices** tab.
* **Short banners.** After **Use for audio**: "Now the audio output". When a device disconnects:
  "*name* disconnected", and if it was the audio output, where the sound went ("Audio back on
  Speakers"). A device that comes back within 30 seconds gets a small "Reconnected" banner instead of
  a full card. A battery that falls to 20%, and again to 10%, opens a **Battery low** card, with an
  icon and the words, not only a colour. Disconnects and reconnects are announced for earbuds and
  the like only: a mouse or keyboard that dozes off and wakes is not news (a mouse that connects after
  more than 30 seconds away does get its card).
* **Quiet by default.** No card for devices that were already connected when Notch started, while the
  panel is open, while the pill is hidden or a full-screen app is in front. With Windows animation
  effects off the ring is skipped and the card simply appears. `"device_banner": false` turns every
  card and banner off; the tab keeps working.
* **The Devices tab.** **Audio output** is a row of chips, one per active playback device with the
  default one filled; click another to make it the default for all roles. **Connected** lists each
  device with its battery and a **Disconnect** button; **Available** lists the paired devices that are
  not connected, each with **Connect**. The lists update when a device reports a change, and the output
  list is read when the tab opens.
* **Connect and Disconnect** are one click, with no confirmation (nothing is lost by a wrong tap). While
  it works the button is replaced by a ring and the row says *Connecting...* or *Disconnecting...*; the
  row moves to the other section when Windows reports the change (a connect takes about 6 to 12
  seconds, a disconnect under one). If the device does not answer (about 12 seconds to connect, 4 to
  disconnect) the row says so, in red, and offers **Try again** and **Open
  Bluetooth settings**; it stays until you try again or leave the tab. A connect you started is shown on
  its row and does not also open the card. If the device you disconnect was the audio output, Windows
  moves the sound back to another one and the chips show which. Disconnect keeps the pairing.
* **Add device** searches for nearby devices that are not paired and pairs the one you pick: **Pair**,
  then, for a keyboard, the code to type on it. The search starts when you open the screen and stops
  when you press **Stop**, leave the screen or after 30 seconds, so nothing scans in the background.
  It offers earbuds, headphones, speakers, mice, keyboards and gamepads (what the device says it is);
  a device that does not say, and phones, are left to Windows' own Bluetooth settings.
* **Bluetooth off.** The tab says so and offers **Turn on Bluetooth** (Windows may ask first) and
  **Bluetooth settings**. With nothing paired at all it offers **Add device** instead.
* **The visualizer follows the switch.** The Media tab's bars listen to the default output; after you
  switch, they re-attach to the new device within about a second.

**What "exact" means.** The percentage is what Windows itself reports for the device
(`DEVPKEY_Bluetooth_Battery`, the same value Windows Settings shows), read when Windows reports it.
When Windows has no value Notch says **Battery unknown** rather than guess. Limits you should know
about:

* AirPods and most earbuds report one value at best, and Windows does not say what it measures:
  on some earbuds (seen on a OnePlus Buds 4) it is the case, not a bud. Left bud, right bud and
  case separately need Apple's undocumented Bluetooth advertisements or a vendor's own protocol,
  which Notch does not read.
* Mice behind a vendor receiver (Logitech Unifying, Bolt), and PlayStation controllers over Bluetooth,
  report nothing through Windows' standard property, so they show **Battery unknown**. Notch does not
  speak vendor protocols, and it does not use `Windows.Gaming.Input`, so an Xbox controller shows
  whatever Windows exposes for it as a Bluetooth device (coarse steps, or unknown).
* Only devices **paired over Bluetooth** are seen (classic Bluetooth and Bluetooth LE); a USB
  controller or one on the Xbox wireless adapter is not. Only what Windows classes as audio, mouse,
  keyboard or gamepad is shown; phones, watches and PCs are ignored.
* **Connect and Disconnect** work on devices Windows lists as classic Bluetooth, which includes
  earbuds, most keyboards and many gamepads. Disconnect is the Bluetooth driver's own request to drop
  the link (`IOCTL_BTH_DISCONNECT_DEVICE`); Windows has no call to connect, so Connect switches the
  device's audio (or input) service off and on again, which takes Windows about six seconds and makes it
  connect to the device. Both are what the
  Win32 Bluetooth API offers rather than documented features, so a device that is off, out of range or
  asleep will not answer, and a device may reconnect by itself after a disconnect (many earbuds do when
  they are taken out of the case). After a Connect the earbuds' playback device can take 15 to 30
  seconds to come up; the output chips follow it. Bluetooth LE devices, such as many modern mice, are connected by
  Windows itself: they are listed, without a button. Pairings Windows keeps for a Bluetooth adapter
  this PC no longer has are listed as paired by Windows but cannot be connected, so they are hidden.
* **Add device** answers the two pairing requests a device without a screen makes ("pair?" and "type
  this code on it"). A device that needs a PIN typed here, or two numbers compared, is paired from
  Windows' Bluetooth settings. Nearby devices are listed once even when Windows sees them through both
  Bluetooth and Bluetooth LE, and at most twelve.
* **Use for audio** needs the device's playback device to exist, which can take a couple of seconds
  after it connects; if there is none yet the banner says so and nothing changes. It uses
  `IPolicyConfig`, an interface Windows does not document (the one SoundSwitch and NirCmd use), so a
  future Windows update could break the switch; the rest of Notch does not depend on it.
* "Audio back on ..." is a best effort: Notch checks whether the device was the default output when
  the disconnect arrives, and where the sound went about a second later. If Windows has already moved
  the sound by then, you get the plain "disconnected" banner.

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
| `device_banner` | `true` | The card and banners when a Bluetooth device connects, disconnects or runs low; `false` leaves only the **Devices** tab |
| `shelf_hours` | `24` | Files leave the shelf after this long (`0` = never) |
| `visualizer` | `true` | The media bars move with the music (the **Bars** switch on the Media tab); `false` never samples audio |
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
rasteriser (WARP) takes about 1 ms of the main thread per animation frame, but its own worker
threads add several more milliseconds of CPU for each frame drawn, which is why only the
visualizer, which redraws many times a second, shows up in the CPU figures below.

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
* **Media is callback-driven.** One worker thread asks Windows to call back when the track, the play
  state or the position changes, and sleeps in between; nothing polls. The cover is decoded once per
  track and kept (about 260 KB) only while something plays.
* **Devices are callback-driven too, and cost a little all the time.** One worker thread and two Windows
  device watchers (classic Bluetooth and Bluetooth LE) ask Windows to call back when a paired device
  appears or connects, and a third small watcher per *connected* device reports its battery when it
  changes; the thread sleeps in between and nothing polls, so there is no timer and no CPU while
  nothing changes. The price is that the two main watchers run for as long as Notch does: measured on
  this machine, about 7.5 MB more working set (2.7 MB of it private) and four more threads, with 0 ms of
  CPU over 15 s idle (a debug test build, with nothing connected; the per-device watchers were not
  separately measured). The Bluetooth radio's on / off state is one more callback. The playback devices
  are read only when you open the Devices tab, press **Use for audio** or a device connects or goes.
* **Connect, Disconnect and Add device cost something only while you use them.** A click starts a short
  thread that waits for the answer and ends. **Add device** starts two more Windows watchers (the
  search) that stop when you leave the screen or after 30 seconds. While a ring is turning, the
  Devices tab redraws about 30 times a second, and not otherwise.
* **The visualizer costs something, only while it shows.** Each redraw of the pill costs a few
  milliseconds of CPU on the software rasteriser, so the closed pill's bars update 8 times a
  second and the Media tab's 20. The listening thread starts when music plays and the bars are visible
  and ends otherwise.
* **Click-through.** The window region is clipped to the visible pill, so clicks around it reach the
  windows underneath.
* **Hides itself** while a full-screen app or game is in front (event-driven, no polling).

Measured on this branch (Windows 11, software rasteriser): 0.0 ms of CPU over 15 s idle with nothing
playing, and 0.0 ms with music playing and **Bars** off; with music playing and the bars on, about
2 to 3% of one core on the closed pill and 15 to 20% while the Media tab is open (a test track that
never sits still; with `use_gpu` on, about 2% and 9%). About 14 MB private working set idle and 19 MB while a cover is held (before the Devices watchers
above, which add about 3 MB private), a 956 KB executable.

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
