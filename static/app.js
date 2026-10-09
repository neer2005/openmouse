/* OpenMouse phone client: trackpad gestures, keyboard relay, media keys. */
(function () {
  'use strict';

  var qs = new URLSearchParams(location.search);
  var token = qs.get('token') || '';
  var wsProto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  var wsUrl = wsProto + '//' + location.host + '/ws?token=' + encodeURIComponent(token);

  var statusEl = document.getElementById('status');
  var pad = document.getElementById('pad');
  document.body.style.touchAction = 'manipulation';

  /* ---------- WebSocket with auto-reconnect ---------- */
  var ws = null, reconnectDelay = 1000;

  function send(msg) {
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(msg));
    }
    // events while disconnected are dropped (except nothing critical)
  }

  function connect() {
    ws = new WebSocket(wsUrl);
    ws.onopen = function () {
      statusEl.classList.add('on');
      reconnectDelay = 1000;
      send({ t: 'ping' });
    };
    ws.onclose = function () {
      statusEl.classList.remove('on');
      setTimeout(connect, reconnectDelay);
      reconnectDelay = Math.min(reconnectDelay * 2, 10000);
    };
    ws.onerror = function () { try { ws.close(); } catch (e) {} };
  }
  connect();

  /* ---------- trackpad gestures ---------- */
  var sensitivity = 2;
  document.getElementById('sens').addEventListener('input', function (e) {
    sensitivity = parseFloat(e.target.value);
  });

  var TAP_MAX_MS = 300, TAP_MAX_PX = 16, HOLD_MS = 380, SCROLL_PX_PER_TICK = 24;
  var fingers = new Map();   // touchId -> {x,y,sx,sy,st}
  var holdTimer = null;
  var dragging = false;
  var scrollAcc = 0;
  var pair = null;           // two/three-finger tap candidate {n, st, moved}

  function clearHold() { if (holdTimer) { clearTimeout(holdTimer); holdTimer = null; } }

  function endDrag() {
    if (dragging) {
      dragging = false;
      pad.classList.remove('dragging');
      send({ t: 'release', button: 'left' });
    }
  }

  pad.addEventListener('contextmenu', function (e) { e.preventDefault(); });

  pad.addEventListener('touchstart', function (e) {
    e.preventDefault();
    for (var i = 0; i < e.changedTouches.length; i++) {
      var t = e.changedTouches[i];
      fingers.set(t.identifier, { x: t.clientX, y: t.clientY, sx: t.clientX, sy: t.clientY, st: performance.now() });
    }
    if (fingers.size === 1) {
      clearHold();
      pair = null;
      holdTimer = setTimeout(function () {
        holdTimer = null;
        if (fingers.size === 1 && !dragging) {
          var f = fingers.values().next().value;
          if (Math.hypot(f.x - f.sx, f.y - f.sy) < TAP_MAX_PX) {
            dragging = true;
            pad.classList.add('dragging');
            send({ t: 'press', button: 'left' });
          }
        }
      }, HOLD_MS);
    } else if (fingers.size === 2 || fingers.size === 3) {
      clearHold();
      pair = { n: fingers.size, st: performance.now(), moved: false };
    } else {
      clearHold();
      pair = null;
    }
  }, { passive: false });

  pad.addEventListener('touchmove', function (e) {
    e.preventDefault();
    if (fingers.size === 1) {
      var t = e.changedTouches[0];
      var f = fingers.get(t.identifier);
      if (!f) return;
      var dx = (t.clientX - f.x) * sensitivity;
      var dy = (t.clientY - f.y) * sensitivity;
      f.x = t.clientX; f.y = t.clientY;
      var dist = Math.hypot(f.x - f.sx, f.y - f.sy);
      if (dragging || dist > TAP_MAX_PX) {
        if (!dragging) clearHold(); // moved: cancel tap/hold
        send({ t: 'move', dx: Math.round(dx * 10) / 10, dy: Math.round(dy * 10) / 10 });
      }
    } else if (fingers.size === 2) {
      var dySum = 0, n = 0;
      for (var i = 0; i < e.changedTouches.length; i++) {
        var t2 = e.changedTouches[i];
        var f2 = fingers.get(t2.identifier);
        if (!f2) continue;
        dySum += t2.clientY - f2.y;
        f2.x = t2.clientX; f2.y = t2.clientY; n++;
        if (Math.hypot(f2.x - f2.sx, f2.y - f2.sy) > TAP_MAX_PX && pair) pair.moved = true;
      }
      if (n) {
        scrollAcc += dySum / n;
        var ticks = Math.trunc(scrollAcc / SCROLL_PX_PER_TICK);
        if (ticks !== 0) {
          send({ t: 'scroll', dx: 0, dy: ticks }); // natural: fingers down = scroll up
          scrollAcc -= ticks * SCROLL_PX_PER_TICK;
        }
      }
    } else if (pair) {
      pair.moved = true;
    }
  }, { passive: false });

  function onTouchEnd(e) {
    e.preventDefault();
    var now = performance.now();
    var countBefore = fingers.size;
    var tapCandidates = [];
    for (var i = 0; i < e.changedTouches.length; i++) {
      var t = e.changedTouches[i];
      var f = fingers.get(t.identifier);
      if (f) {
        tapCandidates.push({
          dt: now - f.st,
          dist: Math.hypot(t.clientX - f.sx, t.clientY - f.sy)
        });
        fingers.delete(t.identifier);
      }
    }
    clearHold();

    if (dragging) {
      if (fingers.size === 0) endDrag();
      pair = null;
      return;
    }

    // multi-finger tap -> right (2) / middle (3) click
    if (pair && fingers.size === 0 && !pair.moved) {
      var allQuick = tapCandidates.length === pair.n &&
        tapCandidates.every(function (c) { return c.dt < TAP_MAX_MS && c.dist < TAP_MAX_PX; }) &&
        (now - pair.st) < TAP_MAX_MS;
      if (allQuick) {
        send({ t: 'click', button: pair.n === 3 ? 'middle' : 'right' });
        pair = null;
        return;
      }
    }
    pair = null;

    // single-finger tap -> left click
    if (countBefore === 1 && tapCandidates.length === 1) {
      var c = tapCandidates[0];
      if (c.dt < TAP_MAX_MS && c.dist < TAP_MAX_PX) {
        send({ t: 'click', button: 'left' });
      }
    }
  }
  pad.addEventListener('touchend', onTouchEnd, { passive: false });
  pad.addEventListener('touchcancel', function (e) {
    for (var i = 0; i < e.changedTouches.length; i++) {
      fingers.delete(e.changedTouches[i].identifier);
    }
    clearHold();
    endDrag();
    pair = null;
  }, { passive: false });
  document.addEventListener('visibilitychange', function () {
    if (document.hidden) endDrag();
  });

  /* ---------- click buttons ---------- */
  document.getElementById('btn-left').addEventListener('click', function () {
    send({ t: 'click', button: 'left' });
  });
  document.getElementById('btn-right').addEventListener('click', function () {
    send({ t: 'click', button: 'right' });
  });

  /* ---------- panels ---------- */
  var kbPanel = document.getElementById('kb-panel');
  var mediaPanel = document.getElementById('media-panel');
  var powerPanel = document.getElementById('power-panel');
  var btnKb = document.getElementById('btn-kb');
  var btnMedia = document.getElementById('btn-media');
  var btnPower = document.getElementById('btn-power');

  function togglePanel(panel, btn) {
    var open = panel.classList.contains('open');
    kbPanel.classList.remove('open');
    mediaPanel.classList.remove('open');
    powerPanel.classList.remove('open');
    btnKb.classList.remove('active');
    btnMedia.classList.remove('active');
    btnPower.classList.remove('active');
    if (!open) { panel.classList.add('open'); btn.classList.add('active'); }
  }
  btnMedia.addEventListener('click', function () { togglePanel(mediaPanel, btnMedia); });
  btnPower.addEventListener('click', function () { togglePanel(powerPanel, btnPower); });

  /* ---------- keyboard relay via hidden input ---------- */
  var kbProxy = document.getElementById('kb-proxy');
  btnKb.addEventListener('click', function () {
    togglePanel(kbPanel, btnKb);
    kbProxy.focus({ preventScroll: true });
  });
  // keep focus so the OS keyboard stays up while typing
  kbProxy.addEventListener('blur', function () {
    if (kbPanel.classList.contains('open')) {
      setTimeout(function () { kbProxy.focus({ preventScroll: true }); }, 50);
    }
  });
  kbProxy.addEventListener('input', function () {
    var v = kbProxy.value;
    if (v) { send({ t: 'text', text: v }); kbProxy.value = ''; }
  });
  var SPECIAL = {
    Backspace: 'backspace', Enter: 'enter', Tab: 'tab', Escape: 'esc',
    ArrowUp: 'up', ArrowDown: 'down', ArrowLeft: 'left', ArrowRight: 'right',
    Delete: 'delete'
  };
  kbProxy.addEventListener('keydown', function (e) {
    var name = SPECIAL[e.key];
    if (name) {
      e.preventDefault();
      send({ t: 'key', key: name });
    }
    // printable characters flow through the 'input' event instead
  });

  /* ---------- special keys / hotkeys / media buttons ---------- */
  document.querySelectorAll('[data-key]').forEach(function (el) {
    el.addEventListener('click', function () { send({ t: 'key', key: el.dataset.key }); });
  });
  document.querySelectorAll('[data-hotkey]').forEach(function (el) {
    el.addEventListener('click', function () {
      send({ t: 'hotkey', keys: el.dataset.hotkey.split(',') });
    });
  });
  document.querySelectorAll('[data-media]').forEach(function (el) {
    el.addEventListener('click', function () { send({ t: 'media', action: el.dataset.media }); });
  });

  /* ---------- power actions (confirm first — no accidental shutdowns) ---------- */
  var POWER_LABELS = { sleep: 'put the PC to sleep', logoff: 'log off',
                       restart: 'restart the PC', shutdown: 'SHUT DOWN the PC' };
  document.querySelectorAll('[data-power]').forEach(function (el) {
    el.addEventListener('click', function () {
      var action = el.dataset.power;
      if (window.confirm('Are you sure you want to ' + (POWER_LABELS[action] || action) + '?')) {
        send({ t: 'power', action: action });
      }
    });
  });
})();
