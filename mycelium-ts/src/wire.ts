/**
 * Two rules for what this SDK puts on the wire, in one place (post-360 hardening, row G; the Python
 * SDK's `mycelium._pool.path_segment` / `whole_seconds` are the same rules).
 */

/**
 * `value` percent-encoded as **one** URL path segment. Every caller-supplied value that becomes a path
 * segment — a signal kind, a prompt's `ns`/`name`, a federation domain, a handle or guard id — goes
 * through here, so a `/` cannot add a segment and a `?` or `#` cannot start a query or fragment; the
 * gateway decodes the segment back.
 *
 * A value of `.` or `..` throws before any request: the URL parser resolves it as a dot segment — encoded
 * too (the WHATWG URL standard reads `%2e%2e` as `..`) — so the request would reach a different route.
 * No gateway route takes one as a name. Before 0.2.4 the prompt routes and the handle/guard ids were
 * interpolated raw.
 */
export function pathSegment(value: string): string {
  const text = String(value);
  if (text === "." || text === "..") {
    throw new Error(`${JSON.stringify(text)} cannot travel as a URL path segment: it is resolved as a dot segment`);
  }
  return encodeURIComponent(text);
}

/**
 * A timeout the gateway reads as `u64` seconds: a fraction rounds **up**, never below `floor` (default 1).
 * Sending `0.3` was refused with 422 before the request was looked at, and a fraction on `rpc/call` /
 * `scatter` became the route's default silently. The tuple `take` routes pass `floor` 0 — there `0` is the
 * documented poll.
 */
export function wholeSeconds(secs: number, floor = 1): number {
  return Math.max(floor, Math.ceil(secs));
}
