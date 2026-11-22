import { parseArgs } from "node:util";

import { NAME_MAX_UNITS, PROFILE_MESSAGE_MAX_UNITS, ROOM_LETTERS } from "./relay.ts";

export const USAGE = `Hosts a model in a PictoChat room, through a running picto-relay server.

Usage: just bot --room <A-D> --model <id> [--name <nickname>]

  --room <A-D>       Room to join. With the ESP32, a DS must already be in it.
  --model <id>       Model to host, as AI Gateway names it, such as
                     anthropic/claude-haiku-5.5. It must accept images.
  --name <nickname>  The model's nickname in the room, up to ${NAME_MAX_UNITS} characters.
                     Default: fx
  --server <url>     The relay server. Default: $PICTO_RELAY_URL, else
                     http://127.0.0.1:3001
  --base-url <url>   Use an OpenAI-compatible server (a local model, say)
                     instead of AI Gateway. Its key, if it needs one, is read
                     from $MODEL_API_KEY.
  --help             Show this.

AI Gateway needs a credential: link this repository to a Vercel project (vercel link),
or put an AI Gateway key in $AI_GATEWAY_API_KEY.`;

export interface Args {
  /** Room number, 0 for A. */
  room: number;
  model: string;
  name: string;
  server: string;
  baseUrl: string | undefined;
}

/** What is wrong with the command line, for showing with the usage. */
export class UsageError extends Error {}

/**
 * Reads the command line. Returns nothing when asked for help.
 * `env` supplies the defaults a flag overrides.
 */
export function readArgs(argv: string[], env: NodeJS.ProcessEnv): Args | undefined {
  let values;
  try {
    ({ values } = parseArgs({
      args: argv,
      options: {
        room: { type: "string" },
        model: { type: "string" },
        name: { type: "string", default: "fx" },
        server: { type: "string", default: env.PICTO_RELAY_URL ?? "http://127.0.0.1:3001" },
        "base-url": { type: "string" },
        help: { type: "boolean", default: false },
      },
    }));
  } catch (error) {
    throw new UsageError(error instanceof Error ? error.message : String(error));
  }
  if (values.help) return undefined;

  if (!values.room) throw new UsageError("Say which room to join: --room A, B, C or D");
  // A room is its letter; its number, as the API counts them, is accepted too.
  const room = /^[0-3]$/.test(values.room)
    ? Number(values.room)
    : ROOM_LETTERS.indexOf(values.room.toUpperCase() as (typeof ROOM_LETTERS)[number]);
  if (room < 0) throw new UsageError(`There is no room "${values.room}": rooms are A, B, C and D`);

  const model = values.model?.trim();
  if (!model) throw new UsageError("Say which model to host: --model <id>");

  const name = values.name.trim();
  if (name === "" || name.length > NAME_MAX_UNITS) {
    throw new UsageError(`A nickname is 1 to ${NAME_MAX_UNITS} characters`);
  }

  for (const [flag, url] of [
    ["--server", values.server],
    ["--base-url", values["base-url"]],
  ] as const) {
    if (url !== undefined && !URL.canParse(url)) throw new UsageError(`${flag} is not a URL: ${url}`);
  }

  return { room, model, name, server: values.server, baseUrl: values["base-url"] };
}

/**
 * The model's name and the version of fx hosting it, as a DS profile message.
 * That is short, so this is the longest form that fits: the whole model id,
 * else the id without its provider, else that with the version closed up,
 * else as much of the name as leaves room for the version.
 */
export function profileMessage(model: string, fxVersion: string): string {
  const name = model.slice(model.lastIndexOf("/") + 1);
  const tight = ` fx${fxVersion}`;
  const fitting = [`${model} fx ${fxVersion}`, `${name} fx ${fxVersion}`, `${name}${tight}`].find(
    (message) => message.length <= PROFILE_MESSAGE_MAX_UNITS,
  );
  return fitting ?? `${name.slice(0, PROFILE_MESSAGE_MAX_UNITS - tight.length)}${tight}`;
}
