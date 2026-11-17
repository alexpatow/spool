import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// `bun run dev` serves the studio with reloading, and passes its calls on to
// a server that is running: `spool dev`, or the one `bun run demo` starts.
const server = process.env.SPOOL_URL ?? "http://127.0.0.1:7765";

export default defineConfig({
  plugins: [react()],
  // The files find each other wherever the server puts them.
  base: "./",
  server: {
    proxy: {
      "/v1": server,
      "/healthz": server,
    },
  },
  build: {
    // The server has three files to serve, under names that do not change.
    // It answers for each with a tag of its contents, so none needs a hash
    // in its name.
    rolldownOptions: {
      output: {
        entryFileNames: "studio.js",
        assetFileNames: "studio[extname]",
      },
    },
    modulePreload: false,
    assetsInlineLimit: 0,
    reportCompressedSize: false,
  },
});
