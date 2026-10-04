//! R1 of `docs/plans/realignment-repairs.md`: the journal truncates its torn tail before the first
//! append, owns its file, writes each frame in one call, and stops after a failed append.
//!
//! Every test here was written first and seen to fail on the unrepaired code (the commit says
//! which). The shapes are the external review's F02 and F12 probes, inverted.

use super::*;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "mycelium-journal-repair-{tag}-{}-{}",
        std::process::id(),
        fastrand::u64(..)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("create temp dir");
    d
}

/// One complete record `one`, then a frame claiming `claims` bytes with only `present` of them —
/// the shape a crash mid-append leaves.
fn torn(path: &Path, claims: u32, present: &[u8]) {
    let mut bytes = vec![3u8, 0, 0, 0, b'o', b'n', b'e'];
    bytes.extend_from_slice(&claims.to_le_bytes());
    bytes.extend_from_slice(present);
    std::fs::write(path, bytes).expect("write the torn journal");
}

/// **F02, the probe inverted.** A torn frame on disk, then two acknowledged appends, then a
/// restart: both acknowledged records are readable, at the sequence numbers their receipts named.
///
/// On the unrepaired code the appends land *behind* the torn frame, whose claimed length then
/// swallows them: the reader returns `["one", "hi\x05\0\0\0aft"]` — a garbled record — and neither
/// acknowledged record exists anywhere.
#[tokio::test]
async fn an_append_after_a_torn_tail_is_readable_after_a_restart() {
    let d = dir("torn-append");
    let path = d.join("evidence.log");
    torn(&path, 9, b"hi");

    let j = Journal::open(&path, "test/journal").unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        7,
        "open truncates the torn frame before any append"
    );
    let a = j.append(b"after".to_vec()).await.expect("appended");
    let b = j.append(b"second".to_vec()).await.expect("appended");
    assert_eq!((a.seq, b.seq), (1, 2));
    assert_eq!(a.durability, LocalDurability::OnDisk);
    drop(j);

    let records = read_journal(&path).unwrap();
    assert_eq!(
        records,
        vec![b"one".to_vec(), b"after".to_vec(), b"second".to_vec()],
        "every acknowledged record is readable after the restart, in order"
    );
    let j = Journal::open(&path, "test/journal").unwrap();
    assert_eq!(j.append(b"third".to_vec()).await.unwrap().seq, 3);
    let _ = std::fs::remove_dir_all(&d);
}

/// A crash can land inside the 4-byte length prefix too. Those 1–3 bytes are a torn tail, not a
/// clean end, and `open` removes them the same way.
#[tokio::test]
async fn a_partial_length_prefix_is_a_torn_tail_too() {
    let d = dir("partial-prefix");
    let path = d.join("evidence.log");
    let mut bytes = vec![3u8, 0, 0, 0, b'o', b'n', b'e'];
    bytes.extend_from_slice(&[5u8, 0]);
    std::fs::write(&path, bytes).unwrap();

    let j = Journal::open(&path, "test/journal").unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 7);
    let a = j.append(b"after".to_vec()).await.unwrap();
    assert_eq!(a.seq, 1);
    drop(j);
    assert_eq!(read_journal(&path).unwrap(), vec![b"one".to_vec(), b"after".to_vec()]);
    let _ = std::fs::remove_dir_all(&d);
}

/// **F12.** A second handle on the same path in the same process is refused while the first is
/// open, and admitted once it closes — with the sequence continued, not restarted.
#[tokio::test]
async fn a_second_owner_in_the_same_process_is_refused_until_the_first_closes() {
    let d = dir("same-process");
    let path = d.join("evidence.log");

    let first = Journal::open(&path, "test/journal").unwrap();
    first.append(b"one".to_vec()).await.unwrap();

    let err = match Journal::open(&path, "test/journal") {
        Err(e) => e,
        Ok(_) => panic!("a second owner must be refused while the first holds the journal"),
    };
    assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock, "{err}");
    assert!(err.to_string().contains("evidence.log"), "the refusal names the journal: {err}");

    drop(first);
    let again = Journal::open(&path, "test/journal").expect("the lock is released on close");
    assert_eq!(again.append(b"two".to_vec()).await.unwrap().seq, 1);
    let _ = std::fs::remove_dir_all(&d);
}

/// **F12, across processes.** The lock is the OS's, so a second *process* is refused as well. The
/// child is this test binary running [`child_holds_the_journal_lock`] under an environment
/// variable; it opens the journal, prints a line, and holds until it is killed.
#[tokio::test]
async fn a_second_owner_in_another_process_is_refused() {
    use std::io::BufRead as _;
    let d = dir("cross-process");
    let path = d.join("evidence.log");

    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "agent::journal::repair_tests::child_holds_the_journal_lock", "--nocapture"])
        .env("MYCELIUM_JOURNAL_LOCK_CHILD", &path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn the child");
    let stdout = child.stdout.take().unwrap();
    let mut lines = std::io::BufReader::new(stdout).lines();
    let ready = loop {
        match lines.next() {
            Some(Ok(l)) if l.contains("holding") => break true,
            Some(Ok(_)) => continue,
            _ => break false,
        }
    };
    assert!(ready, "the child reported holding the journal");

    let refused = Journal::open(&path, "test/journal");
    let _ = child.kill();
    let _ = child.wait();
    let err = match refused {
        Err(e) => e,
        Ok(_) => panic!("a second process must be refused while the child holds the journal"),
    };
    assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock, "{err}");

    let after = Journal::open(&path, "test/journal").expect("released when the child died");
    assert_eq!(after.append(b"x".to_vec()).await.unwrap().seq, 0);
    let _ = std::fs::remove_dir_all(&d);
}

/// The child half of the test above. A no-op unless the parent set the variable.
#[tokio::test]
async fn child_holds_the_journal_lock() {
    let Ok(path) = std::env::var("MYCELIUM_JOURNAL_LOCK_CHILD") else { return };
    let _held = Journal::open(&path, "test/journal").expect("the child opens the journal");
    println!("holding {path}");
    // Held until the parent kills this process. A bound so a stray child cannot outlive a run.
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
}

/// A failed append leaves the file's end unknown — here, a length prefix with no body — so the
/// writer refuses every later append by name instead of acknowledging a record behind it. A
/// reopen scans, truncates the partial frame, and the journal is whole again.
///
/// Seen failing with the poisoning line removed: the third append is acknowledged `OnDisk` at
/// `seq 1`, and after the reopen the reader returns `["one"]` — the acknowledged record gone.
#[tokio::test]
async fn a_failed_append_poisons_the_writer_until_a_reopen_repairs_the_file() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let d = dir("poison");
    let path = d.join("evidence.log");
    let fault = Arc::new(AtomicBool::new(false));
    let j = Journal::open_with_fault(&path, "test/journal", Arc::clone(&fault)).unwrap();
    j.append(b"one".to_vec()).await.unwrap();

    fault.store(true, Ordering::SeqCst);
    let failed = j.append(b"two".to_vec()).await.unwrap_err();
    assert!(matches!(failed, JournalError::Failed(_)), "{failed:?}");
    let poisoned = j.append(b"three".to_vec()).await.unwrap_err();
    assert!(
        matches!(&poisoned, JournalError::Failed(m) if m.contains("poisoned")),
        "every later append is refused by name: {poisoned:?}"
    );
    drop(j);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 7 + 4, "the partial frame is on disk");

    let j = Journal::open(&path, "test/journal").expect("the reopen repairs the file");
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 7, "the partial frame is gone");
    let a = j.append(b"four".to_vec()).await.unwrap();
    assert_eq!(a.seq, 1, "the failed and refused appends took no sequence numbers");
    drop(j);
    assert_eq!(read_journal(&path).unwrap(), vec![b"one".to_vec(), b"four".to_vec()]);
    let _ = std::fs::remove_dir_all(&d);
}

/// **The consumer that carries v2.15.0's property.** An epoch floor recorded after a torn tail
/// survives a restart: `DurableEpochs::open` folds the journal and must find the record. On the
/// unrepaired code the fold meets a garbled frame and refuses to open at all.
#[tokio::test]
async fn an_epoch_recorded_after_a_torn_tail_survives_a_restart() {
    use crate::mandate::authority::DurableEpochs;
    let d = dir("epochs");
    let path = d.join("epochs.journal");
    // A torn tail with no complete record before it: the file a crash on the very first append leaves.
    std::fs::write(&path, [40u8, 0, 0, 0, b'{', b'"']).unwrap();

    {
        let e = DurableEpochs::open(&path).expect("a torn tail does not stop the node");
        e.record("cap/fleet", 2).await.expect("recorded");
        assert_eq!(e.floor("cap/fleet"), Some(2));
    }
    let reopened = DurableEpochs::open(&path).expect("the repaired journal opens");
    assert_eq!(reopened.floor("cap/fleet"), Some(2), "the floor recorded after the torn tail survives");
    let _ = std::fs::remove_dir_all(&d);
}
