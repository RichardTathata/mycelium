//! Critique plan row CR2, first PR — **the use marker and its release**: the forged-record tests the owner-signed
//! rule's condition (d) asks for, at the door that made a use and at a second door of the same class.
//!
//! "A member's write" here is a write made at another node of the fleet through its own KV handle — a plain member
//! `C` with no authority — and awaited at the reader by a structural poll of the reader's store. Where the order of
//! arrival is the point (a tombstone delivered before the marker), the two records are applied to the reader's store
//! in that order at the stamps gossip would carry.

use super::*;
use crate::agent::grant_use::{reads_used, record_use, MarkerWrite, Odd, UseKind, UseRecord, GRANT_USE_NOT_STORED, GRANT_USE_SUPERSEDED};
use crate::agent::http::{ae_preflight, release_grant_claim_after, Preflight};
use crate::mandate::grant::SignedMandateGrant;
use bytes::Bytes;

type Ctx = Arc<crate::agent::TaskCtx>;

fn marker_key(grant: &SignedMandateGrant) -> String {
    format!("grant/{}/{}/used/gateway", crate::agent::gateway_authority::grant_holder_segment(FORECASTER), grant.mandate.term.as_str())
}

/// The store's entry at `key`: `None` never written, `Some(None)` a tombstone, `Some(Some(_))` a value.
fn held(ctx: &Ctx, key: &str) -> Option<Option<Bytes>> {
    ctx.kv_state.store.pin().get(key).map(|e| e.data.clone())
}

fn record_at(ctx: &Ctx, key: &str) -> Option<UseRecord> {
    serde_json::from_slice(&held(ctx, key)??).ok()
}

/// One presentation of `grant` at `ctx`'s `/mcp` door.
async fn present(ctx: &Ctx, resource: &str, args: &serde_json::Value, grant: &SignedMandateGrant, key: &ed25519_dalek::SigningKey) -> Preflight {
    let caller = crate::agent::gateway_caller::ResolvedPrincipal { principal: FORECASTER.into(), scopes: vec!["mcp:invoke".into()], subject_token: None };
    let params = serde_json::json!({"_meta": presented(grant, resource, args, key)});
    ae_preflight(ctx, Some(&caller), "tools/call", resource, args, &params, "gateway:mcp/tools/call").await
}

fn admitted(p: &Preflight) -> bool {
    matches!(p, Preflight::Proceed { grant_claim: Some(_), .. })
}

/// Refused **as used** — not for an authority this node could not establish.
fn refused_as_used(p: &Preflight) -> bool {
    matches!(p, Preflight::Refuse(r) if r.decision().reason.contains("already used"))
}

/// The provider's refusal before anything ran, as a tool's serve loop answers it.
fn refused_before_running() -> Result<Bytes, crate::agent::gateway_caller::GatewayDispatchError> {
    Ok(Bytes::from(serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32022, "message": "caller context refused: the client's signature on this call was refused: client_key_unknown", "data": {"reason": "client_key_unknown"}}}).to_string()))
}

/// A tombstone at `key` stamped two seconds ahead of this node's clock — what a delete by a member whose clock runs
/// ahead looks like once it has been applied. A local write made within those two seconds loses to it.
fn plant_tombstone_ahead(ctx: &Ctx, key: &str) {
    let stamp = crate::hlc::pack(crate::hlc::physical_ms(ctx.hlc.tick()) + 2_000, 0);
    let update = crate::framing::make_gossip_update_stamped(&ctx.node_id, ctx.default_ttl, Arc::from(key), Bytes::new(), true, stamp);
    crate::store::apply_and_notify(&ctx.kv_state, &update);
}

/// A gateway with the declared authority that verifies grants from any fleet member — a second gateway honours the
/// first one's grants.
async fn fleet_gateway(dir: &Path, http_port: u16, n: &Node) -> Arc<GossipAgent> {
    let (id, mut cfg) = config(dir, http_port, n);
    if let Some(a) = cfg.authority.as_mut() {
        a.entitle_grants = crate::config::GrantEntitlement::Fleet;
    }
    let a = Arc::new(GossipAgent::new(id, cfg));
    a.start().await.expect("the gateway starts from its declaration");
    a
}

/// Shut `a` down and wait until nothing holds its context: its journals are owned by one handle at a time, so a
/// restart over the same directory needs the last one gone.
async fn stopped(a: Arc<GossipAgent>, ctx: Ctx) {
    a.shutdown().await;
    let held = Arc::downgrade(&ctx);
    drop((a, ctx));
    assert!(until(|| held.strong_count() == 0, Duration::from_secs(10)).await, "the stopped node's context is released");
}

fn serve_tool(a: &GossipAgent) {
    let handle = a.mcp().register_mcp_tool(TOOL, serde_json::json!({"type": "object"}), |_| async { Ok(serde_json::json!("sunny")) });
    std::mem::forget(handle);
}

/// **At the door that made the use: the journal answers, and only this door's own release gives a use back** (CR2).
/// One node, its gateway door, five grants:
///
/// - *Planted first*: a tombstone at the marker key before the grant's first use — the call is refused as used, and
///   counted (`tombstone`).
/// - *The receipt*: a tombstone lands between the door's read and its marker write, at a stamp ahead of the door's.
///   The write's receipt says `Superseded`; the call is refused `grant_use_superseded`, the grant then reads used, and
///   the write is counted (`superseded`).
/// - *Mid-claim*: a grant claimed and not released reads used after a restart — and still does once nothing is left
///   at its key (a tombstone over the marker, collected): the journal alone holds the use.
/// - *Released*: a grant claimed, released (the signed release is the key's value), read back unused after a restart
///   and admitted once; then its **earlier release written back over the later use** releases nothing, before a
///   restart and after one.
/// - *Released, then tombstoned*: a member's tombstone over this door's release reads used — a spent grant, never a
///   re-armed one.
///
/// Seen failing first, one build per plant (the PR lists them): the read as it was before CR2 (presence only); the
/// receipt's `Superseded` taken as written; the journal not folded at open; the journal not read before the key.
#[tokio::test]
async fn the_door_that_made_a_use_reads_its_journal_and_only_its_own_release_gives_it_back() {
    // Every scenario is read before the test fails, so a fault shows each reading it moves, not only the first.
    let mut moved: Vec<String> = Vec::new();
    macro_rules! check {
        ($cond:expr, $($why:tt)+) => {
            if !$cond {
                moved.push(format!($($why)+));
            }
        };
    }
    let dir = scratch("cr2-own");
    let (http_port, port) = (alloc_port(), alloc_port());
    let node = Node { certs: dir.join("tls"), bootstrap: vec![], authority: true, port: Some(port) };
    let (a, _ran) = started_as(&dir, http_port, &node, true).await;
    let key = forecaster_key(&a, &dir);
    let ctx = Arc::clone(&a.task_ctx);
    let mut grants = Vec::new();
    for city in ["Brest", "Caen", "Dijon", "Evian", "Foix", "Gray"] {
        let args = serde_json::json!({"city": city});
        let (asked, grant) = ask_and_issue(http_port, &args).await;
        grants.push((asked["resource"].as_str().unwrap().to_string(), args, grant));
    }
    let [planted, raced, mid_claim, released, burnt, tombstoned] = <[_; 6]>::try_from(grants).ok().unwrap();
    let at = |ctx: &Ctx, g: &(String, serde_json::Value, SignedMandateGrant)| {
        let (ctx, key, g) = (Arc::clone(ctx), key.clone(), g.clone());
        async move { present(&ctx, &g.0, &g.1, &g.2, &key).await }
    };

    // Planted first: a member's tombstone at the predictable key, before any use.
    mycelium_core::ops::kv_delete(&ctx, Arc::from(marker_key(&planted.2).as_str()));
    let planted_first = at(&ctx, &planted).await;
    let tombstones = ctx.grant_uses.counted(Odd::Tombstone);

    // The receipt: the tombstone lands after the door's read and before its write.
    let raced_key = marker_key(&raced.2);
    {
        let (c, k) = (Arc::clone(&ctx), raced_key.clone());
        *ctx.grant_uses.before_marker_write.lock().unwrap() = Some(Box::new(move || plant_tombstone_ahead(&c, &k)));
    }
    let superseded = at(&ctx, &raced).await;
    let superseded_count = ctx.grant_uses.counted(Odd::Superseded);
    let raced_again = at(&ctx, &raced).await;
    let raced_own = ctx.grant_uses.own(&raced_key);
    // …and the write half alone, on a key nothing has used: `Ok(false)` is the receipt's `Superseded`.
    let lone = "grant/node:127.0.0.1:1/t-lone/used/gateway";
    plant_tombstone_ahead(&ctx, lone);
    let lone_write = record_use(&ctx, lone, "0123456789abcdef").await;
    let lone_reads = reads_used(&ctx, lone);

    // Mid-claim, and released.
    let claimed = at(&ctx, &mid_claim).await;
    let released_key = marker_key(&released.2);
    let first = at(&ctx, &released).await;
    let gave_back = release_grant_claim_after(&ctx, &first, &refused_before_running()).await;
    let release_1 = held(&ctx, &released_key).flatten();
    let release_record = record_at(&ctx, &released_key);
    let burnt_first = at(&ctx, &burnt).await;
    let burnt_back = release_grant_claim_after(&ctx, &burnt_first, &refused_before_running()).await;
    mycelium_core::ops::kv_delete(&ctx, Arc::from(marker_key(&burnt.2).as_str()));
    let burnt_retry = at(&ctx, &burnt).await;

    // Tombstoned: a grant this door used, its marker then tombstoned by a member — read again after the restart.
    let tombstoned_used = at(&ctx, &tombstoned).await;
    let tombstoned_key = marker_key(&tombstoned.2);
    mycelium_core::ops::kv_delete(&ctx, Arc::from(tombstoned_key.as_str()));

    // A restart: the same directory, the same ports. The old agent and its context are dropped first — the journals
    // are owned by one handle at a time.
    stopped(a, ctx).await;
    let (a, _ran) = started_as(&dir, http_port, &node, true).await;
    let ctx = Arc::clone(&a.task_ctx);
    let mid_claim_after = at(&ctx, &mid_claim).await;
    let raced_after = at(&ctx, &raced).await;
    let tombstone_held = held(&ctx, &tombstoned_key) == Some(None);
    let tombstoned_after = at(&ctx, &tombstoned).await;
    // The horizon, at this door: a tombstone over the marker has been collected, so the key reads *never written*
    // (the entry removed from the store, as the tombstone sweep removes it). Only the journal holds the use.
    let mid_claim_key: Arc<str> = Arc::from(marker_key(&mid_claim.2).as_str());
    ctx.kv_state.store.pin().remove(&mid_claim_key);
    let never_written = held(&ctx, &mid_claim_key).is_none();
    let after_horizon = at(&ctx, &mid_claim).await;
    let retry = at(&ctx, &released).await; // the release, read back: admitted once — the later use
    let later_use = record_at(&ctx, &released_key);
    // The earlier genuine release, written back over the later use at a newer stamp.
    mycelium_core::ops::kv_set(&ctx, Arc::from(released_key.as_str()), release_1.clone().unwrap_or_default());
    let replayed_holds = held(&ctx, &released_key).flatten() == release_1;
    let after_replay = at(&ctx, &released).await;
    stopped(a, ctx).await;
    let (a, _ran) = started_as(&dir, http_port, &node, true).await;
    let ctx = Arc::clone(&a.task_ctx);
    let after_replay_and_restart = at(&ctx, &released).await;
    let replayed_still_holds = held(&ctx, &released_key).flatten() == release_1;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);

    check!(refused_as_used(&planted_first), "PLANTED: a tombstone before the first use admitted the call: {}", describe(&planted_first));
    check!(tombstones == 1, "the tombstone read as used is not counted once: {tombstones}");
    check!(
        matches!(&superseded, Preflight::Refuse(r) if r.decision().reason.contains(GRANT_USE_SUPERSEDED)),
        "RECEIPT: the marker write lost to the tombstone and the call was not refused by name: {}",
        describe(&superseded)
    );
    check!(superseded_count == 1, "the superseded write is not counted once: {superseded_count}");
    check!(refused_as_used(&raced_again), "the grant reads used after the superseded write: {}", describe(&raced_again));
    check!(raced_own.is_some_and(|u| !u.released), "the use is in this door's own record");
    check!(lone_write == Ok(MarkerWrite::Superseded), "the write half does not report the receipt's Superseded: {lone_write:?}");
    check!(lone_reads, "and the key reads used");
    check!(admitted(&claimed), "{}", describe(&claimed));
    check!(refused_as_used(&mid_claim_after), "RESTART: a grant claimed before the restart was admitted after it: {}", describe(&mid_claim_after));
    check!(refused_as_used(&raced_after), "RESTART: the superseded use is read back: {}", describe(&raced_after));
    check!(admitted(&tombstoned_used) && tombstone_held, "the grant was used and its marker's key holds the member's tombstone after the restart: {}", describe(&tombstoned_used));
    check!(refused_as_used(&tombstoned_after), "TOMBSTONE, the same gateway after a restart: a member's tombstone re-armed the grant at the door that used it: {}", describe(&tombstoned_after));
    check!(admitted(&retry), "FOLD: the release was not read back after the restart, the retry refused: {}", describe(&retry));
    check!(never_written, "the marker's key reads never written");
    check!(refused_as_used(&after_horizon), "JOURNAL: with nothing at the key, the door that made the use admitted the grant again: {}", describe(&after_horizon));
    check!(admitted(&first), "{}", describe(&first));
    check!(gave_back.as_deref() == Some("client_key_unknown"), "the release was not made: {gave_back:?}");
    check!(release_record.is_some(), "RELEASE: the key holds no use record after the release");
    let blank = UseRecord { v: 0, kind: UseKind::Use, grant: String::new(), door: String::new(), node: String::new(), use_id: String::new(), at_ms: 0, signer: String::new(), sig: String::new() };
    let release_record = release_record.unwrap_or_else(|| blank.clone());
    check!((release_record.kind, release_record.node.as_str(), release_record.door.as_str()) == (UseKind::Release, a.node_id().to_string().as_str(), "gateway"), "RELEASE: not a signed release naming the door and its node: {release_record:?}");
    check!(!release_record.sig.is_empty() && release_record.grant.ends_with(released.2.mandate.term.as_str()), "signed, naming the grant: {release_record:?}");
    check!(later_use.is_some(), "the later use left no marker");
    let later_use = later_use.unwrap_or(blank);
    check!(later_use.kind == UseKind::Use, "the later use's marker is not a use: {later_use:?}");
    check!(later_use.use_id != release_record.use_id, "the later use is not another use");
    check!(replayed_holds && replayed_still_holds, "the earlier release is the key's value, as a member's replay leaves it");
    check!(refused_as_used(&after_replay), "REPLAY: an earlier genuine release over a later use released it: {}", describe(&after_replay));
    check!(refused_as_used(&after_replay_and_restart), "REPLAY, after a restart: {}", describe(&after_replay_and_restart));
    check!(admitted(&burnt_first) && burnt_back.is_some(), "{}", describe(&burnt_first));
    check!(refused_as_used(&burnt_retry), "a tombstone over this door's own release reads used: {}", describe(&burnt_retry));
    assert!(moved.is_empty(), "{} reading(s) moved:\n{}", moved.len(), moved.join("\n"));
}

/// **A marker the store refuses admits nothing; a release is written only over this door's own marker** (#786's review, findings 1 and 6). One node, its gateway door, two grants:
///
/// - *Not stored*: the node's store is at its live-entry cap (`max_store_entries` — the live count set to the cap,
///   as a member's flood leaves it), so the marker's write is `Refused` and nothing is applied. Taken as written,
///   the call ran with nothing at the key, and every other gateway — reading *never written* — admitted the same
///   grant. Now the call is refused `grant_use_not_stored`, counted; this node's key is still never written; and
///   the use stands in the door's journal, so with room in the store again the grant reads used here — the record
///   was synced and gossiped before the apply was refused, so a restart or a peer may hold it.
/// - *Not this door's marker*: the door claims a grant, another value replaces its marker at the key (as another
///   door's marker does when two admit at once), and the door then gives its use back. The release is not written
///   over that value: nothing is given back, the value stands and reads used, and it is counted (`superseded`).
///   **And the journal still shows the use** (#786's confirm round, N1): once nothing is left at the key the door
///   still reads used.
///
/// Seen failing first, one build per plant: a `Refused` receipt taken as written (the call admitted, nothing at the
/// key); the release written over whatever the key holds (the other value replaced by this door's release); the
/// release journaled before the key is checked (the use read unused once the key was empty).
#[tokio::test]
async fn a_marker_the_store_refuses_admits_nothing_and_a_release_is_written_only_over_this_doors_marker() {
    let mut moved: Vec<String> = Vec::new();
    macro_rules! check {
        ($cond:expr, $($why:tt)+) => {
            if !$cond {
                moved.push(format!($($why)+));
            }
        };
    }
    const CAP: usize = 1_000_000;
    let dir = scratch("cr2-cap");
    let http_port = alloc_port();
    let (id, mut cfg) = config(&dir, http_port, &Node { certs: dir.join("tls"), bootstrap: vec![], authority: true, port: None });
    cfg.max_store_entries = CAP;
    let a = Arc::new(GossipAgent::new(id, cfg));
    a.start().await.expect("the node starts from its declaration");
    serve_tool(&a);
    let key = forecaster_key(&a, &dir);
    let ctx = Arc::clone(&a.task_ctx);
    let mut grants = Vec::new();
    for city in ["Meaux", "Niort"] {
        let args = serde_json::json!({"city": city});
        let (asked, grant) = ask_and_issue(http_port, &args).await;
        grants.push((asked["resource"].as_str().unwrap().to_string(), args, grant));
    }
    let [capped, replaced] = <[_; 2]>::try_from(grants).ok().unwrap();
    let at = |g: &(String, serde_json::Value, SignedMandateGrant)| {
        let (ctx, key, g) = (Arc::clone(&ctx), key.clone(), g.clone());
        async move { present(&ctx, &g.0, &g.1, &g.2, &key).await }
    };

    // Not stored: the store at its cap.
    let capped_key = marker_key(&capped.2);
    let live = ctx.kv_state.live_count.swap(CAP, Ordering::SeqCst);
    let at_cap = at(&capped).await;
    ctx.kv_state.live_count.store(live, Ordering::SeqCst);
    let key_after = held(&ctx, &capped_key);
    let journal_after = ctx.grant_uses.own(&capped_key);
    let not_stored = ctx.grant_uses.counted(Odd::NotStored);
    let with_room = at(&capped).await;

    // Not this door's marker: another value at the key when the door gives its use back.
    let replaced_key = marker_key(&replaced.2);
    let claimed = at(&replaced).await;
    let other = Bytes::from_static(b"another door's marker");
    mycelium_core::ops::kv_set(&ctx, Arc::from(replaced_key.as_str()), other.clone());
    let gave_back = release_grant_claim_after(&ctx, &claimed, &refused_before_running()).await;
    let stands = held(&ctx, &replaced_key).flatten();
    let superseded = ctx.grant_uses.counted(Odd::Superseded);
    let retry = at(&replaced).await;
    // The give-back was refused, so the journal still shows the use — and with nothing left at the key (the other
    // value tombstoned and collected, as at the horizon) this door still reads used.
    let journal_after_refusal = ctx.grant_uses.own(&replaced_key);
    ctx.kv_state.store.pin().remove(&Arc::<str>::from(replaced_key.as_str()));
    let key_gone = held(&ctx, &replaced_key).is_none();
    let after_horizon = at(&replaced).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);

    check!(
        matches!(&at_cap, Preflight::Refuse(r) if r.decision().reason.contains(GRANT_USE_NOT_STORED)),
        "NOT STORED: the store refused the marker and the call was not refused by name: {}",
        describe(&at_cap)
    );
    check!(key_after.is_none(), "nothing is at the marker's key: {key_after:?}");
    check!(journal_after.as_ref().is_some_and(|u| !u.released), "the use does not stand in the door's journal: {journal_after:?}");
    check!(not_stored == 1, "the refused write is not counted once: {not_stored}");
    check!(refused_as_used(&with_room), "with room in the store again the grant reads used at this door: {}", describe(&with_room));
    check!(admitted(&claimed), "{}", describe(&claimed));
    check!(gave_back.is_none(), "OWN MARKER: a use was given back although the key no longer held this door's marker: {gave_back:?}");
    check!(stands.as_ref() == Some(&other), "OWN MARKER: the release was written over another value at the key: {stands:?}");
    check!(superseded == 1, "the release not written is not counted once: {superseded}");
    check!(refused_as_used(&retry), "the other value reads used: {}", describe(&retry));
    check!(journal_after_refusal.as_ref().is_some_and(|u| !u.released), "NOT GIVEN BACK: the journal shows the use released although the give-back was refused: {journal_after_refusal:?}");
    check!(key_gone, "the key reads never written");
    check!(refused_as_used(&after_horizon), "NOT GIVEN BACK, at the horizon: with nothing at the key the door admitted a use it did not give back: {}", describe(&after_horizon));
    assert!(moved.is_empty(), "{} reading(s) moved:\n{}", moved.len(), moved.join("\n"));
}

/// **At a second gateway: anything but *never written* reads used, and a release made elsewhere releases nothing**
/// (CR2; the decision of 2026-11-13). Three nodes: gateway `A` issues the grants and makes the uses, gateway `B` is
/// the second door, `C` is a plain member that writes what any member can. A control grant, never used, is admitted
/// at `B` — so every refusal below is the marker's, not an authority `B` could not establish. Then, each on its own
/// grant, read at `B` and at `A`:
///
/// - *Tombstoned*: `C` tombstones the marker of a grant used at `A`. Used at `B` (`tombstone`), used at `A`, and
///   used at `A` after a restart with the tombstone still the key's value.
/// - *Delivered first*: the tombstone reaches `B` before the marker it out-stamps; the marker then loses LWW.
/// - *Garbage*: `C` overwrites the marker with bytes that are no record (`unverifiable`).
/// - *Replayed*: `A` claims and releases, then claims again — the use that ran; `C` writes the earlier genuine
///   release back (`release_elsewhere` at `B`).
/// - *Forged*: `C` signs, with its own key, a release naming `A` and `A`'s use (`release_forged`).
/// - *A member's own pair*: over `A`'s use `C` writes its own signed marker, then its own signed release of it — both
///   verify, as `C`'s (`release_elsewhere`).
/// - *The old form*: a marker as a gateway before CR2 wrote it, a timestamp, is honoured as used (`legacy`).
///
/// Seen failing first, one build per plant (the PR lists them): the read as it was before CR2; a value that does not
/// verify read as absent; a verified release honoured at any door; a release honoured without verifying it; the
/// old form not recognised.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_gateway_reads_anything_but_never_written_as_used() {
    // Every scenario is read before the test fails, so a fault shows each reading it moves, not only the first.
    let mut moved: Vec<String> = Vec::new();
    macro_rules! check {
        ($cond:expr, $($why:tt)+) => {
            if !$cond {
                moved.push(format!($($why)+));
            }
        };
    }
    let (dir_a, dir_b, dir_c) = (scratch("cr2-a"), scratch("cr2-b"), scratch("cr2-c"));
    let certs = dir_a.join("tls");
    let (http_a, http_b, http_c, port_a) = (alloc_port(), alloc_port(), alloc_port(), alloc_port());
    let node_a = Node { certs: certs.clone(), bootstrap: vec![], authority: true, port: Some(port_a) };
    // One declaration for both gateways — a grant is bound to the policy revision it was issued under, so a second
    // gateway admits it only under the same unit — naming the tool where it runs, at A (a bare `tool:` resource is
    // the declaring node's own).
    let at_a = format!("tool:{TOOL}@127.0.0.1:{port_a}");
    let unit = UNIT.replace("resource       = \"tool:weather.forecast\"", &format!("resource = \"{at_a}\"")).replace("resource  = \"tool:weather.forecast\"", &format!("resource = \"{at_a}\""));
    for d in [&dir_a, &dir_b] {
        std::fs::write(d.join("units").join("weather.toml"), &unit).unwrap();
    }
    let a = fleet_gateway(&dir_a, http_a, &node_a).await;
    serve_tool(&a);
    let b = fleet_gateway(&dir_b, http_b, &Node { certs: certs.clone(), bootstrap: vec![a.node_id().clone()], authority: true, port: None }).await;
    let (c, _) = started_as(&dir_c, http_c, &Node { certs: certs.clone(), bootstrap: vec![a.node_id().clone()], authority: false, port: None }, false).await;
    let (ca, cb, cc) = (Arc::clone(&a.task_ctx), Arc::clone(&b.task_ctx), Arc::clone(&c.task_ctx));
    let key = forecaster_key_in(&a, &certs, &dir_a.join("client"));
    let within = Duration::from_secs(30);
    let tool_known = || !crate::store::scan_kv_prefix(&ca.kv_state, &format!("tools/{TOOL}/")).is_empty();
    assert!(until(tool_known, within).await, "A advertises the tool");

    let mut grants = Vec::new();
    for city in ["Agen", "Albi", "Auch", "Bron", "Cluny", "Digne", "Gap", "Laon"] {
        let args = serde_json::json!({"city": city});
        let (asked, grant) = ask_and_issue(http_a, &args).await;
        grants.push((asked["resource"].as_str().unwrap().to_string(), args, grant));
    }
    let at = |ctx: &Ctx, g: &(String, serde_json::Value, SignedMandateGrant)| {
        let (ctx, key, g) = (Arc::clone(ctx), key.clone(), g.clone());
        async move { present(&ctx, &g.0, &g.1, &g.2, &key).await }
    };
    // Peer-ready, structurally: B and C hold the last grant's records, so they hold the earlier ones' too.
    let last_checkpoint = format!("grant/{}/{}/checkpoint", crate::agent::gateway_authority::grant_holder_segment(FORECASTER), grants[7].2.mandate.term.as_str());
    assert!(until(|| held(&cb, &last_checkpoint).is_some() && held(&cc, &last_checkpoint).is_some(), within).await, "the grants reached B and C");
    let [control, tombstoned, delivered_first, garbage, replayed, forged, own_pair, old_form] = <[_; 8]>::try_from(grants).ok().unwrap();

    // The control: B admits a grant A issued and nobody used (its client-key fold may lag; a refusal claims nothing).
    let mut control_at_b = at(&cb, &control).await;
    let deadline = std::time::Instant::now() + within;
    while !admitted(&control_at_b) && std::time::Instant::now() < deadline {
        assert!(!refused_as_used(&control_at_b), "the control was never used: {}", describe(&control_at_b));
        tokio::time::sleep(Duration::from_millis(100)).await;
        control_at_b = at(&cb, &control).await;
    }
    let why = || {
        let presented: crate::agent::gateway_authority::PresentedMandate = serde_json::from_value(presented(&control.2, &control.0, &control.1, &key)["mandate"].clone()).unwrap();
        let authority = cb.execution_authority.get().expect("B has an authority");
        let (assessed, _) = authority.assess_arguments_at(crate::agent::gateway_authority::CountAt::Gateway, FORECASTER, "tools/call", &control.0, &control.1, Some(&presented), cb.hlc.decision_now_ms(), &crate::agent::gateway_member_keys(&cb));
        format!("{:?}", assessed.binding.map(|b| b.state))
    };
    assert!(admitted(&control_at_b), "CONTROL: the second gateway admits an unused grant of the first: {}; B's assessment: {}", describe(&control_at_b), why());

    // C holds A's marker (so its clock is past the marker's stamp) and then writes `value` over it; both gateways hold it.
    let member_writes = |k: String, value: Option<Bytes>| {
        let (ca, cb, cc) = (Arc::clone(&ca), Arc::clone(&cb), Arc::clone(&cc));
        async move {
            assert!(until(|| held(&cc, &k).is_some(), within).await, "the member holds the marker at {k}");
            match &value {
                None => mycelium_core::ops::kv_delete(&cc, Arc::from(k.as_str())),
                Some(v) => mycelium_core::ops::kv_set(&cc, Arc::from(k.as_str()), v.clone()),
            };
            assert!(until(|| held(&ca, &k) == Some(value.clone()) && held(&cb, &k) == Some(value.clone()), within).await, "the member's write reached both gateways at {k}");
        }
    };
    let c_key = cc.tls.get().expect("C has an identity").signing_key();
    let trusts = |ctx: &Ctx, node: &NodeId| !crate::agent::gateway_caller::verifying_keys_for(ctx, node).is_empty();
    assert!(until(|| trusts(&cb, c.node_id()) && trusts(&cb, a.node_id()), within).await, "B holds a key for A and for C");

    // Tombstoned.
    let k_tomb = marker_key(&tombstoned.2);
    let used_at_a = at(&ca, &tombstoned).await;
    assert!(admitted(&used_at_a), "{}", describe(&used_at_a));
    member_writes(k_tomb.clone(), None).await;
    let tomb_b = at(&cb, &tombstoned).await;
    let tomb_a = at(&ca, &tombstoned).await;
    check!(refused_as_used(&tomb_b), "TOMBSTONE, second gateway: a member's tombstone re-armed the grant: {}", describe(&tomb_b));
    check!(refused_as_used(&tomb_a), "TOMBSTONE, the same gateway: {}", describe(&tomb_a));

    // Delivered first: at B, the tombstone, then the marker it out-stamps — a use A never made here, signed as A signs.
    let k_first = marker_key(&delivered_first.2);
    let stamp = cb.hlc.tick();
    let a_marker = UseRecord::signed(&ca, UseKind::Use, &k_first, "00112233445566778899aabbccddeeff").unwrap().to_bytes();
    crate::store::apply_and_notify(&cb.kv_state, &crate::framing::make_gossip_update_stamped(c.node_id(), cb.default_ttl, Arc::from(k_first.as_str()), Bytes::new(), true, stamp + 1));
    let marker_lost = crate::store::apply_and_notify_reporting(&cb.kv_state, &crate::framing::make_gossip_update_stamped(a.node_id(), cb.default_ttl, Arc::from(k_first.as_str()), Bytes::from(a_marker), false, stamp));
    let first_b = at(&cb, &delivered_first).await;
    assert_eq!(marker_lost, mycelium_core::receipt::LocalApplication::Superseded, "the marker arrived second and lost");
    check!(refused_as_used(&first_b), "DELIVERED FIRST: a tombstone that arrived before the marker reads unused: {}", describe(&first_b));

    // Garbage at a higher stamp.
    let k_garbage = marker_key(&garbage.2);
    let garbage_used = at(&ca, &garbage).await;
    assert!(admitted(&garbage_used), "{}", describe(&garbage_used));
    member_writes(k_garbage.clone(), Some(Bytes::from_static(b"\x00not a record"))).await;
    let garbage_b = at(&cb, &garbage).await;
    let garbage_a = at(&ca, &garbage).await;
    check!(refused_as_used(&garbage_b), "GARBAGE, second gateway: a value that does not verify reads unused: {}", describe(&garbage_b));
    check!(refused_as_used(&garbage_a), "GARBAGE, the same gateway: {}", describe(&garbage_a));

    // Replayed: A's earlier genuine release, over the use that ran.
    let k_replay = marker_key(&replayed.2);
    let first = at(&ca, &replayed).await;
    let gave_back = release_grant_claim_after(&ca, &first, &refused_before_running()).await;
    let release_1 = held(&ca, &k_replay).flatten();
    let ran = at(&ca, &replayed).await;
    assert!(admitted(&first) && gave_back.is_some() && admitted(&ran), "RELEASE: claimed, given back, claimed again at the door that released: {} / {gave_back:?} / {}", describe(&first), describe(&ran));
    assert!(until(|| record_at(&cc, &k_replay).is_some_and(|r| r.kind == UseKind::Use), within).await, "C holds the later use's marker");
    check!(release_1.as_ref().is_some_and(|v| serde_json::from_slice::<UseRecord>(v).is_ok_and(|r| r.kind == UseKind::Release)), "RELEASE: the key holds no signed release after A gave its use back");
    member_writes(k_replay.clone(), Some(release_1.unwrap_or_else(|| Bytes::from_static(b"{}")))).await;
    let replay_b = at(&cb, &replayed).await;
    let replay_a = at(&ca, &replayed).await;
    check!(refused_as_used(&replay_b), "REPLAY, second gateway: an earlier genuine release released a later use: {}", describe(&replay_b));
    check!(refused_as_used(&replay_a), "REPLAY, the same gateway: {}", describe(&replay_a));

    // Forged: a release of A's use, naming A, under C's key.
    let k_forged = marker_key(&forged.2);
    let forged_used = at(&ca, &forged).await;
    assert!(admitted(&forged_used), "{}", describe(&forged_used));
    let mut forgery = record_at(&ca, &k_forged).expect("A's marker");
    forgery.kind = UseKind::Release;
    forgery.sign_with(&c_key);
    member_writes(k_forged.clone(), Some(Bytes::from(forgery.to_bytes()))).await;
    let forged_b = at(&cb, &forged).await;
    let forged_a = at(&ca, &forged).await;
    check!(refused_as_used(&forged_b), "FORGED, second gateway: a release under another node's key released the use: {}", describe(&forged_b));
    check!(refused_as_used(&forged_a), "FORGED, the same gateway: {}", describe(&forged_a));

    // A member's own pair: C's marker, then C's release of it, over A's use. Both are C's and both verify.
    let k_pair = marker_key(&own_pair.2);
    let pair_used = at(&ca, &own_pair).await;
    assert!(admitted(&pair_used), "{}", describe(&pair_used));
    let c_marker = UseRecord::signed(&cc, UseKind::Use, &k_pair, "ffeeddccbbaa99887766554433221100").unwrap();
    let c_release = UseRecord::signed(&cc, UseKind::Release, &k_pair, "ffeeddccbbaa99887766554433221100").unwrap();
    member_writes(k_pair.clone(), Some(Bytes::from(c_marker.to_bytes()))).await;
    member_writes(k_pair.clone(), Some(Bytes::from(c_release.to_bytes()))).await;
    let pair_b = at(&cb, &own_pair).await;
    let pair_a = at(&ca, &own_pair).await;
    check!(refused_as_used(&pair_b), "OWN PAIR, second gateway: a member's own marker and release re-armed another door's use: {}", describe(&pair_b));
    check!(refused_as_used(&pair_a), "OWN PAIR, the same gateway: {}", describe(&pair_a));

    // The old form: what a gateway before CR2 wrote at its use. Written by C; no door here made the use.
    let k_old = marker_key(&old_form.2);
    mycelium_core::ops::kv_set(&cc, Arc::from(k_old.as_str()), Bytes::from_static(b"1794000000000"));
    assert!(until(|| held(&cb, &k_old).is_some(), within).await, "the old-form marker reached B");
    let old_b = at(&cb, &old_form).await;
    check!(refused_as_used(&old_b), "OLD FORM: a pre-CR2 marker is honoured as used: {}", describe(&old_b));

    let counted_b: Vec<(&str, u64)> = Odd::ALL.iter().map(|o| (o.label(), cb.grant_uses.counted(*o))).collect();

    // A restarts alone: its journal, and the tombstone still the key's value.
    stopped(c, cc).await;
    stopped(b, cb).await;
    stopped(a, ca).await;
    let a = fleet_gateway(&dir_a, http_a, &node_a).await;
    serve_tool(&a);
    let ca = Arc::clone(&a.task_ctx);
    let tomb_held_after = held(&ca, &k_tomb);
    let journal_after = ca.grant_uses.own(&k_tomb);
    let tomb_after_restart = at(&ca, &tombstoned).await;
    let replay_after_restart = at(&ca, &replayed).await;
    a.shutdown().await;
    for d in [&dir_a, &dir_b, &dir_c] {
        let _ = std::fs::remove_dir_all(d);
    }

    check!(tomb_held_after == Some(None), "after the restart the key no longer holds the member's tombstone (a door wrote over it): {tomb_held_after:?}");
    check!(journal_after.is_some_and(|u| !u.released), "JOURNAL: the use was not read back from the journal after the restart");
    check!(refused_as_used(&tomb_after_restart), "TOMBSTONE, the same gateway after a restart: {}", describe(&tomb_after_restart));
    check!(refused_as_used(&replay_after_restart), "REPLAY, the same gateway after a restart: {}", describe(&replay_after_restart));
    let expected = vec![("tombstone", 2), ("unverifiable", 1), ("legacy", 1), ("release_elsewhere", 2), ("release_forged", 1), ("release_replayed", 0), ("superseded", 0), ("not_stored", 0)];
    check!(counted_b == expected, "COUNTED at the second gateway: each odd reading once — two tombstones, the replayed release and the member's own both made elsewhere: {counted_b:?}, expected {expected:?}");
    assert!(moved.is_empty(), "{} reading(s) moved:\n{}", moved.len(), moved.join("\n"));
}
