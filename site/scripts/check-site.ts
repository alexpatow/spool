/**
 * Checks the pages as written and the site as built.
 *
 *   bun run check:site            after `bun run build`
 *   bun run check:site --source   only the checks that need no build
 *
 * As written, in `content/docs`:
 *   - every page has a title and a description, and is listed in a `meta.json`
 *   - a page is Markdown only: no imports, exports or components, so that its
 *     `.md` says what its HTML says
 *   - no em dashes, in the pages or in the site's own source
 *
 * As built, by starting the built site and asking it:
 *   - every page answers, as HTML, as `.md`, and as Markdown when a client
 *     asks for `text/markdown`
 *   - every link to a page of this site resolves, and so does its `#fragment`
 *   - `/llms.txt` lists every page, and `/llms-full.txt` holds every page
 *   - `/sitemap.xml` lists every page
 */
import { spawn } from "bun";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { createServer } from "node:net";
import { join, relative, resolve } from "node:path";

const siteDir = resolve(import.meta.dir, "..");
const docsDir = join(siteDir, "content/docs");
const sourceOnly = process.argv.includes("--source");

const failures: string[] = [];
const fail = (where: string, message: string) => failures.push(`${where}: ${message}`);

function walk(dir: string, keep: (path: string) => boolean): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return walk(path, keep);
    return keep(path) ? [path] : [];
  });
}

/** The address of a page, from its file: `concepts/matching.mdx` is `/docs/concepts/matching`. */
function urlOf(file: string): string {
  const path = relative(docsDir, file)
    .replace(/\.mdx$/, "")
    .replace(/(^|\/)index$/, "");
  return path === "" ? "/docs" : `/docs/${path}`;
}

function listed(dir: string): Set<string> {
  const meta = join(dir, "meta.json");
  if (!existsSync(meta)) return new Set();
  const { pages } = JSON.parse(readFileSync(meta, "utf8")) as { pages: string[] };
  const found = new Set<string>();
  for (const entry of pages) {
    if (entry.startsWith("---")) continue;
    if (entry.startsWith("...")) {
      for (const file of listed(join(dir, entry.slice(3)))) found.add(file);
      continue;
    }
    found.add(join(dir, `${entry}.mdx`));
  }
  return found;
}

interface Page {
  file: string;
  url: string;
  title: string;
}

function checkSource(): Page[] {
  const files = walk(docsDir, (path) => path.endsWith(".mdx"));
  const inMeta = listed(docsDir);
  const pages: Page[] = [];

  for (const file of inMeta) {
    if (!existsSync(file)) fail(relative(siteDir, file), "listed in a meta.json, and there is no such file");
  }

  for (const file of files) {
    const name = relative(siteDir, file);
    const text = readFileSync(file, "utf8");
    if (!inMeta.has(file)) fail(name, "not listed in a meta.json, so it is in no sidebar");

    const front = /^---\n([\s\S]*?)\n---\n/.exec(text);
    const field = (key: string) =>
      new RegExp(`^${key}:\\s*(.+)$`, "m").exec(front?.[1] ?? "")?.[1]?.trim() ?? "";
    const title = field("title");
    if (!title) fail(name, "no title in its front matter");
    if (!field("description")) fail(name, "no description in its front matter");
    pages.push({ file, url: urlOf(file), title });

    let fenced = false;
    text
      .slice(front?.[0].length ?? 0)
      .split("\n")
      .forEach((line, index) => {
        if (/^\s*(```|~~~)/.test(line)) fenced = !fenced;
        if (fenced) return;
        const where = `${name}:${index + 1 + (front?.[0].split("\n").length ?? 1) - 1}`;
        if (/^(import|export)\s/.test(line)) fail(where, "a page is Markdown only: no import or export");
        if (/^\s*<\/?[A-Za-z]/.test(line.replace(/`[^`]*`/g, ""))) {
          fail(where, "a page is Markdown only: no components or HTML");
        }
      });
  }

  const written = [
    ...files,
    ...walk(join(siteDir, "src"), (path) => /\.(tsx?|css)$/.test(path)),
    ...walk(join(siteDir, "scripts"), (path) => path.endsWith(".ts")),
  ];
  for (const file of written) {
    readFileSync(file, "utf8")
      .split("\n")
      .forEach((line, index) => {
        if (line.includes("\u2014")) fail(`${relative(siteDir, file)}:${index + 1}`, "an em dash");
      });
  }

  return pages;
}

async function freePort(): Promise<number> {
  return new Promise((done, refuse) => {
    const server = createServer();
    server.once("error", refuse);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = typeof address === "object" && address ? address.port : 0;
      server.close(() => done(port));
    });
  });
}

/** The `id`s a page of HTML has, which are what a `#fragment` can name. */
function idsOf(html: string): Set<string> {
  return new Set([...html.matchAll(/\sid="([^"]+)"/g)].map((match) => match[1] ?? ""));
}

function linksOf(html: string): string[] {
  return [...html.matchAll(/<a\s[^>]*?href="([^"]+)"/g)].map((match) =>
    (match[1] ?? "").replaceAll("&amp;", "&"),
  );
}

async function checkBuilt(pages: Page[]) {
  if (!existsSync(join(siteDir, ".next/BUILD_ID"))) {
    fail("site", "not built: run `bun run build` first, or pass --source");
    return;
  }

  const port = await freePort();
  const origin = `http://127.0.0.1:${port}`;
  const next = join(siteDir, "node_modules/.bin/next");
  const server = spawn([next, "start", "-p", String(port), "-H", "127.0.0.1"], {
    cwd: siteDir,
    env: { ...process.env, PORT: String(port) },
    stdout: "ignore",
    stderr: "inherit",
  });

  try {
    const deadline = Date.now() + 30000;
    for (;;) {
      try {
        if ((await fetch(`${origin}/llms.txt`)).ok) break;
      } catch {
        // Not listening yet.
      }
      if (Date.now() > deadline || server.exitCode !== null) {
        fail("site", "the built site did not start");
        return;
      }
      await Bun.sleep(200);
    }

    const cache = new Map<string, Promise<{ status: number; type: string; body: string }>>();
    const get = (path: string, headers?: Record<string, string>) => {
      const key = `${path} ${JSON.stringify(headers ?? {})}`;
      let answer = cache.get(key);
      if (!answer) {
        answer = fetch(`${origin}${path}`, { ...(headers && { headers }), redirect: "manual" }).then(
          async (response) => ({
            status: response.status,
            type: response.headers.get("content-type") ?? "",
            body: await response.text(),
          }),
        );
        cache.set(key, answer);
      }
      return answer;
    };

    const llms = await get("/llms.txt");
    const full = await get("/llms-full.txt");
    const sitemap = await get("/sitemap.xml");
    if (llms.status !== 200 || !llms.type.startsWith("text/plain")) {
      fail("/llms.txt", `answered ${llms.status} ${llms.type}`);
    }
    if (!llms.body.startsWith("# ")) fail("/llms.txt", "does not start with a title");
    if (!/^> .+/m.test(llms.body)) fail("/llms.txt", "has no summary line");
    if (full.status !== 200 || !full.type.startsWith("text/plain")) {
      fail("/llms-full.txt", `answered ${full.status} ${full.type}`);
    }

    // The addresses in these files are absolute, on the origin the site was built for.
    const pathOf = (address: string) => address.replace(/^https?:\/\/[^/]+/, "");
    const inIndex = new Set(
      [...llms.body.matchAll(/\]\((\S+?)\)/g)].map((match) => pathOf(match[1] ?? "")),
    );
    const inSitemap = new Set(
      [...sitemap.body.matchAll(/<loc>([^<]+)<\/loc>/g)].map((match) => pathOf(match[1] ?? "")),
    );
    for (const path of inIndex) {
      if ((await get(path)).status !== 200) fail("/llms.txt", `links to ${path}, which does not answer`);
    }

    for (const page of pages) {
      const html = await get(page.url);
      if (html.status !== 200 || !html.type.startsWith("text/html")) {
        fail(page.url, `answered ${html.status} ${html.type}`);
        continue;
      }
      if (!html.body.includes(`type="text/markdown"`)) {
        fail(page.url, "does not name its Markdown in a <link rel=\"alternate\">");
      }

      const markdown = await get(`${page.url}.md`);
      if (markdown.status !== 200 || !markdown.type.startsWith("text/markdown")) {
        fail(`${page.url}.md`, `answered ${markdown.status} ${markdown.type}`);
      } else {
        const front = /^---\n([\s\S]*?)\n---\n/.exec(markdown.body)?.[1] ?? "";
        for (const key of ["title", "description", "url", "docs_index"]) {
          if (!new RegExp(`^${key}: \\S`, "m").test(front)) {
            fail(`${page.url}.md`, `no \`${key}\` in its front matter`);
          }
        }
        if (!markdown.body.includes(`\n# ${page.title}\n`)) {
          fail(`${page.url}.md`, "does not have the page's title as its heading");
        }
        if (/<[A-Z][A-Za-z]*[\s/>]/.test(markdown.body.replace(/```[\s\S]*?```/g, ""))) {
          fail(`${page.url}.md`, "has a component left in it");
        }
        const body = markdown.body.replace(/^---\n[\s\S]*?\n---\n\n/, "").trim();
        if (!full.body.includes(body)) fail("/llms-full.txt", `does not hold ${page.url}`);
      }

      const negotiated = await get(page.url, { accept: "text/markdown" });
      if (negotiated.body !== markdown.body) {
        fail(page.url, "asked for as text/markdown, does not answer with its Markdown");
      }

      if (!inIndex.has(`${page.url}.md`)) fail("/llms.txt", `does not list ${page.url}`);
      if (!inSitemap.has(page.url)) fail("/sitemap.xml", `does not list ${page.url}`);
    }

    // Links, from every page and from the home page.
    for (const from of ["/", ...pages.map((page) => page.url)]) {
      const html = await get(from);
      for (const href of new Set(linksOf(html.body))) {
        if (!href.startsWith("/") && !href.startsWith("#")) continue;
        const [path = "", fragment] = href.split("#");
        const target = await get(path === "" ? from : path);
        if (target.status !== 200) {
          fail(from, `links to ${href}, which answered ${target.status}`);
          continue;
        }
        if (fragment && !idsOf(target.body).has(decodeURIComponent(fragment))) {
          fail(from, `links to ${href}, and that page has nothing with that id`);
        }
      }
    }
  } finally {
    server.kill();
    await server.exited;
  }
}

const pages = checkSource();
if (!sourceOnly) await checkBuilt(pages);

if (failures.length > 0) {
  console.error(`${failures.length} failed:\n`);
  for (const failure of failures) console.error(`  ${failure}`);
  process.exit(1);
}
console.log(
  sourceOnly
    ? `${pages.length} pages checked as written`
    : `${pages.length} pages checked as written and as built`,
);
