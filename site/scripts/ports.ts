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
