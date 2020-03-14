//! Implementation of the Tock scheduler that existed prior to this PR. This scheduler has
//! significant flaws -- higher priority apps with specific interrupt timings can indefinitely
//! starve lower priority apps, for example -- but leaving this in to show it is still possible
//! and to allow for testing that seperates out whether any breaking changes that result are
//! a result of the changed interface or the changed scheduling behavior.

use crate::capabilities;
use crate::common::dynamic_deferred_call::DynamicDeferredCall;
use crate::debug;
use crate::ipc;
use crate::platform::mpu::MPU;
use crate::platform::systick::SysTick;
use crate::platform::{Chip, Platform};
use crate::process::{self, ProcessType};
use crate::sched::{Kernel, ProcessCollection, ProcessIter, Scheduler};
use crate::syscall::{ContextSwitchReason, Syscall};
use core::cell::Cell;

/// Priority Scheduler requires no additional per-process state
/// All ProcessState types must implement default
#[derive(Default)]
pub struct EmptyProcState {}

pub struct ProcessArray {
    inner: &'static mut [(Option<&'static dyn process::ProcessType>, EmptyProcState)],
    index: Cell<usize>,
    iter_cnt: Cell<usize>,
}

impl ProcessArray {
    pub fn new(
        processes: &'static mut [(Option<&'static dyn process::ProcessType>, EmptyProcState)],
    ) -> Self {
        Self {
            inner: processes,
            index: Cell::new(0),
            iter_cnt: Cell::new(0),
        }
    }
}

impl ProcessCollection for ProcessArray {
    fn load_process_with_id(&mut self, proc: Option<&'static dyn ProcessType>, idx: usize) {
        self.inner[idx] = (proc, EmptyProcState {});
    }

    fn get_proc_by_id(&self, process_index: usize) -> Option<&'static dyn ProcessType> {
        self.inner[process_index].0
    }

    // Should only be used by ProcessIter
    fn next(&self) -> Option<&dyn ProcessType> {
        let mut idx = self.index.get();

        if idx >= self.inner.len() {
            return None;
        }

        while self.inner[idx].0.is_none() {
            if idx < self.inner.len() - 1 {
                idx += 1;
            } else {
                return None;
            }
        }
        self.index.set(idx + 1);
        return self.inner[idx].0;
    }

    // Should only be used by ProcessIter
    fn reset(&self) {
        self.iter_cnt.set(self.iter_cnt.get() - 1);
        self.index.set(0);
    }

    /// Get an iterator over all active processes
    fn iter(&'static self) -> Option<ProcessIter> {
        if self.iter_cnt.get() != 0 {
            debug!("Logical Error -- iter called twice");
            None
        } else {
            self.iter_cnt.set(1); //take lock on iterating the Container
            let ret = ProcessIter { inner: self };
            Some(ret)
        }
    }

    /// Return how many processes this board supports.
    fn len(&self) -> usize {
        self.inner.len()
    }

    fn active(&self) -> usize {
        self.inner
            .iter()
            .fold(0, |acc, p| if p.0.is_some() { acc + 1 } else { acc })
    }
}

pub struct PrioritySched {
    kernel: &'static Kernel,
    processes: &'static ProcessArray,
}

impl PrioritySched {
    /// How long a process can run before being pre-empted
    const DEFAULT_TIMESLICE_US: u32 = 10000;
    /// Skip re-scheduling a process if its quanta is nearly exhausted
    const MIN_QUANTA_THRESHOLD_US: u32 = 500;
    pub fn new(kernel: &'static Kernel, processes: &'static ProcessArray) -> PrioritySched {
        PrioritySched {
            kernel: kernel,
            processes: processes,
        }
    }

    unsafe fn do_process<P: Platform, C: Chip>(
        &self,
        platform: &P,
        chip: &C,
        process: &dyn process::ProcessType,
        ipc: Option<&crate::ipc::IPC>,
    ) {
        let appid = process.appid();
        let systick = chip.systick();
        systick.reset();
        systick.set_timer(Self::DEFAULT_TIMESLICE_US);
        systick.enable(false);

        loop {
            if chip.has_pending_interrupts() {
                break;
            }

            if systick.overflowed() || !systick.greater_than(Self::MIN_QUANTA_THRESHOLD_US) {
                process.debug_timeslice_expired();
                break;
            }

            match process.get_state() {
                process::State::Running => {
                    // Running means that this process expects to be running,
                    // so go ahead and set things up and switch to executing
                    // the process.
                    process.setup_mpu();
                    chip.mpu().enable_mpu();
                    systick.enable(true);
                    let context_switch_reason = process.switch_to();
                    systick.enable(false);
                    chip.mpu().disable_mpu();

                    // Now the process has returned back to the kernel. Check
                    // why and handle the process as appropriate.
                    self.kernel
                        .process_return(appid, context_switch_reason, process, platform);
                    match context_switch_reason {
                        Some(ContextSwitchReason::SyscallFired {
                            syscall: Syscall::YIELD,
                        }) => {
                            // There might be already enqueued callbacks
                            continue;
                        }
                        Some(ContextSwitchReason::TimesliceExpired) => {
                            // break to handle other processes
                            break;
                        }
                        Some(ContextSwitchReason::Interrupted) => {
                            // break to handle other processes
                            break;
                        }
                        _ => {}
                    }
                }
                process::State::Yielded | process::State::Unstarted => match process.dequeue_task()
                {
                    // If the process is yielded it might be waiting for a
                    // callback. If there is a task scheduled for this process
                    // go ahead and set the process to execute it.
                    None => {
                        break;
                    }
                    Some(cb) => self.kernel.handle_callback(cb, process, ipc),
                },
                process::State::Fault => {
                    // We should never be scheduling a process in fault.
                    panic!("Attempted to schedule a faulty process");
                }
                process::State::StoppedRunning => {
                    break;
                    // Do nothing
                }
                process::State::StoppedYielded => {
                    break;
                    // Do nothing
                }
                process::State::StoppedFaulted => {
                    break;
                    // Do nothing
                }
            }
        }
        systick.reset();
    }
}

impl Scheduler for PrioritySched {
    type ProcessState = EmptyProcState;

    fn kernel_loop<P: Platform, C: Chip>(
        &'static mut self,
        platform: &P,
        chip: &C,
        ipc: Option<&ipc::IPC>,
        _capability: &dyn capabilities::MainLoopCapability,
    ) {
        loop {
            unsafe {
                chip.service_pending_interrupts();
                DynamicDeferredCall::call_global_instance_while(|| !chip.has_pending_interrupts());

                for p in self.processes.inner.iter() {
                    p.0.map(|process| {
                        self.do_process(platform, chip, process, ipc);
                    });
                    if chip.has_pending_interrupts()
                        || DynamicDeferredCall::global_instance_calls_pending().unwrap_or(false)
                    {
                        break;
                    }
                }

                chip.atomic(|| {
                    if !chip.has_pending_interrupts()
                        && !DynamicDeferredCall::global_instance_calls_pending().unwrap_or(false)
                        && self.kernel.processes_blocked()
                    {
                        chip.sleep();
                    }
                });
            };
        }
    }
}
