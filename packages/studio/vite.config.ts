import stylex from "@stylexjs/unplugin/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import type { Plugin } from "vite";

// `bun run dev` serves the studio with reloading, and passes its calls on to
// a server that is running: `spool dev`, or the one `bun run demo` starts.
const server = process.env.SPOOL_URL ?? "http://127.0.0.1:7765";

/**
 * Gives the one style sheet the name the server serves it under. The plugin
 * that compiles the styles writes the sheet a second time, under a name the
 * bundler makes up to keep the two apart.
 */
function styleSheet(fileName: string): Plugin {
  return {
    name: "studio-style-sheet",
    enforce: "post",
    generateBundle(_options, bundle) {
      const sheets = Object.values(bundle).filter((file) => file.type === "asset" && file.fileName.endsWith(".css"));
      const sheet = sheets[0];
      if (sheets.length !== 1 || sheet?.type !== "asset") {
        this.error(`the studio has one style sheet, and the build made ${sheets.length}`);
      }
      const was = sheet.fileName;
      if (was === fileName) return;
      delete bundle[was];
      this.emitFile({ type: "asset", fileName, source: sheet.source });
      for (const file of Object.values(bundle)) {
        if (file.type === "asset" && typeof file.source === "string") file.source = file.source.replaceAll(was, fileName);
      }
    },
  };
}

export default defineConfig({
  // The styles of the components are written beside them and compiled into
  // the one style sheet, so the page needs no style of its own at run time.
  plugins: [stylex({ useCSSLayers: true }), react(), styleSheet("studio.css")],
  // A screen's address is a path, so the page names its files from the root.
  base: "/",
  server: {
    proxy: {
      "/v1": server,
      "/healthz": server,
    },
  },
  build: {
    // The server has a fixed set of files to serve, under names that do not
    // change. It answers for each with a tag of its contents, so none needs
    // a hash in its name.
    rolldownOptions: {
      output: {
        entryFileNames: "studio.js",
        assetFileNames: "[name][extname]",
      },
    },
    // The script is one file by design, and the server sends it compressed.
    chunkSizeWarningLimit: 1000,
    modulePreload: false,
    assetsInlineLimit: 0,
    reportCompressedSize: false,
  },
});
