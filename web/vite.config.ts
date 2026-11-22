import react from "@vitejs/plugin-react";
import { loadEnv } from "vite";
import { defineConfig } from "vitest/config";

// The client is built in one of two modes (see src/api/transport.ts):
//
// - The default talks to the relay server, which embeds `dist/`. During
//   development, run the server (`just server-memory`) and this dev server
//   (`just web-dev`) side by side: API calls and WebSockets are proxied to the
//   server. Set PICTO_RELAY_URL to reach one that is not on the default
//   address.
// - `--mode serial` needs no server: the page drives the ESP32 itself, over
//   Web Serial (`just demo`).
export default defineConfig(({ mode }) => {
  const server = loadEnv(mode, ".", "PICTO_").PICTO_RELAY_URL ?? "http://127.0.0.1:3001";
  const serial = mode === "serial";
  return {
    plugins: [react()],
    define: { "import.meta.env.VITE_TRANSPORT": JSON.stringify(serial ? "serial" : "relay") },
    // `public/` holds the firmware the page can flash: not for the relay server to carry.
    publicDir: serial ? "public" : false,
    server: {
      proxy: serial ? undefined : { "/api": { target: server, ws: true } },
      // The glyph table is imported from the server crate, outside this folder.
      fs: { allow: [".."] },
    },
    worker: { format: "es" },
    build: { target: "es2022" },
    test: { environment: "node" },
  };
});
