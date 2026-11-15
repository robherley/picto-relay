import react from "@vitejs/plugin-react";
import { loadEnv } from "vite";
import { defineConfig } from "vitest/config";

// The Rust server embeds `dist/`. During development, run the server
// (`just server-memory`) and this dev server (`just web-dev`) side by side:
// API calls and WebSockets are proxied to the server. Set PICTO_RELAY_URL to
// reach a server that is not on the default address.
export default defineConfig(({ mode }) => {
  const server = loadEnv(mode, ".", "PICTO_").PICTO_RELAY_URL ?? "http://127.0.0.1:3001";
  return {
    plugins: [react()],
    server: { proxy: { "/api": { target: server, ws: true } } },
    build: { target: "es2022" },
    test: { environment: "node" },
  };
});
