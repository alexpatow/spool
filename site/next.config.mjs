import { createMDX } from "fumadocs-mdx/next";

/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,
  // Next.js would otherwise write an AGENTS.md of its own into this directory.
  agentRules: false,
};

export default createMDX()(config);
