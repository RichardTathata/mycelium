/**
 * Lossless JSON for 64-bit integers (realignment repairs S1; the review's F06).
 *
 * The gateway sends HLCs, signal nonces and log cursors as JSON **numbers**. An HLC is
 * `(ms << 16) | logical` — about 1.2e17 today, above 2^53 — so `JSON.parse` rounds it to the nearest
 * double, and two HLCs one tick apart become the same value. The wire is not changed (the Python SDK
 * and every curl consumer read it correctly); this module reads and writes it without loss.
 *
 * `parseLossless` quotes every integer literal that is **not** a safe integer before handing the text
 * to `JSON.parse`, so such a value arrives as a decimal string and `BigInt(value)` is exact. A safe
 * integer stays a number, which `BigInt` also converts exactly. Strings are skipped with their escapes,
 * so a digit run inside a string is never touched. No dependency; Node ≥ 18.
 *
 * `stringifyLossless` writes a `bigint` as a bare JSON integer, which `JSON.stringify` refuses to do.
 */

/** Parse `text` as JSON, delivering every integer outside the safe range as a decimal string. */
export function parseLossless(text: string): unknown {
  let out = "";
  let i = 0;
  let copiedTo = 0;
  const n = text.length;
  while (i < n) {
    const c = text.charCodeAt(i);
    if (c === 34 /* " */) {
      // Skip a string literal, honouring backslash escapes.
      i++;
      while (i < n) {
        const d = text.charCodeAt(i);
        if (d === 92 /* \ */) i += 2;
        else if (d === 34) { i++; break; }
        else i++;
      }
      continue;
    }
    if (c === 45 /* - */ || (c >= 48 && c <= 57)) {
      const start = i;
      i++;
      let integer = true;
      while (i < n) {
        const d = text.charCodeAt(i);
        if (d >= 48 && d <= 57) { i++; continue; }
        if (d === 46 /* . */ || d === 101 /* e */ || d === 69 /* E */ || d === 43 /* + */ || d === 45 /* - */) {
          integer = false; i++; continue;
        }
        break;
      }
      if (integer) {
        const literal = text.slice(start, i);
        if (!Number.isSafeInteger(Number(literal))) {
          out += text.slice(copiedTo, start) + '"' + literal + '"';
          copiedTo = i;
        }
      }
      continue;
    }
    i++;
  }
  return JSON.parse(copiedTo === 0 ? text : out + text.slice(copiedTo));
}

const BIGINT_MARK = "\u0000bigint:";

/** `JSON.stringify` that writes a `bigint` as a bare JSON integer, exactly. */
export function stringifyLossless(value: unknown): string {
  const text = JSON.stringify(value, (_key, v) => (typeof v === "bigint" ? BIGINT_MARK + v.toString() : v));
  return text.replace(/"\\u0000bigint:(-?\d+)"/g, "$1");
}

/** A 64-bit value as the SDK receives it — a safe number or a decimal string — as a `bigint`. */
export function toBigInt(v: unknown): bigint {
  if (typeof v === "bigint") return v;
  if (typeof v === "number" || typeof v === "string") return BigInt(v);
  throw new TypeError(`expected a 64-bit integer, got ${typeof v}`);
}
