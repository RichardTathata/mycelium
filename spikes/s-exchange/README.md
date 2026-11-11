# Spike S-exchange — evidence (throwaway; never merges)

Keycloak `quay.io/keycloak/keycloak@sha256:4883630ef9db14031cde3e60700c9a9a8eaf1b5c24db1589d6a2d43de38ba2a9`
(26.2.5), two instances in Docker on 2026-11-11:

    docker run -d --name sx-kc-a -p 127.0.0.1:18080:8080 -e KC_BOOTSTRAP_ADMIN_USERNAME=admin \
      -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin <image> start-dev
    docker run -d --name sx-kc-b -p 127.0.0.1:18081:8080 -e KC_BOOTSTRAP_ADMIN_USERNAME=admin \
      -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin <image> start-dev --features=dynamic-scopes

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
