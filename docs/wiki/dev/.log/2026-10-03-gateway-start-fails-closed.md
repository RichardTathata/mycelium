## [2026-10-03] ingest | a gateway that cannot come up is start()'s error

**What:** `http::prepare_gateway` (bind + TLS material) runs in `start()`; `run_http_server` takes the
prepared listener. A busy port or an unreadable `[gateway_tls]` path refuses the start, naming the gateway.

**Durable knowledge:** "spawn a task that binds" is the shape that turns an operator's configuration error
into a log line behind a ready node — the same family as the token table the build could not enforce
(v2.18.1): accepted, inert, reported fine. The rule: everything a task needs that can fail on configuration
is resolved before the task is spawned, in the function whose error the caller sees.
