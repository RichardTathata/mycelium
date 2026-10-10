## [2026-10-10] ingest | the front doors route by intent

The root README's first choice is now by role and intent: under the hero, the sentence and one
plain-language line, an intent router (`<!-- router:start/end -->`) with four routes — *Build a fleet*
(guide, tutorials, the five steps), *Run a fleet* (the operator journey, readiness, observability), *See
it work* (four examples), *Check the evidence* (`what-is-proven.md`) — then a six-row capability compass
into `docs/capabilities.md`, then the five steps. `examples/README.md` leads with a *What do you want to
see?* chooser of the same four (`conway`, `provisioning_viz`, `procurement_authority`, `diagnostics`),
then the learning path, then the matrix. The operations door keeps its own journey and no longer counts as
a carrier of the five steps — `docs/positioning.md` listed it as one although it never carried them.

Gate: `scripts/check-front-doors.py` (called by `check-positioning.sh`) checks structure and targets,
not prose — route labels and order, each route's agreed targets, every link and anchor resolving, the
README's See-it-work examples equal to the chooser's, the examples page's order, the operator journey
reaching readiness and observability. `scripts/test-check-front-doors.py` (CI's documentation-coherence
job, the test universe, `make check`) proves ten planted breaks fail it.

Pages touched: `dev/examples.md` (reader routes). Canon: `docs/positioning.md` § *The routes and the
five-step path*.
