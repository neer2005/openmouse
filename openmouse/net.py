"""Small network helpers for OpenMouse."""
from __future__ import annotations

import socket


def get_lan_ip() -> str:
    """Best-guess LAN IPv4 address of this machine.

    Opens a UDP socket to a public address (no packets are sent) and reads
    back the local address the OS would route through.
    """
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.connect(("8.8.8.8", 80))
            return s.getsockname()[0]
    except OSError:
        return "127.0.0.1"
