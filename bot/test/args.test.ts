import assert from "node:assert/strict";
import { test } from "node:test";

import { UsageError, profileMessage, readArgs } from "../src/args.ts";

test("the command line names a room, a model and a nickname", () => {
  assert.deepEqual(readArgs(["--room", "b", "--model", "anthropic/claude-haiku-5.5", "--name", "Claude"], {}), {
    room: 1,
    model: "anthropic/claude-haiku-5.5",
    name: "Claude",
    server: "http://127.0.0.1:3001",
    baseUrl: undefined,
  });
  const defaults = readArgs(["--room", "3", "--model", "m"], { PICTO_RELAY_URL: "http://ds.test:9" });
  assert.equal(defaults?.room, 3);
  assert.equal(defaults?.name, "fx");
  assert.equal(defaults?.server, "http://ds.test:9");
  assert.equal(readArgs(["--help"], {}), undefined);
});

test("a bad command line says what to fix", () => {
  for (const argv of [
    [],
    ["--room", "E", "--model", "m"],
    ["--room", "A"],
    ["--room", "A", "--model", "m", "--name", "elevenchars"],
    ["--room", "A", "--model", "m", "--server", "nonsense"],
    ["--room", "A", "--model", "m", "--color", "3"],
  ]) {
    assert.throws(() => readArgs(argv, {}), UsageError, argv.join(" "));
  }
});

test("the profile message is the model and the fx version, cut to fit a DS profile", () => {
  assert.equal(profileMessage("gpt-6-luna", "0.0.17"), "gpt-6-luna fx 0.0.17");
  assert.equal(profileMessage("anthropic/claude-haiku-5.5", "0.0.17"), "claude-haiku-5.5 fx 0.0.17");
  assert.equal(profileMessage("anthropic/claude-sonnet-5.5", "0.0.17"), "claude-sonnet-5.5 fx0.0.17");
  assert.equal(profileMessage("provider/" + "m".repeat(40), "0.0.17"), `${"m".repeat(17)} fx0.0.17`);
  for (const model of ["a/b", "anthropic/claude-sonnet-5.5", "x".repeat(60)]) {
    assert.ok(profileMessage(model, "10.20.30").length <= 26, model);
  }
});
