/**
 * Renders `spool.html` to the video the site's link previews play, `public/videos/spool.mp4`.
 *
 *   bun run render:video                               the video, over public/videos/spool.mp4
 *   bun video/render.ts --out spool.mp4                the video, anywhere
 *   bun video/render.ts --still 4.3=frame.png          one frame, as many as you like
 *   bun video/render.ts --serve                        serve it, to watch at /?play
 *
 * It starts headless Chrome, draws each frame with the page's `seek(t)`, takes
 * a screenshot of it, and pipes the screenshots to ffmpeg, so the timing is
 * exact whatever the machine. Needs Chrome (or `CHROME=/path/to/chrome`) and ffmpeg.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { site } from "../src/lib/site.ts";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const WIDTH = 1920;
const HEIGHT = 1080;

/* ------------------------------------------------------------------ */
/* What the page draws                                                 */
/* ------------------------------------------------------------------ */

const fonts = {
  "hanken.woff2": "@fontsource-variable/hanken-grotesk/files/hanken-grotesk-latin-wght-normal.woff2",
  "newsreader.woff2": "@fontsource-variable/newsreader/files/newsreader-latin-opsz-normal.woff2",
  "commit-mono-400.woff2": "@fontsource/commit-mono/files/commit-mono-latin-400-normal.woff2",
  "commit-mono-700.woff2": "@fontsource/commit-mono/files/commit-mono-latin-700-normal.woff2",
};

/** The mark and the wordmark, as `brand.tsx` draws them. */
function brand() {
  const source = readFileSync(join(here, "../src/components/brand.tsx"), "utf8");
  const constant = (name: string) => {
    const found = source.match(new RegExp(`const ${name} =\\s*((?:"[^"]*"\\s*\\+?\\s*)+);`));
    if (!found?.[1]) throw new Error(`brand.tsx: no ${name}`);
    return [...found[1].matchAll(/"([^"]*)"/g)].map((part) => part[1]).join("");
  };
  const letters = source.match(/<path d="(M[^"]+)" fill="currentColor"/)?.[1];
  if (!letters) throw new Error("brand.tsx: no wordmark letters");
  return { rules: constant("RULES"), threads: constant("THREADS"), letters };
}

function data(): string {
  const spool = { brand: brand(), headline: site.headline };
  const faces = [
    ["Hanken Grotesk Variable", "hanken.woff2", "100 900"],
    ["Newsreader Variable", "newsreader.woff2", "200 800"],
    ["Commit Mono", "commit-mono-400.woff2", "400"],
    ["Commit Mono", "commit-mono-700.woff2", "700"],
  ]
    .map(([family, file, weight]) => `@font-face{font-family:"${family}";src:url(/fonts/${file}) format("woff2");font-weight:${weight};font-display:block}`)
    .join("");
  return (
    `window.SPOOL = ${JSON.stringify(spool)};\n` +
    `(() => { const style = document.createElement("style"); style.textContent = ${JSON.stringify(faces)}; document.head.appendChild(style); })();\n`
  );
}

function serve(): Promise<{ server: Server; url: string }> {
  const server = createServer((request, response) => {
    const path = new URL(request.url ?? "/", "http://localhost").pathname;
    if (path === "/" || path === "/index.html") {
      response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      response.end(readFileSync(join(here, "spool.html")));
    } else if (path === "/data.js") {
      response.writeHead(200, { "content-type": "text/javascript; charset=utf-8" });
      response.end(data());
    } else if (path.startsWith("/fonts/") && path.slice(7) in fonts) {
      response.writeHead(200, { "content-type": "font/woff2" });
      response.end(readFileSync(require.resolve(fonts[path.slice(7) as keyof typeof fonts])));
    } else {
      response.writeHead(404).end();
    }
  });
  return new Promise((resolve) =>
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = typeof address === "object" && address ? address.port : 0;
      resolve({ server, url: `http://127.0.0.1:${port}/` });
    }),
  );
}

/* ------------------------------------------------------------------ */
/* Chrome, over the DevTools protocol                                  */
/* ------------------------------------------------------------------ */

function chromePath(): string {
  const candidates = [
    process.env.CHROME,
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/usr/bin/google-chrome",
    "/usr/local/bin/google-chrome",
    "/usr/bin/chromium",
  ];
  const found = candidates.find((path) => path && existsSync(path));
  if (!found) throw new Error("No Chrome found. Set CHROME to its path.");
  return found;
}

async function browser() {
  const profile = mkdtempSync(join(tmpdir(), "spool-video-"));
  const chrome = spawn(chromePath(), [
    "--headless=new",
    "--remote-debugging-port=0",
    `--user-data-dir=${profile}`,
    "--no-first-run",
    "--no-default-browser-check",
    "--hide-scrollbars",
    "--force-color-profile=srgb",
    "--force-device-scale-factor=1",
    `--window-size=${WIDTH},${HEIGHT}`,
    "about:blank",
  ]);
  const endpoint = await new Promise<string>((resolve, reject) => {
    let said = "";
    chrome.stderr.on("data", (chunk: Buffer) => {
      said += chunk.toString();
      const found = said.match(/DevTools listening on (ws:\/\/\S+)/);
      if (found?.[1]) resolve(found[1]);
    });
    chrome.on("exit", (code) => reject(new Error(`Chrome exited (${code}): ${said}`)));
  });

  const socket = new WebSocket(endpoint);
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  let next = 0;
  const waiting = new Map<number, { resolve: (value: any) => void; reject: (error: Error) => void }>();
  const listeners: ((method: string, params: any) => void)[] = [];
  socket.onmessage = (event) => {
    const message = JSON.parse(String(event.data));
    if (message.id !== undefined) {
      const pending = waiting.get(message.id);
      waiting.delete(message.id);
      if (message.error) pending?.reject(new Error(message.error.message));
      else pending?.resolve(message.result);
    } else {
      for (const listen of listeners) listen(message.method, message.params);
    }
  };
  const send = (method: string, params: object = {}, sessionId?: string): Promise<any> =>
    new Promise((resolve, reject) => {
      const id = ++next;
      waiting.set(id, { resolve, reject });
      socket.send(JSON.stringify({ id, method, params, sessionId }));
    });

  const { targetId } = await send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true });
  const page = (method: string, params: object = {}) => send(method, params, sessionId);
  await page("Page.enable");
  await page("Runtime.enable");
  await page("Emulation.setDeviceMetricsOverride", { width: WIDTH, height: HEIGHT, deviceScaleFactor: 1, mobile: false });
  listeners.push((method, params) => {
    if (method === "Runtime.exceptionThrown") console.error(params.exceptionDetails?.exception?.description ?? params);
  });

  return {
    async open(url: string) {
      await page("Page.navigate", { url });
      for (let tries = 0; tries < 200; tries++) {
        const { result } = await page("Runtime.evaluate", { expression: "JSON.stringify(window.timeline ?? null)", returnByValue: true });
        if (result.value && result.value !== "null") return JSON.parse(result.value) as { duration: number; fps: number };
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      throw new Error("The composition never got ready.");
    },
    async frame(t: number): Promise<Buffer> {
      await page("Runtime.evaluate", { expression: `seek(${t})`, awaitPromise: true });
      const { data } = await page("Page.captureScreenshot", { format: "png", fromSurface: true, optimizeForSpeed: true });
      return Buffer.from(data, "base64");
    },
    async close() {
      socket.close();
      const gone = new Promise((resolve) => chrome.once("exit", resolve));
      chrome.kill();
      await gone;
      // Chrome's helpers can still be writing to the profile; it is in tmp, so a leftover is harmless.
      try {
        rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
      } catch {}
    },
  };
}

/* ------------------------------------------------------------------ */
/* Main                                                                */
/* ------------------------------------------------------------------ */

const args = process.argv.slice(2);
const flag = (name: string) => {
  const at = args.indexOf(name);
  return at >= 0 ? args[at + 1] : undefined;
};
const stills = args.flatMap((arg, i) => (args[i - 1] === "--still" ? [arg] : []));
const out = flag("--out");

const { server, url } = await serve();
if (args.includes("--serve")) {
  console.log(`${url}?play`);
} else {
  const chrome = await browser();
  try {
    const { duration, fps } = await chrome.open(url);
    for (const still of stills) {
      const [at, path] = still.split("=");
      if (!at || !path) throw new Error(`--still wants <seconds>=<file.png>, not ${still}`);
      writeFileSync(path, await chrome.frame(Number(at)));
      console.log(`${path}: ${at}s`);
    }
    if (out) {
      const ffmpeg = spawn("ffmpeg", [
        "-y", "-loglevel", "error",
        "-f", "image2pipe", "-framerate", String(fps), "-c:v", "png", "-i", "-",
        "-c:v", "libx264", "-preset", "veryslow", "-tune", "animation", "-crf", "28",
        "-pix_fmt", "yuv420p", "-movflags", "+faststart", out,
      ], { stdio: ["pipe", "inherit", "inherit"] });
      const done = new Promise<void>((resolve, reject) =>
        ffmpeg.on("exit", (code) => (code === 0 ? resolve() : reject(new Error(`ffmpeg exited ${code}`)))),
      );
      const frames = Math.round(duration * fps);
      for (let i = 0; i < frames; i++) {
        const png = await chrome.frame(i / fps);
        if (!ffmpeg.stdin.write(png)) await new Promise((resolve) => ffmpeg.stdin.once("drain", resolve));
        if (i % fps === 0) process.stdout.write(`\r${(i / fps).toFixed(0)}s of ${duration.toFixed(1)}s`);
      }
      ffmpeg.stdin.end();
      await done;
      console.log(`\r${out}: ${frames} frames, ${duration.toFixed(1)}s`);
    }
  } finally {
    await chrome.close();
    server.close();
  }
}
