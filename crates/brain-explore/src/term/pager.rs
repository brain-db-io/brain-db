//! Auto-pager.
//!
//! Spawns `$PAGER` (defaulting to `less -R` so ANSI colors survive) when
//! the rendered output overflows the terminal height and stdout is a
//! TTY. Otherwise writes straight to stdout. Use via [`Pager::page`]:
//! build the body into a `String`, then hand it over. Direct streaming
//! through a paged process is more efficient but harder to get right;
//! the shell's outputs are always small enough for buffering.

use std::env;
use std::io::{self, Write};
use std::process::{Child, Command, Stdio};

use super::policy::TermPolicy;

/// Stateless pager wrapper. Cheap to construct per command dispatch.
pub struct Pager {
    policy: TermPolicy,
}

impl Pager {
    #[must_use]
    pub fn new(policy: TermPolicy) -> Self {
        Self { policy }
    }

    /// Write `body` to stdout, paging through `$PAGER` if it overflows
    /// the terminal height and stdout is a TTY.
    pub fn page(&self, body: &str) -> io::Result<()> {
        let line_count = body.lines().count();
        let should_page = self.policy.stdout_is_tty && line_count > self.policy.height;
        if !should_page {
            let mut stdout = io::stdout().lock();
            stdout.write_all(body.as_bytes())?;
            return stdout.flush();
        }
        match spawn_pager() {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.as_mut() {
                    if let Err(e) = stdin.write_all(body.as_bytes()) {
                        // Pager process closed early (q before EOF) is a
                        // SIGPIPE — common and not an error.
                        if e.kind() != io::ErrorKind::BrokenPipe {
                            return Err(e);
                        }
                    }
                }
                let _ = child.wait();
                Ok(())
            }
            Err(_) => {
                // Falling back to stdout is more useful than failing
                // hard — the user gets the output, just unpaginated.
                let mut stdout = io::stdout().lock();
                stdout.write_all(body.as_bytes())?;
                stdout.flush()
            }
        }
    }
}

fn spawn_pager() -> io::Result<Child> {
    let pager = env::var("PAGER").unwrap_or_else(|_| "less -R".to_string());
    let mut parts = pager.split_whitespace();
    let prog = parts.next().unwrap_or("less");
    let args: Vec<&str> = parts.collect();
    Command::new(prog).args(args).stdin(Stdio::piped()).spawn()
}
