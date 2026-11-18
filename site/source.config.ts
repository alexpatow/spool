import { defineConfig } from "fumadocs-mdx/config";

export default defineConfig({
  mdxOptions: {
    // Code is highlighted when the site is built. Each token carries both
    // colours as CSS variables, and `globals.css` picks one by theme.
    rehypeCodeOptions: {
      themes: { light: "github-light", dark: "github-dark" },
      defaultColor: false,
      icon: false,
    },
  },
});
