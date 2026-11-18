"use client";

import * as stylex from "@stylexjs/stylex";
import { useEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { CheckIcon, CopyIcon, FileTextIcon } from "@/components/ui/icons";
import { spacing } from "@/styles/shelf/tokens.stylex";

/**
 * For handing a page to an assistant: copy it as Markdown, or open the
 * Markdown. Both use the page's `.md` address, the one agents read.
 */
export function PageActions({ markdownUrl }: { markdownUrl: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);

  const copy = async () => {
    const text = fetch(markdownUrl).then((response) => {
      if (!response.ok) throw new Error(`${markdownUrl}: ${response.status}`);
      return response.text();
    });
    try {
      // Safari only allows a clipboard write that starts inside the click,
      // so it is given the pending text and not the text itself.
      if (typeof ClipboardItem !== "undefined" && navigator.clipboard.write) {
        const blob = text.then((value) => new Blob([value], { type: "text/plain" }));
        await navigator.clipboard.write([new ClipboardItem({ "text/plain": blob })]);
      } else {
        await navigator.clipboard.writeText(await text);
      }
      setCopied(true);
      clearTimeout(timer.current);
      timer.current = setTimeout(() => setCopied(false), 1500);
    } catch {
      // A refused write changes nothing; the link beside it still works.
    }
  };

  return (
    <div {...stylex.props(styles.row)}>
      <Button variant="outline" size="sm" onClick={copy}>
        {copied ? <CheckIcon /> : <CopyIcon />}
        {copied ? "Copied" : "Copy page"}
      </Button>
      <Button variant="outline" size="sm" nativeButton={false} render={<a href={markdownUrl} />}>
        <FileTextIcon />
        View as Markdown
      </Button>
    </div>
  );
}

const styles = stylex.create({
  row: {
    gap: spacing["2"],
    display: "flex",
    flexWrap: "wrap",
    marginTop: spacing["4"],
  },
});
