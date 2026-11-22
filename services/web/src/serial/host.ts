// The host's side of the USB protocol, from the Rust crates the relay server
// uses, compiled to WebAssembly (crates/picto-relay-wasm). The page reads and
// writes the serial port and keeps time; this says what to send and what the
// answers mean.
//
// The module takes its input a byte at a time and gives its output the same
// way, so it needs no generated bindings: see the crate's documentation.

/** The module's exported functions. */
interface Exports {
  input_reset(): void;
  input_push(value: number): void;
  output_len(): number;
  output_at(index: number): number;
  link_constants(): number;
  link_encode(kind: number, id: number): number;
  link_decode(): number;
  host_hello(): number;
  host_join(room: number, color: number): number;
  host_draw(id: number): number;
  host_packet(kind: number, id: number): number;
  host_status_text(status: number): number;
}

/** The protocol's numbers, by their names in `picto-relay-link`. */
export interface Constants {
  PROTOCOL_VERSION: number;
  BAUD: number;
  HELLO: number;
  JOIN: number;
  LEAVE: number;
  PING: number;
  SCAN: number;
  DRAW: number;
  RESPONSE: number;
  STATUS_OK: number;
  STATUS_BUSY: number;
  STATUS_VERSION_MISMATCH: number;
  /** Kinds of packet the firmware sends unasked. */
  STATUS: number;
  RX: number;
  READY: number;
  DELIVERED: number;
  LEFT: number;
  ROOMS: number;
  STATUS_LEN: number;
  LEFT_KICKED: number;
}

/** One framed packet, in either direction. */
export interface Packet {
  kind: number;
  id: number;
  payload: Uint8Array;
}

/** What the firmware says of itself in answer to HELLO. */
export interface Hello {
  /** The protocol version it speaks. */
  version: number | null;
  /** The firmware build, a short git revision. */
  build: string;
  /** Its radio address, as `aa:bb:cc:dd:ee:ff`. */
  station: string | null;
  /** Our identity in rosters and on our messages. */
  client_id: number | null;
}

export interface Member {
  client_id: number;
  name: string;
}

/** What a packet from the firmware means. */
export type FirmwareEvent =
  | { type: "status"; message_id: number; state: number; phase: number; sent: number; total: number; echoed: number }
  | { type: "ready"; client_id: number; slot: number; channel: number }
  | { type: "participants"; participants: Member[] }
  | { type: "profile"; station_id: number; name: string; profile_message: string; profile_color: number }
  | { type: "message"; station_id: number; name: string; message_id: number; palette_bitmap: string }
  | { type: "delivered"; message_id: number }
  | { type: "left"; detail: string }
  | { type: "gap" }
  | { type: "rooms"; counts: (number | null)[] };

const NOTHING = new Uint8Array(0);
const text = new TextDecoder();

export class Host {
  readonly constants: Constants;
  private readonly wasm: Exports;

  private constructor(wasm: Exports) {
    this.wasm = wasm;
    this.constants = JSON.parse(text.decode(this.call(NOTHING, () => wasm.link_constants()))) as Constants;
  }

  /** Starts the module from its compiled bytes. */
  static async load(module: Uint8Array | ArrayBuffer): Promise<Host> {
    const { instance } = await WebAssembly.instantiate(module as BufferSource, {});
    return new Host(instance.exports as unknown as Exports);
  }

  /** Runs one operation on `input`. Throws its explanation if it fails. */
  private call(input: Uint8Array, operation: () => number): Uint8Array {
    this.wasm.input_reset();
    for (const value of input) this.wasm.input_push(value);
    const ok = operation() === 1;
    const output = new Uint8Array(this.wasm.output_len());
    for (let index = 0; index < output.length; index++) output[index] = this.wasm.output_at(index);
    if (!ok) throw new Error(text.decode(output));
    return output;
  }

  /** Frames a packet for the wire. */
  encode(kind: number, id: number, payload: Uint8Array = NOTHING): Uint8Array {
    return this.call(payload, () => this.wasm.link_encode(kind, id));
  }

  /** Takes bytes as they came off the wire and returns the packets they complete. */
  decode(bytes: Uint8Array): Packet[] {
    const packed = this.call(bytes, () => this.wasm.link_decode());
    const view = new DataView(packed.buffer);
    const packets: Packet[] = [];
    // Each packet: kind, ID (u32 LE), payload length (u16 LE), payload.
    for (let at = 0; at < packed.length; ) {
      const length = view.getUint16(at + 5, true);
      packets.push({
        kind: packed[at]!,
        id: view.getUint32(at + 1, true),
        payload: packed.subarray(at + 7, at + 7 + length),
      });
      at += 7 + length;
    }
    return packets;
  }

  /** Reads the data of the firmware's answer to HELLO, and remembers our radio address. */
  hello(data: Uint8Array): Hello {
    return JSON.parse(text.decode(this.call(data, () => this.wasm.host_hello()))) as Hello;
  }

  /** Builds the JOIN payload for a room and profile, and starts a room for it. */
  join(room: number, name: string, message: string, color: number): Uint8Array {
    const profile = new TextEncoder().encode(`${name}\0${message}`);
    return this.call(profile, () => this.wasm.host_join(room, color));
  }

  /** Turns a drawing, as an API palette bitmap, into the payloads of the DRAW commands that upload it. */
  draw(id: number, bitmap: Uint8Array): Uint8Array[] {
    const packed = this.call(bitmap, () => this.wasm.host_draw(id));
    const view = new DataView(packed.buffer);
    const commands: Uint8Array[] = [];
    // Each command: length (u16 LE), then payload.
    for (let at = 0; at < packed.length; ) {
      const length = view.getUint16(at, true);
      commands.push(packed.subarray(at + 2, at + 2 + length));
      at += 2 + length;
    }
    return commands;
  }

  /** Says what a packet from the firmware means. Responses to commands are not for this. */
  packet({ kind, id, payload }: Packet): FirmwareEvent[] {
    return JSON.parse(text.decode(this.call(payload, () => this.wasm.host_packet(kind, id)))) as FirmwareEvent[];
  }

  /** Names a RESPONSE status. */
  statusText(status: number): string {
    return text.decode(this.call(NOTHING, () => this.wasm.host_status_text(status)));
  }
}
