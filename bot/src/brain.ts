import { readFileSync } from "node:fs";

import { createFxAgent, memory } from "libfx";
import type { FxAgentHostTool, FxAgentOptions, FxSessionPromptInput, FxToolResult } from "libfx";

import { BOX_WIDTH, INK_NAMES, NATIVE_HEIGHT, TAG_HEIGHT, TAG_RIGHT, DRAW_LEFT } from "./canvas.ts";
import type { Canvas, Ink } from "./canvas.ts";
import { png } from "./png.ts";
import { MAX_SHAPES, MAX_THICKNESS, ShapeError, draw } from "./shapes.ts";
import { FIRST_LINE_LETTERS, LINES, LINE_LETTERS, MESSAGE_LETTERS, type } from "./text.ts";

// The model, hosted with libfx (https://fx.sh/docs/lib): what it is told
// about PictoChat, and the two tools it answers the room with.

/** The version of fx hosting the model: that of the libfx package installed. */
export function fxVersion(): string {
  // libfx does not export its package.json; it sits beside its entry point.
  const manifest = new URL("package.json", import.meta.resolve("libfx"));
  return (JSON.parse(readFileSync(manifest, "utf8")) as { version: string }).version;
}

/** Most messages one `write` may spread its text over. */
export const MAX_PAGES = 3;

export interface BrainOptions {
  /** Model id, as the provider names it. */
  model: string;
  /** The bot's nickname in the room. */
  name: string;
  /** Letter of the room it is in. */
  room: string;
  /** An OpenAI-compatible server to use instead of AI Gateway. */
  baseUrl?: string | undefined;
}

/**
 * The credential for AI Gateway, if the environment has one: an API key, or
 * the OIDC token of the Vercel project the bot is linked to, which
 * `vercel env pull` writes to `.env.local`.
 */
export function gatewayCredential(env: NodeJS.ProcessEnv): string | undefined {
  return env.AI_GATEWAY_API_KEY || env.VERCEL_OIDC_TOKEN || undefined;
}

/**
 * Characters to aim for in an answer, so it fits one message: a full message
 * less what wrapping at words usually wastes, rounded down to a round number.
 */
export const MESSAGE_BUDGET = Math.floor((MESSAGE_LETTERS * 0.9) / 10) * 10;

/** Puts a drawing in the room. Resolves once it arrived, or rejects with why not. */
export type Send = (drawing: Canvas) => Promise<void>;

/** The system prompt: where the model is, and how to talk and draw there. */
export function instructions({ model, name, room }: BrainOptions): string {
  const inks = INK_NAMES.map((ink, index) => `${index} ${ink}`).join(", ");
  return `You are ${model}, and you have joined PictoChat room ${room} under the nickname "${name}".

PictoChat is the chat program built into the Nintendo DS. The other people in the room are nearby, each holding a DS and writing on its small touch screen with a stylus. You reach the room through a radio bridge, so to them you are one more person in the chat.

## What you receive

Every PictoChat message is a small drawing: ${BOX_WIDTH} by ${NATIVE_HEIGHT} pixels, in up to 15 inks on white paper. People type with an on-screen keyboard, write by hand, sketch, or mix all three in one message. Each message from the room reaches you as an image labelled with its sender's nickname. Read it like a note passed across a table. Handwriting with a stylus is often rough, so make your best guess at it instead of asking someone to write again. The gray area in each image is the frame around the message, where the sender's name tag sits; it is not part of what they drew.

Several people may be in the room, and you are told when someone joins or leaves. Not every message is meant for you. When people are talking to each other, you may stay out of it.

## How you answer

The room sees only what you send with your two tools, \`write\` and \`draw\`. If you answer in plain text without calling a tool, that text is typed into the chat in black, as if you had called \`write\`. To say nothing, answer with nothing at all.

Once your tool calls have sent your answer, stop. Do not add a summary or a note about what you did: nobody sees it.

This is a tiny screen and a casual chat. Answer the way people do there: a sentence or two, friendly and playful. Never use Markdown, because asterisks, backticks and pound signs show up as themselves.

### The size of a message

One message holds very little text: ${LINES} lines, the first with room for ${FIRST_LINE_LETTERS} characters beside your name tag and the other ${LINES - 1} with room for ${LINE_LETTERS} each. That is ${MESSAGE_LETTERS} characters when every line is full, and words that do not fit at the end of a line move whole to the next, which wastes some. So keep each answer to about ${MESSAGE_BUDGET} characters, spaces included, and it will fit in one message. Count before you send. If a question deserves a longer answer than that, give the short version and offer to go on, rather than sending a wall of text.

### write

\`write\` types text into the chat the way the DS keyboard does. Text too long for one message continues in further messages, ${MAX_PAGES} at most. Each one takes a moment to reach the room and pushes the conversation off the screen, so treat a second message as the exception.

The DS font has Latin letters with accents, Japanese kana and common kanji, and these symbols: ★ ☆ ♥ ♡ ♪ ☀ ☁ ☂ ☃ ← ↑ → ↓ ○ ● ◎ ■ □ ▲ △ ▼ ▽ ◆ ◇ ♠ ♣ ♦ … ∞. It has no emoji. A character the font lacks is drawn as an empty box.

\`ink\` picks the color of the text: an ink number, or "rainbow". Black is the usual choice; use color for fun or emphasis.

### draw

\`draw\` sends a picture. When someone says "draw", or asks for a picture, a sketch or a doodle, answer with \`draw\`: almost always a picture is what they want, even when the request is odd or hard. Never describe in words the picture you would draw, and do not ask what they would like when you can simply pick something and draw it. Draw unasked too, whenever a picture says it better, or more amusingly, than words would.

- The canvas is ${BOX_WIDTH} pixels wide and ${NATIVE_HEIGHT} tall. x runs right from 0 and y runs down from 0.
- Your name tag covers the top-left corner (x below ${TAG_RIGHT - DRAW_LEFT} and y below ${TAG_HEIGHT}). Nothing drawn there shows.
- A picture is a list of shapes, drawn in order, later shapes over earlier ones: line, rect, ellipse, polygon, polyline, path (SVG path data, curves included) and text.
- \`ink\` is the color of a shape's outline, black unless you say otherwise. \`fill\` colors its inside. For a solid shape with no outline, give both the same ink. \`thickness\` is the outline's width, 1 to ${MAX_THICKNESS}.
- The inks are: ${inks}; or "rainbow", which changes color every 6 pixels across. Gray is very pale. Ink 0 paints paper, which erases what is under it.
- Every pixel is exactly one ink: there is no blending, shading or transparency. At this size, bold and simple reads best. Use outlines 2 thick, large filled areas and few details, and fill the canvas rather than drawing something small in a corner.
- A text shape puts DS-font text anywhere in the picture, so one message can hold a drawing and its caption. Capital letters are 9 pixels tall and about 6 wide; \`y\` is the baseline they stand on.

You are shown each picture as it appears on their screens. The room has already seen it by then, so send another only if it came out wrong in a way that matters.`;
}

/** JSON Schema for an ink: checked properly in the tools, which explain what is wrong. */
const INK_SCHEMA = { type: ["integer", "string"] };

/** The tools the model answers the room with. */
export function tools(send: Send): FxAgentHostTool[] {
  return [
    {
      name: "write",
      description: `Type text into the chat, as the DS keyboard would. One message holds about ${MESSAGE_BUDGET} characters; longer text spills into further messages, which you should avoid.`,
      inputSchema: {
        type: "object",
        properties: {
          text: {
            type: "string",
            description: `What to say, in ${MESSAGE_BUDGET} characters or fewer so it fits one message. Line breaks are kept.`,
          },
          ink: { ...INK_SCHEMA, description: 'Ink number 1–15, or "rainbow". Default 1, black.' },
        },
        required: ["text"],
      },
      // Sending a message twice shows it twice.
      idempotent: false,
      async execute(input: { text?: unknown; ink?: unknown }) {
        if (typeof input.text !== "string") throw new Error('"text" must be a string');
        const ink = input.ink ?? 1;
        const valid =
          ink === "rainbow" || (typeof ink === "number" && Number.isInteger(ink) && ink >= 1 && ink <= 15);
        if (!valid) throw new Error('"ink" must be an ink number 1–15 or "rainbow"');
        const pages = type(input.text, ink as Ink);
        if (pages.length === 0) throw new Error("There is nothing to write: the text is empty");
        if (pages.length > MAX_PAGES) {
          throw new Error(
            `That text needs ${pages.length} messages and the most is ${MAX_PAGES}. Nothing was sent: say it in fewer words.`,
          );
        }
        for (const page of pages) await send(page);
        return pages.length === 1
          ? "Sent."
          : `Sent, but it took ${pages.length} messages. Keep answers to about ${MESSAGE_BUDGET} characters so they fit in one.`;
      },
    },
    {
      name: "draw",
      description: `Send a picture to the chat: a list of shapes on a ${BOX_WIDTH} by ${NATIVE_HEIGHT} pixel canvas. Returns the picture as the room sees it.`,
      inputSchema: {
        type: "object",
        properties: {
          shapes: {
            type: "array",
            maxItems: MAX_SHAPES,
            description: "Shapes in drawing order; later ones cover earlier ones.",
            items: {
              type: "object",
              properties: {
                type: {
                  type: "string",
                  enum: ["line", "rect", "ellipse", "polygon", "polyline", "path", "text"],
                  description:
                    "line: x1 y1 x2 y2. rect: x y width height. ellipse: cx cy rx ry. polygon (closed) and polyline (open): points. path: d. text: x y text.",
                },
                ink: { ...INK_SCHEMA, description: 'Outline or text ink: 0–15 or "rainbow". Default 1, black.' },
                fill: { ...INK_SCHEMA, description: 'Ink for the inside: 0–15 or "rainbow". Omit for no fill.' },
                thickness: { type: "integer", minimum: 1, maximum: MAX_THICKNESS, description: "Outline width in pixels. Default 1." },
                x: { type: "number" },
                y: { type: "number", description: "For text, the baseline." },
                width: { type: "number" },
                height: { type: "number" },
                x1: { type: "number" },
                y1: { type: "number" },
                x2: { type: "number" },
                y2: { type: "number" },
                cx: { type: "number" },
                cy: { type: "number" },
                rx: { type: "number" },
                ry: { type: "number" },
                points: {
                  type: "array",
                  items: { type: "array", items: { type: "number" }, minItems: 2, maxItems: 2 },
                  description: "[x, y] pairs.",
                },
                d: { type: "string", description: "SVG path data, such as M10 40 Q30 10 50 40 Z." },
                text: { type: "string" },
              },
              required: ["type"],
            },
          },
        },
        required: ["shapes"],
      },
      idempotent: false,
      async execute(input: { shapes?: unknown }): Promise<FxToolResult> {
        let picture: Canvas;
        try {
          picture = draw(input.shapes);
        } catch (error) {
          // Only what is wrong with the request; anything else is a bug to surface.
          if (error instanceof ShapeError) throw new Error(`${error.message}. Nothing was sent.`);
          throw error;
        }
        if (picture.blank) {
          throw new Error("Nothing was sent: the picture is blank. Check the shapes are on the canvas.");
        }
        await send(picture);
        return {
          type: "libfx.tool-result",
          text: "Sent. This is the picture as the room sees it.",
          images: [{ type: "image", mimeType: "image/png", data: Buffer.from(png(picture)).toString("base64") }],
        };
      },
    },
  ];
}

/** What libfx puts in place of an answer when the model gives none. */
const SILENCE = "Done.";

/** How one of the model's turns went. */
export interface Reply {
  /** What the model said outside its tools. */
  text: string;
  /** Why the turn failed, if it did. */
  error?: string;
}

/** Called as the model works, for the operator's log. */
export type Watch = (event: { tool: string; input: unknown } | { failed: string; why: string }) => void;

/** The model and its one conversation, kept in memory for as long as the bot runs. */
export class Brain {
  private readonly agent;
  private readonly watch: Watch;

  constructor(options: BrainOptions, send: Send, watch: Watch = () => {}) {
    this.watch = watch;
    const provider: Partial<FxAgentOptions> = options.baseUrl
      ? {
          provider: {
            name: "custom",
            baseUrl: options.baseUrl,
            // libfx cannot ask the server what its models can do, and sends
            // images only to a model listed as able to see. The bot is no use
            // without tools and sight, so say this model has both.
            models: { [options.model]: { supportsToolUse: true, supportsVision: true } },
          },
          ...(process.env.MODEL_API_KEY
            ? { credentials: () => ({ apiKey: process.env.MODEL_API_KEY! }) }
            : {}),
        }
      : { apiKey: gatewayCredential(process.env) };
    this.agent = createFxAgent({
      model: options.model,
      instructions: instructions(options),
      tools: tools(send),
      durability: memory(),
      ...provider,
    } as FxAgentOptions);
  }

  /** Gives the model what happened in the room and lets it answer through its tools. */
  async reply(input: FxSessionPromptInput): Promise<Reply> {
    const turn = this.agent.prompt(input);
    let text = "";
    for await (const event of turn) {
      if (event.type === "text_delta") text += event.delta;
      else if (event.type === "tool_start") this.watch({ tool: event.name, input: event.input });
      else if (event.type === "tool_end" && event.isError) {
        this.watch({ failed: event.name, why: event.content ?? "" });
      }
    }
    const result = await turn.result;
    // libfx 0.0.17 reports a turn in which the model said nothing as the
    // text "Done.". The model stayed quiet; so does the bot.
    if (text.trim() === SILENCE) text = "";
    if (result.stopReason === "end_turn") return { text };
    const why = result.error?.message ?? `the model stopped early (${result.stopReason})`;
    return { text, error: why };
  }

  close(): Promise<void> {
    return this.agent.close();
  }
}
