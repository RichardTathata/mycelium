## [2026-10-05] ingest | realignment repairs S2's gateway half — the signal SSE data names its kind

**What:** `src/agent/http.rs` — `gw_signal_sse` and `signal_sse_handler` put `"kind"` in each event's
data; their rustdoc; `signal_sse_data_carries_the_kind` (new, seen failing first with the data object
lacking the field); the changelog; the plan row; this log.

**Durable knowledge:**

- **An SSE body should stand alone.** The kind lived only in the SSE event name, and `mycelium-ts` read
  `raw.kind` from the body, so it was always undefined. S2 fixed the SDK to read the event name; this
  makes a body-only reader right too. Additive, the event name unchanged.
