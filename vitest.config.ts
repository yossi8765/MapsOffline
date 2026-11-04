import { defineConfig } from "vitest/config";

// Unit tests of the frontend's pure logic (no Tauri, no React Compiler). DOM tests opt into
// jsdom per file with `// @vitest-environment jsdom`.
export default defineConfig({
  test: {
    include: ["src/**/*.test.{ts,tsx}"],
    environment: "node",
  },
});
