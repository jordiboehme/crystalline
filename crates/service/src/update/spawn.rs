//! The one door `crystalline update now` runs other programs through
//! (`brew`, and on macOS `codesign` and the new binary's `--version`), so a
//! test can hand in a spawner that records the commands instead of running
//! them (S11).

use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Runs a program for the updater.
pub trait Spawner: Send + Sync {
    /// Run `program` with `args`, its input and output the terminal's, and
    /// hand back its exit code (`-1` when a signal ended it).
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<i32>;

    /// Run `program` with `args` and hand back what it printed on stdout.
    /// A program that exits with an error is an `Err`.
    fn output(&self, program: &str, args: &[&str]) -> std::io::Result<String>;

    /// [`Spawner::run`], but a program still running after `limit` is
    /// killed and the call is an error ([`std::io::ErrorKind::TimedOut`]).
    /// The default has no limit; [`SystemSpawner`] enforces it.
    fn run_within(&self, program: &str, args: &[&str], limit: Duration) -> std::io::Result<i32> {
        let _ = limit;
        self.run(program, args)
    }

    /// [`Spawner::output`], but a program still running after `limit` is
    /// killed and the call is an error ([`std::io::ErrorKind::TimedOut`]).
    /// The default has no limit; [`SystemSpawner`] enforces it.
    fn output_within(
        &self,
        program: &str,
        args: &[&str],
        limit: Duration,
    ) -> std::io::Result<String> {
        let _ = limit;
        self.output(program, args)
    }
}

/// Wait for `child` up to `limit`; past it the child is killed and reaped,
/// and the wait is a [`std::io::ErrorKind::TimedOut`] error naming
/// `program`.
fn wait_within(child: &mut Child, program: &str, limit: Duration) -> std::io::Result<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{program} did not end within {} s", limit.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The text of an exit status that is not a success.
fn exit_text(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "a signal".to_string(), |code| code.to_string())
}

/// The spawner of a real run: the program is started as it is named.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemSpawner;

impl Spawner for SystemSpawner {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<i32> {
        let status = Command::new(program)
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
        Ok(status.code().unwrap_or(-1))
    }

    fn run_within(&self, program: &str, args: &[&str], limit: Duration) -> std::io::Result<i32> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;
        Ok(wait_within(&mut child, program, limit)?
            .code()
            .unwrap_or(-1))
    }

    fn output_within(
        &self,
        program: &str,
        args: &[&str],
        limit: Duration,
    ) -> std::io::Result<String> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        // Read on a thread of its own, so a full pipe never blocks the
        // child and the wait. A grandchild that keeps the pipe open past a
        // kill only keeps this thread, never the caller.
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let status = wait_within(&mut child, program, limit)?;
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "{program} exited with {}",
                exit_text(status)
            )));
        }
        let bytes = reader
            .join()
            .map_err(|_| std::io::Error::other(format!("{program}'s output was not read")))??;
        String::from_utf8(bytes)
            .map_err(|_| std::io::Error::other(format!("{program} printed no text")))
    }

    fn output(&self, program: &str, args: &[&str]) -> std::io::Result<String> {
        let output = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "{program} exited with {}",
                exit_text(output.status)
            )));
        }
        String::from_utf8(output.stdout)
            .map_err(|_| std::io::Error::other(format!("{program} printed no text")))
    }
}

/// A spawner for the tests: every call is recorded as one line (`program
/// args...`), `run` answers 0 and `output` what `answer` says.
#[cfg(test)]
pub(crate) struct Scripted<F> {
    answer: F,
    calls: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl<F> Scripted<F>
where
    F: Fn(&str, &[&str]) -> std::io::Result<String> + Send + Sync,
{
    /// A spawner whose `output` is `answer(program, args)`.
    pub(crate) fn new(answer: F) -> Self {
        Scripted {
            answer,
            calls: std::sync::Mutex::default(),
        }
    }

    /// Every call so far, in order.
    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn record(&self, program: &str, args: &[&str]) {
        let mut call = vec![program];
        call.extend_from_slice(args);
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(call.join(" "));
    }
}

#[cfg(test)]
impl<F> Spawner for Scripted<F>
where
    F: Fn(&str, &[&str]) -> std::io::Result<String> + Send + Sync,
{
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<i32> {
        self.record(program, args);
        Ok(0)
    }

    fn output(&self, program: &str, args: &[&str]) -> std::io::Result<String> {
        self.record(program, args);
        (self.answer)(program, args)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_program_past_its_limit_is_killed_and_is_an_error() {
        let began = Instant::now();
        let err = SystemSpawner
            .output_within("/bin/sh", &["-c", "sleep 30"], Duration::from_millis(200))
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut, "{err}");
        let err = SystemSpawner
            .run_within("/bin/sh", &["-c", "sleep 30"], Duration::from_millis(200))
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut, "{err}");
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "{:?}",
            began.elapsed()
        );
    }

    #[test]
    fn a_program_inside_its_limit_answers() {
        let limit = Duration::from_secs(30);
        assert_eq!(
            SystemSpawner
                .output_within("/bin/sh", &["-c", "echo hello"], limit)
                .unwrap(),
            "hello\n"
        );
        assert!(
            SystemSpawner
                .output_within("/bin/sh", &["-c", "exit 3"], limit)
                .is_err()
        );
        assert_eq!(
            SystemSpawner
                .run_within("/bin/sh", &["-c", "exit 3"], limit)
                .unwrap(),
            3
        );
    }
}
