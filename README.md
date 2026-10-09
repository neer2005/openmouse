# OpenMouse 🖱️

Turn your phone into a wireless mouse, keyboard, and trackpad — an open-source
alternative to Remote Mouse. A tiny server runs on your computer and serves a
web remote to your phone over your local WiFi. No accounts, no cloud, no app
store: just open a URL.

## Features (prototype)

- **Trackpad** — 1-finger move, tap to click, press-and-hold to drag,
  2-finger tap for right-click, 2-finger slide to scroll, 3-finger tap for
  middle-click
- **Keyboard** — full text input via your phone's own keyboard, plus Esc,
  arrows, Enter, Backspace, F5, and hotkeys (Alt+F4, Ctrl+C/V)
- **Media controls** — play/pause, prev/next, volume, mute
- **Power controls** (Windows) — sleep, restart, log off, shut down from the
  phone, each with a confirm dialog
- **Pairing** — QR code in the terminal; per-run token auth
- **Discovery** — mDNS advertisement (`_openmouse._tcp.local.`)

## Quick start

### Windows (recommended)

1. Install Python 3.9+ from <https://www.python.org/downloads/>
   (tick **"Add python.exe to PATH"** during setup).
2. Double-click **`run.bat`**. It creates an isolated environment, installs
   everything, opens your browser with the pairing QR, and starts the server.
3. If Windows Firewall asks, allow it on **private** networks.
4. Scan the QR with your phone (same WiFi) — the remote opens in the browser.

To run again later, just double-click `run.bat`.

### macOS / Linux

```bash
pip install -r requirements.txt
python -m openmouse --open-browser
```

Scan the QR code with your phone (must be on the same WiFi) — the remote
opens in your phone's browser. That's it.

Options:

```bash
python -m openmouse --port 7890        # default port
python -m openmouse --open-browser     # open the pairing page automatically
python -m openmouse --null-input       # run without touching real input (testing)
python -m openmouse -v                 # verbose logging
```

### Platform notes

- **Windows**: full support. Power actions (sleep / restart / shut down from
  the phone, with a confirm dialog) are Windows-only. `run.bat` handles setup.
- **macOS**: grant the terminal (or your Python) Accessibility permission
  when prompted, or input simulation won't work.
- **Linux (X11)**: full support via `pynput`.
- **Linux (Wayland)**: input simulation needs a compositor portal; if it
  doesn't work, the server logs a warning and runs in no-op mode.
- Media keys depend on OS support.

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

## Roadmap

- [ ] TLS (self-signed) for the WebSocket
- [ ] PIN pairing instead of URL token
- [ ] Native iOS/Android clients (Flutter) for lower latency
- [ ] Gyroscope "air mouse" mode
- [ ] Customizable buttons / app launcher panel
- [ ] Cross-device clipboard sync

## License

MIT — see [LICENSE](LICENSE).
