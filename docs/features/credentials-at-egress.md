# Credentials attached at egress

## What it is

A [credential binding](../guide/00-concepts.md#credential-binding) names a secret, the one host it is for (optionally
a port and a path prefix), the header form, and where the node reads its value once at `start()`. A caller holds only
the binding's placeholder, `mycelium-credential:<name>`, and puts it where the secret would go. The node's egress gate
swaps in the value only on a request to the bound host, refuses the request before a byte is sent anywhere else, and
drops the credential on a redirect the binding does not authorise. The value never appears in a log, a `Debug`, an
error or the evidence journal.

## When to use it, and when not

- **Use it when** a component, a served model or an activation command needs an API key, and you do not want the key
  in its unit file, its environment or its fields.
- **Not when** the secret is read by the component itself from its own environment or files, or travels in a query
  string or a body: those are outside the gate ([guide 9 § Credentials attached at egress](../guide/09-security.md#credentials-attached-at-egress)).

## Turn it on

Set the bindings in the configuration, in a build with `gateway`; a build without it refuses every binding at start.
Each bound host must be permitted by the [egress allow-list](egress-allow-list.md). No configuration file in the
repository that a test loads sets one; the tests below build bindings themselves.

- [`egress_credentials`](../reference/configuration.md#egress_credentials): `[[egress_credentials]]` entries (or
  `GOSSIP_EGRESS_CREDENTIALS`), validated at start: a name, a host, a port where the host is `localhost` or an IP
  literal, a form and one value source.
- [`credential_placeholder`](../../mycelium-core/src/config.rs): the placeholder a caller holds.
- [`EgressCredentials`](../../src/agent/egress_credentials.rs): the gate. `[[serve]]`'s `credential`,
  `OpenAiBackend::with_credentials` and an activation command's
  [egress proxy](activation-egress-proxy.md) send through it.
- `a_binding_attaches_its_value_at_its_host_in_place_of_the_placeholder`, in
  [`src/agent/egress_credentials_tests.rs`](../../src/agent/egress_credentials_tests.rs).

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for it. The
end-to-end run is a test in [`mycelium-wasm-host/src/stem.rs`](../../mycelium-wasm-host/src/stem.rs):
`a_served_skill_sends_its_binding_value_which_the_stem_never_held`. A redirect off the bound host dropping the
credential is `a_redirect_off_the_bound_host_drops_the_credential`, in
[`src/agent/egress_credentials_tests.rs`](../../src/agent/egress_credentials_tests.rs).

## What it guarantees, and what it does not

- **Guarantee:** [`egress.credentials`](../reference/guarantee-catalogue.md#egress.credentials) — attached only for the
  bound host, refused elsewhere, dropped on a redirect that leaves it, counted per binding.
- **Not built:** a secret a component or an unconfined activation command reads itself; a value an upstream echoes
  back transformed; a proxy the gate's own client is configured with; a placeholder in a query string or a body; and
  the credentials that keep their own paths — `[[serve]]`'s `api_key`, federation, object-store and git
  ([guide 9 § Credentials attached at egress](../guide/09-security.md#credentials-attached-at-egress)).
- **Evidence:** the ledger row
  [a credential is attached at egress only where its binding says](../operations/what-is-proven.md#credential-attached-egress-only-where-binding).

## Go deeper

- **Guide:** [09 § Credentials attached at egress](../guide/09-security.md#credentials-attached-at-egress).
- **Runbook:** [crown jewel § Credentials attached at egress](../operations/crown-jewel.md#credentials-attached-at-egress-post-360-plan-row-l-2350),
  with the table of where a node reads secrets.
- **Design:** no design record; row L of the [post-360 plan](../plans/post-360-hardening.md) is the record.
- **Code:** [`src/agent/egress_credentials.rs`](../../src/agent/egress_credentials.rs) and
  [`CredentialBinding`](../../mycelium-core/src/config.rs).
