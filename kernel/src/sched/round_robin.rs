//! Round Robin Scheduler for Tock

use crate::capabilities;
use crate::common::dynamic_deferred_call::DynamicDeferredCall;
use crate::ipc;
use crate::platform::mpu::MPU;
use crate::platform::systick::SysTick;
use crate::platform::{Chip, Platform};
use crate::process::{self, ProcessType};
use crate::sched::ProcessContainer;
use crate::sched::{Kernel, Scheduler};
use crate::syscall::{ContextSwitchReason, Syscall};
use core::cell::Cell;

// TODO: Not requiring this state to be Copy should be possible and more efficient

/// Stores per process state when using the round robin scheduler
#[derive(Copy, Clone, Default)]
pub struct RRProcState {
    /// To prevent unfair situations that can be created by one app consistently
    /// scheduling an interrupt, yeilding, and then interrupting the subsequent app
    /// shortly after it begins executing, we track the portion of a timeslice that a process
    /// has used and allow it to continue after being interrupted.
    us_used_this_timeslice: u32,
}

// Currently relies on assumption that x processes will reside in first x slots of process array
pub struct RoundRobinSched {
    kernel: &'static Kernel,
    num_procs_installed: usize,
    next_up: Cell<usize>,
    proc_states: &'static mut [Option<RRProcState>],
    //processes: &'static [Option<&'static dyn process::ProcessType>],
    processes: &'static ProcessArray,
}

impl RoundRobinSched {
    /// How long a process can run before being pre-empted
    const DEFAULT_TIMESLICE_US: u32 = 10000;
    /// Skip re-scheduling a process if its quanta is nearly exhausted
    const MIN_QUANTA_THRESHOLD_US: u32 = 500;
    pub fn new(
        kernel: &'static Kernel,
        proc_states: &'static mut [Option<RRProcState>],
        processes: &'static ProcessArray,
    ) -> RoundRobinSched {
        //have to initialize proc state bc default() sets them to None
        let mut num_procs = 0;
        for (i, s) in proc_states.iter_mut().enumerate() {
            if processes.processes[i].is_some() {
                num_procs += 1;
                *s = Some(Default::default());
            }
        }
        RoundRobinSched {
            kernel: kernel,
            num_procs_installed: num_procs,
            next_up: Cell::new(0),
            proc_states: proc_states,
            processes: processes,
        }
    }

    unsafe fn do_process<P: Platform, C: Chip>(
        &mut self,
        platform: &P,
        chip: &C,
        process: &dyn process::ProcessType,
        ipc: Option<&crate::ipc::IPC>,
        proc_timeslice_us: u32,
    ) -> (bool, Option<ContextSwitchReason>) {
        let appid = process.appid();
        let systick = chip.systick();
        systick.reset();
        systick.set_timer(proc_timeslice_us);
        systick.enable(false);
        //track that process was given a chance to execute (bc of case where process has a callback
        //waiting, the callback is handled, then interrupt arrives can cause process not to get a
        //chance to run if that callback being handled puts it in the running state)
        let mut given_chance = false;
        let mut switch_reason_opt = None;
        let mut first = true;

        loop {
            // if this is the first time this loop has iterated, dont break in the
            // case of interrupts. This allows for the scheduler to schedule processes
            // even with interrupts pending if it so chooses.
            if !first {
                if chip.has_pending_interrupts() {
                    break;
                }
            } else {
                first = false;
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
                    given_chance = true;
                    process.setup_mpu();
                    chip.mpu().enable_mpu();
                    systick.enable(true); //Enables systick interrupts
                    let context_switch_reason = process.switch_to();
                    let us_used = proc_timeslice_us - systick.get_value();
                    systick.enable(false); //disables systick interrupts
                    chip.mpu().disable_mpu();
                    self.proc_states[appid.idx()]
                        .as_mut()
                        .map(|mut state| state.us_used_this_timeslice += us_used);
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
                        given_chance = true;
                        break;
                    }
                    Some(cb) => self.kernel.handle_callback(cb, process, ipc),
                },
                process::State::Fault => {
                    // We should never be scheduling a process in fault.
                    panic!("Attempted to schedule a faulty process");
                }
                process::State::StoppedRunning => {
                    given_chance = true;
                    break;
                    // Do nothing
                }
                process::State::StoppedYielded => {
                    given_chance = true;
                    break;
                    // Do nothing
                }
                process::State::StoppedFaulted => {
                    given_chance = true;
                    break;
                    // Do nothing
                }
            }
        }
        systick.reset();
        (given_chance, switch_reason_opt)
    }
}

pub struct ProcessArray {
    processes: &'static [Option<&'static dyn process::ProcessType>],
    index: Cell<usize>,
}

impl ProcessArray {
    pub fn new(processes: &'static [Option<&'static dyn process::ProcessType>]) -> Self {
        Self {
            processes: processes,
            index: Cell::new(0),
        }
    }
}

struct IterProcessArray {
    inner: &'static ProcessArray,
    pos: usize,
}

impl Iterator for IterProcessArray {
    type Item = core::option::Option<&'static dyn ProcessType>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.inner.processes.len() {
            None
        } else {
            self.pos += 1;
            Some(self.inner.processes[self.pos - 1])
        }
    }
}

/*
impl IntoIterator for &'static ProcessArray {
    type Item = &'static core::option::Option<&'static dyn ProcessType>;
    type IntoIter =
        &'static mut core::slice::Iter<'static, core::option::Option<&'static dyn ProcessType>>;

    fn into_iter(&mut self) -> Self::IntoIter {
        self.processes.into_iter()
    }
}
*/

impl ProcessContainer for ProcessArray {
    fn get_proc_by_id(&self, process_index: usize) -> Option<&dyn ProcessType> {
        self.processes[process_index]
    }
    fn next(&self) -> Option<&dyn ProcessType> {
        let mut idx = self.index.get();

        if idx >= self.processes.len() {
            return None;
        }

        while self.processes[idx].is_none() {
            if idx < self.processes.len() - 1 {
                idx += 1;
            }
        }
        self.index.set(idx + 1);
        return self.processes[idx];
    }
    fn reset(&self) {
        self.index.set(0);
    }
    /*
    fn iter(&self) -> IterProcessArray {
        IterProcessArray {
            inner: self,
            pos: 0,
        }
    }

    /// Run a closure on a specific process if it exists. If the process does
    /// not exist (i.e. it is `None` in the `processes` array) then `default`
    /// will be returned. Otherwise the closure will executed and passed a
    /// reference to the process.
    fn process_map_or<F, R>(&self, default: R, process_index: usize, closure: F) -> R
    where
        F: FnOnce(&dyn process::ProcessType) -> R,
    {
        if process_index > self.processes.len() {
            return default;
        }
        self.processes[process_index].map_or(default, |process| closure(process))
    }

    /// Run a closure on every valid process. This will iterate the array of
    /// processes and call the closure on every process that exists.
    fn process_each<F>(&self, closure: F)
    where
        F: Fn(&dyn process::ProcessType),
    {
        for process in self.processes.iter() {
            match process {
                Some(p) => {
                    closure(*p);
                }
                None => {}
            }
        }
    }

    /// Run a closure on every process, but only continue if the closure returns
    /// `FAIL`. That is, if the closure returns any other return code than
    /// `FAIL`, that value will be returned from this function and the iteration
    /// of the array of processes will stop.
    fn process_until<F>(&self, closure: F) -> ReturnCode
    where
        F: Fn(&dyn process::ProcessType) -> ReturnCode,
    {
        for process in self.processes.iter() {
            match process {
                Some(p) => {
                    let ret = closure(*p);
                    if ret != ReturnCode::FAIL {
                        return ret;
                    }
                }
                None => {}
            }
        }
        ReturnCode::FAIL
    }

    /// Run a closure on every valid process. This will iterate the
    /// array of processes and call the closure on every process that
    /// exists. Ths method is available outside the kernel crate but
    /// requires a `ProcessManagementCapability` to use.
    fn process_each_capability<F>(
        &'static self,
        _capability: &dyn capabilities::ProcessManagementCapability,
        closure: F,
    ) where
        F: Fn(usize, &dyn process::ProcessType),
    {
        for (i, process) in self.processes.iter().enumerate() {
            match process {
                Some(p) => {
                    closure(i, *p);
                }
                None => {}
            }
        }
    }
    */

    /// Return how many processes this board supports.
    fn len(&self) -> usize {
        self.processes.len()
    }

    fn active(&self) -> usize {
        self.processes
            .iter()
            .fold(0, |acc, p| if p.is_some() { acc + 1 } else { acc })
    }
}

impl Scheduler for RoundRobinSched {
    type ProcessState = RRProcState;
    //type Container = ProcessArray;

    /// Main loop.
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

                loop {
                    let next = self.next_up.get();
                    if chip.has_pending_interrupts()
                        || DynamicDeferredCall::global_instance_calls_pending().unwrap_or(false)
                        || self.kernel.processes_blocked()
                        || self.num_procs_installed == 0
                    {
                        break;
                    }
                    self.processes.processes[next].map(|process| {
                        let timeslice_us = Self::DEFAULT_TIMESLICE_US
                            - self.proc_states[next].unwrap().us_used_this_timeslice;
                        let (given_chance, switch_reason) =
                            self.do_process(platform, chip, process, ipc, timeslice_us);

                        if given_chance {
                            let mut reschedule = false;
                            let used_so_far =
                                self.proc_states[next].unwrap().us_used_this_timeslice;
                            if switch_reason == Some(ContextSwitchReason::Interrupted) {
                                if Self::DEFAULT_TIMESLICE_US - used_so_far
                                    >= Self::MIN_QUANTA_THRESHOLD_US
                                {
                                    self.proc_states[next].as_mut().map(|mut state| {
                                        state.us_used_this_timeslice = used_so_far;
                                    });
                                    reschedule = true; //Was interrupted before using entire timeslice!
                                }
                                // want to inform scheduler of time passed and to reschedule
                            }
                            if !reschedule {
                                self.proc_states[next].as_mut().map(|mut state| {
                                    state.us_used_this_timeslice = 0;
                                });
                                if next < self.num_procs_installed - 1 {
                                    self.next_up.set(next + 1);
                                } else {
                                    self.next_up.set(0);
                                }
                            }
                        }
                    });
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
