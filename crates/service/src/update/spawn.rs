//! The one door `crystalline update now` runs other programs through
//! (`brew`, and on macOS `codesign` and the new binary's `--version`), so a
//! test can hand in a spawner that records the commands instead of running
//! them (S11).

use std::process::{Command, Stdio};

/// Runs a program for the updater.
pub trait Spawner: Send + Sync {
    /// Run `program` with `args`, its input and output the terminal's, and
    /// hand back its exit code (`-1` when a signal ended it).
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<i32>;

    /// Run `program` with `args` and hand back what it printed on stdout.
    /// A program that exits with an error is an `Err`.
    fn output(&self, program: &str, args: &[&str]) -> std::io::Result<String>;
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

    fn output(&self, program: &str, args: &[&str]) -> std::io::Result<String> {
        let output = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "{program} exited with {}",
                output
                    .status
                    .code()
                    .map_or_else(|| "a signal".to_string(), |code| code.to_string())
            )));
        }
        String::from_utf8(output.stdout)
            .map_err(|_| std::io::Error::other(format!("{program} printed no text")))
    }
}

/// A spawner for the tests: every call is recorded as one line (`program
/// args...`), `run` answers 0 and `output` what `answer` says.
#[cfg(any(test, feature = "testing"))]
pub struct Scripted<F> {
    answer: F,
    calls: std::sync::Mutex<Vec<String>>,
}

#[cfg(any(test, feature = "testing"))]
impl<F> Scripted<F>
where
    F: Fn(&str, &[&str]) -> std::io::Result<String> + Send + Sync,
{
    /// A spawner whose `output` is `answer(program, args)`.
    pub fn new(answer: F) -> Self {
        Scripted {
            answer,
            calls: std::sync::Mutex::default(),
        }
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<String> {
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

#[cfg(any(test, feature = "testing"))]
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
