"""Desktop input simulation for OpenMouse.

Wraps pynput with a clean action API and degrades gracefully to a no-op
controller when no display/input backend is available (headless servers,
Wayland without the right portal, CI, ...). The rest of the app never has
to care which backend is active.
"""
from __future__ import annotations

import logging

log = logging.getLogger("openmouse.input")

# Map of friendly key names (sent by the phone client) to pynput keys.
_SPECIAL_KEYS = {}
_MEDIA_KEYS = {}


def _build_key_maps():
    try:
        from pynput.keyboard import Key
    except Exception:  # pragma: no cover - handled by caller
        return
    _SPECIAL_KEYS.update({
        "enter": Key.enter,
        "backspace": Key.back_space,
        "tab": Key.tab,
        "esc": Key.esc,
        "escape": Key.esc,
        "space": Key.space,
        "up": Key.up,
        "down": Key.down,
        "left": Key.left,
        "right": Key.right,
        "delete": Key.delete,
        "home": Key.home,
        "end": Key.end,
        "pageup": Key.page_up,
        "pagedown": Key.page_down,
        "insert": Key.insert,
        "capslock": Key.caps_lock,
        "shift": Key.shift,
        "ctrl": Key.ctrl,
        "alt": Key.alt,
        "super": Key.cmd,   # Win / Cmd key
        "menu": Key.menu,
    })
    for i in range(1, 13):
        _SPECIAL_KEYS[f"f{i}"] = getattr(Key, f"f{i}")
    _MEDIA_KEYS.update({
        "playpause": Key.media_play_pause,
        "play": Key.media_play_pause,
        "pause": Key.media_play_pause,
        "next": Key.media_next,
        "prev": Key.media_previous,
        "previous": Key.media_previous,
        "stop": Key.media_stop,
        "volup": Key.media_volume_up,
        "volumeup": Key.media_volume_up,
        "voldn": Key.media_volume_down,
        "volumedown": Key.media_volume_down,
        "mute": Key.media_volume_mute,
    })


class InputController:
    """Real input backend backed by pynput."""

    name = "pynput"

    def __init__(self) -> None:
        from pynput.mouse import Controller as MouseController, Button
        from pynput.keyboard import Controller as KeyboardController

        _build_key_maps()
        self._mouse = MouseController()
        self._keyboard = KeyboardController()
        self._Button = Button
        # Touching .position forces backend init; raises on headless X-less
        # systems so the factory below can fall back to NullController.
        self._mouse.position  # noqa: B018

    # -- mouse ---------------------------------------------------------
    def move(self, dx: float, dy: float) -> None:
        self._mouse.move(dx, dy)

    def move_to(self, x: float, y: float) -> None:
        self._mouse.position = (x, y)

    def click(self, button: str = "left", count: int = 1) -> None:
        btn = {"left": self._Button.left, "right": self._Button.right,
               "middle": self._Button.middle}.get(button, self._Button.left)
        self._mouse.click(btn, count)

    def press(self, button: str = "left") -> None:
        btn = {"left": self._Button.left, "right": self._Button.right,
               "middle": self._Button.middle}.get(button, self._Button.left)
        self._mouse.press(btn)

    def release(self, button: str = "left") -> None:
        btn = {"left": self._Button.left, "right": self._Button.right,
               "middle": self._Button.middle}.get(button, self._Button.left)
        self._mouse.release(btn)

    def scroll(self, dx: float, dy: float) -> None:
        self._mouse.scroll(dx, dy)

    # -- keyboard ------------------------------------------------------
    def type_text(self, text: str) -> None:
        if text:
            self._keyboard.type(text)

    def press_key(self, name: str) -> None:
        """Press+release a named special key ('enter', 'f5', ...) or char."""
        key = _SPECIAL_KEYS.get(name.lower())
        if key is None:
            if len(name) == 1:
                key = name
            else:
                log.warning("unknown key name: %r", name)
                return
        self._keyboard.press(key)
        self._keyboard.release(key)

    def media(self, action: str) -> None:
        key = _MEDIA_KEYS.get(action.lower())
        if key is None:
            log.warning("unknown media action: %r", action)
            return
        self._keyboard.press(key)
        self._keyboard.release(key)

    def hotkey(self, *names: str) -> None:
        """Press a chord, e.g. hotkey('ctrl', 'alt', 't')."""
        from pynput.keyboard import Key as _Key  # noqa: F401  (kept local)

        keys = []
        for n in names:
            k = _SPECIAL_KEYS.get(n.lower())
            keys.append(k if k is not None else (n if len(n) == 1 else None))
        keys = [k for k in keys if k is not None]
        if not keys:
            return
        for k in keys:
            self._keyboard.press(k)
        for k in reversed(keys):
            self._keyboard.release(k)

    def power(self, action: str) -> None:
        """System power action: sleep | shutdown | restart | logoff.

        Implemented on Windows; on other platforms logs a warning.
        Shutdown/restart use a 5-second delay so a misclick is recoverable
        with `shutdown /a`.
        """
        import os
        import subprocess

        if os.name != "nt":
            log.warning("power actions are only implemented on Windows")
            return
        cmds = {
            "sleep": ["rundll32.exe", "powrprof.dll,SetSuspendState", "0,1,0"],
            "shutdown": ["shutdown", "/s", "/t", "5",
                         "/c", "OpenMouse remote shutdown"],
            "restart": ["shutdown", "/r", "/t", "5",
                        "/c", "OpenMouse remote restart"],
            "logoff": ["shutdown", "/l"],
        }
        cmd = cmds.get(action.lower())
        if not cmd:
            log.warning("unknown power action: %r", action)
            return
        log.info("power action: %s", action)
        subprocess.Popen(cmd)  # noqa: S603 - fixed command table above


class NullController:
    """No-op backend: logs actions, performs nothing.

    Used when no input backend is available (headless test runs). The
    server and protocol stay fully testable.
    """

    name = "null"

    def _log(self, action: str, *args) -> None:
        log.info("[null-input] %s%r", action, args)

    def move(self, dx, dy): self._log("move", dx, dy)
    def move_to(self, x, y): self._log("move_to", x, y)
    def click(self, button="left", count=1): self._log("click", button, count)
    def press(self, button="left"): self._log("press", button)
    def release(self, button="left"): self._log("release", button)
    def scroll(self, dx, dy): self._log("scroll", dx, dy)
    def type_text(self, text): self._log("type_text", text)
    def press_key(self, name): self._log("press_key", name)
    def media(self, action): self._log("media", action)
    def hotkey(self, *names): self._log("hotkey", names)
    def power(self, action): self._log("power", action)


def create_controller(force_null: bool = False):
    """Return a working InputController, or NullController as fallback."""
    if force_null:
        log.info("using null input backend (forced)")
        return NullController()
    try:
        ctl = InputController()
        log.info("using pynput input backend")
        return ctl
    except Exception as exc:  # noqa: BLE001 - any backend failure -> fallback
        log.warning("pynput backend unavailable (%s); using null backend", exc)
        return NullController()
