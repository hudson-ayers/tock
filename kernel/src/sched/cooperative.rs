//! Cooperative Scheduler for Tock
//! When hardware interrupts occur while a userspace process is executing,
//! this scheduler executes the top half of the interrupt,
//! and then stops executing the userspace process immediately and handles the bottom
//! half of the interrupt. However it then continues executing the same userspace process
//! that was executing.

use crate::callback::AppId;
use crate::capabilities;
use crate::common::dynamic_deferred_call::DynamicDeferredCall;
use crate::common::list::{List, ListLink, ListNode};
use crate::ipc;
use crate::platform::mpu::MPU;
use crate::platform::{Chip, Platform};
use crate::process;
use crate::sched::{Kernel, Scheduler};
use crate::syscall::{ContextSwitchReason, Syscall};

/// A node in the linked list the scheduler uses to track processes
pub struct CoopProcessNode<'a> {
    appid: AppId,
    next: ListLink<'a, CoopProcessNode<'a>>,
}

impl<'a> CoopProcessNode<'a> {
    pub fn new(appid: AppId) -> CoopProcessNode<'a> {
        CoopProcessNode {
            appid: appid,
            next: ListLink::empty(),
        }
    }
}

impl<'a> ListNode<'a, CoopProcessNode<'a>> for CoopProcessNode<'a> {
    fn next(&'a self) -> &'a ListLink<'a, CoopProcessNode> {
        &self.next
    }
}

/// Cooperative Scheduler
pub struct CooperativeSched<'a> {
    kernel: &'static Kernel,
    pub processes: List<'a, CoopProcessNode<'a>>,
}

impl<'a> CooperativeSched<'a> {
    pub const fn new(kernel: &'static Kernel) -> CooperativeSched<'a> {
        CooperativeSched {
            kernel: kernel,
            processes: List::new(),
        }
    }

    /// Executes a process until it yields or is interrupted. Returns whether it was interrupted.
    unsafe fn do_process<P: Platform, C: Chip>(
        &self,
        platform: &P,
        chip: &C,
        process: &dyn process::ProcessType,
        ipc: Option<&crate::ipc::IPC>,
    ) -> bool {
        loop {
            if chip.has_pending_interrupts() {
                break;
            }

            match process.get_state() {
                process::State::Running => {
                    // Running means that this process expects to be running,
                    // so go ahead and set things up and switch to executing
                    // the process.
                    process.setup_mpu();
                    chip.mpu().enable_mpu();
                    let context_switch_reason = process.switch_to();
                    chip.mpu().disable_mpu();

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
                        Some(ContextSwitchReason::Interrupted) => {
                            // break to handle the bottom half of the interrupt
                            return true;
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
        false
    }
}

impl<'a> Scheduler for CooperativeSched<'a> {
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
                    self.kernel.process_map_or((), next, |process| {
                        reschedule = self.do_process(platform, chip, process, ipc);
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
