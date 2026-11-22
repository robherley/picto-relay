// Copies a firmware build into public/firmware/, with the manifest the page's
// flasher reads. Without a build there is nothing to copy, and the page
// offers no flashing: run `just firmware` first to have it.
import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";

const build = new URL("../../../firmware/esp32/build/", import.meta.url).pathname;
const output = new URL("../public/firmware/", import.meta.url).pathname;

rmSync(output, { recursive: true, force: true });
const layout = join(build, "flasher_args.json");
if (!existsSync(layout)) {
  console.log("No firmware build found (firmware/esp32/build): the page will not offer to flash.");
  process.exit(0);
}

/** The revision the firmware reports in its hello, as the build recorded it. */
function revision() {
  try {
    const description = JSON.parse(readFileSync(join(build, "project_description.json"), "utf8"));
    if (description.project_version) return String(description.project_version);
  } catch {
    // Fall through to asking git.
  }
  return execFileSync("git", ["describe", "--always", "--dirty"], { encoding: "utf8" }).trim();
}

mkdirSync(output, { recursive: true });
const { flash_files: files } = JSON.parse(readFileSync(layout, "utf8"));
const parts = Object.entries(files)
  .map(([address, path]) => ({ address: Number(address), path: basename(path), source: join(build, path) }))
  .sort((a, b) => a.address - b.address);
for (const { source, path } of parts) copyFileSync(source, join(output, path));
const manifest = { build: revision(), parts: parts.map(({ path, address }) => ({ path, address })) };
writeFileSync(join(output, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`Firmware ${manifest.build}: ${parts.map(({ path }) => path).join(", ")}`);
