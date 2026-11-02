import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
// @ts-expect-error type error without @types/node package
import process from "node:process";

const host = process.env.TAURI_DEV_HOST;
const debug = !!process.env.TAURI_ENV_DEBUG;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_"],
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    // WebView2 on Windows 10/11 is evergreen Chromium (>= 105 is the Tauri 2 floor).
    target: "chrome105",
    // Vite 8 default minifier (oxc) for JS, lightningcss for CSS. Size: see docs/SIZE.md.
    minify: !debug,
    cssMinify: !debug,
    sourcemap: debug,
    reportCompressedSize: false,
    // No modulepreload polyfill needed in WebView2.
    modulePreload: { polyfill: false },
    chunkSizeWarningLimit: 1200,
    rolldownOptions: {
      output: {
        // Keep the large, rarely-changing map libraries in their own chunks so the app chunk
        // stays small (faster parse when only app code changes; no size cost overall).
        codeSplitting: {
          groups: [
            { name: "maplibre", test: /node_modules[\/](\.pnpm[\/])?maplibre-gl/ },
            { name: "map-vendor", test: /node_modules[\/](\.pnpm[\/][^\/]+[\/]node_modules[\/])?(pmtiles|@protomaps|fflate)/ },
            { name: "react", test: /node_modules[\/](\.pnpm[\/][^\/]+[\/]node_modules[\/])?(react|react-dom|scheduler|zustand)[\/]/ },
          ],
        },
      },
    },
  },
}));
