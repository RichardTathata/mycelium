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
 * Throws ("cannot travel as a URL path segment") before any request for a value that cannot be one
 * segment: `.` or `..`, which the URL parser resolves as a dot segment — encoded too (the WHATWG URL
 * standard reads `%2e%2e` as `..`); the empty string, which reaches a different route (its 404 would read
 * as *not found*); and a string with a lone surrogate, which has no UTF-8 encoding. No gateway route takes
 * one as a name. Before 0.2.4 the prompt routes and the handle/guard ids were interpolated raw.
 */
export function pathSegment(value: string): string {
  const text = String(value);
  if (text === "" || text === "." || text === "..") {
    throw new Error(`${JSON.stringify(text)} cannot travel as a URL path segment: it would reach a different route`);
  }
  try {
    return encodeURIComponent(text);
  } catch {
    throw new Error(`${JSON.stringify(text)} cannot travel as a URL path segment: it has no UTF-8 encoding`);
  }
}

/** The largest value the gateway's `u64` seconds fields can read. */
const U64_MAX = 18446744073709551615n;

/**
 * A duration the gateway reads as `u64` seconds: a fraction rounds **up**, never below `floor` (default 1).
 * Sending `0.3` was refused with 422 before the request was looked at (`emitReliable`, the tuple `take`s,
 * `ingest`, a lock's `ttlSecs`), a fraction on `rpc/call` / `scatter` became the route's default silently,
 * and a fractional `leaseSecs` left an advertisement unleased. The tuple `take` routes pass `floor` 0 —
 * there `0` is the documented poll.
 *
 * Throws, naming `name`, for a value the gateway cannot read — negative, `NaN`, infinite, or past `u64` —
 * rather than mapping it to a default or a different duration.
 */
export function wholeSeconds(secs: number, floor = 1, name = "timeoutSecs"): number {
  if (typeof secs !== "number" || !Number.isFinite(secs) || secs < 0) {
    throw new Error(`${name} must be a finite, non-negative number of seconds, not ${String(secs)}`);
  }
  const whole = Math.max(floor, Math.ceil(secs));
  if (BigInt(whole) > U64_MAX) {
    throw new Error(`${name} ${String(secs)} is past the gateway's limit of ${U64_MAX} seconds`);
  }
  return whole;
}
