//! Tock core scheduler. Defines the central Kernel struct and a trait that
//! different scheduler implementations must implement. Also defines several
//! utility functions to reduce repeated code between different scheduler
//! implementations.

//crate mod multilevel_feedback;
//crate mod priority;
crate mod round_robin;

use core::cell::Cell;
use core::ptr::NonNull;

use crate::callback::{Callback, CallbackId};
use crate::capabilities;
use crate::common::cells::NumericCellExt;
use crate::config;
use crate::debug;
use crate::grant::Grant;
use crate::ipc;
use crate::memop;
use crate::platform::{Chip, Platform};
use crate::process::{self, ProcessType, Task};
use crate::returncode::ReturnCode;
use crate::syscall::{ContextSwitchReason, Syscall};

// Allow different schedulers to store processes in any container
// they choose (Array, Multiple Queues, etc.)
// Also stores scheduler-specific ProcessState alongside Process
//
// TODO: Is this better than scheduling algorithms just storing queues etc. of appids but leaving
// process array unchanged? TBD
//
// Okay, I need to think about this. To let underlying type of ProcessContainer change
// scheduler needs to actually hold the container?
//
// Now I think IntoIterator is no good bc of restrictions on IntoIter type making it not actually
// useful, think I need it to be Iterator afterall...wait no, next(&self mut) is no good..
//
// TODO: Container -> Collection
//
// Looks like collection traits may not actually be an option in Rust. Next best thing may be just
// an iterator? Scheduler could have a reference to whatever data structure, and kernel gets
// iterator of processes? This would break any optimizations based on indexes (but I think this is
// true for any collection trait with the move to appId?)
pub trait ProcessContainer {
    //type ProcessState; // needs to match ProcessState of Scheduler in use
    //def want to store State alongside Process, TODO

    //fn as_any(&self) -> &dyn Any; //required so that scheduler can cast to concrete type used
    //fn new() -> Self; //Needed so static_init!() call doesnt change based on scheduler in use
    //

    fn get_proc_by_id(&self, process_index: usize) -> Option<&dyn ProcessType>;
    fn next(&self) -> Option<&dyn ProcessType>;
    fn reset(&self);

    //fn iter(&self) -> dyn Iterator<Item = core::option::Option<&dyn ProcessType>>;
    //fn iter_proc_state(&self) -> Iterator<ProcessState>; seperate out process state for now
    /*
    fn process_map_or<F, R>(&self, default: R, process_index: usize, closure: F) -> R
    where
        F: FnOnce(&dyn process::ProcessType) -> R;
    fn process_each<F>(&self, closure: F)
    where
        F: Fn(&dyn process::ProcessType);

    fn process_until<F>(&self, closure: F) -> ReturnCode
    where
        F: Fn(&dyn process::ProcessType) -> ReturnCode;

    fn process_each_capability<F>(
        &'static self,
        _capability: &dyn capabilities::ProcessManagementCapability,
        closure: F,
    ) where
        F: Fn(usize, &dyn process::ProcessType);
    */

    fn len(&self) -> usize;
    // consider function to iterate over process IDs then punt over everything to process_map_or
    fn active(&self) -> usize; // Number of process slots occupied
}

pub trait Scheduler {
    type ProcessState;
    //type Container: ProcessContainer;
    //TODO: Add function called when number of processes on board changes, to future-proof
    //for dynamic loading of apps

    //TODO: Move new() into this interface?

    fn kernel_loop<P: Platform, C: Chip>(
        &'static mut self,
        platform: &P,
        chip: &C,
        ipc: Option<&ipc::IPC>,
        _capability: &dyn capabilities::MainLoopCapability,
    );
    //fn processes(&self) -> &'static dyn ProcessContainer;
}

// New idea: back to ProcessContainer trait
/// Main object for the kernel. Each board will need to create one.
pub struct Kernel {
    /// How many "to-do" items exist at any given time. These include
    /// outstanding callbacks and processes in the Running state.
    work: Cell<usize>,
    /// This holds a pointer to the static array of Process pointers.
    //processes: &'static [Option<&'static dyn process::ProcessType>],
    processes: &'static dyn ProcessContainer,
    //processes: &'static dyn IntoIterator<
    //    Item = &'static core::option::Option<&'static dyn ProcessType>,
    //    IntoIter = core::slice::Iter<'static, core::option::Option<&'static dyn ProcessType>>,
    //>,
    /// How many grant regions have been setup. This is incremented on every
    /// call to `create_grant()`. We need to explicitly track this so that when
    /// processes are created they can allocated pointers for each grant.
    grant_counter: Cell<usize>,
    /// Flag to mark that grants have been finalized. This means that the kernel
    /// cannot support creating new grants because processes have already been
    /// created and the data structures for grants have already been
    /// established.
    grants_finalized: Cell<bool>,
}

impl Kernel {
    pub fn new(processes: &'static dyn ProcessContainer) -> Kernel {
        Kernel {
            work: Cell::new(0),
            processes: processes,
            grant_counter: Cell::new(0),
            grants_finalized: Cell::new(false),
        }
    }

    /// Something was scheduled for a process, so there is more work to do.
    crate fn increment_work(&self) {
        self.work.increment();
    }

    /// Something finished for a process, so we decrement how much work there is
    /// to do.
    crate fn decrement_work(&self) {
        self.work.decrement();
    }

    /// Helper function for determining if we should service processes or go to
    /// sleep.
    fn processes_blocked(&self) -> bool {
        self.work.get() == 0
    }

    /// Run a closure on a specific process if it exists. If the process does
    /// not exist (i.e. it is `None` in the `processes` array) then `default`
    /// will be returned. Otherwise the closure will executed and passed a
    /// reference to the process.
    crate fn process_map_or<F, R>(&self, default: R, process_index: usize, closure: F) -> R
    where
        F: FnOnce(&dyn process::ProcessType) -> R,
    {
        if process_index > self.processes.len() {
            return default;
        }
        self.processes
            .get_proc_by_id(process_index)
            .map_or(default, |process| closure(process))
    }

    /// Run a closure on every valid process. This will iterate the array of
    /// processes and call the closure on every process that exists.
    crate fn process_each<F>(&self, closure: F)
    where
        F: Fn(&dyn process::ProcessType),
    {
        while let Some(process) = self.processes.next() {
            closure(process);
        }
        self.processes.reset();
    }

    /// Run a closure on every process, but only continue if the closure returns
    /// `FAIL`. That is, if the closure returns any other return code than
    /// `FAIL`, that value will be returned from this function and the iteration
    /// of the array of processes will stop.
    crate fn process_until<F>(&self, closure: F) -> ReturnCode
    where
        F: Fn(&dyn process::ProcessType) -> ReturnCode,
    {
        while let Some(process) = self.processes.next() {
            let ret = closure(process);
            if ret != ReturnCode::FAIL {
                self.processes.reset();
                return ret;
            }
        }
        self.processes.reset();
        ReturnCode::FAIL
    }

    /// Run a closure on every valid process. This will iterate the
    /// array of processes and call the closure on every process that
    /// exists. Ths method is available outside the kernel crate but
    /// requires a `ProcessManagementCapability` to use.
    pub fn process_each_capability<F>(
        &'static self,
        _capability: &dyn capabilities::ProcessManagementCapability,
        closure: F,
    ) where
        F: Fn(usize, &dyn process::ProcessType),
    {
        let mut i = 0;
        while let Some(process) = self.processes.next() {
            i += 1;
            closure(i, process);
        }
        self.processes.reset();
    }

    /// Return how many processes this board supports.
    crate fn number_of_process_slots(&self) -> usize {
        self.processes.len()
    }

    /// Create a new grant. This is used in board initialization to setup grants
    /// that capsules use to interact with processes.
    ///
    /// Grants **must** only be created _before_ processes are initialized.
    /// Processes use the number of grants that have been allocated to correctly
    /// initialize the process's memory with a pointer for each grant. If a
    /// grant is created after processes are initialized this will panic.
    ///
    /// Calling this function is restricted to only certain users, and to
    /// enforce this calling this function requires the
    /// `MemoryAllocationCapability` capability.
    pub fn create_grant<T: Default>(
        &'static self,
        _capability: &dyn capabilities::MemoryAllocationCapability,
    ) -> Grant<T> {
        if self.grants_finalized.get() {
            panic!("Grants finalized. Cannot create a new grant.");
        }

        // Create and return a new grant.
        let grant_index = self.grant_counter.get();
        self.grant_counter.increment();
        Grant::new(self, grant_index)
    }

    /// Returns the number of grants that have been setup in the system and
    /// marks the grants as "finalized". This means that no more grants can
    /// be created because data structures have been setup based on the number
    /// of grants when this function is called.
    ///
    /// In practice, this is called when processes are created, and the process
    /// memory is setup based on the number of current grants.
    crate fn get_grant_count_and_finalize(&self) -> usize {
        self.grants_finalized.set(true);
        self.grant_counter.get()
    }

    /// Cause all apps to fault.
    ///
    /// This will call `set_fault_state()` on each app, causing the app to enter
    /// the state as if it had crashed (for example with an MPU violation). If
    /// the process is configured to be restarted it will be.
    ///
    /// Only callers with the `ProcessManagementCapability` can call this
    /// function. This restricts general capsules from being able to call this
    /// function, since capsules should not be able to arbitrarily restart all
    /// apps.
    pub fn hardfault_all_apps<C: capabilities::ProcessManagementCapability>(&self, _c: &C) {
        while let Some(process) = self.processes.next() {
            process.set_fault_state();
        }
        self.processes.reset();
    }

    /// Schedulers should call this to handle callbacks for yielded or unstarted apps.
    unsafe fn handle_callback(
        &self,
        cb: Task,
        process: &dyn process::ProcessType,
        ipc: Option<&ipc::IPC>,
    ) {
        match cb {
            Task::FunctionCall(ccb) => {
                if config::CONFIG.trace_syscalls {
                    debug!(
                        "[{:?}] function_call @{:#x}({:#x}, {:#x}, {:#x}, {:#x})",
                        process.appid(),
                        ccb.pc,
                        ccb.argument0,
                        ccb.argument1,
                        ccb.argument2,
                        ccb.argument3,
                    );
                }
                process.set_process_function(ccb);
            }
            Task::IPC((otherapp, ipc_type)) => {
                ipc.map_or_else(
                    || {
                        assert!(false, "Kernel consistency error: IPC Task with no IPC");
                    },
                    |ipc| {
                        ipc.schedule_callback(process.appid(), otherapp, ipc_type);
                    },
                );
            }
        };
    }

    /// Schedulers should call this to handle a process that has returned to the kernel after executing.
    unsafe fn process_return<P: Platform>(
        &self,
        appid: crate::callback::AppId,
        context_switch_reason: Option<ContextSwitchReason>,
        process: &dyn process::ProcessType,
        platform: &P,
    ) {
        match context_switch_reason {
            Some(ContextSwitchReason::Fault) => {
                // Let process deal with it as appropriate.
                process.set_fault_state();
            }
            Some(ContextSwitchReason::SyscallFired { syscall }) => {
                process.debug_syscall_called();

                // Handle each of the syscalls.
                match syscall {
                    Syscall::MEMOP { operand, arg0 } => {
                        let res = memop::memop(process, operand, arg0);
                        if config::CONFIG.trace_syscalls {
                            debug!(
                                "[{:?}] memop({}, {:#x}) = {:#x}",
                                appid,
                                operand,
                                arg0,
                                usize::from(res)
                            );
                        }
                        process.set_syscall_return_value(res.into());
                    }
                    Syscall::YIELD => {
                        if config::CONFIG.trace_syscalls {
                            debug!("[{:?}] yield", appid);
                        }
                        process.set_yielded_state();
                    }
                    Syscall::SUBSCRIBE {
                        driver_number,
                        subdriver_number,
                        callback_ptr,
                        appdata,
                    } => {
                        let callback_id = CallbackId {
                            driver_num: driver_number,
                            subscribe_num: subdriver_number,
                        };
                        process.remove_pending_callbacks(callback_id);

                        let callback = NonNull::new(callback_ptr)
                            .map(|ptr| Callback::new(appid, callback_id, appdata, ptr.cast()));

                        let res = platform.with_driver(driver_number, |driver| match driver {
                            Some(d) => d.subscribe(subdriver_number, callback, appid),
                            None => ReturnCode::ENODEVICE,
                        });
                        if config::CONFIG.trace_syscalls {
                            debug!(
                                "[{:?}] subscribe({:#x}, {}, @{:#x}, {:#x}) = {:#x}",
                                appid,
                                driver_number,
                                subdriver_number,
                                callback_ptr as usize,
                                appdata,
                                usize::from(res)
                            );
                        }
                        process.set_syscall_return_value(res.into());
                    }
                    Syscall::COMMAND {
                        driver_number,
                        subdriver_number,
                        arg0,
                        arg1,
                    } => {
                        let res = platform.with_driver(driver_number, |driver| match driver {
                            Some(d) => d.command(subdriver_number, arg0, arg1, appid),
                            None => ReturnCode::ENODEVICE,
                        });
                        if config::CONFIG.trace_syscalls {
                            debug!(
                                "[{:?}] cmd({:#x}, {}, {:#x}, {:#x}) = {:#x}",
                                appid,
                                driver_number,
                                subdriver_number,
                                arg0,
                                arg1,
                                usize::from(res)
                            );
                        }
                        process.set_syscall_return_value(res.into());
                    }
                    Syscall::ALLOW {
                        driver_number,
                        subdriver_number,
                        allow_address,
                        allow_size,
                    } => {
                        let res = platform.with_driver(driver_number, |driver| {
                            match driver {
                                Some(d) => {
                                    match process.allow(allow_address, allow_size) {
                                        Ok(oslice) => d.allow(appid, subdriver_number, oslice),
                                        Err(err) => err, /* memory not valid */
                                    }
                                }
                                None => ReturnCode::ENODEVICE,
                            }
                        });
                        if config::CONFIG.trace_syscalls {
                            debug!(
                                "[{:?}] allow({:#x}, {}, @{:#x}, {:#x}) = {:#x}",
                                appid,
                                driver_number,
                                subdriver_number,
                                allow_address as usize,
                                allow_size,
                                usize::from(res)
                            );
                        }
                        process.set_syscall_return_value(res.into());
                    }
                }
            }
            Some(ContextSwitchReason::TimesliceExpired) => {}
            Some(ContextSwitchReason::Interrupted) => {}
            None => {
                // Something went wrong when switching to this
                // process. Indicate this by putting it in a fault
                // state.
                process.set_fault_state();
            }
        }
    }
}
