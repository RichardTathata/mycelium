# Spike S-exchange — evidence (throwaway; never merges)

## Part 1 — binding a per-call value (Keycloak 26.2.5)

    IMG=quay.io/keycloak/keycloak@sha256:4883630ef9db14031cde3e60700c9a9a8eaf1b5c24db1589d6a2d43de38ba2a9
    docker run -d --name sx-kc-a -p 127.0.0.1:18080:8080 -e KC_BOOTSTRAP_ADMIN_USERNAME=admin \
      -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin $IMG start-dev
    docker run -d --name sx-kc-b -p 127.0.0.1:18081:8080 -e KC_BOOTSTRAP_ADMIN_USERNAME=admin \
      -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin $IMG start-dev --features=dynamic-scopes

`kc-a-26.2.5.log` and `kc-b-26.2.5.log` are the two containers' whole logs (`docker logs`); B's carries
`Experimental features enabled: dynamic-scopes:v1`. The `run-*.jsonl` files are the probes' raw output, the exchanged
token included on its `TOKEN` line.

Then, in order (stdlib Python 3.13, `python3 -I`):

    probe.py     http://127.0.0.1:18080            > run-a.jsonl    # realm, clients, baseline, four binding attempts
    probe_lib.py http://127.0.0.1:18080            > run-a2.jsonl   # the request-reading mappers attached
    probe.py     http://127.0.0.1:18081 dynamic    > run-b.jsonl
    probe3.py    http://127.0.0.1:18081 dynamic    > run-b3.jsonl   # dynamic scope also optional on the user's client
    probe4.py    http://127.0.0.1:18081 dynamic    > run-b4.jsonl   # per-call values, subject token without one
    probe5.py    http://127.0.0.1:18080            > run-a5.jsonl   # latency
    probe5.py    http://127.0.0.1:18081 dynamic    > run-b5.jsonl

`envelope-size.txt` is `spike_s_exchange_envelope_size` (src/agent/client_grant_tests.rs on this branch), run with
`cargo test --lib --features compliance,a2a -- spike_s_exchange_envelope_size --nocapture`.

## Part 2 — sender-constrained tokens (Keycloak 26.8.2, the current release on 2026-11-11)

`part2/harness.py` holds the image digest itself
(`quay.io/keycloak/keycloak@sha256:723b1d6e254c0fc96d5cffec860c7e0744d6a3277822b865bb538cf891575e41`), makes the throwaway
certificates (the `.crt` files here; the keys are not committed), starts Keycloak over HTTPS with client certificates
requested, runs every probe twice (default features, then `--features=dpop`), and stops and removes its container on
every exit path:

    python3 -I part2/harness.py OUTDIR     # needs Docker, openssl 3 and the `cryptography` package

`results.jsonl` is its raw output (tokens included), `stdout.txt` the same stream, `default.log` and `dpop_feature.log`
the container's whole log for each phase, and `serverinfo-default.json` the server's `/admin/serverinfo` — its
`features` table is where each feature's type (`DEFAULT`, `PREVIEW`, `EXPERIMENTAL`) and state are read.
