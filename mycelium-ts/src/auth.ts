/**
 * Gateway bearer-token plumbing shared by every client class.
 *
 * A Mycelium node with `gateway_auth_token` (or scoped tokens / OIDC) set answers every
 * `/gateway/*` route — and the node-level `/mcp`, `/signals/{kind}`, `/consensus/{slot}` —
 * with 401 unless the request carries `Authorization: Bearer <token>`. Each client takes a
 * `token` option; when omitted, `MYCELIUM_GATEWAY_TOKEN` from the environment is used (Node
 * only — in a browser there is no `process`, pass the token explicitly). No token → no header,
 * the open-gateway (loopback) deployment is unchanged.
 */

/** Environment variable consulted when no `token` option is given. */
export const TOKEN_ENV = "MYCELIUM_GATEWAY_TOKEN";

/** Explicit option wins; then the environment; empty strings count as "none". */
export function resolveToken(token?: string): string | undefined {
  if (token !== undefined) return token === "" ? undefined : token;
  const env = (globalThis as { process?: { env?: Record<string, string | undefined> } })
    .process?.env?.[TOKEN_ENV];
  return env ? env : undefined;
}

/** The headers to merge into every request: `{ Authorization: "Bearer …" }` or `{}`. */
export function authHeaders(token: string | undefined): Record<string, string> {
  return token ? { Authorization: `Bearer ${token}` } : {};
}

/** Constructor option carried by every client. */
export interface AuthOptions {
  /** Gateway bearer token; defaults to `MYCELIUM_GATEWAY_TOKEN` when unset. */
  token?: string;
}
