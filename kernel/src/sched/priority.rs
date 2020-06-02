//! Tock Round Robin Scheduler

use crate::calback::AppId;
use crate::common::list::{List, ListLink, ListNode};
use core::cell::Cell;

// A node in the linked list the scheduler uses to track processes
pub struct RoundRobinProcessNode<'a> {
    appid: AppId,
    next: ListLink<'a, RoundRobinProcessNode<'a>>,
}

impl<'a> RoundRobinProcessNode<'a> {
    pub fn new(appid: AppId) -> RoundRobinProcessNode<'a> {
        RoundRobinProcessNode {
            appid: appid,
            next: ListLink::empty(),
        }
    }
}

impl<'a> ListNode<'a, RoundRobinProcessNode<'a>> for RoundRobinProcessNode<'a> {
    fn next(&'a self) -> &'a ListLink<'a, RoundRobinProcessNode> {
        &self.next
    }
}
/// Crude priority scheduler
pub(crate) struct RoundRobinSched<'a> {
    /// Structure used internally to track next
    proc_list: List<'a, RoundRobinProcessNode<'a>>,

    /// This holds a pointer to the static array of Process pointers.
    processes: &'static [Option<&'static dyn process::ProcessType>],

    running: Cell<usize>,
}

impl<'a> RoundRobinSched<'a> {
    /// How long a process can run before being pre-empted
    const DEFAULT_TIMESLICE_US: u32 = 10000;
    /// Skip re-scheduling a process if its quanta is nearly exhausted
    const MIN_QUANTA_THRESHOLD_US: u32 = 500;

    pub fn new(processes: &'static [Option<&'static dyn process::ProcessType>]) -> Self {
        Self {
            processes,
            running: Cell::new(0),
        }
    }
}

impl Scheduler for RoundRobinSched {
    fn dispatcher(&self) -> Option<&'static dyn process::ProcessType> {
        while self.kernel_tasks_ready() {
            self.service_kernel_tasks();
        }
        self.processes[running]
    }

    fn finalizer(&self) {
        let last = self.running.get();
        let next = if last == self.processes.len() - 1 {
            0
        } else {
            last + 1
        };
        self.running.set(next);
        if self.can_sleep() {
            self.sleep();
        }
    }
}
