/**
 * The pages name two addresses: `first` for `spool dev`, and the port after it
 * for a second server. A run on other ports rewrites both, in one pass, so
 * that an address it has just written is never taken for one to rewrite.
 */
export function movePorts(text: string, from: number, to: number): string {
  if (from === to) return text;
  return text.replace(/127\.0\.0\.1:(\d+)(?!\d)/g, (address, digits: string) => {
    const port = Number(digits);
    if (port === from) return `127.0.0.1:${to}`;
    if (port === from + 1) return `127.0.0.1:${to + 1}`;
    return address;
  });
}

/**
 * A server that names no address listens on the pages' first port, whatever
 * port the run is on. On a run that has moved, each one is told the run's
 * port. That holds for a server the page starts to show it refused, such as a
 * second `spool dev` while the first has the port: left on the pages' port,
 * which is free on such a run, it would start and never end.
 */
export function listenOn(script: string, from: number, to: number): string {
  if (from === to) return script;
  return script
    .split("\n")
    .map((line) =>
      /^\s*spool (dev|serve)\b/.test(line) && !line.includes("--listen")
        ? `${line.trimEnd()} --listen 127.0.0.1:${to}`
        : line,
    )
    .join("\n");
}
