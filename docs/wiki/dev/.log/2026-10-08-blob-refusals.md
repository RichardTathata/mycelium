# 2026-10-08 — a blob fetch tells a refusal from a corrupt copy (#564, PR #567)

`MeshBlobStore::fetch` counted any non-empty reply failing the content address as corrupt, so a provider's RPC layer
refusing the fetch — caller context, provider enforcement — read as corrupt and ended every retry loop on a transient
condition. Now `reply_outcome` checks the address first, then reads a refusal by its `reason`: `PERMANENT_REFUSALS`
(malformed, via_mismatch, unsigned, bad_signature, removed, action_denied, authority_not_established, malformed_call,
caller_context_too_large) count as refused; any other reason, or none, as a holder not yet askable. `BlobMiss::Refused`
(403 `refused`) when every askable holder refused for good. The review found the substrate side too: `rpc_rx` built its
refusal with `format!`, so an error text with quotes produced invalid JSON — read as corrupt, the same bug — and gave no
machine-readable reason; `caller_refusal_body` builds it with `json!` and adds `CallerError::code`.
