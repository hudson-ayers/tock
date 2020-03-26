//! Multilevel feedback queue scheduler for Tock
//! Based on the MLFQ rules described in "Operating Systems: Three Easy Pieces"
//! By Remzi H. Arpaci-Dusseau and Andrea C. Arpaci-Dusseau
//!
//! This scheduler can be summarized by the following rules:
//!
//! Rule 1: If Priority(A) > Priority(B), A runs (B doesn’t).
//! Rule 2: If Priority(A) = Priority(B), A & B run in round-robin fashion using the
//!         time slice (quantum length) of the given queue.
//! Rule 3: When a job enters the system, it is placed at the highest priority (the topmost queue).
//! Rule 4: Once a job uses up its time allotment at a given level (regardless of how
//!         many times it has given up the CPU), its priority is reduced
//!         (i.e., it moves down one queue).
//! Rule 5: After some time period S, move all the jobs in the system to the topmost queue.

use crate::capabilities;
use crate::common::dynamic_deferred_call::DynamicDeferredCall;
use crate::common::list::{List, ListLink, ListNode};
use crate::debug;
use crate::hil::time;
use crate::hil::time::Frequency;
use crate::ipc;
use crate::platform::mpu::MPU;
use crate::platform::systick::SysTick;
use crate::platform::{Chip, Platform};
use crate::process::{self, ProcessType};
use crate::sched::{Kernel, Scheduler};
use crate::sched::{ProcessCollection, ProcessIter};
use crate::syscall::{ContextSwitchReason, Syscall};
use core::cell::Cell;

#[derive(Default)]
struct MfProcState {
    /// Total CPU time used by this process while in current queue
    us_used_this_queue: Cell<u32>,
}

/// Multilevel Feedback Queue Scheduler (MLFQ)
pub struct MLFQProcessNode {
    process: Cell<Option<&'static dyn ProcessType>>, // required bc List does not have mutable references to Nodes
    state: MfProcState,
    next: ListLink<'static, MLFQProcessNode>,
}

impl MLFQProcessNode {
    pub fn new() -> MLFQProcessNode {
        MLFQProcessNode {
            process: Cell::new(None),
            state: MfProcState::default(),
            next: ListLink::empty(),
        }
    }
}

impl ListNode<'static, MLFQProcessNode> for MLFQProcessNode {
    fn next(&'static self) -> &'static ListLink<'static, MLFQProcessNode> {
        &self.next
    }
}

pub struct MLFQSched {
    kernel: &'static Kernel,
    processes: &'static ProcessMultiQueues,
}

impl MLFQSched {
    /// Skip re-scheduling a process if its quanta is nearly exhausted
    const MIN_QUANTA_THRESHOLD_US: u32 = 500;
    /// How often to restore all processes to max priority
    pub const PRIORITY_REFRESH_PERIOD_MS: u32 = 5000;
    pub fn new(kernel: &'static Kernel, processes: &'static ProcessMultiQueues) -> Self {
        Self {
            kernel: kernel,
            processes: processes,
        }
    }

    fn get_timeslice_us(&self, queue_idx: usize) -> u32 {
        match queue_idx {
            0 => 10000,
            1 => 20000,
            2 => 50000,
            _ => panic!("invalid queue idx"),
        }
    }

    fn redeem_all_procs(&self) {
        let mut first = true;
        for queue in self.processes.queues.iter() {
            if first {
                continue;
            }
            first = false;
            match queue.pop_head() {
                Some(proc) => self.processes.queues[0].push_tail(proc),
                None => continue,
            }
        }
    }

    // Note: This do_process differs from the original version in Tock in that it services
    // processes until a yield w/o callbacks or a Timeslice expiration -- it does not break to handle the
    // bottom half of interrupts immediately
    unsafe fn do_process<P: Platform, C: Chip>(
        &mut self,
        platform: &P,
        chip: &C,
        process: &dyn process::ProcessType,
        ipc: Option<&crate::ipc::IPC>,
        timeslice: u32,
        node_ref: &'static MLFQProcessNode,
    ) -> Option<ContextSwitchReason> {
        let appid = process.appid();
        let systick = chip.systick();
        systick.reset();
        systick.set_timer(timeslice);
        systick.enable(false);
        let mut switch_reason_opt = None;

        loop {
            if systick.overflowed()
                || !systick.greater_than(Self::MIN_QUANTA_THRESHOLD_US)
                || node_ref.state.us_used_this_queue.get() > Self::PRIORITY_REFRESH_PERIOD_MS * 1000
            {
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
                    let remaining = systick.get_value();
                    systick.enable(false); //disables systick interrupts
                    chip.mpu().disable_mpu();
                    node_ref
                        .state
                        .us_used_this_queue
                        .set(node_ref.state.us_used_this_queue.get() + (timeslice - remaining));
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
                            // this scheduler defers bottom half handling until yield w/ no
                            // callbacks or timeslice expiration
                            continue;
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
        switch_reason_opt
    }
}

/// Store processes in ready/Wait queues implemented as statically allocated linked lists
pub struct ProcessMultiQueues {
    queues: [&'static mut List<'static, MLFQProcessNode>; Self::NUM_QUEUES],
    num_procs: usize,
    index: Cell<usize>,
    iter_cnt: Cell<usize>,
}

impl ProcessMultiQueues {
    /// Number of Priority Levels
    pub const NUM_QUEUES: usize = 3;
    pub fn new(queues: [&'static mut List<'static, MLFQProcessNode>; Self::NUM_QUEUES]) -> Self {
        Self {
            num_procs: 0,
            queues: queues,
            index: Cell::new(0),
            iter_cnt: Cell::new(0),
        }
    }

    /// Returns the process at the head of the highest priority queue containing a process
    /// that is ready to execute (as determined by `has_tasks()`)
    /// This method moves that node to the head of its queue.
    fn get_next_ready_process_node(&self) -> (Option<&MLFQProcessNode>, usize) {
        for (idx, queue) in self.queues.iter().enumerate() {
            let next = queue
                .iter()
                .find(|node_ref| node_ref.process.get().map_or(false, |proc| proc.ready()));
            if next.is_some() {
                // pop procs to back until we get to match
                loop {
                    let cur = queue.pop_head();
                    match cur {
                        Some(node) => {
                            if node as *const _ == next.unwrap() as *const _ {
                                queue.push_head(node);
                                // match! Put back on front
                                return (next, idx);
                            } else {
                                queue.push_tail(node);
                            }
                        }
                        None => {}
                    }
                }
            }
        }
        (None, 0)
    }
}

impl ProcessCollection for ProcessMultiQueues {
    fn load_process_with_id(&mut self, proc: Option<&'static dyn ProcessType>, idx: usize) {
        let mut i = 0;

        for node in self.queues.iter().flat_map(|it| it.iter()) {
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
        self.queues
            .iter()
            .flat_map(|it| it.iter())
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
        for node in self.queues.iter().flat_map(|it| it.iter()) {
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
        self.queues.iter().flat_map(|it| it.iter()).count()
    }

    fn active(&self) -> usize {
        self.queues
            .iter()
            .flat_map(|it| it.iter())
            .fold(0, |acc, node| {
                if node.process.get().is_some() {
                    acc + 1
                } else {
                    acc
                }
            })
    }
}

impl Scheduler for MLFQSched {
    type Collection = ProcessMultiQueues;

    /// Main loop.
    fn kernel_loop<P: Platform, C: Chip, A: time::Alarm<'static>>(
        &'static mut self,
        platform: &P,
        chip: &C,
        ipc: Option<&ipc::IPC>,
        alarm: &A,
        _capability: &dyn capabilities::MainLoopCapability,
    ) {
        let delta = (Self::PRIORITY_REFRESH_PERIOD_MS * A::Frequency::frequency()) / 1000;
        let mut next_reset = alarm.now().wrapping_add(delta);
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
                    let now = alarm.now();
                    if now >= next_reset {
                        // Promote all processes to highest priority queue
                        let delta =
                            (Self::PRIORITY_REFRESH_PERIOD_MS * A::Frequency::frequency()) / 1000;
                        next_reset = now.wrapping_add(delta);
                        self.redeem_all_procs();
                    }
                    let (node_ref_opt, queue_idx) = self.processes.get_next_ready_process_node();
                    match node_ref_opt {
                        Some(node_ref) => {
                            let mut punish = false;
                            node_ref.process.get().map(|process| {
                                let switch_reason = self.do_process(
                                    platform,
                                    chip,
                                    process,
                                    ipc,
                                    self.get_timeslice_us(queue_idx),
                                    node_ref,
                                );

                                punish =
                                    switch_reason == Some(ContextSwitchReason::TimesliceExpired);
                            });
                            if punish {
                                node_ref.state.us_used_this_queue.set(0);
                                let next_queue = if queue_idx == ProcessMultiQueues::NUM_QUEUES - 1
                                {
                                    queue_idx
                                } else {
                                    queue_idx + 1
                                };
                                self.processes.queues[next_queue].push_tail(
                                    self.processes.queues[queue_idx].pop_head().unwrap(),
                                );
                            } else {
                                self.processes.queues[queue_idx].push_tail(
                                    self.processes.queues[queue_idx].pop_head().unwrap(),
                                );
                            }
                        }
                        None => panic!("Logic err: processes_blocked()=false, no ready process"),
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
