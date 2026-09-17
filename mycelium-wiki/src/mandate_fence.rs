//! The mandate fence: one git transaction, one remote transaction (v3 item 5 PR 3).
//!
//! §5 of [`docs/design/scoped-mandates.md`](../../../docs/design/scoped-mandates.md) puts the
//! mandate check **inside the protected resource's own atomic boundary**, and is precise about what
//! that has to mean for `GitStore`:
//!
//! > *(i)* the mandate check and the content commit are **one git ref transaction** —
//! > `git update-ref --stdin` (`start` / `prepare` / `commit`) updating `refs/mycelium/mandate/…`
//! > and the content ref together, each with its expected old value, so two separately successful
//! > CAS operations can never interleave;
//! >
//! > *(ii)* the curator pushes with **`git push --atomic`** and asserts the mandate ref's current
//! > value with **`--force-with-lease=…`** on **every** push — including ordinary content writes
//! > that leave the mandate unchanged — so the mandate check is part of the same remote ref
//! > transaction as the content update, not an earlier hook-time read.
//!
//! # Why "including content-only writes" is the load-bearing phrase
//!
//! The obvious implementation reads the mandate ref, decides the curator is current, and then
//! pushes the content. Between those two steps an appointment can move. The write then lands under
//! an authority that was revoked microseconds earlier, and nothing in the transcript shows it —
//! the read succeeded, the push succeeded.
//!
//! Asserting the mandate's value **as part of the push** closes that window: the remote accepts all
//! the refs or none, and a mandate that moved makes the whole push fail. That is why the lease is
//! attached even when the mandate itself is not being changed.
//!
//! # Fail closed
//!
//! A remote that does not honour `--atomic` is **not a supported strict-profile remote**. The write
//! is refused, never downgraded to a non-atomic push — a downgrade would silently reintroduce
//! exactly the window the fence exists to close.
//!
//! # What these functions are
//!
//! Pure builders, so the exact bytes and argv are pinned by tests rather than discovered in
//! production. What they cannot cover is stated in [`push_args`]: the remote's behaviour, and the
//! pre-receive hook that verifies the signed epoch, are the other half and are not exercised here.

/// The mandate ref and the value the writer believes is installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MandateFence {
    /// e.g. `refs/mycelium/mandate/norfolk`.
    pub refname: String,
    /// The object id the writer expects that ref to hold.
    pub expected: String,
}

/// Build the `git update-ref --stdin` transaction for a content write.
///
/// With a fence, the transaction is:
///
/// ```text
/// start
/// verify <mandate-ref> <expected>
/// update <content-ref> <new> <old>
/// prepare
/// commit
/// ```
///
/// **One transaction, not two operations.** `verify` inside it means the mandate is checked
/// *through commit* rather than before it, so a concurrent appointment cannot slip between the
/// check and the write.
///
/// Without a fence the transaction is the content update alone — today's behaviour, unchanged, so
/// a store that has not opted in is not affected.
///
/// `old` is the empty string for an unborn branch, which git reads as "must not exist yet".
pub fn update_ref_stdin(
    content_ref: &str,
    new: &str,
    old: &str,
    fence: Option<&MandateFence>,
) -> String {
    let mut s = String::from("start\n");
    if let Some(f) = fence {
        // Before the update, so a reader of the transcript sees the authority check first.
        s.push_str(&format!("verify {} {}\n", f.refname, f.expected));
    }
    s.push_str(&format!("update {content_ref} {new} {old}\n"));
    s.push_str("prepare\ncommit\n");
    s
}

/// Build the `git push` argv.
///
/// With a fence: `--atomic` (all refs or none) **and** `--force-with-lease=<mandate-ref>:<expected>`
/// — on every push, including one that changes only content. See the module docs for why the lease
/// is attached when the mandate is not being changed.
///
/// # What this cannot check
///
/// Whether the **remote** honours `--atomic`, and whether its **pre-receive hook** verifies the
/// signed epoch. Both are the other half of the fence and neither is exercised by a unit test; a
/// remote that ignores atomicity would accept a partial push and these argv would look identical.
/// The caller learns it from the push result, and §5's rule is that such a remote is refused rather
/// than written to.
pub fn push_args<'a>(refspec: &'a str, fence: Option<&'a MandateFence>) -> Vec<String> {
    let mut args = vec!["push".to_string(), "-q".to_string()];
    if let Some(f) = fence {
        args.push("--atomic".to_string());
        args.push(format!("--force-with-lease={}:{}", f.refname, f.expected));
    }
    args.push("origin".to_string());
    args.push(refspec.to_string());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fence() -> MandateFence {
        MandateFence {
            refname: "refs/mycelium/mandate/norfolk".into(),
            expected: "abc123".into(),
        }
    }

    /// **One transaction, and the verify is inside it.** Two separately successful CAS operations
    /// could interleave; one transaction cannot.
    #[test]
    fn the_mandate_verify_and_the_content_update_are_one_transaction() {
        let s = update_ref_stdin("refs/heads/main", "newsha", "oldsha", Some(&fence()));
        assert_eq!(
            s,
            "start\n\
             verify refs/mycelium/mandate/norfolk abc123\n\
             update refs/heads/main newsha oldsha\n\
             prepare\n\
             commit\n"
        );
        assert_eq!(s.matches("start\n").count(), 1, "exactly one transaction");
        assert!(
            s.find("verify ").unwrap() < s.find("update ").unwrap(),
            "the authority check reads first in the transcript"
        );
    }

    /// A store without a fence behaves exactly as before — opting in is opting in.
    #[test]
    fn without_a_fence_the_transaction_is_the_content_update_alone() {
        let s = update_ref_stdin("refs/heads/main", "newsha", "oldsha", None);
        assert_eq!(s, "start\nupdate refs/heads/main newsha oldsha\nprepare\ncommit\n");
        assert!(!s.contains("verify"), "nothing is asserted that was not configured");
    }

    /// The unborn branch: an empty old value is git's "must not exist yet".
    #[test]
    fn an_unborn_branch_asserts_creation() {
        let s = update_ref_stdin("refs/heads/main", "newsha", "", Some(&fence()));
        assert!(s.contains("update refs/heads/main newsha \n"));
    }

    /// **The load-bearing "including".** The lease is attached even when the mandate is not being
    /// changed — otherwise an appointment could move between a hook-time read and the push.
    #[test]
    fn every_push_asserts_the_mandate_even_a_content_only_one() {
        let args = push_args("refs/heads/main:refs/heads/main", Some(&fence()));
        assert_eq!(
            args,
            vec![
                "push",
                "-q",
                "--atomic",
                "--force-with-lease=refs/mycelium/mandate/norfolk:abc123",
                "origin",
                "refs/heads/main:refs/heads/main",
            ]
        );
    }

    /// `--atomic` and the lease travel together: either would be insufficient alone. Without
    /// `--atomic` the remote may accept some refs and not others; without the lease there is
    /// nothing asserting which mandate was in force.
    #[test]
    fn atomic_and_the_lease_are_never_separated() {
        let args = push_args("refs/heads/main:refs/heads/main", Some(&fence()));
        let has_atomic = args.iter().any(|a| a == "--atomic");
        let has_lease = args.iter().any(|a| a.starts_with("--force-with-lease="));
        assert_eq!(has_atomic, has_lease, "one without the other is not the fence");
        assert!(has_atomic);
    }

    #[test]
    fn without_a_fence_the_push_is_unchanged() {
        let args = push_args("refs/heads/main:refs/heads/main", None);
        assert_eq!(args, vec!["push", "-q", "origin", "refs/heads/main:refs/heads/main"]);
    }
}
