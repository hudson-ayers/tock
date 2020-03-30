//! Round Robin Scheduler for Tock

use crate::callback::AppId;
use crate::capabilities;
use crate::common::dynamic_deferred_call::DynamicDeferredCall;
use crate::common::list::{List, ListLink, ListNode};
use crate::ipc;
use crate::platform::mpu::MPU;
use crate::platform::systick::SysTick;
use crate::platform::{Chip, Platform};
use crate::process;
use crate::sched::{Kernel, Scheduler};
use crate::syscall::{ContextSwitchReason, Syscall};

pub struct RRProcessNode<'a> {
    appid: AppId, // TODO: need to add back Cell here?
    next: ListLink<'a, RRProcessNode<'a>>,
}

impl<'a> RRProcessNode<'a> {
    pub fn new(appid: AppId) -> RRProcessNode<'a> {
        RRProcessNode {
            appid: appid,
            next: ListLink::empty(),
        }
    }
}

impl<'a> ListNode<'a, RRProcessNode<'a>> for RRProcessNode<'a> {
    fn next(&'a self) -> &'a ListLink<'a, RRProcessNode> {
        &self.next
    }
}

pub struct RoundRobinSched<'a> {
    kernel: &'static Kernel,
    pub processes: List<'a, RRProcessNode<'a>>,
}

impl<'a> RoundRobinSched<'a> {
    /// How long a process can run before being pre-empted
    const DEFAULT_TIMESLICE_US: u32 = 10000;
    /// Skip re-scheduling a process if its quanta is nearly exhausted
    const MIN_QUANTA_THRESHOLD_US: u32 = 500;
    pub const fn new(kernel: &'static Kernel) -> RoundRobinSched<'a> {
        RoundRobinSched {
            kernel: kernel,
            processes: List::new(),
        }
    }

    unsafe fn do_process<P: Platform, C: Chip>(
        &self,
        platform: &P,
        chip: &C,
        process: &dyn process::ProcessType,
        ipc: Option<&crate::ipc::IPC>,
        rescheduled: bool,
    ) -> Option<ContextSwitchReason> {
        let systick = chip.systick();
        let mut remaining = 0;

        if !rescheduled {
            systick.reset();
            systick.set_timer(Self::DEFAULT_TIMESLICE_US);
            systick.enable(false);
        } else {
            systick.enable(false); // just resume from when interrupted
        }
        let mut switch_reason_opt = None;

        loop {
            if chip.has_pending_interrupts() {
                break;
            }
            if systick.overflowed() || !systick.greater_than(Self::MIN_QUANTA_THRESHOLD_US) {
                switch_reason_opt = Some(ContextSwitchReason::TimesliceExpired);
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
                    systick.enable(true); //Enables systick interrupts
                    let context_switch_reason = process.switch_to();
                    remaining = systick.get_value();
                    systick.enable(false); //disables systick interrupts
                    chip.mpu().disable_mpu();
                    switch_reason_opt = context_switch_reason;

                    // Now the process has returned back to the kernel. Check
                    // why and handle the process as appropriate.
                    self.kernel
                        .process_return(context_switch_reason, process, platform);
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
        if switch_reason_opt == Some(ContextSwitchReason::Interrupted) {
            systick.reset(); // stop counting down
            systick.set_timer(remaining); // store remaining time in systick register
        }
        switch_reason_opt
    }
}

impl<'a> Scheduler for RoundRobinSched<'a> {
    /// Main loop.
    fn kernel_loop<P: Platform, C: Chip>(
        &mut self,
        platform: &P,
        chip: &C,
        ipc: Option<&ipc::IPC>,
        _capability: &dyn capabilities::MainLoopCapability,
    ) {
        let mut reschedule = false;
        loop {
            unsafe {
                chip.service_pending_interrupts();
                DynamicDeferredCall::call_global_instance_while(|| !chip.has_pending_interrupts());

                loop {
                    if chip.has_pending_interrupts()
                        || DynamicDeferredCall::global_instance_calls_pending().unwrap_or(false)
                        || self.kernel.processes_blocked()
                    {
                        break;
                    }
                    let next = self.processes.head().unwrap().appid;
                    let last_rescheduled = reschedule;
                    reschedule = false;
                    self.kernel.process_map_or((), next, |process| {
                        let switch_reason =
                            self.do_process(platform, chip, process, ipc, last_rescheduled);
                        reschedule = switch_reason == Some(ContextSwitchReason::Interrupted);
                    });
                    if !reschedule {
                        self.processes.push_tail(self.processes.pop_head().unwrap());
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
