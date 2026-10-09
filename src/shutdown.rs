//! **Stop signals for a long-running node** — [`ShutdownSignal`].
//!
//! One implementation for the `mycelium` node binary, `mycelium-stem` and the long-running examples, so a stop
//! behaves the same everywhere (doc-coverage run 22 and its reviews):
//!
//! - **SIGINT or SIGTERM.** `docker stop` and a Kubernetes pod stop send SIGTERM; a process that awaits only
//!   Ctrl-C is killed by it, with no orderly shutdown.
//! - **Installed when constructed.** Create it before the node binds and await it afterwards: a handler
//!   registered only when the wait begins leaves startup exposed to the default action, which kills.
//! - **A second signal exits at once.** Once the first has started a shutdown, the handlers stay installed,
//!   so a second signal would otherwise do nothing and a hung shutdown would wait for SIGKILL. The second
//!   exits the process with `128 + signal` (130 for SIGINT, 143 for SIGTERM).
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
