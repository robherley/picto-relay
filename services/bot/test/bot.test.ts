import assert from "node:assert/strict";
import { test } from "node:test";

import { Bot } from "../src/bot.ts";
import type { Link, Mind } from "../src/bot.ts";
import { MAX_PAGES, MESSAGE_BUDGET, fxVersion, gatewayCredential, instructions, tools } from "../src/brain.ts";
import type { Send } from "../src/brain.ts";
import { BLACK, Canvas } from "../src/canvas.ts";
import { greeting } from "../src/greeting.ts";
import type { SessionEvent } from "../src/relay.ts";
import { type } from "../src/text.ts";

const OWN = 7;
const ROB = 4083630471;

/** A relay server in miniature: confirms each drawing, and plays a script of events. */
class FakeRelay implements Link {
  readonly sent: Canvas[] = [];
  private readonly queue: SessionEvent[] = [];
  private wake = () => {};
  private done = false;
  /** What happens to the next drawings sent; delivered when empty. */
  outcomes: SessionEvent["type"][] = [];

  emit(...events: SessionEvent[]): void {
    this.queue.push(...events);
    this.wake();
  }

  end(): void {
    this.done = true;
    this.wake();
  }

  async send(_session: number, drawing: Canvas): Promise<void> {
    this.sent.push(drawing);
    const outcome = this.outcomes.shift() ?? "delivered";
    if (outcome === "delivered" || outcome === "delivery_timed_out") {
      this.emit({ type: outcome, message_id: this.sent.length });
    }
  }

  async *events(): AsyncGenerator<SessionEvent> {
    for (;;) {
      while (this.queue.length > 0) yield this.queue.shift()!;
      if (this.done) return;
      await new Promise<void>((resolve) => {
        this.wake = resolve;
      });
    }
  }
}

function message(name: string, station_id: number, text: string): SessionEvent {
  const palette_bitmap = Buffer.from(type(text, BLACK)[0]!.pack()).toString("base64");
  return { type: "message", station_id, name, message_id: 1, palette_bitmap };
}

/** Runs a bot over a script, with a model that answers by `answer`. */
async function play(
  script: (relay: FakeRelay) => void,
  answer: (send: Send, input: unknown) => Promise<{ text: string; error?: string }>,
) {
  const relay = new FakeRelay();
  const log: string[] = [];
  const bot = new Bot(relay, 1, { model: "test/model", name: "fx", fxVersion: "1.2.3", deliveryTimeoutMs: 50 }, (line) => log.push(line));
  const prompts: unknown[] = [];
  const mind: Mind = {
    async reply(input) {
      prompts.push(input);
      const reply = await answer(bot.send, input);
      // The room goes quiet once the model has answered the last message.
      setTimeout(() => relay.end(), 20);
      return reply;
    },
  };
  script(relay);
  await bot.run(mind);
  return { relay, prompts, log };
}

test("the bot greets the room, then answers a message through the model's tools", async () => {
  const { relay, prompts } = await play(
    (relay) => {
      relay.emit(
        { type: "session_ready", client_id: OWN },
        { type: "participants", room: 0, participants: [{ client_id: OWN, name: "fx" }, { client_id: ROB, name: "Rob" }] },
        { type: "participant_joined", room: 0, participant: { client_id: 9, name: "Bob" } },
        { type: "profile", station_id: ROB, name: "Rob", profile_message: "hi there", profile_color: 2 },
        message("fx", OWN, "my own echo"),
        message("Rob", ROB, "draw a cat"),
      );
    },
    async (send) => {
      await tools(send)[0]!.execute({ text: "Hello Rob!", ink: 5 }, {} as never);
      return { text: "(done)" };
    },
  );

  // Greeting first, then the answer; the echo of its own message is not answered.
  assert.equal(relay.sent.length, 2);
  assert.deepEqual(relay.sent[0]!.pixels, greeting("test/model", "1.2.3").pixels);
  assert.deepEqual(relay.sent[1]!.pixels, type("Hello Rob!", 5)[0]!.pixels);

  assert.equal(prompts.length, 1);
  const [text, image] = prompts[0] as [{ text: string }, { type: string; mimeType: string; data: string }];
  assert.equal(
    text.text,
    "In the room with you: Rob, Bob.\nBob joined the room.\nRob's profile message reads: hi there\nMessage from Rob:",
  );
  assert.equal(image.mimeType, "image/png");
  assert.equal(Buffer.from(image.data, "base64").subarray(1, 4).toString("latin1"), "PNG");
});

test("people already in the room are introduced, not announced as arriving", async () => {
  const { prompts, log } = await play(
    (relay) => {
      relay.emit(
        { type: "backend_state", state: "connecting", detail: "searching" },
        // A DS is first known by its address, then by the name in its profile.
        { type: "participant_joined", room: 0, participant: { client_id: ROB, name: "00:23:31:cc:71:b1" } },
        { type: "session_ready", client_id: OWN },
        { type: "backend_state", state: "uploading", detail: "Drawing 0: 9% uploaded" },
        { type: "participants", room: 0, participants: [{ client_id: OWN, name: "fx" }, { client_id: ROB, name: "Rob" }] },
        message("Rob", ROB, "hello"),
      );
    },
    async () => ({ text: "" }),
  );
  assert.equal((prompts[0] as [{ text: string }])[0].text, "In the room with you: Rob.\nMessage from Rob:");
  assert.ok(log.includes("connecting: searching"));
  assert.ok(!log.some((line) => line.includes("uploaded") || line.includes("joined.")));
});

test("an answer given outside the tools is typed into the room", async () => {
  const { relay } = await play(
    (relay) => relay.emit({ type: "session_ready", client_id: OWN }, message("Rob", ROB, "hello")),
    async () => ({ text: "Hi! ".repeat(400) }),
  );
  // The greeting, then the text, cut off at the most messages one answer may take.
  assert.equal(relay.sent.length, 1 + MAX_PAGES);
});

test("a model that says nothing sends nothing, and a failed turn says so", async () => {
  const quiet = await play(
    (relay) => relay.emit({ type: "session_ready", client_id: OWN }, message("Rob", ROB, "hello")),
    async () => ({ text: "  " }),
  );
  assert.equal(quiet.relay.sent.length, 1);

  const failed = await play(
    (relay) => relay.emit({ type: "session_ready", client_id: OWN }, message("Rob", ROB, "hello")),
    async () => ({ text: "", error: "the key was refused" }),
  );
  assert.deepEqual(
    failed.relay.sent[1]!.pixels,
    type("Sorry, I could not answer: the key was refused", BLACK)[0]!.pixels,
  );
});

test("the model hears when a message does not arrive", async () => {
  let refusal = "";
  await play(
    (relay) => {
      relay.outcomes = ["delivered", "delivery_timed_out"];
      relay.emit({ type: "session_ready", client_id: OWN }, message("Rob", ROB, "hello"));
    },
    async (send) => {
      await assert.rejects(
        async () => tools(send)[0]!.execute({ text: "lost" }, {} as never),
        (error: Error) => {
          refusal = error.message;
          return true;
        },
      );
      return { text: "" };
    },
  );
  assert.match(refusal, /timed out/);
});

test("the tools refuse what a DS could not show, and say why", async () => {
  const sent: Canvas[] = [];
  const [write, draw] = tools(async (drawing) => void sent.push(drawing));
  const context = {} as never;

  await assert.rejects(async () => write!.execute({ text: "word ".repeat(200) }, context), /fewer words/);
  await assert.rejects(async () => write!.execute({ text: "hi", ink: 0 }, context), /ink/);
  await assert.rejects(async () => write!.execute({ text: "  " }, context), /empty/);
  await assert.rejects(async () => draw!.execute({ shapes: [{ type: "blob" }] }, context), /Nothing was sent/);
  await assert.rejects(
    async () => draw!.execute({ shapes: [{ type: "line", x1: 900, y1: 900, x2: 950, y2: 900 }] }, context),
    /blank/,
  );
  assert.equal(sent.length, 0);

  assert.match((await write!.execute({ text: "word ".repeat(60) }, context)) as string, /2 messages/);
  const picture = (await draw!.execute(
    { shapes: [{ type: "ellipse", cx: 114, cy: 40, rx: 30, ry: 20, fill: 7 }] },
    context,
  )) as { images: { data: string }[] };
  assert.equal(sent.length, 3);
  assert.equal(Buffer.from(picture.images[0]!.data, "base64").subarray(1, 4).toString("latin1"), "PNG");
});

test("an API key is used before a linked project's token", () => {
  assert.equal(gatewayCredential({}), undefined);
  assert.equal(gatewayCredential({ AI_GATEWAY_API_KEY: "", VERCEL_OIDC_TOKEN: "token" }), "token");
  assert.equal(gatewayCredential({ AI_GATEWAY_API_KEY: "key", VERCEL_OIDC_TOKEN: "token" }), "key");
});

test("the fx version is the installed libfx's", () => {
  assert.match(fxVersion(), /^\d+\.\d+\.\d+/);
});

test("the model is told how much fits in a message, and to draw when asked", () => {
  const prompt = instructions({ model: "test/model", name: "fx", room: "B" });
  assert.equal(MESSAGE_BUDGET, 150);
  assert.match(prompt, /about 150 characters/);
  assert.match(prompt, /171 characters when every line is full/);
  assert.match(prompt, /When someone says "draw"/);
  assert.match(prompt, /room B under the nickname "fx"/);
});
