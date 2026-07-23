// Stake Dev Tool — share-link feedback widget.
//
// Injected by the share host into the front bundle's index.html when the link
// has feedback enabled. Fully self-contained (no dependencies, inline styles,
// own z-index layer). It:
//   • patches fetch/XHR to observe `/wallet/play` responses so it always knows
//     the current round (mode + book eventId) and shows it in a pill;
//   • lets the visitor annotate the live game (pen / rectangle / ellipse /
//     arrow) and write a note, Excalidraw-style;
//   • captures a best-effort screenshot of the game canvases;
//   • POSTs everything to `/__share/feedback`, stamped with the last played
//     round (the server also keeps a fallback per-session round record).
(function () {
  'use strict';
  if (window.__sdtFeedback) return;
  window.__sdtFeedback = true;

  var Z = 2147483600;
  var FONT = "13px/1.45 system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif";
  var COLORS = ['#ff4d4f', '#ffd43b', '#51cf66', '#4dabf7'];
  var TOOLS = [
    { id: 'pen', label: 'Draw', icon: '✏️' },
    { id: 'rect', label: 'Box', icon: '▭' },
    { id: 'ellipse', label: 'Circle', icon: '◯' },
    { id: 'arrow', label: 'Arrow', icon: '↗' }
  ];

  // --- state -----------------------------------------------------------------
  var sessionId = new URLSearchParams(location.search).get('sessionID') || '';
  var lastRound = null; // { mode, eventId }
  var shapes = [];
  var tool = 'pen';
  var color = COLORS[0];
  var drawing = null; // in-progress shape
  var overlayOpen = false;
  var screenshot = null; // data URL captured when the overlay opens
  var sending = false;

  // --- round tracking (fetch + XHR patches) -----------------------------------
  function onPlayResponse(json) {
    try {
      var round = json && json.round;
      if (!round || round.event == null) return;
      var eventId = parseInt(String(round.event), 10);
      if (!isFinite(eventId)) return;
      lastRound = { mode: String(round.mode || ''), eventId: eventId };
      updatePill();
      updateRoundNote();
    } catch (e) { /* ignore */ }
  }

  function rememberSession(body) {
    try {
      if (typeof body !== 'string') return;
      var json = JSON.parse(body);
      if (json && json.sessionID) sessionId = String(json.sessionID);
    } catch (e) { /* not JSON */ }
  }

  var PLAY_RE = /\/wallet\/play(\?|$)/;

  var origFetch = window.fetch;
  if (origFetch) {
    window.fetch = function (input, init) {
      var promise = origFetch.apply(this, arguments);
      try {
        var url = typeof input === 'string' ? input : (input && input.url) || '';
        if (PLAY_RE.test(url)) {
          if (init && init.body) rememberSession(init.body);
          promise.then(function (res) {
            try {
              res.clone().json().then(onPlayResponse, function () {});
            } catch (e) { /* opaque response */ }
          }, function () {});
        }
      } catch (e) { /* never break the game's fetch */ }
      return promise;
    };
  }

  var origOpen = XMLHttpRequest.prototype.open;
  var origSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url) {
    this.__sdtUrl = String(url || '');
    return origOpen.apply(this, arguments);
  };
  XMLHttpRequest.prototype.send = function (body) {
    try {
      if (this.__sdtUrl && PLAY_RE.test(this.__sdtUrl)) {
        rememberSession(body);
        this.addEventListener('load', function () {
          try { onPlayResponse(JSON.parse(this.responseText)); } catch (e) { /* ignore */ }
        });
      }
    } catch (e) { /* never break the game's XHR */ }
    return origSend.apply(this, arguments);
  };

  // --- DOM helpers -------------------------------------------------------------
  function el(tag, styles, text) {
    var node = document.createElement(tag);
    if (styles) for (var k in styles) node.style[k] = styles[k];
    if (text != null) node.textContent = text;
    return node;
  }

  function chipStyle(active) {
    return {
      font: FONT,
      padding: '5px 10px',
      borderRadius: '8px',
      border: '1px solid ' + (active ? '#4c7dff' : '#2c3446'),
      background: active ? 'rgba(76,125,255,.22)' : 'rgba(15,20,32,.9)',
      color: '#e6e9ef',
      cursor: 'pointer',
      userSelect: 'none'
    };
  }

  function applyStyles(node, styles) {
    for (var k in styles) node.style[k] = styles[k];
  }

  // --- round pill + feedback button ---------------------------------------------
  var pill = el('div', {
    position: 'fixed', left: '12px', bottom: '12px', zIndex: String(Z),
    font: FONT, fontSize: '12px', padding: '6px 12px', borderRadius: '999px',
    background: 'rgba(11,14,20,.85)', border: '1px solid #232a3a', color: '#9aa4b8',
    pointerEvents: 'none', whiteSpace: 'nowrap'
  });
  pill.setAttribute('data-sdt-feedback', 'pill');

  function updatePill() {
    while (pill.firstChild) pill.removeChild(pill.firstChild);
    var label = el('span', { color: '#9aa4b8' }, 'Round ');
    pill.appendChild(label);
    if (lastRound) {
      var value = el('span', { color: '#e6e9ef', fontWeight: '600' },
        lastRound.mode + ' · #' + lastRound.eventId);
      pill.appendChild(value);
    } else {
      pill.appendChild(el('span', { color: '#5b6577' }, '—'));
    }
  }
  updatePill();

  var fbBtn = el('button', {
    position: 'fixed', right: '12px', bottom: '12px', zIndex: String(Z),
    font: FONT, fontWeight: '600', padding: '8px 14px', borderRadius: '999px',
    border: '1px solid #2c3446', background: 'rgba(11,14,20,.9)', color: '#e6e9ef',
    cursor: 'pointer', boxShadow: '0 4px 18px rgba(0,0,0,.4)'
  }, '💬 Feedback');
  fbBtn.type = 'button';
  fbBtn.addEventListener('click', openOverlay);

  // --- toast ---------------------------------------------------------------------
  var toastTimer = null;
  function toast(text) {
    var node = document.getElementById('__sdt-fb-toast');
    if (!node) {
      node = el('div', {
        position: 'fixed', left: '50%', bottom: '24px', transform: 'translateX(-50%)',
        zIndex: String(Z + 9), font: FONT, padding: '10px 16px', borderRadius: '10px',
        background: 'rgba(20,25,36,.96)', border: '1px solid #2c3446', color: '#e6e9ef',
        boxShadow: '0 8px 30px rgba(0,0,0,.5)', transition: 'opacity .25s', opacity: '0'
      });
      node.id = '__sdt-fb-toast';
      document.body.appendChild(node);
    }
    node.textContent = text;
    node.style.opacity = '1';
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(function () { node.style.opacity = '0'; }, 3200);
  }

  // --- screenshot (best effort) ----------------------------------------------------
  function captureScreenshot() {
    try {
      var w = window.innerWidth, h = window.innerHeight;
      if (!w || !h) return null;
      var scale = Math.min(1, 1280 / w);
      var canvas = document.createElement('canvas');
      canvas.width = Math.max(1, Math.round(w * scale));
      canvas.height = Math.max(1, Math.round(h * scale));
      var g = canvas.getContext('2d');
      var bg = '#000';
      try { bg = getComputedStyle(document.body).backgroundColor || '#000'; } catch (e) { /* keep */ }
      g.fillStyle = bg;
      g.fillRect(0, 0, canvas.width, canvas.height);
      var sources = document.querySelectorAll('canvas');
      for (var i = 0; i < sources.length; i++) {
        var src = sources[i];
        if (src.getAttribute('data-sdt-feedback')) continue;
        var rect = src.getBoundingClientRect();
        if (rect.width <= 0 || rect.height <= 0) continue;
        try {
          g.drawImage(src, rect.left * scale, rect.top * scale, rect.width * scale, rect.height * scale);
        } catch (e) { /* tainted or gone — skip */ }
      }
      var url = canvas.toDataURL('image/jpeg', 0.6);
      // Keep the request comfortably under the server's body limit.
      return url.length > 900000 ? null : url;
    } catch (e) {
      return null;
    }
  }

  // --- overlay -------------------------------------------------------------------
  var overlay = el('div', {
    position: 'fixed', inset: '0', zIndex: String(Z + 1), display: 'none'
  });
  overlay.setAttribute('data-sdt-feedback', 'overlay');

  var scrim = el('div', {
    position: 'absolute', inset: '0', background: 'rgba(5,7,12,.25)'
  });
  overlay.appendChild(scrim);

  var drawCanvas = el('canvas', {
    position: 'absolute', inset: '0', width: '100%', height: '100%',
    cursor: 'crosshair', touchAction: 'none'
  });
  drawCanvas.setAttribute('data-sdt-feedback', 'canvas');
  overlay.appendChild(drawCanvas);

  // Toolbar (top center)
  var toolbar = el('div', {
    position: 'absolute', top: '12px', left: '50%', transform: 'translateX(-50%)',
    display: 'flex', gap: '6px', alignItems: 'center', padding: '8px',
    borderRadius: '12px', background: 'rgba(11,14,20,.92)', border: '1px solid #232a3a',
    boxShadow: '0 8px 30px rgba(0,0,0,.45)', flexWrap: 'wrap', justifyContent: 'center',
    maxWidth: 'calc(100vw - 24px)'
  });
  overlay.appendChild(toolbar);

  var toolButtons = {};
  TOOLS.forEach(function (t) {
    var btn = el('button', chipStyle(tool === t.id), t.icon + ' ' + t.label);
    btn.type = 'button';
    btn.title = t.label;
    btn.addEventListener('click', function () {
      tool = t.id;
      refreshToolbar();
    });
    toolButtons[t.id] = btn;
    toolbar.appendChild(btn);
  });

  toolbar.appendChild(el('span', { width: '1px', height: '22px', background: '#2c3446', margin: '0 4px' }));

  var colorButtons = [];
  COLORS.forEach(function (c) {
    var btn = el('button', {
      width: '22px', height: '22px', borderRadius: '50%', cursor: 'pointer',
      background: c, border: '2px solid ' + (color === c ? '#ffffff' : 'transparent'),
      padding: '0'
    });
    btn.type = 'button';
    btn.title = 'Color';
    btn.addEventListener('click', function () {
      color = c;
      refreshToolbar();
    });
    colorButtons.push({ btn: btn, c: c });
    toolbar.appendChild(btn);
  });

  toolbar.appendChild(el('span', { width: '1px', height: '22px', background: '#2c3446', margin: '0 4px' }));

  var undoBtn = el('button', chipStyle(false), '↩ Undo');
  undoBtn.type = 'button';
  undoBtn.addEventListener('click', function () {
    shapes.pop();
    redraw();
  });
  toolbar.appendChild(undoBtn);

  var clearBtn = el('button', chipStyle(false), 'Clear');
  clearBtn.type = 'button';
  clearBtn.addEventListener('click', function () {
    shapes = [];
    redraw();
  });
  toolbar.appendChild(clearBtn);

  function refreshToolbar() {
    TOOLS.forEach(function (t) {
      applyStyles(toolButtons[t.id], chipStyle(tool === t.id));
    });
    colorButtons.forEach(function (entry) {
      entry.btn.style.border = '2px solid ' + (color === entry.c ? '#ffffff' : 'transparent');
    });
  }

  // Bottom panel (comment + actions)
  var panel = el('div', {
    position: 'absolute', left: '50%', bottom: '12px', transform: 'translateX(-50%)',
    width: 'min(560px, calc(100vw - 24px))', padding: '12px', borderRadius: '14px',
    background: 'rgba(11,14,20,.94)', border: '1px solid #232a3a',
    boxShadow: '0 10px 40px rgba(0,0,0,.5)', display: 'flex', flexDirection: 'column', gap: '8px'
  });
  overlay.appendChild(panel);

  var panelTitle = el('div', { font: FONT, fontWeight: '600', color: '#e6e9ef' },
    'Send feedback');
  panel.appendChild(panelTitle);

  var roundNote = el('div', { font: FONT, fontSize: '12px', color: '#9aa4b8' });
  panel.appendChild(roundNote);
  function updateRoundNote() {
    roundNote.textContent = lastRound
      ? 'Attached round: ' + lastRound.mode + ' · #' + lastRound.eventId + ' (last spin played)'
      : 'No spin played yet — the feedback will not reference a round.';
  }

  var inputStyle = {
    font: FONT, color: '#e6e9ef', background: '#0f1420', border: '1px solid #2c3446',
    borderRadius: '9px', padding: '9px 10px', outline: 'none', width: '100%',
    boxSizing: 'border-box'
  };

  var nameInput = el('input', inputStyle);
  nameInput.placeholder = 'Your name (optional)';
  nameInput.maxLength = 120;
  panel.appendChild(nameInput);

  var messageInput = el('textarea', inputStyle);
  messageInput.placeholder = 'Describe the issue or idea… (you can also draw on the game)';
  messageInput.rows = 3;
  messageInput.maxLength = 4000;
  messageInput.style.resize = 'vertical';
  panel.appendChild(messageInput);

  var errorText = el('div', {
    font: FONT, fontSize: '12px', color: '#ff8a8a', display: 'none'
  });
  panel.appendChild(errorText);

  var actions = el('div', { display: 'flex', gap: '8px', justifyContent: 'flex-end' });
  panel.appendChild(actions);

  var cancelBtn = el('button', chipStyle(false), 'Cancel');
  cancelBtn.type = 'button';
  cancelBtn.addEventListener('click', closeOverlay);
  actions.appendChild(cancelBtn);

  var sendBtn = el('button', {
    font: FONT, fontWeight: '600', padding: '7px 18px', borderRadius: '9px',
    border: '0', background: '#4c7dff', color: '#fff', cursor: 'pointer'
  }, 'Send');
  sendBtn.type = 'button';
  sendBtn.addEventListener('click', submit);
  actions.appendChild(sendBtn);

  // Keep typing in the panel from reaching the game's key handlers.
  ['keydown', 'keyup', 'keypress'].forEach(function (evt) {
    panel.addEventListener(evt, function (e) { e.stopPropagation(); });
  });

  // --- drawing -----------------------------------------------------------------
  var ctx = null;

  function sizeCanvas() {
    var dpr = window.devicePixelRatio || 1;
    drawCanvas.width = Math.round(window.innerWidth * dpr);
    drawCanvas.height = Math.round(window.innerHeight * dpr);
    ctx = drawCanvas.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    redraw();
  }

  function drawShape(g, s) {
    g.strokeStyle = s.c;
    g.lineWidth = s.s;
    g.lineCap = 'round';
    g.lineJoin = 'round';
    g.beginPath();
    if (s.t === 'pen') {
      var pts = s.p;
      if (!pts || pts.length === 0) return;
      g.moveTo(pts[0][0], pts[0][1]);
      for (var i = 1; i < pts.length; i++) g.lineTo(pts[i][0], pts[i][1]);
    } else if (s.t === 'rect') {
      g.rect(s.x, s.y, s.w, s.h);
    } else if (s.t === 'ellipse') {
      g.ellipse(s.x + s.w / 2, s.y + s.h / 2, Math.abs(s.w / 2), Math.abs(s.h / 2), 0, 0, Math.PI * 2);
    } else if (s.t === 'arrow') {
      g.moveTo(s.x1, s.y1);
      g.lineTo(s.x2, s.y2);
      // Arrow head.
      var angle = Math.atan2(s.y2 - s.y1, s.x2 - s.x1);
      var head = Math.max(10, s.s * 4);
      g.moveTo(s.x2, s.y2);
      g.lineTo(s.x2 - head * Math.cos(angle - Math.PI / 6), s.y2 - head * Math.sin(angle - Math.PI / 6));
      g.moveTo(s.x2, s.y2);
      g.lineTo(s.x2 - head * Math.cos(angle + Math.PI / 6), s.y2 - head * Math.sin(angle + Math.PI / 6));
    }
    g.stroke();
  }

  function redraw() {
    if (!ctx) return;
    ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);
    for (var i = 0; i < shapes.length; i++) drawShape(ctx, shapes[i]);
    if (drawing) drawShape(ctx, drawing);
  }

  drawCanvas.addEventListener('pointerdown', function (e) {
    if (e.button !== 0 && e.pointerType === 'mouse') return;
    e.preventDefault();
    drawCanvas.setPointerCapture(e.pointerId);
    var x = e.clientX, y = e.clientY;
    if (tool === 'pen') drawing = { t: 'pen', c: color, s: 3, p: [[x, y]] };
    else if (tool === 'rect') drawing = { t: 'rect', c: color, s: 3, x: x, y: y, w: 0, h: 0 };
    else if (tool === 'ellipse') drawing = { t: 'ellipse', c: color, s: 3, x: x, y: y, w: 0, h: 0 };
    else if (tool === 'arrow') drawing = { t: 'arrow', c: color, s: 3, x1: x, y1: y, x2: x, y2: y };
    redraw();
  });

  drawCanvas.addEventListener('pointermove', function (e) {
    if (!drawing) return;
    e.preventDefault();
    var x = e.clientX, y = e.clientY;
    if (drawing.t === 'pen') {
      var pts = drawing.p;
      var last = pts[pts.length - 1];
      if (Math.abs(x - last[0]) + Math.abs(y - last[1]) >= 2 && pts.length < 2000) pts.push([x, y]);
    } else if (drawing.t === 'rect' || drawing.t === 'ellipse') {
      drawing.w = x - drawing.x;
      drawing.h = y - drawing.y;
    } else if (drawing.t === 'arrow') {
      drawing.x2 = x;
      drawing.y2 = y;
    }
    redraw();
  });

  function finishStroke() {
    if (!drawing) return;
    var keep = true;
    if (drawing.t === 'pen') keep = drawing.p.length > 1;
    else if (drawing.t === 'rect' || drawing.t === 'ellipse') {
      // Normalize negative width/height so stored shapes are canonical.
      if (drawing.w < 0) { drawing.x += drawing.w; drawing.w = -drawing.w; }
      if (drawing.h < 0) { drawing.y += drawing.h; drawing.h = -drawing.h; }
      keep = drawing.w > 2 || drawing.h > 2;
    } else if (drawing.t === 'arrow') {
      keep = Math.abs(drawing.x2 - drawing.x1) + Math.abs(drawing.y2 - drawing.y1) > 4;
    }
    if (keep && shapes.length < 200) shapes.push(drawing);
    drawing = null;
    redraw();
  }

  drawCanvas.addEventListener('pointerup', finishStroke);
  drawCanvas.addEventListener('pointercancel', function () { drawing = null; redraw(); });

  window.addEventListener('resize', function () {
    if (overlayOpen) sizeCanvas();
  });

  document.addEventListener('keydown', function (e) {
    if (overlayOpen && e.key === 'Escape') closeOverlay();
  });

  // --- open / close / submit ------------------------------------------------------
  function openOverlay() {
    if (overlayOpen) return;
    overlayOpen = true;
    screenshot = captureScreenshot();
    shapes = [];
    drawing = null;
    errorText.style.display = 'none';
    updateRoundNote();
    overlay.style.display = 'block';
    fbBtn.style.display = 'none';
    sizeCanvas();
  }

  function closeOverlay() {
    overlayOpen = false;
    overlay.style.display = 'none';
    fbBtn.style.display = '';
    drawing = null;
    screenshot = null;
  }

  function fail(message) {
    errorText.textContent = message;
    errorText.style.display = 'block';
    sending = false;
    sendBtn.textContent = 'Send';
    sendBtn.style.opacity = '';
  }

  function submit() {
    if (sending) return;
    var message = messageInput.value.trim();
    if (!message && shapes.length === 0) {
      fail('Write a note or draw something first.');
      return;
    }
    sending = true;
    sendBtn.textContent = 'Sending…';
    sendBtn.style.opacity = '.7';
    errorText.style.display = 'none';

    var payload = {
      sessionID: sessionId,
      message: message,
      viewport: { w: window.innerWidth, h: window.innerHeight }
    };
    var name = nameInput.value.trim();
    if (name) payload.name = name;
    if (shapes.length > 0) payload.drawing = { shapes: shapes };
    if (shapes.length > 0 && screenshot) payload.screenshot = screenshot;
    if (lastRound) {
      payload.mode = lastRound.mode;
      payload.eventId = lastRound.eventId;
    }

    var doFetch = origFetch || window.fetch;
    doFetch('/__share/feedback', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(payload)
    }).then(function (res) {
      if (res.ok) {
        sending = false;
        sendBtn.textContent = 'Send';
        sendBtn.style.opacity = '';
        messageInput.value = '';
        shapes = [];
        closeOverlay();
        toast('Thanks! Your feedback was sent.');
        return;
      }
      if (res.status === 429) {
        fail('Too many submissions — please try again later.');
        return;
      }
      res.json().then(function (body) {
        fail((body && body.error && body.error.message) || 'Could not send feedback.');
      }, function () {
        fail('Could not send feedback.');
      });
    }, function () {
      fail('Network error — could not send feedback.');
    });
  }

  // --- mount ---------------------------------------------------------------------
  function mount() {
    if (!document.body) return;
    document.body.appendChild(pill);
    document.body.appendChild(fbBtn);
    document.body.appendChild(overlay);
  }
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', mount);
  } else {
    mount();
  }
})();
