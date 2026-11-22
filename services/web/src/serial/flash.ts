import type { Device } from "../api/transport";

// Flashes the ESP32 from the page, with Espressif's esptool-js. The firmware
// is served beside the page: `public/firmware/`, filled by `npm run firmware`
// from a firmware build.

/** What `public/firmware/manifest.json` says of the firmware on offer. */
export interface Firmware {
  /** The git revision it was built from. */
  build: string;
  /** The images, and where in flash each goes. */
  parts: { path: string; address: number }[];
}

/** Speed the images are sent at, once the bootloader is listening. */
const FLASH_BAUD = 460_800;
/** How long the reset line is held, as esptool holds it. */
const RESET_MS = 100;

/** The firmware this page can flash, if it was built with any. */
export async function firmwareOnOffer(): Promise<Firmware | undefined> {
  try {
    const response = await fetch("/firmware/manifest.json");
    if (!response.ok) return undefined;
    return (await response.json()) as Firmware;
  } catch {
    return undefined;
  }
}

/**
 * Asks which port the ESP32 is on and writes the firmware to it. Call from a
 * click: the browser only shows its list of ports in answer to one.
 * `onProgress` is told how far along the writing is, from 0 to 1.
 */
export async function flash(
  device: Device,
  firmware: Firmware,
  onProgress: (fraction: number) => void,
): Promise<void> {
  const port = await navigator.serial.requestPort();
  const images = await Promise.all(
    firmware.parts.map(async ({ path, address }) => {
      const response = await fetch(`/firmware/${path}`);
      if (!response.ok) throw new Error(`The firmware image ${path} is missing.`);
      return { address, data: new Uint8Array(await response.arrayBuffer()) };
    }),
  );
  const total = images.reduce((sum, { data }) => sum + data.length, 0);
  const before = images.map((_, index) => images.slice(0, index).reduce((sum, { data }) => sum + data.length, 0));

  // Only a page built to drive the ESP32 carries the flasher.
  const esptool = import.meta.env.VITE_TRANSPORT === "serial" ? await import("esptool-js") : undefined;
  if (!esptool) throw new Error("This page was not built to flash.");
  const { ESPLoader, Transport } = esptool;
  await device.release();
  const transport = new Transport(port, false);
  try {
    const loader = new ESPLoader({ transport, baudrate: FLASH_BAUD });
    // Resets the board into its bootloader and loads the flasher into its memory.
    await loader.main();
    await loader.writeFlash({
      fileArray: images,
      // What the images were built for is already in them.
      flashMode: "keep",
      flashFreq: "keep",
      flashSize: "keep",
      eraseAll: false,
      compress: true,
      reportProgress: (index, written, size) => {
        onProgress((before[index]! + (written / size) * images[index]!.data.length) / total);
      },
    });
    // Restarts the board into the firmware just written: the reset line is
    // pulled, with the boot-mode line left alone, and let go. esptool-js's own
    // hard reset only lets go of a line it never pulled, which leaves the
    // board in its bootloader.
    await transport.setSignals(false, true);
    await new Promise((resolve) => setTimeout(resolve, RESET_MS));
    await transport.setSignals(false, false);
  } finally {
    await transport.disconnect().catch(() => {});
  }
  device.reopen(port);
}
