//! Round Robin Scheduler for Tock

use crate::capabilities;
use crate::common::dynamic_deferred_call::DynamicDeferredCall;
use crate::common::list::{List, ListLink, ListNode};
use crate::debug;
use crate::ipc;
use crate::platform::mpu::MPU;
use crate::platform::systick::SysTick;
use crate::platform::{Chip, Platform};
use crate::process::{self, ProcessType};
use crate::sched::{Kernel, Scheduler};
use crate::sched::{ProcessCollection, ProcessIter};
use crate::syscall::{ContextSwitchReason, Syscall};
use core::cell::Cell;

/// Stores per process state when using the round robin scheduler
#[derive(Default)]
pub struct RRProcState {}

pub struct ProcessNode {
    process: Cell<Option<&'static dyn ProcessType>>, // required bc List does not have mutable references to Nodes
    state: RRProcState,
    next: ListLink<'static, ProcessNode>,
}

impl ProcessNode {
    pub fn new() -> ProcessNode {
        ProcessNode {
            process: Cell::new(None),
            state: RRProcState::default(),
            next: ListLink::empty(),
        }
    }
}

impl ListNode<'static, ProcessNode> for ProcessNode {
    fn next(&'static self) -> &'static ListLink<'static, ProcessNode> {
        &self.next
    }
}

// Currently relies on assumption that x processes will reside in first x slots of process array
pub struct RoundRobinSched {
    kernel: &'static Kernel,
    num_procs_installed: usize,
    processes: &'static ProcessRWQueues,
}

impl RoundRobinSched {
    /// How long a process can run before being pre-empted
    const DEFAULT_TIMESLICE_US: u32 = 10000;
    /// Skip re-scheduling a process if its quanta is nearly exhausted
    const MIN_QUANTA_THRESHOLD_US: u32 = 500;
    pub fn new(kernel: &'static Kernel, processes: &'static ProcessRWQueues) -> RoundRobinSched {
        RoundRobinSched {
            kernel: kernel,
            num_procs_installed: processes.active(),
            processes: processes,
        }
    }

    unsafe fn do_process<P: Platform, C: Chip>(
        &mut self,
        platform: &P,
        chip: &C,
        process: &dyn process::ProcessType,
        ipc: Option<&crate::ipc::IPC>,
        rescheduled: bool,
    ) -> Option<ContextSwitchReason> {
        let appid = process.appid();
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
        if switch_reason_opt == Some(ContextSwitchReason::Interrupted) {
            systick.reset(); // stop counting down
            systick.set_timer(remaining); // store remaining time in systick register
        }
        switch_reason_opt
    }
}

/// Store processes in ready/Wait queues implemented as statically allocated linked lists
pub struct ProcessRWQueues {
    processes: &'static mut List<'static, ProcessNode>,
    num_procs: usize,
    index: Cell<usize>,
    iter_cnt: Cell<usize>,
}

impl ProcessRWQueues {
    pub fn new(processes: &'static mut List<'static, ProcessNode>) -> Self {
        Self {
            num_procs: 0,
            processes: processes,
            index: Cell::new(0),
            iter_cnt: Cell::new(0),
        }
    }
}

impl ProcessCollection for ProcessRWQueues {
    fn load_process_with_id(&mut self, proc: Option<&'static dyn ProcessType>, idx: usize) {
        let mut i = 0;
        for node in self.processes.iter() {
            if i == idx {
                node.process.set(proc);
                self.num_procs += 1;
                return;
            } else {
                i += 1;
            }
        }
        panic!("Failed to load process");
    }

    fn get_proc_by_id(&self, process_index: usize) -> Option<&'static dyn ProcessType> {
        self.processes
            .iter()
            .find(|proc_node| {
                proc_node
                    .process
                    .get()
                    .map_or(false, |proc| proc.appid().idx() == process_index)
            })
            .map_or(None, |node| node.process.get())
    }

    // Should only be used by ProcessIter
    fn next(&self) -> Option<&dyn ProcessType> {
        let idx = self.index.get();
        let mut i = 0;
        for node in self.processes.iter() {
            if node.process.get().is_none() {
                continue;
            } else if i == idx {
                self.index.set(idx + 1);
                return node.process.get();
            } else {
                i += 1;
            }
        }
        None
    }

    // Should only be used by ProcessIter
    fn reset(&self) {
        self.iter_cnt.set(self.iter_cnt.get() - 1);
        self.index.set(0);
    }

    //Used to iterate over all existing processes
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
        self.processes.iter().count()
    }

    fn active(&self) -> usize {
        self.processes.iter().fold(0, |acc, node| {
            if node.process.get().is_some() {
                acc + 1
            } else {
                acc
            }
        })
    }
}

impl Scheduler for RoundRobinSched {
    type ProcessState = RRProcState;
    type Collection = ProcessRWQueues;

    /// Main loop.
    fn kernel_loop<P: Platform, C: Chip>(
        &'static mut self,
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
                        || self.num_procs_installed == 0
                    {
                        break;
                    }
                    let node_ref = self.processes.processes.head().unwrap();
                    let last_rescheduled = reschedule;
                    reschedule = false;
                    node_ref.process.get().map(|process| {
                        let switch_reason =
                            self.do_process(platform, chip, process, ipc, last_rescheduled);
                        reschedule = switch_reason == Some(ContextSwitchReason::Interrupted);
                    });
                    if !reschedule {
                        self.processes
                            .processes
                            .push_tail(self.processes.processes.pop_head().unwrap());
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
