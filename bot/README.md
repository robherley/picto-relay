# Picto Relay bot

Puts a language model in a PictoChat room. People on a Nintendo DS write or
draw to it as they would to anyone else in the room, and it writes and draws
back.

It is a headless client of [picto-relay-server](../crates/picto-relay-server/README.md):
it joins a room through the [JSON API](../docs/json-api.md), and hosts the
model with [libfx](https://fx.sh/docs/lib), the library form of the fx agent.

## Run

libfx reaches models through [AI Gateway](https://vercel.com/docs/ai-gateway),
which needs a credential. Either link `bot/` to a Vercel project, once:

```sh
cd bot && vercel link
```

or put an AI Gateway API key in `AI_GATEWAY_API_KEY`. A linked project needs
no key: `just bot` fetches the project's token (`vercel env pull`, into the
uncommitted `bot/.env.local`) each time it starts. That token lasts 12 hours,
so restart a bot that has run longer. Usage is billed to the project's team.

Then start the server and the bot, naming the room, the model and its nickname:

```sh
just server    # terminal 1: the relay, with the ESP32
just bot --room A --model anthropic/claude-haiku-5.5 --name Claude    # terminal 2
```

With the ESP32, a DS must already be in the room, and the bot takes the
relay's one radio session, so the web client cannot be in a room at the same
time. With `just server-memory` there is no radio and both can join; that is
how to try the bot without a DS.

When it has joined, the bot sends one message: the fx logo, the model's name
and the version of fx hosting it. Its profile message is the model's name and the version of fx hosting
it, such as `claude-haiku-5.5 fx 0.0.17`, shortened to the 26 characters a DS
profile holds. Ctrl+C leaves the room.

```
just bot --room <A-D> --model <id> [--name <nickname>]

  --room <A-D>       Room to join.
  --model <id>       Model to host, as AI Gateway names it. It must accept images.
  --name <nickname>  Nickname in the room, up to 10 characters. Default: fx
  --server <url>     The relay server. Default: $PICTO_RELAY_URL, else
                     http://127.0.0.1:3001
  --base-url <url>   Use an OpenAI-compatible server (a local model, say)
                     instead of AI Gateway. Its key, if any: $MODEL_API_KEY.
```

The terminal shows what happens: who joins and leaves, each message that
arrives, and what the model writes and draws.

## How it works

Every PictoChat message is a drawing, typed text included. So:

- **What the model receives.** Each message from the room becomes a PNG,
  enlarged four times, labelled with the sender's nickname. The model reads
  typing, handwriting and sketches by looking at them, which is why it must
  accept images. Messages that arrive while it is answering are handed over
  together.
- **What the model sends.** It has two tools. `write` types text in the DS
  font, in any ink or rainbow. `draw` takes a list of shapes (lines, boxes,
  ellipses, polygons, SVG paths, text) on the 228 by 80 pixel canvas, and
  shows the model the result. An answer given in plain text, with no tool
  call, is typed into the room in black.
- **What the model is told.** Where it is, how a DS message looks, that one
  message holds about 150 characters and answers should fit in one, and that
  "draw" means draw. The whole briefing is `instructions()` in
  [`src/brain.ts`](src/brain.ts).

Text is drawn from [`font.json`](../crates/picto-relay-server/src/font.json),
the table of DS glyphs the server and web client share, laid out as the server
lays out API text; the tests check the pixels against the server's golden
samples. Text too long for one message carries on in the next, three at most.

The conversation lives in memory for as long as the bot runs.

| File | What it holds |
|---|---|
| [`src/main.ts`](src/main.ts), [`src/args.ts`](src/args.ts) | The command line |
| [`src/bot.ts`](src/bot.ts) | The bot in a room: greeting, messages in, answers out |
| [`src/brain.ts`](src/brain.ts) | The model: its instructions and tools |
| [`src/relay.ts`](src/relay.ts) | The relay server's API |
| [`src/canvas.ts`](src/canvas.ts), [`src/text.ts`](src/text.ts), [`src/shapes.ts`](src/shapes.ts) | Drawings: the surface, typed text, shapes |
| [`src/png.ts`](src/png.ts), [`src/greeting.ts`](src/greeting.ts) | A drawing as a picture for the model; the logo message |

## Develop

It is TypeScript that Node runs as it is (Node 22.18 or later); there is no
build step.

```sh
npm --prefix bot test    # type-check, then the unit tests
```
