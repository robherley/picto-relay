"use strict";

const API = "/api/v1";
const WIDTH = 256;
const HEIGHT = 96;
const ROOM_NAMES = ["A", "B", "C", "D"];
const NATIVE_DRAWING_HEIGHT = 80;
const NAMETAG_BOTTOM_Y = 16;
const NAMETAG_SAFE_MIN_X = 88;
const FULL_SAFE_MIN_X = 28;
const VISIBLE_TEXT_MAX_X = 244;
const LARGE_TEXT = Object.freeze({ scale: 2, originY: 1, lineAdvance: 16, maxLines: 5 });
const COMPACT_TEXT = Object.freeze({ scale: 1, originY: 1, lineAdvance: 8, maxLines: 10 });

const GLYPHS = Object.freeze({
  A: [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
  B: [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
  C: [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111],
  D: [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
  E: [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
  F: [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
  G: [0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
  H: [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
  I: [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111],
  J: [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
  K: [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
  L: [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
  M: [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
  N: [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
  O: [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
  P: [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
  Q: [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
  R: [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
  S: [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
  T: [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
  U: [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
  V: [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
  W: [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010],
  X: [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
  Y: [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
  Z: [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
  0: [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
  1: [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
  2: [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
  3: [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
  4: [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
  5: [0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110],
  6: [0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
  7: [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
  8: [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
  9: [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
  " ": [0, 0, 0, 0, 0, 0, 0],
  ".": [0, 0, 0, 0, 0, 0b01100, 0b01100],
  ",": [0, 0, 0, 0, 0b01100, 0b01100, 0b01000],
  "!": [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100],
  "?": [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
  "-": [0, 0, 0, 0b11111, 0, 0, 0],
  _: [0, 0, 0, 0, 0, 0, 0b11111],
  ":": [0, 0b00100, 0b00100, 0, 0b00100, 0b00100, 0],
  "/": [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
  "@": [0b01110, 0b10001, 0b10111, 0b10101, 0b10111, 0b10000, 0b01111],
  "#": [0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010],
  "'": [0b00100, 0b00100, 0b00010, 0, 0, 0, 0],
  "(": [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
  ")": [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
  "+": [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
  "=": [0, 0, 0b11111, 0, 0b11111, 0, 0],
});
const FALLBACK_GLYPH = Object.freeze([
  0b11111, 0b10001, 0b10101, 0b10101, 0b10101, 0b10001, 0b11111,
]);

const elements = {
  gateway: document.querySelector("#gateway-status"),
  lobby: document.querySelector("#lobby"),
  rooms: document.querySelector("#rooms"),
  name: document.querySelector("#name"),
  chat: document.querySelector("#chat"),
  chatTitle: document.querySelector("#chat-title"),
  sessionStatus: document.querySelector("#session-status"),
  participants: document.querySelector("#participants"),
  messages: document.querySelector("#messages"),
  leave: document.querySelector("#leave"),
  drawing: document.querySelector("#drawing"),
  pen: document.querySelector("#pen"),
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
    const card = document.createElement("article");
    card.className = "room";

    const heading = document.createElement("h2");
    const title = document.createElement("span");
    title.textContent = `Room ${roomName(room)}`;
    const count = document.createElement("span");
    count.className = "room-count";
    count.textContent = `${participantCount} ${participantCount === 1 ? "client" : "clients"}`;
    heading.append(title, count);
    card.append(heading);

    const list = document.createElement("ul");
    if (participantCount === 0) {
      const empty = document.createElement("li");
      empty.className = "empty";
      empty.textContent = "No known clients";
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
      body: JSON.stringify({ name, room }),
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
      addDrawingMessage(event.name, event.bitmap);
      break;
    case "delivered":
      addSystemMessage(`Delivered message ${event.message_id}`);
      break;
    case "delivery_timed_out":
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

function paintBitmap(canvas, bytes) {
  const context = canvas.getContext("2d");
  const image = context.createImageData(WIDTH, HEIGHT);
  image.data.fill(255);
  for (let y = 0; y < HEIGHT; y += 1) {
    for (let x = 0; x < WIDTH; x += 1) {
      const byte = bytes[y * (WIDTH / 8) + Math.floor(x / 8)] || 0;
      if ((byte & (1 << (7 - (x % 8)))) !== 0) {
        const pixel = (y * WIDTH + x) * 4;
        image.data[pixel] = 0;
        image.data[pixel + 1] = 0;
        image.data[pixel + 2] = 0;
      }
    }
  }
  context.putImageData(image, 0, 0);
}

function clearDrawing() {
  drawingContext.fillStyle = "white";
  drawingContext.fillRect(0, 0, WIDTH, HEIGHT);
}

function selectTool(tool) {
  state.tool = tool;
  elements.pen.classList.toggle("selected", tool === "pen");
  elements.eraser.classList.toggle("selected", tool === "eraser");
}

function canvasPoint(event) {
  const bounds = elements.drawing.getBoundingClientRect();
  return {
    x: Math.max(0, Math.min(WIDTH - 1, (event.clientX - bounds.left) * WIDTH / bounds.width)),
    y: Math.max(0, Math.min(HEIGHT - 1, (event.clientY - bounds.top) * HEIGHT / bounds.height)),
  };
}

function drawTo(point) {
  drawingContext.strokeStyle = state.tool === "pen" ? "black" : "white";
  drawingContext.lineWidth = state.tool === "pen" ? 2 : 9;
  drawingContext.lineCap = "round";
  drawingContext.beginPath();
  drawingContext.moveTo(state.lastPoint.x, state.lastPoint.y);
  drawingContext.lineTo(point.x, point.y);
  drawingContext.stroke();
  state.lastPoint = point;
}

function packedDrawing() {
  const pixels = drawingContext.getImageData(0, 0, WIDTH, HEIGHT).data;
  const bitmap = new Uint8Array(WIDTH * HEIGHT / 8);
  for (let y = 0; y < HEIGHT; y += 1) {
    for (let x = 0; x < WIDTH; x += 1) {
      const pixel = (y * WIDTH + x) * 4;
      const darkness = (pixels[pixel] + pixels[pixel + 1] + pixels[pixel + 2]) / 3;
      if (darkness < 180) bitmap[y * (WIDTH / 8) + Math.floor(x / 8)] |= 1 << (7 - (x % 8));
    }
  }
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
      body: JSON.stringify({ bitmap: bitmapBase64(packedDrawing()) }),
    });
    setSessionStatus("Message queued");
  } catch (error) {
    addSystemMessage(`Send failed: ${error.message}`, true);
  }
}

function renderText(text) {
  const bitmap = renderTextWithLayout(text, LARGE_TEXT)
    || renderTextWithLayout(text, COMPACT_TEXT);
  if (bitmap === null) throw new Error("Text is too long for the PictoChat drawing area");
  paintBitmap(elements.drawing, bitmap);
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

  for (const character of text) {
    if (character === "\n" || x + glyphWidth > VISIBLE_TEXT_MAX_X) {
      line += 1;
      x = textLineOriginX(layout, line);
      if (character === "\n") {
        if (line >= layout.maxLines) return null;
        continue;
      }
    }
    if (line >= layout.maxLines) return null;

    const y = layout.originY + line * layout.lineAdvance;
    drawGlyph(bitmap, x, y, layout.scale, glyph(character));
    x += glyphAdvance;
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
