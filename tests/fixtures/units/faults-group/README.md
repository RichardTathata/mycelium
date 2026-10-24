# faults-group — a declaration with a governed group, for `mycelium test --faults partition`

Three units: a dispatcher that defines the `pool` group over `svc/worker`, and two workers that offer it with a
short `ttl_secs` (2 s), so the partition class's evaporation wait is seconds rather than the co-op fixture's
90 s. CI's `embedded` job runs `partition` against it (adopter-surface X6): the dark worker evicted, its offer gone,
the `pool` seat re-filled by the other worker's own join after the cut, the worker back.
