import type { FxSessionPromptInput } from "libfx";

import { BLACK, Canvas } from "./canvas.ts";
import type { Reply, Send } from "./brain.ts";
import { MAX_PAGES } from "./brain.ts";
import { greeting } from "./greeting.ts";
import { png } from "./png.ts";
import type { SessionEvent } from "./relay.ts";
import { type } from "./text.ts";

// The bot in a room: it greets the room when it has joined, hands each
// message people send to the model, and carries the model's answers back.

/** What the bot needs of the model. `Brain` is the real one. */
export interface Mind {
  reply(input: FxSessionPromptInput): Promise<Reply>;
}

/** What the bot needs of the relay server. `Relay` is the real one. */
export interface Link {
  send(session: number, drawing: Canvas): Promise<void>;
  events(session: number): AsyncIterable<SessionEvent>;
}

export interface BotOptions {
  /** Model id, shown in the greeting. */
  model: string;
  /** The bot's nickname. */
  name: string;
  /** Version of fx hosting the model, shown in the greeting. */
  fxVersion: string;
  /** How long to wait for the room to confirm a message arrived. */
  deliveryTimeoutMs?: number;
}

/** Messages handed to the model at once: the most images one prompt takes. */
const MAX_BATCH = 8;

/** Error events that mean the message being sent will not arrive. */
const SEND_FAILURES = ["invalid_drawing", "queue_full", "hardware_unavailable", "hardware_failed"];

/** Backend states that only report a drawing's progress to the room. */
const SENDING_STATES = ["uploading", "sending"];

/** Longest error passed on to the room, in characters: about one message. */
const MAX_APOLOGY = 110;

export class Bot {
  private readonly relay: Link;
  private readonly session: number;
  private readonly options: BotOptions;
  private readonly log: (line: string) => void;
  private mind: Mind | undefined;

  /** The bot's own identity in rosters and on its messages, once joined. */
  private own: number | undefined;
  /** Everyone else in the room, by identity. */
  private readonly members = new Map<number, string>();
  private rosterSeen = false;
  /** Things to tell the model with the next message: who came, who went. */
  private notes: string[] = [];
  private greeted = false;
  /** Whether the model has been told who is in the room. */
  private introduced = false;

  /** Messages waiting for the model. */
  private readonly inbox: { name: string; drawing: Canvas }[] = [];
  private working = false;

  /** Drawings are sent one at a time, each after the last was confirmed. */
  private outbox: Promise<void> = Promise.resolve();
  /** Settles the delivery in flight: with nothing if it arrived, else why not. */
  private settle: ((problem?: string) => void) | undefined;
  /** Drawings that reached the room. */
  private sent = 0;

  constructor(relay: Link, session: number, options: BotOptions, log: (line: string) => void) {
    this.relay = relay;
    this.session = session;
    this.options = options;
    this.log = log;
  }

  /** Puts a drawing in the room, after any already on their way. */
  readonly send: Send = (drawing) => {
    const delivery = this.outbox.then(() => this.deliver(drawing));
    this.outbox = delivery.catch(() => {});
    return delivery;
  };

  private async deliver(drawing: Canvas): Promise<void> {
    const outcome = new Promise<string | undefined>((resolve) => {
      this.settle = resolve;
    });
    const timeout = setTimeout(
      () => this.settle?.("The room did not confirm the message arrived"),
      this.options.deliveryTimeoutMs ?? 30_000,
    );
    try {
      await this.relay.send(this.session, drawing);
      const problem = await outcome;
      if (problem) throw new Error(problem);
      this.sent += 1;
    } finally {
      clearTimeout(timeout);
      this.settle = undefined;
    }
  }

  /** Types text into the room in black, on as many messages as allowed. */
  private async say(text: string): Promise<void> {
    for (const page of type(text, BLACK).slice(0, MAX_PAGES)) await this.send(page);
  }

  /** Runs until the server ends the session. `mind` answers the room's messages. */
  async run(mind: Mind): Promise<void> {
    this.mind = mind;
    for await (const event of this.relay.events(this.session)) this.handle(event);
  }

  private handle(event: SessionEvent): void {
    switch (event.type) {
      case "session_ready":
        this.own = event.client_id;
        this.members.delete(event.client_id);
        if (!this.greeted) {
          this.greeted = true;
          this.log("Joined. Sending the greeting.");
          this.send(greeting(this.options.model, this.options.fxVersion)).catch((error: Error) =>
            this.log(`The greeting was not delivered: ${error.message}`),
          );
        }
        break;
      case "backend_state":
        // The radio reports every step of every drawing it sends; that is too much to read.
        if (!SENDING_STATES.includes(event.state)) this.log(`${event.state}: ${event.detail}`);
        break;
      case "participants": {
        const now = new Map(event.participants.map(({ client_id, name }) => [client_id, name]));
        for (const [id, name] of this.members) if (!now.has(id)) this.left(id, name);
        for (const [id, name] of now) this.joined(id, name, this.rosterSeen && this.own !== undefined);
        this.rosterSeen = true;
        break;
      }
      case "participant_joined":
        // Whoever shows up before the bot has joined was there first; they did not just arrive.
        this.joined(event.participant.client_id, event.participant.name, this.own !== undefined);
        break;
      case "participant_left":
        this.left(event.participant.client_id, event.participant.name);
        break;
      case "profile":
        if (event.station_id !== this.own && event.profile_message.trim() !== "") {
          this.notes.push(`${event.name}'s profile message reads: ${event.profile_message}`);
        }
        break;
      case "message":
        if (event.station_id === this.own) break; // Its own message, echoed back.
        this.log(`${event.name} sent a message.`);
        this.inbox.push({
          name: event.name,
          drawing: Canvas.unpack(Buffer.from(event.palette_bitmap, "base64")),
        });
        void this.work();
        break;
      case "delivered":
        this.settle?.();
        break;
      case "delivery_timed_out":
        this.settle?.("The message did not reach the room: delivery timed out");
        break;
      case "error":
        this.log(`Server error ${event.code}: ${event.message}`);
        if (SEND_FAILURES.includes(event.code)) this.settle?.(event.message);
        break;
    }
  }

  private joined(id: number, name: string, announce: boolean): void {
    if (id === this.own || this.members.get(id) === name) return;
    // Until the bot knows its own identity, its nickname is the best clue.
    if (this.own === undefined && name === this.options.name) return;
    const known = this.members.has(id);
    this.members.set(id, name);
    if (announce && !known) {
      this.log(`${name} joined.`);
      this.notes.push(`${name} joined the room.`);
    }
  }

  private left(id: number, name: string): void {
    if (!this.members.delete(id)) return;
    this.log(`${name} left.`);
    this.notes.push(`${name} left the room.`);
  }

  /** Hands waiting messages to the model, one turn at a time. */
  private async work(): Promise<void> {
    if (this.working || !this.mind) return;
    this.working = true;
    try {
      while (this.inbox.length > 0) {
        const batch = this.inbox.splice(0, MAX_BATCH);
        const notes = this.notes.splice(0);
        if (!this.introduced && this.members.size > 0) {
          notes.unshift(`In the room with you: ${[...this.members.values()].join(", ")}.`);
        }
        this.introduced = true;
        const input = batch.flatMap(({ name, drawing }, index) => [
          {
            type: "text" as const,
            text: [...(index === 0 ? notes : []), `Message from ${name}:`].join("\n"),
          },
          {
            type: "image" as const,
            mimeType: "image/png",
            data: Buffer.from(png(drawing)).toString("base64"),
          },
        ]);

        const before = this.sent;
        const reply = await this.mind.reply(input);
        const said = reply.text.trim();
        if (reply.error) {
          this.log(`The model's turn failed: ${reply.error}`);
          await this.say(`Sorry, I could not answer: ${reply.error.slice(0, MAX_APOLOGY)}`);
        } else if (this.sent === before && said !== "") {
          // An answer given outside the tools still belongs in the room.
          this.log(`Model says: ${said}`);
          await this.say(said);
        } else if (said !== "") {
          this.log(`Model's note, not sent: ${said}`);
        }
      }
    } catch (error) {
      this.log(`Could not answer: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      this.working = false;
    }
    // A message that arrived while the last answer was being typed.
    if (this.inbox.length > 0) void this.work();
  }
}
