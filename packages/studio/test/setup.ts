// Gives the tests a document to render the studio into, and compiles the
// studio's styles as its build does.
//
// The studio is tested against the real server, so the calls it makes go out
// with the `fetch` of the runtime, which is kept here: the one a simulated
// browser brings applies the rules of a page, such as those about origins,
// to a test that is not one.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { transformSync } from "@babel/core";
import type { PluginItem } from "@babel/core";
import jsx from "@babel/plugin-syntax-jsx";
import typescript from "@babel/plugin-syntax-typescript";
import { GlobalRegistrator } from "@happy-dom/global-registrator";
import stylex from "@stylexjs/babel-plugin";
import { plugin } from "bun";

// The styles of the studio are written beside its components and compiled
// away when it is built. The tests load the source, so they compile them the
// same way: a component gets the class names it would have in the build.
const root = join(dirname(fileURLToPath(import.meta.url)), "..");
plugin({
  name: "stylex",
  setup(build) {
    build.onLoad({ filter: /\/packages\/studio\/src\/.*\.tsx?$/ }, ({ path }) => {
      const source = readFileSync(path, "utf8");
      const loader = path.endsWith("x") ? "tsx" : "ts";
      if (!source.includes("@stylexjs/stylex")) return { contents: source, loader };
      const compiled = transformSync(source, {
        filename: path,
        babelrc: false,
        configFile: false,
        plugins: [
          [typescript, { isTSX: path.endsWith("x") }],
          jsx,
          [
            stylex,
            { dev: false, runtimeInjection: false, unstable_moduleResolution: { type: "commonJS", rootDir: root } },
          ],
        ] as PluginItem[],
      });
      if (!compiled?.code) throw new Error(`the styles of ${path} did not compile`);
      return { contents: compiled.code, loader };
    });
  },
});

const kept = {
  fetch: globalThis.fetch,
  Request: globalThis.Request,
  Response: globalThis.Response,
  Headers: globalThis.Headers,
  AbortController: globalThis.AbortController,
  AbortSignal: globalThis.AbortSignal,
};

GlobalRegistrator.register({ url: "http://studio.test/" });
Object.assign(globalThis, kept);

// Tells React that what the tests do to the page is to be waited for.
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
