## [2026-10-01] ingest | X2, fifth slice: a real model deployed by a stem

**What:** the stem-examples suite's `model_deploy` profile — a pinned Ollama container, the TinyStories
GGUF downloaded by `examples/units/model_deploy/prepare.sh`, a profile written with the weights' content
address, a librarian stem, and a model-host stem whose `[[activation]]` (D21) renders the profile,
uploads the weights and creates the model through Ollama's HTTP API (`ollama-activate.sh`). The driver
checks the governed SYSTEM prompt via `/api/show` and real tokens via `/api/generate`.

**Durable knowledge:**
- **The activation speaks HTTP, not the CLI.** The stem image carries curl; the Ollama CLI would bring
  its runtime libraries. `POST /api/blobs/sha256:<digest>` then `POST /api/create` with `files` is the
  CLI's own path, and the profile's SYSTEM prompt travels in the create call — so the config that runs
  is the config that arrived signed.
- **Scripts for anything with braces.** `[[activation]]` refuses unknown `{…}` placeholders, so a JSON
  body inline would be refused; the scripts take `{rendered}` and the model name as arguments.

**Pages touched:** plan row X2, the compose header, `examples/units/README.md`, CHANGELOG.
