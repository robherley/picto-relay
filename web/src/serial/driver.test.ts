// @ts-expect-error The web client is typed for browsers; only this test reads a file.
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../api/client";
import { fromBase64 } from "../pictochat/drawing";
import { Driver, FirmwareMismatch } from "./driver";
import incoming from "./fixtures/incoming-drawing.json";
import { type Hello, Host, type Packet } from "./host";

// The driver against a stand-in for the firmware that speaks the real USB
// framing. The module is built by `npm run wasm`, which `npm test` runs first.
const wasm = new Uint8Array(readFileSync(new URL("./picto-relay.wasm", import.meta.url)) as ArrayBuffer);

const MAC = [0xac, 0x15, 0x18, 0xd6, 0x94, 0x34];
/** The station ID the firmware's address gives us: its last four octets. */
const CLIENT_ID = 0x18d69434;
const PROFILE = { name: "Rob", message: "hello", color: 11 };

/** As much of the firmware as the driver can tell apart from the real one. */
class Firmware {
  /** Commands taken, once each however often they were sent. */
  readonly commands: Packet[] = [];
  /** Packets to lose before hearing anything. */
  deaf = 0;
  version: number;
  counts = [2, 255, 0, 255];
  private readonly host: Host;
  private readonly send: (bytes: Uint8Array) => void;
  private status: ReturnType<typeof setInterval> | undefined;

  constructor(host: Host, send: (bytes: Uint8Array) => void) {
    this.host = host;
    this.send = send;
    this.version = host.constants.PROTOCOL_VERSION;
  }

  emit(kind: number, id = 0, payload: number[] | Uint8Array = []): void {
    this.send(this.host.encode(kind, id, Uint8Array.from(payload)));
  }

  /** Starts or stops the periodic status report of a joined firmware. */
  report(on: boolean): void {
    clearInterval(this.status);
    const status = new Array<number>(this.host.constants.STATUS_LEN).fill(0);
    if (on) this.status = setInterval(() => this.emit(this.host.constants.STATUS, 0, status), 500);
  }

  kinds(): number[] {
    return this.commands.map((command) => command.kind);
  }

  receive(bytes: Uint8Array): void {
    const c = this.host.constants;
    for (const command of this.host.decode(bytes)) {
      if (this.deaf > 0) {
        this.deaf -= 1;
        continue;
      }
      if (!this.commands.some((seen) => seen.id === command.id)) this.commands.push(command);
      const respond = (status: number, data: number[] = []) => this.emit(c.RESPONSE, command.id, [status, ...data]);
      switch (command.kind) {
        case c.HELLO: {
          const matches = command.payload[0] === this.version;
          respond(matches ? c.STATUS_OK : c.STATUS_VERSION_MISMATCH, [this.version, ...MAC, ...new TextEncoder().encode("abc1234")]);
          break;
        }
        case c.SCAN:
          respond(c.STATUS_OK);
          setTimeout(() => this.emit(c.ROOMS, 0, this.counts), 300);
          break;
        case c.JOIN:
          respond(c.STATUS_OK);
          this.report(true);
          break;
        case c.LEAVE:
          this.report(false);
          respond(c.STATUS_OK);
          break;
        default:
          respond(c.STATUS_OK);
      }
    }
  }
}

interface Rig {
  driver: Driver;
  firmware: Firmware;
  c: Host["constants"];
  events: SessionEvent[];
  rooms: (number | null)[][];
  hellos: Hello[];
  failures: Error[];
  /** The events of one type seen so far. */
  of<T extends SessionEvent["type"]>(type: T): Extract<SessionEvent, { type: T }>[];
  /** Joins room B and lets the room let us in. */
  enter(): Promise<void>;
}

async function rig(): Promise<Rig> {
  const host = await Host.load(wasm);
  const events: SessionEvent[] = [];
  const rooms: (number | null)[][] = [];
  const hellos: Hello[] = [];
  const failures: Error[] = [];
  const firmware: Firmware = new Firmware(await Host.load(wasm), (bytes) => driver.feed(bytes));
  const driver: Driver = new Driver(host, async (bytes) => firmware.receive(bytes), {
    attached: (hello) => hellos.push(hello),
    rooms: (counts) => rooms.push(counts),
    event: (event) => events.push(event),
    failed: (problem) => failures.push(problem),
  });
  const c = host.constants;
  return {
    driver, firmware, c, events, rooms, hellos, failures,
    of: (type) => events.filter((event) => event.type === type) as never,
    async enter() {
      driver.join(1, PROFILE);
      await vi.advanceTimersByTimeAsync(100);
      firmware.emit(c.READY, 0, [...incoming.host, 3, 1, 7]);
      await vi.advanceTimersByTimeAsync(10);
    },
  };
}

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("attaching and looking for rooms", () => {
  it("says hello, has the firmware leave, and reports what a scan finds", async () => {
    const { driver, firmware, c, rooms, hellos } = await rig();
    const hello = await driver.start();
    expect(hello).toEqual({ version: 3, build: "abc1234", station: "ac:15:18:d6:94:34", client_id: CLIENT_ID });
    expect(hellos).toEqual([hello]);
    await vi.advanceTimersByTimeAsync(400);
    expect(rooms).toEqual([[2, null, 0, null]]);
    expect(firmware.kinds()).toEqual([c.HELLO, c.LEAVE, c.HELLO, c.LEAVE, c.SCAN]);

    // And again every few seconds, for as long as nobody joins a room.
    firmware.counts = [3, 255, 0, 1];
    await vi.advanceTimersByTimeAsync(5_400);
    expect(rooms.at(-1)).toEqual([3, null, 0, 1]);
    driver.stop();
  });

  it("sends a command again when the ESP32 did not hear it", async () => {
    const { driver, firmware } = await rig();
    firmware.deaf = 2;
    const started = driver.start();
    await vi.advanceTimersByTimeAsync(400);
    expect((await started).client_id).toBe(CLIENT_ID);
    driver.stop();
  });

  it("gives up on an ESP32 that never answers", async () => {
    const { driver, firmware } = await rig();
    firmware.deaf = Infinity;
    const started = driver.start();
    started.catch(() => {});
    await vi.advanceTimersByTimeAsync(3_200);
    await expect(started).rejects.toThrow("did not answer");
  });

  it("refuses firmware that speaks another version of the protocol", async () => {
    const { driver, firmware } = await rig();
    firmware.version = 2;
    const failure = await driver.start().catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(FirmwareMismatch);
    expect((failure as Error).message).toContain("protocol 2");
    expect((failure as Error).message).toContain("abc1234");
  });

  it("reports no rooms when a scan comes to nothing", async () => {
    const { driver, firmware, rooms } = await rig();
    await driver.start();
    firmware.deaf = Infinity;
    await vi.advanceTimersByTimeAsync(3_500);
    expect(rooms).toEqual([[null, null, null, null]]);
    driver.stop();
  });
});

describe("in a room", () => {
  it("joins with the profile and is ready when the room lets it in", async () => {
    const { driver, firmware, c, events, of, enter } = await rig();
    await driver.start();
    await enter();

    const join = firmware.commands.find((command) => command.kind === c.JOIN)!;
    expect(join.payload).toHaveLength(74);
    expect(join.payload[0]).toBe(1);
    expect(new TextDecoder("utf-16le").decode(join.payload.subarray(1, 7))).toBe("Rob");
    expect(join.payload[73]).toBe(11);
    expect(of("backend_state").map(({ state }) => state)).toEqual(["connecting", "connecting", "ready"]);
    expect(events.at(-1)).toEqual({ type: "session_ready", client_id: CLIENT_ID });

    // The firmware is told every second that the page is still here.
    const pings = () => firmware.kinds().filter((kind) => kind === c.PING).length;
    await vi.advanceTimersByTimeAsync(3_000);
    expect(pings()).toBe(3);
    // No scanning while in a room.
    expect(firmware.kinds().filter((kind) => kind === c.SCAN)).toHaveLength(0);
    driver.stop();
  });

  it("reports a drawing someone else sent, once it has all arrived", async () => {
    const { driver, firmware, c, of, enter } = await rig();
    await driver.start();
    await enter();
    for (const body of incoming.bodies) firmware.emit(c.RX, 0, fromBase64(body));
    await vi.advanceTimersByTimeAsync(10);
    const [message, ...more] = of("message");
    expect(more).toEqual([]);
    expect(message).toMatchObject({ station_id: incoming.station_id, palette_bitmap: incoming.palette_bitmap });
    driver.stop();
  });

  it("uploads a drawing and reports it when the room has it", async () => {
    const { driver, firmware, c, of, enter } = await rig();
    await driver.start();
    await enter();
    const bitmap = fromBase64(incoming.palette_bitmap);
    driver.send(77, bitmap);
    await vi.advanceTimersByTimeAsync(200);

    const draws = firmware.commands.filter((command) => command.kind === c.DRAW);
    expect(draws.length).toBeGreaterThan(1);
    const view = (payload: Uint8Array) => new DataView(payload.buffer, payload.byteOffset);
    // Each: transfer ID, position, total length, then the bytes at that position.
    expect(draws.map(({ payload }) => view(payload).getUint32(0, true))).toEqual(draws.map(() => 77));
    const total = view(draws[0]!.payload).getUint16(6, true);
    expect(draws.reduce((sum, { payload }) => sum + payload.length - 8, 0)).toBe(total);
    expect(of("message")).toEqual([]);

    firmware.emit(c.DELIVERED, 77);
    await vi.advanceTimersByTimeAsync(10);
    expect(of("message")).toEqual([
      { type: "message", station_id: CLIENT_ID, name: "Rob", message_id: 77, palette_bitmap: incoming.palette_bitmap },
    ]);
    expect(of("delivered")).toEqual([{ type: "delivered", message_id: 77 }]);
    driver.stop();
  });

  it("refuses a blank drawing without ending the session", async () => {
    const { driver, of, enter } = await rig();
    await driver.start();
    await enter();
    driver.send(5, new Uint8Array(12288));
    await vi.advanceTimersByTimeAsync(200);
    expect(of("error").map(({ code }) => code)).toEqual(["invalid_drawing"]);
    expect(of("delivery_timed_out")).toEqual([{ type: "delivery_timed_out", message_id: 5 }]);
    expect(of("backend_state").at(-1)?.state).toBe("ready");
    driver.stop();
  });

  it("rejoins when the DS drops the session, and gives up on drawings in flight", async () => {
    const { driver, firmware, c, of, enter } = await rig();
    await driver.start();
    await enter();
    driver.send(8, fromBase64(incoming.palette_bitmap));
    await vi.advanceTimersByTimeAsync(200);

    firmware.emit(c.LEFT, 0, [c.LEFT_KICKED, 3, 0]);
    await vi.advanceTimersByTimeAsync(10);
    expect(of("delivery_timed_out")).toEqual([{ type: "delivery_timed_out", message_id: 8 }]);
    const state = of("backend_state").at(-1)!;
    expect(state.state).toBe("reconnecting");
    expect(state.detail).toContain("reason 3");

    const joins = () => firmware.kinds().filter((kind) => kind === c.JOIN).length;
    expect(joins()).toBe(1);
    await vi.advanceTimersByTimeAsync(1_200);
    expect(joins()).toBe(2);
    driver.stop();
  });

  it("rejoins when the firmware goes quiet", async () => {
    const { driver, firmware, of, enter } = await rig();
    await driver.start();
    await enter();
    firmware.report(false);
    await vi.advanceTimersByTimeAsync(5_200);
    expect(of("backend_state").at(-1)?.state).toBe("reconnecting");
    driver.stop();
  });

  it("empties the member list when the room cannot be joined at all", async () => {
    const { driver, firmware, c, of } = await rig();
    await driver.start();
    driver.join(0, PROFILE);
    await vi.advanceTimersByTimeAsync(100);
    firmware.emit(c.LEFT, 0, [1, 0, 0]);
    await vi.advanceTimersByTimeAsync(10);
    expect(of("backend_state").at(-1)?.detail).toContain("open the room on a DS first");
    expect(of("participants")).toEqual([{ type: "participants", room: 0, participants: [] }]);
    driver.stop();
  });

  it("leaves when asked, and goes back to looking for rooms", async () => {
    const { driver, firmware, c, rooms, enter } = await rig();
    await driver.start();
    await enter();
    const before = firmware.kinds().length;
    await driver.leave();
    expect(firmware.kinds().slice(before)).toContain(c.LEAVE);
    await vi.advanceTimersByTimeAsync(500);
    expect(rooms).toEqual([[2, null, 0, null]]);
    // The session is over: nothing more is said to it.
    const pings = firmware.kinds().filter((kind) => kind === c.PING).length;
    await vi.advanceTimersByTimeAsync(3_000);
    expect(firmware.kinds().filter((kind) => kind === c.PING)).toHaveLength(pings);
    driver.stop();
  });

  it("does not join with a profile the firmware would refuse", async () => {
    const { driver, firmware, c, of } = await rig();
    await driver.start();
    driver.join(0, { ...PROFILE, message: "m".repeat(27) });
    await vi.advanceTimersByTimeAsync(200);
    expect(firmware.kinds()).not.toContain(c.JOIN);
    expect(of("backend_state").at(-1)?.state).toBe("left");
    driver.stop();
  });
});
