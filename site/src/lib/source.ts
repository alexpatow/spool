import { loader } from "fumadocs-core/source";
import { defineDocs } from "fumadocs-mdx/macro";

const docs = defineDocs({
  dir: "content/docs",
  // The Markdown agents read. Headings come without the `[#id]` suffix.
  docs: { postprocess: { includeProcessedMarkdown: { headingIds: false } } },
});

export const source = loader({ baseUrl: "/docs", source: docs.toFumadocsSource() });
