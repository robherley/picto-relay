import { defineConfig } from "@hey-api/openapi-ts";

// Generates the typed API client from the document the Rust server produces.
export default defineConfig({
  input: "../docs/openapi.json",
  output: "src/api/generated",
});
