# ingest — the nightly outage: TCC was real but not the whole story (2026-08-17)

The first unattended 02:00 run after the Local-Network grant failed again — all three suites,
same ~60 s registry-metadata DeadlineExceeded — including the scale round on a VM session that
had working network the previous afternoon. TCC therefore cannot explain last night; the machine
never sleeps (`pmset: sleep 0`) and no Wi-Fi link/sleep events appear in the 01:50–02:10 window.
Two corrections to yesterday's entry: (1) the runner clone **auto-fast-forwards each run**
("runner checkout updated to …") — yesterday's "does not auto-pull, a month stale" claim was
wrong (it lags at most one day); (2) lima/colima install dates (Jul 3 / Jun 4) do not line up
with the 07-15 outage start, weakening TCC as the *original* cause. Current suspect: an
upstream/WAN outage window around 02:00 (router/ISP maintenance — AP stays up, so invisible to
macOS logs). Discriminator deployed: host-side probe (`netprobe.sh` + `com.mycelium.netprobe`
launchd agent, no VM/TCC in the path) at 01:58/02:02/04:30 → `netprobe.log`; smoke-tested green
(gw/dns ok, registry 401-as-expected). If 02:00 probes dead and 04:30 healthy → move the
nightly's StartCalendarInterval. Page updated: [scale-tests](../testing/scale-tests.md); the
ratings Run-59 addendum carries a dated Addendum 2 recording its own premature closure.
