//! Where the local models run (the embedding model and the contradiction
//! model): on the Metal GPU of an Apple Silicon Mac when one is usable, on the
//! CPU otherwise. Each model makes the pick on its own load.
//!
//! On an Apple Silicon Mac the loader first tries Metal device 0, builds the
//! model there and runs one warm-up forward pass, reading the result back so
//! an error the GPU only reports at sync surfaces here. Any failure on that
//! path (no device, an unsupported op, a failed warm-up) falls back to the CPU
//! with one warn line, and the report says why. Only a failure on the CPU is
//! returned to the caller, so a GPU problem never triggers the corrupt-cache
//! self-heal. Every other platform (Linux, Windows, Intel Macs) runs on the
//! CPU and reports plain `cpu`, because there was nothing to fall back from.
//!
//! `CRYSTALLINE_ACCELERATION=off` forces the CPU.
//!
//! The report type is plain data and exists in every build, so status and
//! doctor can render it without the local model stack compiled in.

use std::fmt;

/// The environment variable that forces the CPU when set to `off`.
pub const ACCELERATION_ENV: &str = "CRYSTALLINE_ACCELERATION";

/// Whether this build can run a local model on a GPU at all: an Apple Silicon
/// Mac with the local model stack compiled in.
pub const ACCELERATION_BUILT_IN: bool = cfg!(all(
    feature = "local-embeddings",
    target_os = "macos",
    target_arch = "aarch64"
));

/// The kind of device a local model runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Cpu,
    Metal,
}

impl DeviceKind {
    /// The short lowercase name status and doctor print.
    pub fn as_str(self) -> &'static str {
        match self {
            DeviceKind::Cpu => "cpu",
            DeviceKind::Metal => "metal",
        }
    }
}

/// Why a model runs on the CPU although this build could use a GPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpuReason {
    /// [`ACCELERATION_ENV`] is `off`.
    Off,
    /// The GPU was tried and refused: the reason, one line.
    Fallback(String),
}

/// The device a loaded model runs on, and why it is the CPU when a GPU was
/// possible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceReport {
    pub kind: DeviceKind,
    pub reason: Option<CpuReason>,
}

impl DeviceReport {
    pub fn cpu() -> DeviceReport {
        DeviceReport {
            kind: DeviceKind::Cpu,
            reason: None,
        }
    }

    pub fn metal() -> DeviceReport {
        DeviceReport {
            kind: DeviceKind::Metal,
            reason: None,
        }
    }

    /// The fallback reason, when the GPU was tried and refused.
    pub fn fallback(&self) -> Option<&str> {
        match &self.reason {
            Some(CpuReason::Fallback(r)) => Some(r),
            _ => None,
        }
    }
}

impl fmt::Display for DeviceReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = self.kind.as_str();
        match &self.reason {
            Some(CpuReason::Fallback(reason)) => write!(f, "{kind} (fallback: {reason})"),
            Some(CpuReason::Off) => write!(f, "{kind} (off by {ACCELERATION_ENV})"),
            None => f.write_str(kind),
        }
    }
}

/// Whether a value of [`ACCELERATION_ENV`] forces the CPU: `off` in any case,
/// surrounding space ignored. Anything else, and no value, leaves the GPU on.
pub fn acceleration_off_value(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim().eq_ignore_ascii_case("off"))
}

/// Whether [`ACCELERATION_ENV`] forces the CPU in this process.
pub fn acceleration_off() -> bool {
    acceleration_off_value(std::env::var(ACCELERATION_ENV).ok().as_deref())
}

/// The device a model load in this process would try, without loading a
/// model: plain CPU where no GPU is built in, the override when it is set,
/// otherwise whether a Metal device opens. `None` on a build without the
/// local model stack, which runs no model at all. `doctor` prints this; it
/// cannot see a warm-up failure, which only a real load (and with it
/// `status`) reports.
pub fn probe() -> Option<DeviceReport> {
    #[cfg(feature = "local-embeddings")]
    {
        Some(loader::probe())
    }
    #[cfg(not(feature = "local-embeddings"))]
    {
        None
    }
}

#[cfg(feature = "local-embeddings")]
pub(crate) use loader::load_on_best_device;
#[cfg(all(feature = "local-embeddings", test))]
pub(crate) use loader::{Accelerator, load_with};

#[cfg(feature = "local-embeddings")]
mod loader {
    use std::any::Any;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use candle_core::Device;

    use super::{ACCELERATION_BUILT_IN, CpuReason, DeviceKind, DeviceReport, acceleration_off};
    use crate::error::Result;

    /// What the loader tries first.
    pub(crate) enum Accelerator {
        /// No GPU on this platform: plain CPU.
        None,
        /// A GPU is possible but switched off by the environment.
        Off,
        /// A GPU could not be opened: CPU, with the reason.
        Unusable(String),
        /// Try this device first.
        ///
        /// Only Apple Silicon builds construct it outside the tests; elsewhere
        /// it is matched but never built, so the lint is allowed there (not
        /// expected: the test build constructs it on every platform).
        #[cfg_attr(
            not(all(target_os = "macos", target_arch = "aarch64")),
            allow(dead_code)
        )]
        Device(Device, DeviceKind),
    }

    /// The accelerator for this process, given whether the environment
    /// switched it off.
    pub(super) fn accelerator(off: bool) -> Accelerator {
        if !ACCELERATION_BUILT_IN {
            return Accelerator::None;
        }
        if off {
            return Accelerator::Off;
        }
        guarded_open(platform_accelerator)
    }

    /// Run `open` and turn a panic into [`Accelerator::Unusable`]: candle's
    /// Metal code unwraps in places, and a GPU problem must never take the
    /// process, or the caller's recovery, with it.
    pub(super) fn guarded_open(open: impl FnOnce() -> Accelerator) -> Accelerator {
        match catch_unwind(AssertUnwindSafe(open)) {
            Ok(accelerator) => accelerator,
            Err(payload) => Accelerator::Unusable(format!(
                "opening the Metal device panicked: {}",
                panic_text(payload.as_ref())
            )),
        }
    }

    /// The message of a caught panic, one line.
    fn panic_text(payload: &(dyn Any + Send)) -> String {
        let text = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "no message".to_string());
        one_line(&text)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn platform_accelerator() -> Accelerator {
        // candle 0.11 indexes the device list without a bounds check, so on a
        // Mac with no Metal device (a virtual machine without a GPU) opening
        // one panics instead of failing. Read the list first.
        if candle_metal_kernels::metal::Device::all().is_empty() {
            return Accelerator::Unusable("no Metal device on this machine".to_string());
        }
        match Device::new_metal(0) {
            Ok(device) => Accelerator::Device(device, DeviceKind::Metal),
            Err(e) => Accelerator::Unusable(format!(
                "no usable Metal device: {}",
                one_line(&e.to_string())
            )),
        }
    }

    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    fn platform_accelerator() -> Accelerator {
        Accelerator::None
    }

    /// [`super::probe`] for a build with the local model stack.
    pub(super) fn probe() -> DeviceReport {
        match accelerator(acceleration_off()) {
            Accelerator::None => DeviceReport::cpu(),
            Accelerator::Off => DeviceReport {
                kind: DeviceKind::Cpu,
                reason: Some(CpuReason::Off),
            },
            Accelerator::Unusable(reason) => DeviceReport {
                kind: DeviceKind::Cpu,
                reason: Some(CpuReason::Fallback(reason)),
            },
            Accelerator::Device(_, kind) => DeviceReport { kind, reason: None },
        }
    }

    /// Build a model on the best device: the accelerator when one is usable
    /// and the model both builds and passes `warm` there, the CPU otherwise.
    /// Only a CPU failure is returned as an error, so a caller's recovery
    /// (the corrupt-cache self-heal) never runs because of a GPU.
    pub(crate) fn load_on_best_device<T>(
        what: &str,
        build: impl Fn(&Device) -> Result<T>,
        warm: impl Fn(&T) -> Result<()>,
    ) -> Result<(T, DeviceReport)> {
        load_with(what, accelerator(acceleration_off()), build, warm)
    }

    /// [`load_on_best_device`] with the accelerator handed in: the seam the
    /// fallback tests drive.
    pub(crate) fn load_with<T>(
        what: &str,
        accelerator: Accelerator,
        build: impl Fn(&Device) -> Result<T>,
        warm: impl Fn(&T) -> Result<()>,
    ) -> Result<(T, DeviceReport)> {
        let reason = match accelerator {
            Accelerator::None => None,
            Accelerator::Off => Some(CpuReason::Off),
            Accelerator::Unusable(reason) => Some(CpuReason::Fallback(reason)),
            Accelerator::Device(device, kind) => {
                // A panic in candle's GPU code (it unwraps in places) is caught
                // here, so it falls back like an error instead of unwinding
                // out of the caller's blocking task, where it would read as a
                // corrupt cache and trigger the re-download.
                let attempt = catch_unwind(AssertUnwindSafe(|| {
                    build(&device).and_then(|model| warm(&model).map(|()| model))
                }));
                match attempt {
                    Ok(Ok(model)) => {
                        tracing::info!("{what}: running on {}", kind.as_str());
                        return Ok((model, DeviceReport { kind, reason: None }));
                    }
                    Ok(Err(e)) => Some(CpuReason::Fallback(format!(
                        "{} failed: {}",
                        kind.as_str(),
                        one_line(&e.to_string())
                    ))),
                    Err(payload) => Some(CpuReason::Fallback(format!(
                        "{} panicked: {}",
                        kind.as_str(),
                        panic_text(payload.as_ref())
                    ))),
                }
            }
        };
        let model = build(&Device::Cpu)?;
        let report = DeviceReport {
            kind: DeviceKind::Cpu,
            reason,
        };
        if report.fallback().is_some() {
            tracing::warn!("{what}: running on {report}");
        } else {
            tracing::info!("{what}: running on {report}");
        }
        Ok((model, report))
    }

    /// The first line of an error, so a report stays one line.
    fn one_line(text: &str) -> String {
        text.lines().next().unwrap_or("").trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_renders_the_device_and_why_it_is_the_cpu() {
        assert_eq!(DeviceReport::cpu().to_string(), "cpu");
        assert_eq!(DeviceReport::metal().to_string(), "metal");
        let fell = DeviceReport {
            kind: DeviceKind::Cpu,
            reason: Some(CpuReason::Fallback(
                "no usable Metal device: none found".to_string(),
            )),
        };
        assert_eq!(
            fell.to_string(),
            "cpu (fallback: no usable Metal device: none found)"
        );
        assert_eq!(fell.fallback(), Some("no usable Metal device: none found"));
        let off = DeviceReport {
            kind: DeviceKind::Cpu,
            reason: Some(CpuReason::Off),
        };
        assert_eq!(off.to_string(), "cpu (off by CRYSTALLINE_ACCELERATION)");
        assert_eq!(off.fallback(), None);
    }

    #[test]
    fn only_off_switches_the_gpu_off() {
        assert!(acceleration_off_value(Some("off")));
        assert!(acceleration_off_value(Some(" OFF ")));
        assert!(!acceleration_off_value(None));
        assert!(!acceleration_off_value(Some("")));
        assert!(!acceleration_off_value(Some("on")));
        assert!(!acceleration_off_value(Some("0")));
    }

    #[test]
    fn a_gpu_is_built_in_only_on_apple_silicon() {
        assert_eq!(
            ACCELERATION_BUILT_IN,
            cfg!(feature = "local-embeddings")
                && cfg!(target_os = "macos")
                && cfg!(target_arch = "aarch64")
        );
    }

    #[cfg(feature = "local-embeddings")]
    mod fallback {
        use std::cell::RefCell;

        use candle_core::Device;

        use super::super::loader::{Accelerator, accelerator, guarded_open, load_with};
        use super::super::{ACCELERATION_BUILT_IN, CpuReason, DeviceKind, DeviceReport};
        use crate::error::IndexError;

        /// The CPU stands in for the GPU: the seam only needs a device the
        /// closures are handed alongside the kind to report.
        fn fake_gpu() -> Accelerator {
            Accelerator::Device(Device::Cpu, DeviceKind::Metal)
        }

        #[test]
        fn a_working_gpu_is_used_and_reported() {
            let (model, report) = load_with("test", fake_gpu(), |_| Ok(7u8), |_| Ok(())).unwrap();
            assert_eq!(model, 7);
            assert_eq!(report, DeviceReport::metal());
        }

        #[test]
        fn a_failed_warm_up_falls_back_to_the_cpu_and_says_why() {
            let builds = RefCell::new(0);
            let (_, report) = load_with(
                "test",
                fake_gpu(),
                |_| {
                    *builds.borrow_mut() += 1;
                    Ok(*builds.borrow())
                },
                |n| {
                    if *n == 1 {
                        Err(IndexError::Embedding("no kernel for op xyz\ndetail".into()))
                    } else {
                        panic!("the CPU path runs no warm-up")
                    }
                },
            )
            .unwrap();
            assert_eq!(*builds.borrow(), 2, "built once on each device");
            assert_eq!(report.kind, DeviceKind::Cpu);
            let reason = report.fallback().unwrap();
            assert!(reason.starts_with("metal failed: "), "{reason}");
            assert!(reason.contains("no kernel for op xyz"), "{reason}");
            assert!(!reason.contains('\n'), "one line: {reason}");
        }

        #[test]
        fn a_failed_build_on_the_gpu_falls_back_to_the_cpu() {
            let calls = RefCell::new(0);
            let (_, report) = load_with(
                "test",
                fake_gpu(),
                |_| {
                    *calls.borrow_mut() += 1;
                    if *calls.borrow() == 1 {
                        Err(IndexError::Embedding("out of memory".into()))
                    } else {
                        Ok(())
                    }
                },
                |_| Ok(()),
            )
            .unwrap();
            assert_eq!(report.kind, DeviceKind::Cpu);
            assert!(report.fallback().unwrap().contains("out of memory"));
        }

        #[test]
        fn a_gpu_that_cannot_be_opened_falls_back_with_the_reason() {
            let (_, report) = load_with(
                "test",
                Accelerator::Unusable("no usable Metal device: none".to_string()),
                |_| Ok(()),
                |_| panic!("the CPU path runs no warm-up"),
            )
            .unwrap();
            assert_eq!(
                report.to_string(),
                "cpu (fallback: no usable Metal device: none)"
            );
        }

        #[test]
        fn the_env_override_runs_on_the_cpu_without_trying_the_gpu() {
            let (_, report) = load_with(
                "test",
                Accelerator::Off,
                |_| Ok(()),
                |_| panic!("the CPU path runs no warm-up"),
            )
            .unwrap();
            assert_eq!(report.reason, Some(CpuReason::Off));
            // And the switch reaches the pick on a build that has a GPU; a
            // build without one reports plain CPU either way.
            let (off, none) = match accelerator(true) {
                Accelerator::Off => (true, false),
                Accelerator::None => (false, true),
                _ => panic!("off must never open a GPU"),
            };
            assert_eq!(off, ACCELERATION_BUILT_IN);
            assert_eq!(none, !ACCELERATION_BUILT_IN);
        }

        #[test]
        fn a_gpu_failure_never_reaches_the_caller_but_a_cpu_failure_does() {
            let calls = RefCell::new(0);
            let err = load_with(
                "test",
                fake_gpu(),
                |_| {
                    *calls.borrow_mut() += 1;
                    Err::<(), _>(IndexError::Embedding(format!(
                        "corrupt weights {}",
                        calls.borrow()
                    )))
                },
                |_| Ok(()),
            )
            .unwrap_err();
            assert_eq!(*calls.borrow(), 2);
            assert!(err.to_string().contains("corrupt weights 2"), "{err}");
        }

        #[test]
        fn a_panic_on_the_gpu_falls_back_to_the_cpu_and_says_why() {
            let builds = RefCell::new(0);
            let (model, report) = load_with(
                "test",
                fake_gpu(),
                |_| {
                    *builds.borrow_mut() += 1;
                    if *builds.borrow() == 1 {
                        panic!("called `Option::unwrap()` on a `None` value\nmore");
                    }
                    Ok(*builds.borrow())
                },
                |_| panic!("the CPU path runs no warm-up"),
            )
            .unwrap();
            assert_eq!(model, 2, "built again on the CPU");
            let reason = report.fallback().unwrap();
            assert!(reason.starts_with("metal panicked: "), "{reason}");
            assert!(reason.contains("unwrap()"), "{reason}");
            assert!(!reason.contains('\n'), "one line: {reason}");
        }

        #[test]
        fn a_panicking_warm_up_falls_back_to_the_cpu() {
            let builds = RefCell::new(0);
            let (_, report) = load_with(
                "test",
                fake_gpu(),
                |_| {
                    *builds.borrow_mut() += 1;
                    Ok(*builds.borrow())
                },
                |n| {
                    if *n == 1 {
                        panic!("{}", String::from("command buffer lost"));
                    }
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(report.kind, DeviceKind::Cpu);
            assert!(
                report.fallback().unwrap().contains("command buffer lost"),
                "{report}"
            );
        }

        #[test]
        fn a_panicking_device_open_is_an_unusable_gpu() {
            match guarded_open(|| panic!("index out of bounds: the len is 0")) {
                Accelerator::Unusable(reason) => {
                    assert!(
                        reason.starts_with("opening the Metal device panicked: "),
                        "{reason}"
                    );
                    assert!(reason.contains("the len is 0"), "{reason}");
                }
                _ => panic!("a panic must read as an unusable GPU"),
            }
            assert!(matches!(
                guarded_open(|| Accelerator::None),
                Accelerator::None
            ));
        }

        #[test]
        fn a_platform_without_a_gpu_reports_plain_cpu() {
            let (_, report) = load_with("test", Accelerator::None, |_| Ok(()), |_| Ok(())).unwrap();
            assert_eq!(report, DeviceReport::cpu());
        }

        /// Linux, Windows and Intel Macs never open a GPU, whatever the
        /// environment says.
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        #[test]
        fn a_non_apple_silicon_build_always_picks_the_cpu() {
            const { assert!(!ACCELERATION_BUILT_IN) };
            assert!(matches!(accelerator(false), Accelerator::None));
            assert!(matches!(accelerator(true), Accelerator::None));
        }
    }
}
