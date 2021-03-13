//! Secure Time Scheduler

use crate::common::list::{List, ListLink, ListNode};
use crate::hil::time;
use crate::hil::time::Ticks;
use crate::platform::Chip;
use crate::procs::ProcessType;
use crate::sched::{Kernel, Scheduler, SchedulingDecision, StoppedExecutingReason};
use crate::syscall::Syscall;
use core::cell::Cell;

pub const MAX_TASKS: usize = 20; // could be made board configurable

/// Types of kernel tasks for which WCET requests can be issued.
pub enum KernelTask {
    BottomHalfInterrupt { interrupt: u8 },
    TopHalfInterrupt { interrupt: u8 },
    SystemCall(Syscall),
}

pub struct KernelWork {
    work_type: KernelTask,
    duration: u32,
    deadline: u32,
}

pub struct UserspaceResult {
    appid: crate::AppId,
    result: crate::syscall::GenericSyscallReturnValue, // TODO: Should this be set earlier
    deadline: u32,
}

/// Types of work this scheduler might want to schedule
pub enum Task {
    /// Non-preemptible, in-kernel work. This includes system calls and bottom-half
    /// interrupt handlers that have not yet run.
    Kernel(KernelWork),

    /// After a system call or bottom half interrupt runs, there may be results
    /// that need to be delivered to a process *at* a given deadline. Delivery of
    /// results is separate from performing the work that generates the results,
    /// and must be scheduled separately.
    UserspaceResultDelivery(UserspaceResult),

    /// A userspace process that is ready to execute, but has no deadline.
    /// This could be a process that has never started, or a process that was
    /// preempted before issuing a system call.
    ReadyProcess(crate::AppId),
}

/// A node in the linked list the scheduler uses to track processes
/// Each node holds a pointer to a slot in the processes array
pub struct STTaskNode<'a> {
    task: Option<Task>,
    next: ListLink<'a, STTaskNode<'a>>,
}

impl<'a> STTaskNode<'a> {
    pub fn new(task: Option<Task>) -> Self {
        Self {
            task,
            next: ListLink::empty(),
        }
    }
}

impl<'a> ListNode<'a, STTaskNode<'a>> for STTaskNode<'a> {
    fn next(&'a self) -> &'a ListLink<'a, STTaskNode> {
        &self.next
    }
}

/// Secure Time Scheduler
pub struct SecureTimeSched<'a, F, A>
where
    F: FnOnce(KernelTask) -> u32,
    A: 'static + time::Alarm<'static>,
{
    alarm: &'static A,
    time_remaining: Cell<u32>,

    /// Ordered list of tasks that are ready to execute
    pub ready_tasks: List<'a, STTaskNode<'a>>,

    /// Ordered list of tasks which could become ready to execute at
    /// any time in response to an interrupt
    pub potential_tasks: List<'a, STTaskNode<'a>>,
    last_rescheduled: Cell<bool>,
    wcet_lookup_func: F,
}

impl<'a, F: FnOnce(KernelTask) -> u32, A: 'static + time::Alarm<'static>>
    SecureTimeSched<'a, F, A>
{
    /// How long a process can run before being pre-empted
    const DEFAULT_TIMESLICE_US: u32 = 1000;
    pub const fn new(alarm: &'static A, wcet_lookup_func: F) -> Self {
        Self {
            alarm,
            time_remaining: Cell::new(Self::DEFAULT_TIMESLICE_US),
            ready_tasks: List::new(),
            potential_tasks: List::new(),
            last_rescheduled: Cell::new(false),
            wcet_lookup_func,
        }
    }

    /// Helper function that determines if the secure time
    /// scheduler can spend `time_us` microseconds on a new task
    /// without making it possible for outstanding tasks to miss their deadline
    fn can_afford_slack(&self, _time_us: u32) -> bool {
        unimplemented!();
    }

    /// Helper function that returns the most urgent outstanding task.
    /// This is the task that should be serviced next to avoid any
    /// tasks missing their deadlines.
    fn get_most_urgent_task(&self) -> Option<Task> {
        unimplemented!();
    }

    /// Helper function that returns a userspace process which is
    /// ready to execute and has no deadline at which it should be
    /// scheduled. If multiple userspace processes fall in this
    /// category, they should be selected in a round-robin order.
    fn get_next_ready_process(&self) -> Option<Task> {
        unimplemented!();
    }

    /// Helper function that returns `true` if there are userspace
    /// processes which are ready to execute and have no deadline,
    /// and `false` otherwise.
    fn no_deadlines_processes_waiting(&self) -> bool {
        unimplemented!();
    }

    /// Function that returns the next `Task` which should be run.
    fn schedule_next(&self) -> Option<Task> {
        if self.no_deadlines_processes_waiting()
            && self.can_afford_slack(Self::DEFAULT_TIMESLICE_US)
        {
            self.get_next_ready_process()
        } else {
            self.get_most_urgent_task()
        }
    }
}

impl<'a, C: Chip, F: FnOnce(KernelTask) -> u32, A: 'static + time::Alarm<'static>> Scheduler<C>
    for SecureTimeSched<'a, F, A>
{
    fn next(&self, _kernel: &Kernel) -> SchedulingDecision {
        match self.schedule_next() {
            Some(Task::UserspaceResultDelivery(proc)) => {
                // TODO: Handle wraparound, deadline types/units.
                while self.alarm.now().into_u32() < proc.deadline {} //spin!
                SchedulingDecision::RunProcess((proc.appid, Some(Self::DEFAULT_TIMESLICE_US)))
            }
            Some(Task::ReadyProcess(proc)) => {
                SchedulingDecision::RunProcess((proc, Some(Self::DEFAULT_TIMESLICE_US)))
            }
            Some(Task::Kernel(_)) => {
                SchedulingDecision::TrySleep // TODO: verify main loop will not
                                             //sleep if kernel tasks are ready
            }
            None => {
                // No processes ready
                SchedulingDecision::TrySleep
            }
        }
    }

    fn result(&self, result: StoppedExecutingReason, execution_time_us: Option<u32>) {
        unimplemented!();
    }

    // this approach is inefficient compared to one where the core kernel
    // scheduler is changed further, but this allows us to get the behavior we
    // want without modifying sched.rs
    unsafe fn do_kernel_work_now(&self, _chip: &C) -> bool {
        match self.schedule_next() {
            Some(Task::Kernel(_)) => true,
            _ => false,
        }
    }

    unsafe fn execute_kernel_work(&self, chip: &C) {
        match self.schedule_next() {
            Some(Task::Kernel(_)) => {
                unimplemented!();
                // TODO: handle interrupt handlers and system calls here
            }
            _ => panic!("Should not be called if kernel work is not the next task"),
        }
    }
}
