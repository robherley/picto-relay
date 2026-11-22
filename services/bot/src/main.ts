import { USAGE, UsageError, profileMessage, readArgs } from "./args.ts";
import type { Args } from "./args.ts";
import { Bot } from "./bot.ts";
import { Brain, fxVersion, gatewayCredential } from "./brain.ts";
import { ROOM_LETTERS, Relay } from "./relay.ts";

// picto-relay-bot: hosts a model in a PictoChat room. See README.md.

function log(line: string): void {
  console.log(`${new Date().toLocaleTimeString()}  ${line}`);
}

/** A short description of a tool call, for the log. */
function describe(tool: string, input: unknown): string {
  const fields = (input ?? {}) as { text?: unknown; shapes?: unknown };
  if (tool === "write") return `writes: ${String(fields.text)}`;
  if (tool === "draw" && Array.isArray(fields.shapes)) return `draws ${fields.shapes.length} shapes`;
  return `calls ${tool}`;
}

async function main(args: Args): Promise<number> {
  if (!args.baseUrl && !gatewayCredential(process.env)) {
    console.error(
      "No AI Gateway credential. Link the repository to a Vercel project (vercel link), set AI_GATEWAY_API_KEY, or pass --base-url.",
    );
    return 2;
  }

  const relay = new Relay(args.server);
  const room = ROOM_LETTERS[args.room]!;
  const version = fxVersion();
  const session = await relay.join({
    name: args.name,
    room: args.room,
    profile_message: profileMessage(args.model, version),
  });
  log(`Joining room ${room} as ${args.name}, hosting ${args.model}. Ctrl+C leaves.`);

  const bot = new Bot(relay, session, { ...args, fxVersion: version }, log);
  const brain = new Brain({ ...args, room }, bot.send, (event) => {
    if ("tool" in event) log(`Model ${describe(event.tool, event.input)}`);
    else log(`Model's ${event.failed} was refused: ${event.why}`);
  });

  // Leave the room on the way out, so the DS shows the bot going.
  let leaving = false;
  for (const signal of ["SIGINT", "SIGTERM"] as const) {
    process.once(signal, () => {
      leaving = true;
      log("Leaving.");
      relay
        .leave(session)
        .catch(() => {})
        .finally(() => process.exit(0));
    });
  }

  await bot.run(brain);
  if (leaving) return 0;
  console.error("The relay server ended the session.");
  return 1;
}

try {
  const args = readArgs(process.argv.slice(2), process.env);
  if (args) {
    process.exitCode = await main(args);
    // A turn still running would keep the process alive; the session is over.
    process.exit();
  } else {
    console.log(USAGE);
  }
} catch (error) {
  if (error instanceof UsageError) {
    console.error(`${error.message}\n\n${USAGE}`);
    process.exitCode = 2;
  } else {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
