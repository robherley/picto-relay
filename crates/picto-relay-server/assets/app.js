"use strict";

// Provisional DSi palette from capture-based reverse engineering:
// https://github.com/PeterWarrington/pictochat-interceptor/blob/main/pictochat_decode.py
// Index 2 is preserved on the wire; its display meaning is unknown.
const INK_PALETTE = [[255,255,255],[0,0,0],[0,0,0],[255,82,164],[255,51,102],
  [244,42,55],[255,91,40],[255,145,34],[255,195,35],[255,224,45],[174,224,55],
  [65,183,76],[22,151,166],[62,205,224],[49,139,220],[46,75,190]];

const API = "/api/v1";
const WIDTH = 256;
const HEIGHT = 96;
const ROOM_NAMES = ["A", "B", "C", "D"];
const NATIVE_DRAWING_HEIGHT = 80;
// Conservative pixel-aligned bounds from the user's filled DS drawing sample.
// Keep the 256×96 transport surface; only this inset is drawable on the device.
const DRAWING_LEFT = 24;
const DRAWING_RIGHT = 252;
const DRAWING_TOP_LEFT = 80;
const DRAWING_NOTCH_HEIGHT = 16;

function isDrawablePixel(x, y) {
  return y >= 0 && y < NATIVE_DRAWING_HEIGHT
    && x >= (y < DRAWING_NOTCH_HEIGHT ? DRAWING_TOP_LEFT : DRAWING_LEFT)
    && x < DRAWING_RIGHT;
}

function paintDrawingGuide() {
  drawingContext.fillStyle = "#e4e9e5";
  drawingContext.fillRect(0, 0, DRAWING_TOP_LEFT, DRAWING_NOTCH_HEIGHT);
  drawingContext.fillRect(0, DRAWING_NOTCH_HEIGHT, DRAWING_LEFT, NATIVE_DRAWING_HEIGHT - DRAWING_NOTCH_HEIGHT);
  drawingContext.fillRect(DRAWING_RIGHT, 0, WIDTH - DRAWING_RIGHT, NATIVE_DRAWING_HEIGHT);
  drawingContext.fillRect(0, NATIVE_DRAWING_HEIGHT, WIDTH, HEIGHT - NATIVE_DRAWING_HEIGHT);
}
const NAMETAG_BOTTOM_Y = 16;
const NAMETAG_SAFE_MIN_X = 88;
const FULL_SAFE_MIN_X = 28;
const VISIBLE_TEXT_MAX_X = 244;
const LARGE_TEXT = Object.freeze({ scale: 2, originY: 1, lineAdvance: 16, maxLines: 5 });
const COMPACT_TEXT = Object.freeze({ scale: 1, originY: 1, lineAdvance: 8, maxLines: 10 });

const FALLBACK_GLYPH = Object.freeze([
  0b11111, 0b10001, 0b10101, 0b10101, 0b10101, 0b10001, 0b11111,
]);

const elements = {
  gateway: document.querySelector("#gateway-status"),
  lobby: document.querySelector("#lobby"),
  rooms: document.querySelector("#rooms"),
  name: document.querySelector("#name"),
  profileMessage: document.querySelector("#profile-message"),
  profileColor: document.querySelector("#profile-color"),
  chat: document.querySelector("#chat"),
  chatTitle: document.querySelector("#chat-title"),
  sessionStatus: document.querySelector("#session-status"),
  participants: document.querySelector("#participants"),
  messages: document.querySelector("#messages"),
  leave: document.querySelector("#leave"),
  drawing: document.querySelector("#drawing"),
  pen: document.querySelector("#pen"),
  inkSwatches: document.querySelectorAll("[data-ink]"),
  eraser: document.querySelector("#eraser"),
  clear: document.querySelector("#clear"),
  sendDrawing: document.querySelector("#send-drawing"),
  textForm: document.querySelector("#text-form"),
  text: document.querySelector("#text"),
};

const state = {
  sessionId: null,
  room: null,
  tool: "pen",
  penInk: 1,
  inkPixels: new Uint8Array(WIDTH * HEIGHT),
  rainbowStep: 0,
  drawing: false,
  lastPoint: null,
  rooms: [],
  roomSocket: null,
  roomReconnectTimer: null,
  eventSocket: null,
  eventReconnectTimer: null,
};

const drawingContext = elements.drawing.getContext("2d", { willReadFrequently: true });

async function api(path, options = {}) {
  const response = await fetch(`${API}${path}`, {
    ...options,
    headers: options.body ? { "content-type": "application/json" } : undefined,
  });
  if (response.ok) {
    return response.status === 204 ? null : response.json();
  }
  let message = `${response.status} ${response.statusText}`;
  try {
    const error = await response.json();
    message = error.message || message;
  } catch (_) {
    // The HTTP status remains useful when a proxy returns a non-JSON error page.
  }
  throw new Error(message);
}

function roomName(room) {
  return ROOM_NAMES[room] || "?";
}

function setSessionStatus(message) {
  elements.sessionStatus.textContent = message;
}

function renderRooms() {
  elements.rooms.replaceChildren();
  for (let room = 0; room < ROOM_NAMES.length; room += 1) {
    const summary = state.rooms.find((candidate) => candidate.room === room);
    const participants = summary?.participants || [];
    const participantCount = summary?.participant_count ?? participants.length;
    const observed = summary?.observed !== false;
    const card = document.createElement("article");
    card.className = "room";

    const heading = document.createElement("h2");
    const title = document.createElement("span");
    title.textContent = `Room ${roomName(room)}`;
    const count = document.createElement("span");
    count.className = "room-count";
    count.textContent = observed ? `${participantCount} ${participantCount === 1 ? "client" : "clients"}` : "Not observed";
    heading.append(title, count);
    card.append(heading);

    const list = document.createElement("ul");
    if (participantCount === 0) {
      const empty = document.createElement("li");
      empty.className = "empty";
      empty.textContent = observed ? "No known clients" : "No recent room beacon found";
      list.append(empty);
    } else {
      for (const participant of participants) {
        const item = document.createElement("li");
        item.textContent = participant.name;
        list.append(item);
      }
      const unnamedCount = participantCount - participants.length;
      if (unnamedCount > 0) {
        const unnamed = document.createElement("li");
        unnamed.className = "empty";
        unnamed.textContent = `${unnamedCount} physical ${unnamedCount === 1 ? "client" : "clients"} detected`;
        list.append(unnamed);
      }
    }
    card.append(list);

    const join = document.createElement("button");
    join.type = "button";
    join.textContent = state.room === room ? "Joined" : "Join";
    join.disabled = state.sessionId !== null;
    join.addEventListener("click", () => joinRoom(room));
    card.append(join);
    elements.rooms.append(card);
  }
}

function websocketUrl(path) {
  const url = new URL(path, window.location.href);
  url.protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  return url;
}

function connectRoomSocket() {
  if (state.roomSocket?.readyState === WebSocket.OPEN
      || state.roomSocket?.readyState === WebSocket.CONNECTING) {
    return;
  }
  window.clearTimeout(state.roomReconnectTimer);
  const socket = new WebSocket(websocketUrl(`${API}/rooms/ws`));
  state.roomSocket = socket;
  socket.addEventListener("message", (message) => {
    try {
      const response = JSON.parse(message.data);
      state.rooms = response.rooms;
      renderRooms();
    } catch (error) {
      elements.gateway.textContent = `Invalid room update · ${error.message}`;
    }
  });
  socket.addEventListener("close", () => {
    if (state.roomSocket !== socket) return;
    state.roomSocket = null;
    state.roomReconnectTimer = window.setTimeout(connectRoomSocket, 1000);
  });
}

async function refreshStatus() {
  try {
    const status = await api("/status");
    elements.gateway.textContent = `${status.backend} · ${status.bitmap_width}×${status.bitmap_height}`;
    document.getElementById("room-hint").textContent = status.backend === "esp32"
      ? "Nearby counts refresh across Wi-Fi channels every few seconds. Join to learn names; discovery pauses during a chat."
      : status.backend === "memory"
        ? "Counts show connected web clients."
        : "Counts include nearby DS clients. Names appear after the relay joins.";
  } catch (error) {
    elements.gateway.textContent = `Offline · ${error.message}`;
  }
}

async function joinRoom(room) {
  const name = elements.name.value.trim();
  if (!name) {
    elements.name.focus();
    return;
  }
  try {
    const response = await api("/sessions", {
      method: "POST",
      body: JSON.stringify({ name, room, profile_message: elements.profileMessage.value, profile_color: Number(elements.profileColor.value) }),
    });
    state.sessionId = response.session_id;
    state.room = response.room;
    elements.chatTitle.textContent = `Room ${roomName(state.room)}`;
    elements.lobby.hidden = true;
    elements.chat.hidden = false;
    elements.messages.replaceChildren();
    setSessionStatus("Joining…");
    clearDrawing();
    renderRooms();
    connectEventSocket(state.sessionId);
  } catch (error) {
    window.alert(`Could not join: ${error.message}`);
  }
}

async function leaveRoom() {
  const sessionId = state.sessionId;
  state.sessionId = null;
  state.room = null;
  closeEventSocket();
  if (sessionId !== null) {
    try {
      await api(`/sessions/${sessionId}`, { method: "DELETE" });
    } catch (error) {
      addSystemMessage(`Leave failed: ${error.message}`, true);
    }
  }
  elements.chat.hidden = true;
  elements.lobby.hidden = false;
  elements.participants.replaceChildren();
  renderRooms();
}

function connectEventSocket(sessionId) {
  if (state.sessionId !== sessionId) return;
  window.clearTimeout(state.eventReconnectTimer);
  const socket = new WebSocket(websocketUrl(`${API}/sessions/${sessionId}/events/ws`));
  state.eventSocket = socket;
  socket.addEventListener("message", (message) => {
    if (state.sessionId !== sessionId) return;
    try {
      handleEvent(JSON.parse(message.data));
    } catch (error) {
      addSystemMessage(`Invalid event: ${error.message}`, true);
    }
  });
  socket.addEventListener("close", () => {
    if (state.eventSocket !== socket || state.sessionId !== sessionId) return;
    state.eventSocket = null;
    setSessionStatus("Event stream reconnecting…");
    state.eventReconnectTimer = window.setTimeout(() => connectEventSocket(sessionId), 1000);
  });
}

function closeEventSocket() {
  window.clearTimeout(state.eventReconnectTimer);
  state.eventReconnectTimer = null;
  if (state.eventSocket !== null) {
    const socket = state.eventSocket;
    state.eventSocket = null;
    socket.close(1000, "session closed");
  }
}

function handleEvent(event) {
  switch (event.type) {
    case "profile":
      // Profile details are available to API consumers; roster events drive this list.
      break;
    case "backend_state":
      setSessionStatus(`${event.state} · ${event.detail}`);
      break;
    case "session_ready":
      setSessionStatus(`Connected as client ${event.client_id}`);
      break;
    case "participants":
      if (event.room === state.room) renderParticipants(event.participants);
      break;
    case "participant_joined":
      if (event.room === state.room) addPresenceMessage(event.participant, true);
      break;
    case "participant_left":
      if (event.room === state.room) addPresenceMessage(event.participant, false);
      break;
    case "message":
      addDrawingMessage(event.name, event.palette_bitmap || event.bitmap);
      break;
    case "delivered":
      setSessionStatus(`Delivered message ${event.message_id}`);
      addSystemMessage(`Delivered message ${event.message_id}`);
      break;
    case "delivery_timed_out":
      setSessionStatus(`Delivery timed out for message ${event.message_id}`);
      addSystemMessage(`Delivery timed out for message ${event.message_id}`, true);
      break;
    case "error":
      addSystemMessage(`${event.code}: ${event.message}`, true);
      break;
    default:
      addSystemMessage(`Unknown event: ${event.type}`);
  }
}

function renderParticipants(participants) {
  elements.participants.replaceChildren();
  for (const participant of participants) {
    const item = document.createElement("li");
    item.textContent = participant.name;
    elements.participants.append(item);
  }
}

function addSystemMessage(message, error = false) {
  const item = document.createElement("div");
  item.className = `message system${error ? " error" : ""}`;
  item.textContent = message;
  elements.messages.append(item);
  elements.messages.scrollTop = elements.messages.scrollHeight;
}

function addPresenceMessage(participant, joined) {
  const item = document.createElement("div");
  item.className = `message system presence ${joined ? "joined" : "left"}`;
  item.textContent = `${participant.name} ${joined ? "joined" : "left"}.`;
  elements.messages.append(item);
  elements.messages.scrollTop = elements.messages.scrollHeight;
}

function addDrawingMessage(name, encodedBitmap) {
  const item = document.createElement("article");
  item.className = "message";
  const heading = document.createElement("header");
  heading.textContent = name;
  const canvas = document.createElement("canvas");
  canvas.width = WIDTH;
  canvas.height = HEIGHT;
  item.append(heading, canvas);
  elements.messages.append(item);

  try {
    const binary = atob(encodedBitmap);
    const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
    paintBitmap(canvas, bytes);
  } catch (error) {
    item.append(document.createTextNode(`Invalid bitmap: ${error.message}`));
  }
  elements.messages.scrollTop = elements.messages.scrollHeight;
}

function unpackBitmap(bytes) {
  const color = bytes.length === WIDTH * HEIGHT / 2;
  if (!color && bytes.length !== WIDTH * HEIGHT / 8) throw new Error("Invalid bitmap length");
  const pixels = new Uint8Array(WIDTH * HEIGHT);
  for (let i = 0; i < pixels.length; i++) {
    pixels[i] = color ? (bytes[i >> 1] >> ((i & 1) * 4)) & 15
      : (bytes[i >> 3] >> (7 - (i & 7))) & 1;
  }
  return pixels;
}

function paintBitmap(canvas, bytes) {
  const pixels = unpackBitmap(bytes);
  if (canvas === elements.drawing) state.inkPixels.set(pixels);
  const context = canvas.getContext("2d");
  const image = context.createImageData(WIDTH, HEIGHT);
  for (let i = 0; i < pixels.length; i++) {
    image.data.set(INK_PALETTE[pixels[i]], i * 4);
    image.data[i * 4 + 3] = 255;
  }
  context.putImageData(image, 0, 0);
  if (canvas === elements.drawing) paintDrawingGuide();
}

function clearDrawing() {
  state.inkPixels.fill(0);
  state.rainbowStep = 0;
  drawingContext.fillStyle = "white";
  drawingContext.fillRect(0, 0, WIDTH, HEIGHT);
  paintDrawingGuide();
}

function selectInk(ink) {
  if (ink !== "rainbow" && (!Number.isInteger(ink) || ink < 1 || ink > 15 || ink === 2)) return;
  state.penInk = ink;
  selectTool("pen");
}

function selectTool(tool) {
  state.tool = tool;
  for (const swatch of elements.inkSwatches) {
    const selected = tool === "pen" && swatch.dataset.ink === String(state.penInk);
    swatch.classList.toggle("selected", selected);
    swatch.setAttribute("aria-pressed", String(selected));
  }
  elements.pen.classList.toggle("selected", tool === "pen");
  elements.eraser.classList.toggle("selected", tool === "eraser");
  for (const name of ["pen", "eraser"]) elements[name].setAttribute("aria-pressed", String(tool === name));
}

function canvasPoint(event) {
  const bounds = elements.drawing.getBoundingClientRect();
  return {
    x: Math.max(0, Math.min(WIDTH - 1, (event.clientX - bounds.left) * WIDTH / bounds.width)),
    y: Math.max(0, Math.min(HEIGHT - 1, (event.clientY - bounds.top) * HEIGHT / bounds.height)),
  };
}

function drawTo(point) {
  const dx = point.x - state.lastPoint.x, dy = point.y - state.lastPoint.y;
  const steps = Math.max(1, Math.ceil(Math.hypot(dx, dy)));
  for (let step = 1; step <= steps; step++) {
    const x = Math.round(state.lastPoint.x + dx * step / steps);
    const y = Math.round(state.lastPoint.y + dy * step / steps);
    const ink = state.tool === "eraser" ? 0 : state.penInk === "rainbow"
      ? 3 + Math.floor(state.rainbowStep++ / 6) % 13 : state.penInk;
    const radius = state.tool === "eraser" ? 4 : 1;
    drawingContext.fillStyle = `rgb(${INK_PALETTE[ink].join(",")})`;
    for (let oy = -radius; oy <= radius; oy++) {
      for (let ox = -radius; ox <= radius; ox++) {
        const px = x + ox, py = y + oy;
        if (ox * ox + oy * oy > radius * radius || !isDrawablePixel(px, py)) continue;
        state.inkPixels[py * WIDTH + px] = ink;
        drawingContext.fillRect(px, py, 1, 1);
      }
    }
  }
  state.lastPoint = point;
}

function packedDrawing() {
  const bitmap = new Uint8Array(WIDTH * HEIGHT / 2);
  for (let i = 0; i < state.inkPixels.length; i++) bitmap[i >> 1] |= state.inkPixels[i] << ((i & 1) * 4);
  return bitmap;
}

function bitmapBase64(bitmap) {
  let binary = "";
  for (const byte of bitmap) binary += String.fromCharCode(byte);
  return btoa(binary);
}

async function sendCurrentDrawing() {
  if (state.sessionId === null) return;
  try {
    await api(`/sessions/${state.sessionId}/messages`, {
      method: "POST",
      body: JSON.stringify({ palette_bitmap: bitmapBase64(packedDrawing()) }),
    });
    clearDrawing();
    setSessionStatus("Message queued");
  } catch (error) {
    addSystemMessage(`Send failed: ${error.message}`, true);
  }
}

function renderText(text) {
  const bitmap = renderTextWithLayout(text, LARGE_TEXT)
    || renderTextWithLayout(text, COMPACT_TEXT);
  if (bitmap === null) throw new Error("Text is too long for the PictoChat drawing area");
  const pixels = unpackBitmap(bitmap);
  for (let i = 0; i < pixels.length; i++) {
    if (pixels[i]) pixels[i] = state.penInk === "rainbow"
      ? 3 + Math.floor((i % WIDTH) / 6) % 13 : state.penInk;
  }
  state.inkPixels.set(pixels);
  paintBitmap(elements.drawing, packedDrawing());
}

function renderTextWithLayout(text, layout) {
  const lastGlyphRow = layout.originY
    + (layout.maxLines - 1) * layout.lineAdvance
    + 7 * layout.scale;
  if (lastGlyphRow > NATIVE_DRAWING_HEIGHT) return null;

  const bitmap = new Uint8Array(WIDTH * HEIGHT / 8);
  const glyphWidth = 5 * layout.scale;
  const glyphAdvance = 6 * layout.scale;
  let line = 0;
  let x = textLineOriginX(layout, line);

  let pendingSpaces = 0;
  const nextLine = () => {
    line += 1;
    x = textLineOriginX(layout, line);
    pendingSpaces = 0;
    return line < layout.maxLines;
  };

  for (const token of text.replace(/\r\n?/g, "\n").match(/\n|[^\S\n]+|[^\s]+/gu) || []) {
    if (token === "\n") {
      if (!nextLine()) return null;
      continue;
    }
    if (/^\s+$/u.test(token)) {
      pendingSpaces += Array.from(token).length;
      continue;
    }

    const characters = Array.from(token);
    const wordWidth = (characters.length - 1) * glyphAdvance + glyphWidth;
    const gap = x > textLineOriginX(layout, line) ? pendingSpaces * glyphAdvance : 0;
    // Move the whole word first. A word wider than an empty line is split below.
    if (x > textLineOriginX(layout, line) && x + gap + wordWidth > VISIBLE_TEXT_MAX_X) {
      if (!nextLine()) return null;
    } else {
      x += gap;
    }
    pendingSpaces = 0;

    for (const character of characters) {
      if (x + glyphWidth > VISIBLE_TEXT_MAX_X && !nextLine()) return null;
      const y = layout.originY + line * layout.lineAdvance;
      drawGlyph(bitmap, x, y, layout.scale, glyph(character));
      x += glyphAdvance;
    }
  }

  return bitmap;
}

function textLineOriginX(layout, line) {
  const y = layout.originY + line * layout.lineAdvance;
  return y <= NAMETAG_BOTTOM_Y ? NAMETAG_SAFE_MIN_X : FULL_SAFE_MIN_X;
}

function glyph(character) {
  const codePoint = character.codePointAt(0);
  const asciiUppercase = codePoint >= 0x61 && codePoint <= 0x7a
    ? String.fromCodePoint(codePoint - 0x20)
    : character;
  return GLYPHS[asciiUppercase] || FALLBACK_GLYPH;
}

function drawGlyph(bitmap, originX, originY, scale, rows) {
  for (let row = 0; row < rows.length; row += 1) {
    for (let column = 0; column < 5; column += 1) {
      if ((rows[row] & (1 << (4 - column))) === 0) continue;
      for (let offsetY = 0; offsetY < scale; offsetY += 1) {
        for (let offsetX = 0; offsetX < scale; offsetX += 1) {
          setBitmapPixel(
            bitmap,
            originX + column * scale + offsetX,
            originY + row * scale + offsetY,
          );
        }
      }
    }
  }
}

function setBitmapPixel(bitmap, x, y) {
  const byte = y * (WIDTH / 8) + Math.floor(x / 8);
  bitmap[byte] |= 1 << (7 - (x % 8));
}

for (const swatch of elements.inkSwatches) {
  const ink = swatch.dataset.ink === "rainbow" ? "rainbow" : Number(swatch.dataset.ink);
  const fill = ink === "rainbow"
    ? `linear-gradient(135deg, ${INK_PALETTE.slice(3).map(rgb => `rgb(${rgb.join(",")})`).join(",")})`
    : `rgb(${INK_PALETTE[ink].join(",")})`;
  swatch.style.setProperty("--ink-color", fill);
  swatch.addEventListener("click", () => selectInk(ink));
}
selectTool("pen");

elements.leave.addEventListener("click", leaveRoom);
elements.pen.addEventListener("click", () => selectTool("pen"));
elements.eraser.addEventListener("click", () => selectTool("eraser"));
elements.clear.addEventListener("click", clearDrawing);
elements.sendDrawing.addEventListener("click", sendCurrentDrawing);
elements.textForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const text = elements.text.value.trim();
  if (!text) return;
  try {
    renderText(text);
    await sendCurrentDrawing();
    elements.text.value = "";
  } catch (error) {
    addSystemMessage(error.message, true);
  }
});

elements.drawing.addEventListener("pointerdown", (event) => {
  state.drawing = true;
  state.lastPoint = canvasPoint(event);
  elements.drawing.setPointerCapture(event.pointerId);
  drawTo(state.lastPoint);
});
elements.drawing.addEventListener("pointermove", (event) => {
  if (state.drawing) drawTo(canvasPoint(event));
});
elements.drawing.addEventListener("pointerup", () => { state.drawing = false; });
elements.drawing.addEventListener("pointercancel", () => { state.drawing = false; });

window.addEventListener("beforeunload", () => {
  window.clearTimeout(state.roomReconnectTimer);
  window.clearTimeout(state.eventReconnectTimer);
  state.roomSocket?.close();
  state.eventSocket?.close();
  if (state.sessionId !== null) {
    fetch(`${API}/sessions/${state.sessionId}`, { method: "DELETE", keepalive: true });
  }
});

clearDrawing();
refreshStatus();
connectRoomSocket();
