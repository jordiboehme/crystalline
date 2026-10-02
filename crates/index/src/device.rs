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
//! The pick also covers the time after the load. When an inference call on
//! the GPU fails or panics after a good load and warm-up, the model's
//! [`ModelSlot`] logs one warn line, reloads that model on the CPU once,
//! retries the failed call there and keeps the model on the CPU for the rest
//! of the process; the report then says `cpu (metal failed at runtime: ..)`.
//! Each model has its own slot, so the other model keeps its device.
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
    /// The model loaded on the GPU, then an inference call failed there and
    /// the model moved to the CPU for the rest of the process: the whole
    /// phrase, one line (`metal failed at runtime: <error>`).
    Runtime(String),
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

    /// The fallback reason, when the GPU was tried and refused at load.
    pub fn fallback(&self) -> Option<&str> {
        match &self.reason {
            Some(CpuReason::Fallback(r)) => Some(r),
            _ => None,
        }
    }

    /// Why the model left the GPU after a good load, when it did.
    pub fn runtime_failure(&self) -> Option<&str> {
        match &self.reason {
            Some(CpuReason::Runtime(r)) => Some(r),
            _ => None,
        }
    }
}

impl fmt::Display for DeviceReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = self.kind.as_str();
        match &self.reason {
            Some(CpuReason::Fallback(reason)) => write!(f, "{kind} (fallback: {reason})"),
            Some(CpuReason::Runtime(reason)) => write!(f, "{kind} ({reason})"),
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
pub use slot::ModelSlot;

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
    pub(super) fn panic_text(payload: &(dyn Any + Send)) -> String {
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
    pub(super) fn one_line(text: &str) -> String {
        text.lines().next().unwrap_or("").trim().to_string()
    }
}

#[cfg(feature = "local-embeddings")]
mod slot {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

    use super::loader::{one_line, panic_text};
    use super::{CpuReason, DeviceKind, DeviceReport};
    use crate::error::{IndexError, Result};

    /// Rebuilds a model on the CPU, with no warm-up and no self-heal.
    type Rebuild<T> = Box<dyn Fn() -> Result<T> + Send + Sync>;

    /// One loaded model and the device it runs on, moved to the CPU when the
    /// GPU fails after a good load.
    ///
    /// [`ModelSlot::run`] hands every inference call the current model. A call
    /// on the CPU runs as it is. A call on the GPU runs under `catch_unwind`;
    /// when it errors or panics, the slot logs one warn line, rebuilds the
    /// model on the CPU, swaps it in with a [`CpuReason::Runtime`] report and
    /// retries the call once there. From then on nothing calls the GPU for
    /// this model until the process restarts.
    ///
    /// Concurrency: the model sits behind an `Arc` that a call clones under a
    /// short read lock, so no lock is held across inference (a caught panic
    /// never poisons one). The swap takes its own mutex and checks again
    /// under it, so requests that fail on the GPU at the same time cause one
    /// rebuild: the first does it, the others find the CPU model and retry on
    /// it. A request still running on the old GPU model finishes there or
    /// fails and retries on the CPU; the GPU model is dropped with its last
    /// request.
    ///
    /// When the CPU rebuild fails, the GPU model is still given up: the call
    /// returns a clear error, and the next call tries the rebuild again
    /// instead of the GPU.
    pub struct ModelSlot<T> {
        what: &'static str,
        live: RwLock<Live<T>>,
        swap: Mutex<()>,
        rebuild: Rebuild<T>,
        error: fn(String) -> IndexError,
        injected: AtomicUsize,
    }

    struct Live<T> {
        /// `None` once the GPU model was given up and the CPU rebuild failed.
        model: Option<Arc<T>>,
        report: DeviceReport,
    }

    impl<T: Send + Sync> ModelSlot<T> {
        /// A slot for `model`, loaded on the device `report` names. `rebuild`
        /// builds the same model on the CPU; `error` labels the slot's own
        /// errors the way the model's other errors are labeled.
        pub fn new(
            what: &'static str,
            model: T,
            report: DeviceReport,
            error: fn(String) -> IndexError,
            rebuild: impl Fn() -> Result<T> + Send + Sync + 'static,
        ) -> ModelSlot<T> {
            ModelSlot {
                what,
                live: RwLock::new(Live {
                    model: Some(Arc::new(model)),
                    report,
                }),
                swap: Mutex::new(()),
                rebuild: Box::new(rebuild),
                error,
                injected: AtomicUsize::new(0),
            }
        }

        /// The device the model runs on now, with the reason when it is the
        /// CPU on a machine that has a GPU.
        pub fn report(&self) -> DeviceReport {
            self.read().report.clone()
        }

        /// Run one inference call on the current model, moving the model to
        /// the CPU and retrying once there when the call fails on the GPU.
        pub fn run<R>(&self, call: impl Fn(&T) -> Result<R>) -> Result<R> {
            let (model, kind) = {
                let live = self.read();
                (live.model.clone(), live.report.kind)
            };
            let cause = match model {
                Some(model) if kind == DeviceKind::Cpu => return call(&model),
                Some(model) => {
                    let attempt = catch_unwind(AssertUnwindSafe(|| {
                        if self.take_injected() {
                            return Err((self.error)("injected runtime failure".to_string()));
                        }
                        call(&model)
                    }));
                    match attempt {
                        Ok(Ok(out)) => return Ok(out),
                        Ok(Err(e)) => Some(format!(
                            "{} failed at runtime: {}",
                            kind.as_str(),
                            one_line(&e.to_string())
                        )),
                        Err(payload) => Some(format!(
                            "{} panicked at runtime: {}",
                            kind.as_str(),
                            panic_text(payload.as_ref())
                        )),
                    }
                }
                None => None,
            };
            let cpu = self.move_to_cpu(cause)?;
            call(&cpu)
        }

        /// Make the next `calls` inference calls on the GPU fail as if the GPU
        /// had, before they reach the model: the seam the runtime fallback is
        /// tested through on any machine. A model on the CPU ignores it.
        #[doc(hidden)]
        pub fn inject_runtime_failure(&self, calls: usize) {
            self.injected.store(calls, Ordering::SeqCst);
        }

        fn take_injected(&self) -> bool {
            self.injected
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        }

        /// The CPU model, rebuilding it once when the slot still holds the GPU
        /// model (or none). `cause` is the failure that gave up the GPU; `None`
        /// when an earlier call already gave it up.
        fn move_to_cpu(&self, cause: Option<String>) -> Result<Arc<T>> {
            let _swap = self.swap.lock().unwrap_or_else(|e| e.into_inner());
            {
                let mut live = self.write();
                if live.report.kind == DeviceKind::Cpu
                    && let Some(model) = &live.model
                {
                    return Ok(model.clone());
                }
                if live.report.kind != DeviceKind::Cpu {
                    let reason = cause.unwrap_or_else(|| {
                        format!("{} failed at runtime", live.report.kind.as_str())
                    });
                    tracing::warn!(
                        "{}: {reason}; moving it to the cpu for the rest of this process",
                        self.what
                    );
                    live.model = None;
                    live.report = DeviceReport {
                        kind: DeviceKind::Cpu,
                        reason: Some(CpuReason::Runtime(reason)),
                    };
                }
            }
            let built = match catch_unwind(AssertUnwindSafe(|| (self.rebuild)())) {
                Ok(built) => built,
                Err(payload) => Err((self.error)(format!(
                    "the cpu rebuild panicked: {}",
                    panic_text(payload.as_ref())
                ))),
            };
            match built {
                Ok(model) => {
                    let model = Arc::new(model);
                    self.write().model = Some(model.clone());
                    tracing::info!("{}: running on the cpu", self.what);
                    Ok(model)
                }
                Err(e) => Err((self.error)(format!(
                    "the {} left the gpu after a runtime failure and could not be loaded on \
                     the cpu: {e}",
                    self.what
                ))),
            }
        }

        fn read(&self) -> RwLockReadGuard<'_, Live<T>> {
            self.live.read().unwrap_or_else(|e| e.into_inner())
        }

        fn write(&self) -> RwLockWriteGuard<'_, Live<T>> {
            self.live.write().unwrap_or_else(|e| e.into_inner())
        }
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
        let runtime = DeviceReport {
            kind: DeviceKind::Cpu,
            reason: Some(CpuReason::Runtime(
                "metal failed at runtime: inference: lost".to_string(),
            )),
        };
        assert_eq!(
            runtime.to_string(),
            "cpu (metal failed at runtime: inference: lost)"
        );
        assert_eq!(runtime.fallback(), None);
        assert_eq!(
            runtime.runtime_failure(),
            Some("metal failed at runtime: inference: lost")
        );
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

    /// The runtime fallback, with a stub model whose "GPU" instance fails on
    /// demand: no GPU needed, so it runs on Linux CI as well.
    #[cfg(feature = "local-embeddings")]
    mod runtime {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};

        use super::super::{CpuReason, DeviceKind, DeviceReport, ModelSlot};
        use crate::error::{IndexError, Result};

        /// What the stub's two instances count and how the GPU one fails.
        #[derive(Default)]
        struct Counts {
            gpu_calls: AtomicUsize,
            cpu_calls: AtomicUsize,
            rebuilds: AtomicUsize,
        }

        struct Stub {
            gpu: bool,
            counts: Arc<Counts>,
        }

        impl Stub {
            /// The doubled input, counted per device.
            fn double(&self, x: u32) -> u32 {
                let calls = if self.gpu {
                    &self.counts.gpu_calls
                } else {
                    &self.counts.cpu_calls
                };
                calls.fetch_add(1, Ordering::SeqCst);
                x * 2
            }
        }

        /// A slot holding the GPU instance, rebuilding a CPU one.
        fn gpu_slot(counts: &Arc<Counts>) -> ModelSlot<Stub> {
            let rebuild_counts = counts.clone();
            ModelSlot::new(
                "test model",
                Stub {
                    gpu: true,
                    counts: counts.clone(),
                },
                DeviceReport::metal(),
                IndexError::Embedding,
                move || {
                    rebuild_counts.rebuilds.fetch_add(1, Ordering::SeqCst);
                    Ok(Stub {
                        gpu: false,
                        counts: rebuild_counts.clone(),
                    })
                },
            )
        }

        /// Fails on the GPU instance with a two-line error, works on the CPU.
        fn gpu_breaks(stub: &Stub, x: u32) -> Result<u32> {
            let out = stub.double(x);
            if stub.gpu {
                Err(IndexError::Embedding(
                    "inference: command buffer error\ndetail".into(),
                ))
            } else {
                Ok(out)
            }
        }

        #[test]
        fn no_failure_means_no_change() {
            let counts = Arc::new(Counts::default());
            let slot = gpu_slot(&counts);
            for x in 0..5 {
                assert_eq!(slot.run(|m| Ok(m.double(x))).unwrap(), x * 2);
            }
            assert_eq!(slot.report(), DeviceReport::metal());
            assert_eq!(counts.gpu_calls.load(Ordering::SeqCst), 5);
            assert_eq!(counts.rebuilds.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn a_gpu_error_moves_the_model_to_the_cpu_and_retries_there() {
            let counts = Arc::new(Counts::default());
            let slot = gpu_slot(&counts);
            assert_eq!(slot.run(|m| gpu_breaks(m, 21)).unwrap(), 42, "retried");
            let report = slot.report();
            assert_eq!(report.kind, DeviceKind::Cpu);
            let reason = report.runtime_failure().unwrap();
            assert_eq!(
                reason,
                "metal failed at runtime: embedding error: inference: command buffer error"
            );
            assert_eq!(
                report.to_string(),
                format!("cpu ({reason})"),
                "status shows the runtime reason"
            );
            assert_eq!(report.fallback(), None, "not a load-time fallback");

            // Nothing calls the GPU again.
            assert_eq!(slot.run(|m| gpu_breaks(m, 5)).unwrap(), 10);
            assert_eq!(counts.gpu_calls.load(Ordering::SeqCst), 1);
            assert_eq!(counts.cpu_calls.load(Ordering::SeqCst), 2);
            assert_eq!(counts.rebuilds.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn a_gpu_panic_moves_the_model_to_the_cpu() {
            let counts = Arc::new(Counts::default());
            let slot = gpu_slot(&counts);
            let out = slot
                .run(|m| {
                    if m.gpu {
                        panic!("called `Result::unwrap()` on an `Err` value\nmore");
                    }
                    Ok(m.double(4))
                })
                .unwrap();
            assert_eq!(out, 8);
            let reason = slot.report().runtime_failure().unwrap().to_string();
            assert!(
                reason.starts_with("metal panicked at runtime: "),
                "{reason}"
            );
            assert!(reason.contains("unwrap()"), "{reason}");
            assert!(!reason.contains('\n'), "one line: {reason}");
        }

        #[test]
        fn an_injected_failure_reaches_the_fallback_without_touching_the_model() {
            let counts = Arc::new(Counts::default());
            let slot = gpu_slot(&counts);
            slot.inject_runtime_failure(1);
            assert_eq!(slot.run(|m| Ok(m.double(3))).unwrap(), 6);
            assert_eq!(counts.gpu_calls.load(Ordering::SeqCst), 0);
            assert_eq!(counts.cpu_calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                slot.report().to_string(),
                "cpu (metal failed at runtime: embedding error: injected runtime failure)"
            );
        }

        #[test]
        fn a_model_on_the_cpu_returns_its_error_and_never_rebuilds() {
            let counts = Arc::new(Counts::default());
            let slot = ModelSlot::new(
                "test model",
                Stub {
                    gpu: false,
                    counts: counts.clone(),
                },
                DeviceReport::cpu(),
                IndexError::Embedding,
                || panic!("a CPU model is never rebuilt"),
            );
            slot.inject_runtime_failure(5);
            assert_eq!(
                slot.run(|m| Ok(m.double(1))).unwrap(),
                2,
                "injection ignored"
            );
            let err = slot
                .run(|_| Err::<u32, _>(IndexError::Embedding("tokenizing: bad".into())))
                .unwrap_err();
            assert!(err.to_string().contains("tokenizing: bad"), "{err}");
            assert_eq!(slot.report(), DeviceReport::cpu());
        }

        #[test]
        fn a_failed_cpu_rebuild_errors_clearly_and_the_gpu_stays_given_up() {
            let counts = Arc::new(Counts::default());
            let attempts = Arc::new(AtomicUsize::new(0));
            let (rebuild_counts, rebuild_attempts) = (counts.clone(), attempts.clone());
            let slot = ModelSlot::new(
                "test model",
                Stub {
                    gpu: true,
                    counts: counts.clone(),
                },
                DeviceReport::metal(),
                IndexError::Embedding,
                move || {
                    if rebuild_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                        Err(IndexError::Embedding("loading weights: gone".into()))
                    } else {
                        Ok(Stub {
                            gpu: false,
                            counts: rebuild_counts.clone(),
                        })
                    }
                },
            );
            let err = slot.run(|m| gpu_breaks(m, 1)).unwrap_err().to_string();
            assert!(err.contains("could not be loaded on the cpu"), "{err}");
            assert!(err.contains("loading weights: gone"), "{err}");
            assert_eq!(slot.report().kind, DeviceKind::Cpu);

            // The next call tries the rebuild again, never the GPU.
            assert_eq!(slot.run(|m| gpu_breaks(m, 2)).unwrap(), 4);
            assert_eq!(counts.gpu_calls.load(Ordering::SeqCst), 1);
            assert_eq!(attempts.load(Ordering::SeqCst), 2);
            assert!(matches!(slot.report().reason, Some(CpuReason::Runtime(_))));
        }

        /// Every request is in flight on the GPU when it fails: one rebuild,
        /// every request answered on the CPU, no hang.
        #[test]
        fn concurrent_failures_cause_exactly_one_rebuild() {
            const REQUESTS: usize = 8;
            let counts = Arc::new(Counts::default());
            let slot = Arc::new(gpu_slot(&counts));
            let barrier = Arc::new(Barrier::new(REQUESTS));
            let handles: Vec<_> = (0..REQUESTS as u32)
                .map(|x| {
                    let (slot, barrier) = (slot.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        slot.run(|m| {
                            if m.gpu {
                                // All requests hold the GPU model before any
                                // of them fails.
                                barrier.wait();
                            }
                            gpu_breaks(m, x)
                        })
                    })
                })
                .collect();
            let mut answers: Vec<u32> = handles
                .into_iter()
                .map(|h| h.join().unwrap().unwrap())
                .collect();
            answers.sort_unstable();
            let want: Vec<u32> = (0..REQUESTS as u32).map(|x| x * 2).collect();
            assert_eq!(answers, want);
            assert_eq!(counts.rebuilds.load(Ordering::SeqCst), 1, "one reload");
            assert_eq!(counts.gpu_calls.load(Ordering::SeqCst), REQUESTS);
            assert_eq!(counts.cpu_calls.load(Ordering::SeqCst), REQUESTS);
            assert_eq!(slot.report().kind, DeviceKind::Cpu);
        }
    }
}
