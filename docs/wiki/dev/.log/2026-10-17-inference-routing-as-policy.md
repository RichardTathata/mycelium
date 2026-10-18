## [2026-10-17] ingest | Inference routing as policy (post-360 row R)

The user decided §5's question a row. A routing door now tells the gateway's preflight the destination it is deciding on
(name and `Locality` — the provider's own `llm-meta` declaration, `unknown` when none) and the request's data class (a
principal floor, a client declaration that may only raise it, otherwise the most restrictive class), and the evidence
names both with the candidates refused before. `mycelium-reason` 0.10.0's router runs the preflight per candidate and
dispatches to the first permitted, so *confidential stays in the domain* routes rather than refuses when an in-domain
model exists. Every door a model call leaves by was enumerated (the plan's R block): the façade is covered, the
substrate's `/gateway/llm/*` doors and provider enforcement fail closed under a class rule (neither establishes a
class), the node's in-process calls and a provider's backend are not covered. Declared, never detected. The
independent review found the doors that *establish* the facts: a `cap:write` client could declare any model in-domain
through `capability/advertise` / `units/declare` — now refused, the whole `llm-meta` namespace after round 1
(`reserved_namespace`); round 1 also found `rpc/serve` handing a client every `llm.invoke` (a kind in the URL path,
which the enumeration greps missed — now refused for the node-served kinds), the default class not a floor (now it
is), and the provisioner as a third writer of `llm-meta` (closed); round 2 found the sibling of the `rpc/serve`
finding — any kind the node serves in process, e.g. a hosted component's — closed at the root in the handler table
(`register_client`: refused beside an in-process receiver, evicted when one registers), and a classifying router that
routed before its evaluator was attached (now refused until one is); a member writing another
node's ad stays open (an ad's key is not bound to its writer). A class rule names one class, so policies are
written as allow-lists.

Pages: `dev/dev.md`, `dev/companions/companions.md`. Repo: `src/agent/route_facts.rs`, guide 20 § Deciding where data
may go, `00-concepts.md`, guide 15's `llm-meta` table, `deprecations.md` §50, the plan's row R and §5.
