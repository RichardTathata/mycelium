//! **Stop signals for a long-running node** — [`ShutdownSignal`].
//!
//! One implementation for the `mycelium` node binary, `mycelium-stem` and the long-running examples, so a stop
//! behaves the same everywhere (doc-coverage run 22 and its reviews):
//!
//! - **SIGINT or SIGTERM.** `docker stop` and a Kubernetes pod stop send SIGTERM; a process that awaits only
//!   Ctrl-C gets no orderly shutdown from it — killed outright, or, as a container's PID 1, left running
//!   until the grace period ends in SIGKILL.
//! - **Installed when constructed** (Unix). Create it before the node binds and await it afterwards: a handler
//!   registered only when the wait begins leaves startup exposed to the default action, which kills. Elsewhere
//!   nothing is registered until [`wait`](ShutdownSignal::wait), which awaits Ctrl-C.
//! - **A second signal exits at once.** Once the first has started a shutdown, the handlers stay installed,
//!   so a second signal would otherwise do nothing and a hung shutdown would wait for SIGKILL. The second
//!   exits the process with `128 + signal` (130 for SIGINT, 143 for SIGTERM). A forced exit is
//!   **SIGKILL-equivalent**: no destructors run, WAL records not yet synced are lost, and a file being written
//!   (a stem's `--trace-dir` output) can be left truncated; on-disk state is repaired at the next start. The
//!   watcher is a task, so it runs only while the runtime can poll it — a synchronous block after the first
//!   signal, or work after the runtime is dropped, is not interruptible by it.
//!
//! Do not create one in an interactive program that should keep Ctrl-C's default: a handler nobody awaits
//! swallows the signal.

/// SIGINT or SIGTERM (Unix), Ctrl-C elsewhere — see the [module docs](self).
pub struct ShutdownSignal {
    #[cfg(unix)]
    int:  tokio::signal::unix::Signal,
    #[cfg(unix)]
    term: tokio::signal::unix::Signal,
}

impl ShutdownSignal {
    /// Install the handlers now. Must be called inside a Tokio runtime.
    pub fn install() -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            Ok(Self { int: signal(SignalKind::interrupt())?, term: signal(SignalKind::terminate())? })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    /// Wait for the first stop signal; from then on, a second one exits the process at once with
    /// `128 + signal` — the shutdown the first started may hang, and the handlers would otherwise
    /// swallow every later signal.
    #[cfg_attr(not(unix), allow(unused_mut))]
    pub async fn wait(mut self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            tokio::select! {
                _ = self.int.recv() => {}
                _ = self.term.recv() => {}
            }
            // A SIGINT and a SIGTERM both pending (a wrapper forwarding TERM while Ctrl-C also reaches the
            // process group) are one stop, not two: take whatever else is already pending before the
            // watcher starts, so it counts only a signal that arrives later.
            std::future::poll_fn(|cx| {
                let _ = self.int.poll_recv(cx);
                let _ = self.term.poll_recv(cx);
                std::task::Poll::Ready(())
            }).await;
            let (mut int, mut term) = (self.int, self.term);
            tokio::spawn(async move {
                let code = tokio::select! {
                    _ = int.recv() => 130,
                    _ = term.recv() => 143,
                };
                eprintln!("second stop signal: exiting without finishing the shutdown");
                std::process::exit(code);
            });
            Ok(())
        }
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await?;
            tokio::spawn(async {
                if tokio::signal::ctrl_c().await.is_ok() {
                    eprintln!("second stop signal: exiting without finishing the shutdown");
                    std::process::exit(130);
                }
            });
            Ok(())
        }
    }
}
