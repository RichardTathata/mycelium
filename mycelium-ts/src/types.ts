/** An admitted signal delivered to a local handler. */
export interface Signal {
  kind: string;
  sender: string;
  /** Raw payload bytes. */
  payload: Buffer;
  /** Random 64-bit nonce used for deduplication. */
  nonce: bigint;
}

/** A single entry in an ordered log stream. */
export interface LogEntry {
  /** HLC timestamp — use as a cursor for range scans. */
  hlc: bigint;
  value: Buffer;
}

/** An incoming RPC request. Call `rpc_respond` to complete the round-trip. */
export interface RpcRequest {
  kind: string;
  /** Hex-encoded 8-byte correlation nonce. */
  nonceHex: string;
  sender: string;
  payload: Buffer;
  /**
   * Present when a *gateway* dispatched the call for one of its HTTP/SDK clients (core v3
   * item 7): the client's resolved principal (`oidc:…` / `token:#i` / `token:legacy` /
   * `anonymous`), the gateway node (`via`), the scopes granted for the request, and whether
   * the node verified the gateway's signature. Absent for a direct in-mesh call, where
   * `sender` *is* the principal. A request whose context failed verification is never delivered.
   */
  caller?: GatewayCaller;
}

/** The verified caller context behind a gateway-dispatched RPC (core v3 item 7). */
export interface GatewayCaller {
  principal: string;
  via: string;
  scopes: string[];
  attested: boolean;
}

/** A mailbox event delivered to this node. */
export interface MailboxEvent {
  kind: string;
  sender: string;
  payload: Buffer;
}

/** Demand pressure for a capability. */
export interface DemandStatus {
  ns: string;
  name: string;
  providers: number;
  requirers: number;
  demandPressure: number;
}

/** Held while a distributed lock is acquired. Use as async context manager. */
/**
 * Outcome of a consensus-backed write (`consistentSet`, `crossGroupPropose`). The commit is
 * cluster-wide — the call resolves only after quorum. `persisted` reports whether the committed
 * slot also reached **the gateway node's own stable storage** (its WAL append is forced to
 * fdatasync in every sync mode): `true` = on disk there; `false` = committed and applied, but
 * that node's WAL append failed (writer stopped / disk error — logged at error there; recovered
 * from peers by anti-entropy after a restart — treat a run of `false` as a disk fault on the node
 * you are talking to); `null` = the gateway predates v2.4.2 and did not report the field.
 *
 * `persisted` folds two states into `true` — *on disk* and *nothing was promised* (no persistence
 * configured on that node). `localDurability` (v2.8.0, the receipt vocabulary of
 * `docs/design/contracts-receipts.md`) separates them with the same names the Rust
 * `LocalDurability` uses; `localDurabilityError` is set only for `"failed"`; both are `null` when
 * the gateway predates v2.8.0.
 */
export interface CommitResult {
  persisted: boolean | null;
  localDurability: LocalDurability | null;
  localDurabilityError: string | null;
}

/**
 * The local-durability rung, as the gateway names it: `"on_disk"` (the forced fdatasync
 * returned — the one state that establishes durability) · `"buffered"` (accepted by the WAL, not
 * yet synced; never produced by a consensus commit, which forces the sync) · `"not_configured"`
 * (no persistence on that node: nothing promised, nothing claimed) · `"failed"` (not
 * established; see `localDurabilityError`). A newer gateway may add a name; it arrives as a
 * string rather than being dropped.
 */
export type LocalDurability = "on_disk" | "buffered" | "not_configured" | "failed" | (string & {});

export class LockGuard {
  /** Opaque guard ID used to release via HTTP. */
  readonly guardId: string;
  /** Monotonic fencing token (consensus ballot). */
  readonly token: bigint;

  private readonly _release: () => Promise<void>;

  constructor(guardId: string, token: bigint, release: () => Promise<void>) {
    this.guardId = guardId;
    this.token = token;
    this._release = release;
  }

  /** Releases the lock. */
  async release(): Promise<void> {
    return this._release();
  }

  async [Symbol.asyncDispose](): Promise<void> {
    return this._release();
  }
}

/** Held while a capability advertisement is live. Drop to retract. */
export class CapabilityHandle {
  /** Opaque handle ID used to retract via HTTP. */
  readonly handleId: string;
  private readonly _drop: () => Promise<void>;
  private readonly _heartbeat?: () => Promise<void>;

  constructor(
    handleId: string,
    drop: () => Promise<void>,
    heartbeat?: () => Promise<void>,
  ) {
    this.handleId = handleId;
    this._drop = drop;
    this._heartbeat = heartbeat;
  }

  /** Retracts (tombstones) the capability. */
  async drop(): Promise<void> {
    return this._drop();
  }

  /**
   * Renews the lease on an advertisement made with `leaseSecs`. Must be
   * called within every `leaseSecs` window or the node retracts the advert.
   * Throws if the advertisement was made without a lease.
   */
  async heartbeat(): Promise<void> {
    if (!this._heartbeat) {
      throw new Error(
        "capability advertised without leaseSecs — no lease to renew",
      );
    }
    return this._heartbeat();
  }

  async [Symbol.asyncDispose](): Promise<void> {
    return this._drop();
  }
}
