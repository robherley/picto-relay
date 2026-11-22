import type { SessionEvent } from "../api/client";
import type { Profile } from "../hooks/useProfile";
import { ROOM_LETTERS } from "../pictochat/constants";
import { toBase64 } from "../pictochat/drawing";
import type { Hello, Host, Member, Packet } from "./host";

// Drives the ESP32 over a serial line, as the relay server's worker does
// (crates/picto-relay-server/src/esp32.rs): it attaches, scans for rooms while
// idle, joins the room it is asked to, keeps the session alive, uploads
// drawings and rejoins when the session drops. What goes in each command and
// what each packet means comes from `Host`; reading and writing the line is
// left to whoever owns it.
//
// The timings are the server's.

/** How long a command may go unanswered, or stay busy, before it has failed. */
const REQUEST_DEADLINE_MS = 3_000;
/** How often an unanswered or busy command is resent. */
const REQUEST_RETRY_MS = 150;
/** How often the idle firmware is asked to sweep the channels for rooms. */
const SCAN_INTERVAL_MS = 5_000;
/** Longest a full channel sweep may take. */
const SCAN_TIMEOUT_MS = 6_000;
/** Pause before rejoining after a session failed. */
const RECONNECT_DELAY_MS = 1_000;
/** How often the firmware is told the host is still there. */
const PING_INTERVAL_MS = 1_000;
/** Longest finding the room and exchanging profiles may take before rejoining. */
const JOIN_TIMEOUT_MS = 30_000;
/** Longest gap between firmware status reports before rejoining. */
const STATUS_TIMEOUT_MS = 5_000;
/** Longest a drawing may take from upload to delivery. */
const DELIVERY_TIMEOUT_MS = 40_000;
/** Drawings the firmware holds at once: one being sent and one waiting. */
const MAX_UPLOADED_DRAWINGS = 2;
/** Drawings that may wait here behind those. */
const MAX_QUEUED_DRAWINGS = 4;
/** How often a session checks its deadlines and its queue. */
const WATCH_INTERVAL_MS = 100;

/** The firmware speaks another version of the USB protocol and must be flashed again. */
export class FirmwareMismatch extends Error {
  readonly hello: Hello;

  constructor(hello: Hello, wanted: number) {
    super(
      `The ESP32's firmware (build ${hello.build || "unknown"}) speaks protocol ${hello.version ?? "?"}, ` +
        `and this page speaks ${wanted}. Flash the firmware again.`,
    );
    this.hello = hello;
  }
}

/** Where the driver reports to. */
export interface Sink {
  /** The firmware answered HELLO. */
  attached(hello: Hello): void;
  /** Members seen in rooms A to D by a scan. `null` where no room was heard, or all of them when the scan failed. */
  rooms(counts: (number | null)[]): void;
  /** Something happened in the room we are joining or in. */
  event(event: SessionEvent): void;
  /** The driver has stopped: the firmware does not match this page. */
  failed(problem: Error): void;
}

/** The room we have been asked to be in, across however many sessions that takes. */
interface Stay {
  room: number;
  profile: Profile;
  /** Who the room was last said to hold. Unset until the host's first roster. */
  roster?: Member[];
}

/** A drawing on its way: waiting here, or held by the firmware. */
interface Drawing {
  id: number;
  bitmap: Uint8Array;
  /** When the firmware must have delivered it by, once uploaded. */
  deadline: number;
}

/** One attempt at being in the room, from JOIN until it ends or fails. */
interface Session {
  stay: Stay;
  /** The room has let us in. */
  ready: boolean;
  clientId: number;
  joinedAt: number;
  lastStatus: number;
  queue: Drawing[];
  /** The drawing being uploaded now, if any. */
  uploading?: Drawing;
  sending: Drawing[];
  pinging: boolean;
  timers: ReturnType<typeof setInterval>[];
  /** Settles when the session is over: resolved if we left, rejected with why if it failed. */
  over: Promise<void>;
  end: () => void;
  fail: (why: string) => void;
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

export class Driver {
  private readonly host: Host;
  private readonly write: (bytes: Uint8Array) => Promise<void>;
  private readonly sink: Sink;
  private nextId = 1;
  /** Commands go out one at a time; this is the tail of the line. */
  private turn: Promise<unknown> = Promise.resolve();
  /** The response being waited for. */
  private awaited: { id: number; resolve: (payload: Uint8Array | undefined) => void } | undefined;
  private roomsAwaited: ((counts: (number | null)[] | undefined) => void) | undefined;
  private hello: Hello | undefined;
  private stay: Stay | undefined;
  private session: Session | undefined;
  /** The scan in flight, if any: a join waits for it. */
  private scanning: Promise<void> | undefined;
  private scanTimer: ReturnType<typeof setTimeout> | undefined;
  private stopped = false;

  constructor(host: Host, write: (bytes: Uint8Array) => Promise<void>, sink: Sink) {
    this.host = host;
    this.write = write;
    this.sink = sink;
  }

  // ---- The line --------------------------------------------------------------

  /** Takes bytes as they arrive from the ESP32. */
  feed(bytes: Uint8Array): void {
    for (const packet of this.host.decode(bytes)) {
      if (packet.kind !== this.host.constants.RESPONSE) this.heard(packet);
      else if (this.awaited?.id === packet.id) this.awaited.resolve(packet.payload);
    }
  }

  /** Waits for the response to a command, for at most `ms`. */
  private response(id: number, ms: number): Promise<Uint8Array | undefined> {
    return new Promise((resolve) => {
      const waiting = { id, resolve: (payload: Uint8Array | undefined) => done(payload) };
      const timer = setTimeout(() => done(undefined), ms);
      const done = (payload: Uint8Array | undefined) => {
        clearTimeout(timer);
        if (this.awaited === waiting) this.awaited = undefined;
        resolve(payload);
      };
      this.awaited = waiting;
    });
  }

  /**
   * Sends a command until the firmware gives a final answer. Resolves to its
   * status and response data; a busy device or a lost packet is retried.
   */
  private exchange(kind: number, data?: Uint8Array): Promise<{ status: number; data: Uint8Array }> {
    const run = async () => {
      if (this.stopped) throw new Error("The ESP32 is disconnected.");
      const id = this.nextId++;
      const wire = this.host.encode(kind, id, data);
      const deadline = Date.now() + REQUEST_DEADLINE_MS;
      while (Date.now() < deadline && !this.stopped) {
        const resendAt = Date.now() + REQUEST_RETRY_MS;
        // Listening starts before the command goes out: the answer can be quick.
        let reply = this.response(id, REQUEST_RETRY_MS);
        await this.write(wire);
        for (;;) {
          const payload = await reply;
          if (!payload) break;
          const status = payload[0];
          if (status !== undefined && status !== this.host.constants.STATUS_BUSY) {
            return { status, data: payload.subarray(1) };
          }
          // Busy: wait out the rest of the interval, then ask again.
          const left = resendAt - Date.now();
          if (left <= 0) break;
          reply = this.response(id, left);
        }
      }
      throw new Error(`The ESP32 did not answer command ${kind}.`);
    };
    const result = this.turn.then(run, run);
    this.turn = result.catch(() => {});
    return result;
  }

  /** Sends a command that is expected to succeed and resolves to its response data. */
  private async request(kind: number, data?: Uint8Array): Promise<Uint8Array> {
    const answer = await this.exchange(kind, data);
    if (answer.status === this.host.constants.STATUS_OK) return answer.data;
    throw new Error(`The ESP32 refused command ${kind}: ${this.host.statusText(answer.status)}.`);
  }

  /** Attaches to the firmware, and has it leave whatever room an earlier owner left it in. */
  private async attach(): Promise<Hello> {
    const { HELLO, LEAVE, PROTOCOL_VERSION, STATUS_OK, STATUS_VERSION_MISMATCH } = this.host.constants;
    const { status, data } = await this.exchange(HELLO, Uint8Array.of(PROTOCOL_VERSION));
    const hello = this.host.hello(data);
    if (status === STATUS_VERSION_MISMATCH) throw new FirmwareMismatch(hello, PROTOCOL_VERSION);
    if (status !== STATUS_OK || hello.client_id === null) {
      throw new Error(`The ESP32 refused to attach: ${this.host.statusText(status)}.`);
    }
    await this.request(LEAVE);
    if (!this.hello) this.sink.attached(hello);
    this.hello = hello;
    return hello;
  }

  // ---- Lifecycle -------------------------------------------------------------

  /** Attaches to the firmware and starts looking for rooms. Rejects if it is not our firmware. */
  async start(): Promise<Hello> {
    const hello = await this.attach();
    this.scanSoon(0);
    return hello;
  }

  /** Stops for good: the line is going away. */
  stop(): void {
    this.stopped = true;
    clearTimeout(this.scanTimer);
    this.stay = undefined;
    this.session?.fail("The ESP32 was disconnected.");
    this.awaited?.resolve(undefined);
    this.roomsAwaited?.(undefined);
  }

  private abandon(problem: Error): void {
    this.stop();
    this.sink.failed(problem);
  }

  // ---- Looking for rooms -----------------------------------------------------

  private scanSoon(ms: number): void {
    clearTimeout(this.scanTimer);
    if (!this.stopped && !this.stay) this.scanTimer = setTimeout(() => void this.scan(), ms);
  }

  /** Has the idle firmware sweep every channel once, and reports the rooms it saw. */
  private async scan(): Promise<void> {
    if (this.stopped || this.stay || this.scanning) return;
    const sweep = async () => {
      try {
        await this.attach();
        await this.request(this.host.constants.SCAN);
        const counts = await new Promise<(number | null)[] | undefined>((resolve) => {
          const timer = setTimeout(() => resolve(undefined), SCAN_TIMEOUT_MS);
          this.roomsAwaited = (heard) => {
            clearTimeout(timer);
            resolve(heard);
          };
        });
        this.roomsAwaited = undefined;
        if (!this.stay && !this.stopped) this.sink.rooms(counts ?? ROOM_LETTERS.map(() => null));
      } catch (error) {
        if (error instanceof FirmwareMismatch) this.abandon(error);
        else if (!this.stay && !this.stopped) this.sink.rooms(ROOM_LETTERS.map(() => null));
      }
    };
    this.scanning = sweep();
    await this.scanning;
    this.scanning = undefined;
    this.scanSoon(SCAN_INTERVAL_MS);
  }

  // ---- Being in a room -------------------------------------------------------

  private emit(event: SessionEvent): void {
    if (!this.stopped) this.sink.event(event);
  }

  /** Joins a room, and stays in it, rejoining as often as it takes, until `leave`. */
  join(room: number, profile: Profile): void {
    const stay: Stay = { room, profile };
    this.stay = stay;
    clearTimeout(this.scanTimer);
    void this.remain(stay);
  }

  private async remain(stay: Stay): Promise<void> {
    const { room, profile } = stay;
    const wanted = () => this.stay === stay && !this.stopped;
    while (wanted()) {
      let session: Session | undefined;
      try {
        this.emit({ type: "backend_state", state: "connecting", detail: "Talking to the ESP32" });
        await this.scanning;
        await this.attach();
        if (!wanted()) break;
        let join: Uint8Array;
        try {
          join = this.host.join(room, profile.name.trim(), profile.message, profile.color);
        } catch (error) {
          // A profile the firmware would refuse will be refused every time.
          this.stay = undefined;
          this.emit({ type: "backend_state", state: "left", detail: message(error) });
          break;
        }
        this.emit({
          type: "backend_state",
          state: "connecting",
          detail: `Searching Wi-Fi channels for Room ${ROOM_LETTERS[room]}`,
        });
        await this.request(this.host.constants.JOIN, join);
        session = this.open(stay);
        await session.over;
        this.close(session);
        break;
      } catch (error) {
        if (session) this.close(session);
        if (error instanceof FirmwareMismatch) {
          this.abandon(error);
          return;
        }
        if (!wanted()) break;
        this.emit({ type: "backend_state", state: "reconnecting", detail: message(error) });
        // A session that was up and dropped is usually back within a second
        // with the same people in the room, so the member list is kept and
        // corrected by the next roster. If the attempt never got in, the room
        // is gone: nobody is there any more.
        if (!session?.ready) {
          if (stay.roster) {
            this.tell(stay, []);
          } else {
            stay.roster = [];
            this.emit({ type: "participants", room, participants: [] });
          }
        }
        await sleep(RECONNECT_DELAY_MS);
      }
    }
    if (!this.stay) this.scanSoon(0);
  }

  private open(stay: Stay): Session {
    let end = () => {};
    let fail = (_: string) => {};
    const over = new Promise<void>((resolve, reject) => {
      end = resolve;
      fail = (why) => reject(new Error(why));
    });
    // Nobody may be waiting yet when a session fails.
    over.catch(() => {});
    const now = Date.now();
    const session: Session = {
      stay,
      ready: false,
      clientId: this.hello?.client_id ?? 0,
      joinedAt: now,
      lastStatus: now,
      queue: [],
      sending: [],
      pinging: false,
      timers: [],
      over,
      end,
      fail,
    };
    session.timers.push(
      setInterval(() => this.ping(session), PING_INTERVAL_MS),
      setInterval(() => this.watch(session), WATCH_INTERVAL_MS),
    );
    this.session = session;
    return session;
  }

  /** Ends a session's timers and gives up on the drawings it had not delivered. */
  private close(session: Session): void {
    for (const timer of session.timers) clearInterval(timer);
    if (this.session === session) this.session = undefined;
    const lost = [...session.sending, ...(session.uploading ? [session.uploading] : []), ...session.queue];
    session.sending = [];
    session.queue = [];
    for (const { id } of lost) this.emit({ type: "delivery_timed_out", message_id: id });
  }

  private ping(session: Session): void {
    if (session.pinging) return;
    session.pinging = true;
    this.request(this.host.constants.PING)
      .catch((error: unknown) => session.fail(message(error)))
      .finally(() => {
        session.pinging = false;
      });
  }

  private watch(session: Session): void {
    const now = Date.now();
    if (!session.ready && now - session.joinedAt > JOIN_TIMEOUT_MS) {
      session.fail("Joining the DS room stalled; rejoining");
    } else if (now - session.lastStatus > STATUS_TIMEOUT_MS) {
      session.fail("The ESP32 stopped reporting; rejoining");
    } else if (session.sending.some((drawing) => now >= drawing.deadline)) {
      session.fail("A drawing was not delivered in time; rejoining");
    } else {
      void this.upload(session);
    }
  }

  /** Leaves the room and goes back to looking for rooms. */
  async leave(): Promise<void> {
    if (!this.stay) return;
    this.stay = undefined;
    this.session?.end();
    try {
      await this.request(this.host.constants.LEAVE);
    } catch {
      // The next scan attaches again and leaves then.
    }
    this.scanSoon(0);
  }

  /** Queues a drawing, as an API palette bitmap, for the room we are in. */
  send(id: number, bitmap: Uint8Array): void {
    const session = this.session;
    if (!session) {
      this.emit({ type: "delivery_timed_out", message_id: id });
      return;
    }
    if (session.queue.length >= MAX_QUEUED_DRAWINGS) {
      this.emit({ type: "error", code: "queue_full", message: "Too many drawings are waiting to be sent." });
      this.emit({ type: "delivery_timed_out", message_id: id });
      return;
    }
    session.queue.push({ id, bitmap, deadline: 0 });
    void this.upload(session);
  }

  /** Hands the firmware the next waiting drawing. It accepts one while it is still sending another. */
  private async upload(session: Session): Promise<void> {
    if (!session.ready || session.uploading || session.sending.length >= MAX_UPLOADED_DRAWINGS) return;
    const drawing = session.queue.shift();
    if (!drawing) return;
    session.uploading = drawing;
    try {
      let commands: Uint8Array[];
      try {
        commands = this.host.draw(drawing.id, drawing.bitmap);
      } catch (error) {
        session.uploading = undefined;
        this.emit({ type: "error", code: "invalid_drawing", message: message(error) });
        this.emit({ type: "delivery_timed_out", message_id: drawing.id });
        return;
      }
      for (const command of commands) await this.request(this.host.constants.DRAW, command);
      if (this.session !== session) return;
      session.uploading = undefined;
      drawing.deadline = Date.now() + DELIVERY_TIMEOUT_MS;
      session.sending.push(drawing);
    } catch (error) {
      session.fail(message(error));
    }
  }

  /** Reports who came and went between the last roster and this one, then the roster. */
  private tell(stay: Stay, members: Member[]): void {
    const before = stay.roster;
    stay.roster = members;
    const { room } = stay;
    if (before) {
      for (const participant of before) {
        if (!members.some((member) => member.client_id === participant.client_id)) {
          this.emit({ type: "participant_left", room, participant });
        }
      }
      for (const participant of members) {
        if (!before.some((member) => member.client_id === participant.client_id)) {
          this.emit({ type: "participant_joined", room, participant });
        }
      }
    }
    this.emit({ type: "participants", room, participants: members });
  }

  /** Acts on a packet from the firmware that is not a response. */
  private heard(packet: Packet): void {
    const session = this.session;
    for (const event of this.host.packet(packet)) {
      if (event.type === "rooms") {
        this.roomsAwaited?.(event.counts);
        continue;
      }
      if (!session) continue;
      const { stay } = session;
      switch (event.type) {
        case "status":
          session.lastStatus = Date.now();
          break;
        case "ready": {
          session.ready = true;
          session.clientId = event.client_id;
          this.emit({
            type: "backend_state",
            state: "ready",
            detail: `Joined Room ${ROOM_LETTERS[stay.room]} in slot ${event.slot}`,
          });
          const self = stay.roster?.find((member) => member.client_id === event.client_id);
          if (self) this.emit({ type: "participant_joined", room: stay.room, participant: self });
          this.emit({ type: "session_ready", client_id: event.client_id });
          break;
        }
        case "participants":
          this.tell(stay, event.participants);
          break;
        case "profile":
        case "message":
          this.emit(event);
          break;
        case "delivered": {
          const index = session.sending.findIndex((drawing) => drawing.id === event.message_id);
          const [drawing] = index < 0 ? [] : session.sending.splice(index, 1);
          if (!drawing) break;
          // Our own drawing, back from the host DS: everyone has it.
          this.emit({
            type: "message",
            station_id: session.clientId,
            name: stay.profile.name.trim(),
            message_id: drawing.id,
            palette_bitmap: toBase64(drawing.bitmap),
          });
          this.emit({ type: "delivered", message_id: drawing.id });
          break;
        }
        case "gap":
          this.emit({
            type: "error",
            code: "usb_rx_gap",
            message: "The ESP32 fell behind; a drawing that was arriving may be lost.",
          });
          break;
        case "left":
          session.fail(`${event.detail}; rejoining`);
          break;
      }
    }
  }
}
