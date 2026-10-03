## [2026-10-03] ingest | unreadable persisted state fails closed

**What:** `persistence::WalEnd` (Clean · Torn{offset} · Corrupt{offset}); `replay` errs on a corrupt
snapshot or WAL record; `do_snapshot` aborts on `Corrupt`; `PersistenceConfig::on_unreadable`
(Refuse default · Quarantine → `*.unreadable-N`); guarantee `persist.unreadable_refused`.

**Durable knowledge:** the old reader could not tell a crash from corruption — both "stopped the walk"
— and the two readers agreeing on where to stop was presented as safety ("replay and the merge never
disagree") when it was exactly what let the merge *truncate* what replay had *skipped*. The distinction
is mechanical: a torn tail is the file ending inside a record; corruption is a whole record that does
not decode, or a bad length prefix with non-zero data after it. Quarantine never deletes: "start from
what was readable, keep the rest for a human" is the only honest fail-open, and it is opt-in.
