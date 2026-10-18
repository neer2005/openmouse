# OpenMouse 🖱️

Turn your phone into a wireless mouse, keyboard, and trackpad — an open-source
alternative to Remote Mouse. A tiny native server runs on your computer and
serves a web remote to your phone over your local WiFi. No accounts, no cloud,
no app store, no Python: just open a URL.

## Download (Windows)

Get the latest **`openmouse.exe`** from the
[Releases page](https://github.com/neer2005/openmouse/releases). Just run it
and scan the QR code with your phone (same WiFi). After that,
updates install themselves via **Check for updates…** in the tray menu.

> Windows SmartScreen may flag the unsigned exe on first run:
> click "More info" → "Run anyway".

## Features

- **Trackpad** — 1-finger move, tap to click, press-and-hold to drag,
  2-finger tap for right-click, 2-finger slide to scroll, 3-finger tap for
  middle-click; scroll rail with ▲/▼ buttons and collapsible click
  controls in the bottom toolbar
- **Keyboard** — ⌨️ brings up your phone's own keyboard for typing; the Fn
  button opens the shortcut panel: Esc, Tab, Ins, End, arrows, Enter,
  Backspace, F5, and hotkeys (⊞ Win, Alt+Tab, ⊞+D, Alt+F4, Ctrl+C/V)
- **Media controls** — play/pause, prev/next, volume, mute
- **Power controls** (Windows) — sleep, restart, log off, shut down from the
  phone, each with a confirm dialog
- **System tray** — runs quietly in the tray (console hidden); autostart
  option, Show QR / Copy pairing URL, and **Check for updates…** for
  one-click self-updates
- **Pairing** — QR popup on launch that closes itself once your phone
  pairs; fresh auth token on every run, valid only on your local WiFi

## From source

```bash
cargo run --release
```

Options:

```
openmouse --port 7890        # default port
openmouse --open-browser     # open the pairing page automatically
openmouse -v                 # verbose logging
```

### Platform notes

- **Windows**: full support via `SendInput` — mouse, keyboard (any Unicode
  text), media keys, and power actions.
- **Other platforms**: the server builds and runs, but input simulation is a
  no-op stub (actions are logged). Windows is the target for now.

## Protocol

Phone → server over WebSocket (`/ws?token=...`), JSON messages:

| Message | Meaning |
|---|---|
| `{"t":"move","dx":12,"dy":-3}` | relative mouse motion |
| `{"t":"click","button":"left"}` | click (`left`/`right`/`middle`, optional `count`) |
| `{"t":"press","button":"left"}` / `{"t":"release",...}` | mouse down/up (drags) |
| `{"t":"scroll","dx":0,"dy":3}` | scroll ticks |
| `{"t":"text","text":"hi"}` | type literal text |
| `{"t":"key","key":"enter"}` | special key (`enter`, `esc`, `f5`, arrows, …) |
| `{"t":"media","action":"playpause"}` | media key |
| `{"t":"hotkey","keys":["ctrl","c"]}` | key chord |
| `{"t":"power","action":"sleep"}` | `sleep`/`shutdown`/`restart`/`logoff` (Windows) |
| `{"t":"ping"}` | → `{"t":"pong"}` |

## Security

- The token is generated fresh on every run and embedded in the pairing URL.
  Anyone on your LAN with the URL can control the machine — treat the QR
  like a password and don't share screenshots of it.
- Traffic is plain HTTP/WebSocket on your local network (no TLS yet).
  Don't expose the port to the internet.

## License

MIT — see [LICENSE](LICENSE).
