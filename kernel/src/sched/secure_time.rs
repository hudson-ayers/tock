//! Secure Time Scheduler

use crate::common::list::{List, ListLink, ListNode};
use crate::platform::Chip;
use crate::procs::ProcessType;
use crate::sched::{Kernel, Scheduler, SchedulingDecision, StoppedExecutingReason};
use crate::syscall::Syscall;
use core::cell::Cell;

/// Types of kernel tasks for which WCET requests can be issued.
pub enum KernelTask {
    BottomHalfInterrupt { interrupt: u8 },
    TopHalfInterrupt { interrupt: u8 },
    SystemCall(Syscall),
}

/// A node in the linked list the scheduler uses to track processes
/// Each node holds a pointer to a slot in the processes array
pub struct STProcessNode<'a> {
    proc: &'static Option<&'static dyn ProcessType>,
    next: ListLink<'a, STProcessNode<'a>>,
}

impl<'a> STProcessNode<'a> {
    pub fn new(proc: &'static Option<&'static dyn ProcessType>) -> STProcessNode<'a> {
        STProcessNode {
            proc,
            next: ListLink::empty(),
        }
    }
}

impl<'a> ListNode<'a, STProcessNode<'a>> for STProcessNode<'a> {
    fn next(&'a self) -> &'a ListLink<'a, STProcessNode> {
        &self.next
    }
}

/// Secure Time Scheduler
pub struct SecureTimeSched<'a, F>
where
    F: FnOnce(KernelTask) -> u32,
{
    time_remaining: Cell<u32>,
    pub processes: List<'a, STProcessNode<'a>>,
    last_rescheduled: Cell<bool>,
    wcet_lookup_func: F,
}

impl<'a, F: FnOnce(KernelTask) -> u32> SecureTimeSched<'a, F> {
    /// How long a process can run before being pre-empted
    const DEFAULT_TIMESLICE_US: u32 = 10000;
    pub const fn new(wcet_lookup_func: F) -> Self {
        Self {
            time_remaining: Cell::new(Self::DEFAULT_TIMESLICE_US),
            processes: List::new(),
            last_rescheduled: Cell::new(false),
            wcet_lookup_func,
        }
    }
}

impl<'a, C: Chip, F: FnOnce(KernelTask) -> u32> Scheduler<C> for SecureTimeSched<'a, F> {
    fn next(&self, kernel: &Kernel) -> SchedulingDecision {
        if kernel.processes_blocked() {
            // No processes ready
            SchedulingDecision::TrySleep
        } else {
            let mut next = None; // This will be replaced, bc a process is guaranteed
                                 // to be ready if processes_blocked() is false

            // Find next ready process. Place any *empty* process slots, or not-ready
            // processes, at the back of the queue.
            for node in self.processes.iter() {
                match node.proc {
                    Some(proc) => {
                        if proc.ready() {
                            next = Some(proc.appid());
                            break;
                        }
                        self.processes.push_tail(self.processes.pop_head().unwrap());
                    }
                    None => {
                        self.processes.push_tail(self.processes.pop_head().unwrap());
                    }
                }
            }
            let timeslice = if self.last_rescheduled.get() {
                self.time_remaining.get()
            } else {
                // grant a fresh timeslice
                self.time_remaining.set(Self::DEFAULT_TIMESLICE_US);
                Self::DEFAULT_TIMESLICE_US
            };
            assert!(timeslice != 0);

            SchedulingDecision::RunProcess((next.unwrap(), Some(timeslice)))
        }
    }

    fn result(&self, result: StoppedExecutingReason, execution_time_us: Option<u32>) {
        let execution_time_us = execution_time_us.unwrap(); // should never fail
        let reschedule = match result {
            StoppedExecutingReason::KernelPreemption => {
                if self.time_remaining.get() > execution_time_us {
                    self.time_remaining
                        .set(self.time_remaining.get() - execution_time_us);
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        self.last_rescheduled.set(reschedule);
        if !reschedule {
            self.processes.push_tail(self.processes.pop_head().unwrap());
        }
    }
}
