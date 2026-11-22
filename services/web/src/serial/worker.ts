import { Driver, FirmwareMismatch } from "./driver";
import { Host } from "./host";
import type { DeviceState, FromWorker, ToWorker } from "./messages";
import wasmUrl from "./picto-relay.wasm?url";

// Owns the serial port and drives the ESP32 from a worker. The firmware gives
// up on a host it has not heard from for five seconds, and browsers slow a
// page's timers once its tab is hidden; a worker's keep running.

/** As much of a worker's own scope as this file uses. */
interface Scope {
  navigator: Navigator;
  postMessage(message: FromWorker): void;
  onmessage: ((event: MessageEvent<ToWorker>) => void) | null;
}
const scope = self as unknown as Scope;

/** Bytes buffered between the port and us: several of the firmware's largest packets. */
const BUFFER_BYTES = 8192;
/** Tries at saying hello after opening the port. */
const HELLO_TRIES = 3;
/** How long the reset line is held to restart the board, as esptool holds it. */
const RESET_MS = 100;

let host: Host | undefined;
/** The port in use, and what undoes each thing done to it. */
let open: { port: SerialPort; driver: Driver; release: () => Promise<void> } | undefined;
/** Opening and closing happen one at a time. */
let turn: Promise<unknown> = Promise.resolve();

const say = (state: DeviceState) => scope.postMessage({ type: "state", state });
const text = (error: unknown) => (error instanceof Error ? error.message : String(error));

async function closePort(): Promise<void> {
  const was = open;
  open = undefined;
  if (!was) return;
  was.driver.stop();
  await was.release().catch(() => {});
}

/**
 * Restarts the board into its firmware, the way esptool does: the adapter's
 * RTS line pulls the chip's reset, and DTR, which would choose the bootloader,
 * is left alone. A board can be sitting in its bootloader, after flashing or
 * after a program that opened the port carelessly; there it answers nothing.
 */
async function restart(port: SerialPort): Promise<void> {
  try {
    await port.setSignals({ dataTerminalReady: false, requestToSend: true });
    await new Promise((resolve) => setTimeout(resolve, RESET_MS));
    await port.setSignals({ dataTerminalReady: false, requestToSend: false });
  } catch {
    // An adapter without these lines: there is nothing to pull.
  }
}

/** Opens the first of the ports the user has allowed that will open, preferring the one described. */
async function firstPort(wanted?: SerialPortInfo): Promise<SerialPort | undefined> {
  const ports = (await scope.navigator.serial?.getPorts()) ?? [];
  const matches = (port: SerialPort) => {
    const info = port.getInfo();
    return info.usbVendorId === wanted?.usbVendorId && info.usbProductId === wanted?.usbProductId;
  };
  const ordered = wanted ? [...ports.filter(matches), ...ports.filter((port) => !matches(port))] : ports;
  for (const port of ordered) {
    try {
      await port.open({ baudRate: host!.constants.BAUD, bufferSize: BUFFER_BYTES });
      return port;
    } catch {
      // In use, or gone: try the next.
    }
  }
  return undefined;
}

async function openPort(wanted: SerialPortInfo | undefined, asked: boolean): Promise<void> {
  await closePort();
  say({ phase: "connecting" });
  host ??= await Host.load(await (await fetch(wasmUrl)).arrayBuffer());
  const port = await firstPort(wanted);
  if (!port?.readable || !port.writable) {
    // On loading the page there may simply be nothing plugged in yet.
    say({
      phase: "disconnected",
      problem: asked ? "That port could not be opened. Is another program using it?" : undefined,
    });
    return;
  }

  const writer = port.writable.getWriter();
  const reader = port.readable.getReader();
  const driver = new Driver(host, (bytes) => writer.write(bytes), {
    attached: () => {},
    rooms: (counts) => scope.postMessage({ type: "rooms", counts }),
    event: (event) => scope.postMessage({ type: "event", event }),
    failed: (problem) => {
      say({ phase: "outdated", detail: problem.message });
      void closePort();
    },
  });
  const mine = {
    port,
    driver,
    release: async () => {
      await reader.cancel().catch(() => {});
      reader.releaseLock();
      writer.releaseLock();
      await port.close();
    },
  };
  open = mine;

  // Reads until the port closes. If it closes under us, the ESP32 was unplugged.
  void (async () => {
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        driver.feed(value);
      }
    } catch {
      // Reported below.
    }
    if (open !== mine) return;
    await closePort();
    say({ phase: "disconnected", problem: "The ESP32 was unplugged." });
  })();

  for (let attempt = 1; open === mine; attempt++) {
    try {
      const hello = await driver.start();
      say({ phase: "connected", build: hello.build });
      return;
    } catch (error) {
      if (open !== mine) return;
      if (error instanceof FirmwareMismatch) {
        await closePort();
        say({ phase: "outdated", detail: text(error) });
        return;
      }
      if (attempt === HELLO_TRIES) {
        await closePort();
        say({ phase: "silent" });
        return;
      }
      // No answer: it may be in its bootloader. Restart it and ask again.
      await restart(port);
    }
  }
}

scope.onmessage = ({ data }) => {
  switch (data.type) {
    case "open": {
      const asked = data.port !== undefined;
      turn = turn
        .then(() => openPort(data.port, asked))
        .catch((error: unknown) => say({ phase: "disconnected", problem: text(error) }));
      break;
    }
    case "close":
      turn = turn.then(closePort).then(() => {
        say({ phase: "disconnected" });
        scope.postMessage({ type: "closed" });
      });
      break;
    case "join":
      open?.driver.join(data.room, data.profile);
      break;
    case "leave":
      void open?.driver.leave();
      break;
    case "send":
      open?.driver.send(data.id, data.bitmap);
      break;
  }
};
