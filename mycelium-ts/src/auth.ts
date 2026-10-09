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

/** The URL schemes a client accepts. */
export type Scheme = "http" | "https";

/**
 * `${scheme}://${host}:${port}` — the one place every client builds its base URL. `https` reaches a
 * gateway serving TLS (`gateway_tls`, or a TLS-terminating proxy in front of it); `http` is the
 * default, so an existing loopback deployment is unchanged. Before 0.2.3 every client hard-coded
 * `http://`, so a gateway serving HTTPS was unreachable from this SDK and the bearer travelled in
 * cleartext off loopback.
 *
 * Certificate verification is Node's own and stays on: there is no option here to turn it off. A
 * private fleet CA (the node-cert mode of `gateway_tls` serves the cluster CA's `ca-cert.pem`) is
 * trusted by starting the process with `NODE_EXTRA_CA_CERTS=/path/to/ca-cert.pem` — the built-in
 * `fetch` takes no per-call CA option without a dependency on `undici`, which this SDK does not add.
 */
export function baseUrl(host: string, port: number, scheme: Scheme = "http"): string {
  if (scheme !== "http" && scheme !== "https") {
    throw new Error(`scheme must be "http" or "https", not ${JSON.stringify(scheme)}`);
  }
  return `${scheme}://${host}:${port}`;
}

/** Constructor options carried by every client. */
export interface AuthOptions {
  /** Gateway bearer token; defaults to `MYCELIUM_GATEWAY_TOKEN` when unset. */
  token?: string;
  /** `"http"` (default) or `"https"` for a gateway serving TLS. See {@link baseUrl} for the CA. */
  scheme?: Scheme;
}
