import { createMDX } from "fumadocs-mdx/next";

/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,
  // `packages/ui` is TypeScript with StyleX in it, compiled here like the
  // site's own files.
  transpilePackages: ["spool-ui"],
  // Next.js would otherwise write an AGENTS.md of its own into this directory.
  agentRules: false,
  // A docs address answers with HTML or Markdown by what the request accepts.
  async headers() {
    return [
      { source: "/docs", headers: [{ key: "Vary", value: "Accept" }] },
      { source: "/docs/:path*", headers: [{ key: "Vary", value: "Accept" }] },
    ];
  },
  // A short address to share. The page lives in the docs so that it has a
  // Markdown twin and a place in `llms.txt` like every other page.
  async redirects() {
    return [{ source: "/use-cases", destination: "/docs/use-cases", permanent: true }];
  },
  // Every page is also Markdown at its own address plus `.md`.
  async rewrites() {
    return [
      { source: "/docs.md", destination: "/llms.mdx/docs.md" },
      { source: "/docs/:path*.md", destination: "/llms.mdx/docs/:path*.md" },
    ];
  },
};

export default createMDX()(config);
