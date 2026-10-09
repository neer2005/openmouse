"""OpenMouse server: serves the phone web UI and relays input over WebSocket.

Run with:  python -m openmouse

Flow:
  1. Server starts on --port (default 7890) and prints a pairing URL + QR code.
  2. On your phone (same WiFi), scan the QR or open the URL.
  3. The phone UI opens a WebSocket back to the server and streams input
     events as JSON messages (see PROTOCOL below).

PROTOCOL (client -> server, JSON over WebSocket):
  {"t":"move","dx":12.5,"dy":-3.0}   relative mouse motion (CSS px, scaled client-side)
  {"t":"click","button":"left"}       button: left|right|middle; optional "count" (1|2)
  {"t":"press","button":"left"}       mouse button down (for drags)
  {"t":"release","button":"left"}     mouse button up
  {"t":"scroll","dx":0,"dy":3}        scroll wheel ticks
  {"t":"text","text":"hello"}         type literal text
  {"t":"key","key":"enter"}           special key by name (see input._SPECIAL_KEYS)
  {"t":"media","action":"playpause"}  media key (see input._MEDIA_KEYS)
  {"t":"hotkey","keys":["ctrl","c"]}  chord of special keys / single chars
  {"t":"power","action":"sleep"}      sleep|shutdown|restart|logoff (Windows)
  {"t":"ping"}                        -> server replies {"t":"pong"}

Auth: every HTTP route except / and /qr.png, and the WebSocket endpoint,
require ?token=<server-generated secret>. The token is embedded in the
pairing URL / QR code. Anyone on your LAN with the URL can control the
machine, so treat the QR like a password and regenerate by restarting.
"""
from __future__ import annotations

import argparse
import asyncio
import io
import logging
import secrets
import socket

from aiohttp import web

from . import __name__ as _pkg  # noqa: F401  (keeps linters calm)
from .input import create_controller
from .net import get_lan_ip

log = logging.getLogger("openmouse.server")

STATIC_DIR = __import__("os").path.join(
    __import__("os").path.dirname(__file__), "static")


def _qr_png(url: str) -> bytes:
    import qrcode

    img = qrcode.make(url, box_size=8, border=2)
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def build_app(controller, token: str, lan_ip: str, port: int) -> web.Application:
    app = web.Application()
    app["controller"] = controller
    app["token"] = token
    app["clients"] = set()

    def authorized(request: web.Request) -> bool:
        return secrets.compare_digest(
            request.query.get("token", ""), token)

    async def index(request: web.Request) -> web.Response:
        # Desktop landing page: pairing info + QR.
        pair_url = f"http://{lan_ip}:{port}/remote?token={token}"
        html = f"""<!doctype html>
<html><head><meta charset="utf-8"><title>OpenMouse</title>
<style>body{{font-family:system-ui,sans-serif;max-width:640px;margin:3rem auto;
padding:0 1rem;line-height:1.5}}code{{background:#eee;padding:.2em .4em;
border-radius:4px}}img{{border:1px solid #ddd;border-radius:8px}}</style></head>
<body>
<h1>&#128433; OpenMouse server running</h1>
<p>On your phone (same WiFi), scan this QR code or open:</p>
<p><img src="/qr.png" width="280" height="280" alt="pairing QR"></p>
<p><code>{pair_url}</code></p>
<p>Backend: <b>{controller.name}</b> &middot;
Connected phones: <b id="n">0</b></p>
<script>setInterval(async()=>{{try{{const r=await fetch('/api/status?token={token}');
const j=await r.json();document.getElementById('n').textContent=j.clients;}}catch(e){{}}}},2000);</script>
</body></html>"""
        return web.Response(text=html, content_type="text/html")

    async def qr(request: web.Request) -> web.Response:
        pair_url = f"http://{lan_ip}:{port}/remote?token={token}"
        return web.Response(body=_qr_png(pair_url), content_type="image/png")

    async def remote_ui(request: web.Request) -> web.Response:
        if not authorized(request):
            return web.Response(status=403, text="bad token - rescan the QR code")
        return web.FileResponse(__import__("os").path.join(STATIC_DIR, "index.html"))

    async def static_file(request: web.Request) -> web.Response:
        if not authorized(request):
            return web.Response(status=403, text="bad token")
        name = request.match_info["name"]
        if name not in ("app.js", "style.css"):
            return web.Response(status=404, text="not found")
        return web.FileResponse(__import__("os").path.join(STATIC_DIR, name))

    async def status(request: web.Request) -> web.Response:
        if not authorized(request):
            return web.Response(status=403, text="bad token")
        return web.json_response({
            "backend": controller.name,
            "clients": len(app["clients"]),
        })

    async def ws_handler(request: web.Request) -> web.WebSocketResponse:
        if not authorized(request):
            return web.Response(status=403, text="bad token")
        ws = web.WebSocketResponse(heartbeat=30)
        await ws.prepare(request)
        app["clients"].add(ws)
        log.info("phone connected (%d total)", len(app["clients"]))
        try:
            async for msg in ws:
                if msg.type != web.WSMsgType.TEXT:
                    continue
                await handle_message(msg.data, ws)
        finally:
            app["clients"].discard(ws)
            log.info("phone disconnected (%d total)", len(app["clients"]))
        return ws

    async def handle_message(raw: str, ws: web.WebSocketResponse) -> None:
        import json

        try:
            data = json.loads(raw)
        except ValueError:
            return
        t = data.get("t")
        c = controller
        try:
            if t == "ping":
                await ws.send_json({"t": "pong"})
            elif t == "move":
                c.move(float(data.get("dx", 0)), float(data.get("dy", 0)))
            elif t == "click":
                c.click(str(data.get("button", "left")),
                        int(data.get("count", 1)))
            elif t == "press":
                c.press(str(data.get("button", "left")))
            elif t == "release":
                c.release(str(data.get("button", "left")))
            elif t == "scroll":
                c.scroll(float(data.get("dx", 0)), float(data.get("dy", 0)))
            elif t == "text":
                c.type_text(str(data.get("text", ""))[:500])
            elif t == "key":
                c.press_key(str(data.get("key", "")))
            elif t == "media":
                c.media(str(data.get("action", "")))
            elif t == "hotkey":
                keys = data.get("keys", [])
                if isinstance(keys, list) and keys:
                    c.hotkey(*[str(k) for k in keys[:4]])
            elif t == "power":
                c.power(str(data.get("action", "")))
            else:
                log.warning("unknown message type: %r", t)
        except Exception:  # noqa: BLE001 - one bad event must not kill the socket
            log.exception("error handling %r", t)

    app.router.add_get("/", index)
    app.router.add_get("/qr.png", qr)
    app.router.add_get("/remote", remote_ui)
    app.router.add_get("/static/{name}", static_file)
    app.router.add_get("/api/status", status)
    app.router.add_get("/ws", ws_handler)
    return app


def main(argv=None) -> None:
    parser = argparse.ArgumentParser(
        prog="openmouse", description="OpenMouse server - phone as wireless mouse/keyboard")
    parser.add_argument("--port", type=int, default=7890)
    parser.add_argument("--host", default="0.0.0.0",
                        help="interface to bind (default: all)")
    parser.add_argument("--null-input", action="store_true",
                        help="don't touch real input (testing)")
    parser.add_argument("--open-browser", action="store_true",
                        help="open the pairing page in the default browser")
    parser.add_argument("-v", "--verbose", action="store_true")
    args = parser.parse_args(argv)

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s: %(message)s")

    controller = create_controller(force_null=args.null_input)
    token = secrets.token_urlsafe(24)
    lan_ip = get_lan_ip()
    pair_url = f"http://{lan_ip}:{args.port}/remote?token={token}"

    # Best-effort mDNS advertisement so the phone could discover us later.
    def _advertise():
        try:
            from zeroconf import ServiceInfo, Zeroconf
            info = ServiceInfo(
                "_openmouse._tcp.local.",
                "OpenMouse._openmouse._tcp.local.",
                addresses=[socket.inet_aton(lan_ip)],
                port=args.port,
                properties={"token": token},
            )
            zc = Zeroconf()
            zc.register_service(info)
            log.info("mDNS: advertised as OpenMouse._openmouse._tcp.local.")
            return zc
        except Exception as exc:  # noqa: BLE001
            log.warning("mDNS unavailable: %s", exc)
            return None

    _mdns = _advertise()  # keep a reference: GC would unregister the service

    print("\n  OpenMouse server running")
    print(f"  Backend : {controller.name}")
    print(f"  Phone   : {pair_url}\n")
    try:
        import qrcode

        qr = qrcode.QRCode(box_size=1, border=1)
        qr.add_data(pair_url)
        qr.print_ascii(invert=True)
        print()
    except Exception:  # noqa: BLE001
        pass
    print("  Scan the QR with your phone (same WiFi). Ctrl+C to stop.\n")

    app = build_app(controller, token, lan_ip, args.port)

    if args.open_browser:
        import threading
        import webbrowser

        url = f"http://127.0.0.1:{args.port}/"
        threading.Timer(1.5, webbrowser.open, args=[url]).start()

    web.run_app(app, host=args.host, port=args.port, print=None)


if __name__ == "__main__":
    main()
