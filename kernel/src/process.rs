//! Support for creating and running userspace applications.

use core::cell::Cell;
use core::cmp;
use core::convert::TryInto;
use core::fmt;
use core::fmt::Write;
use core::ptr::{write_volatile, NonNull};
use core::{mem, ptr, slice, str};

use crate::capabilities::ProcessManagementCapability;
use crate::common::cells::{MapCell, NumericCellExt};
use crate::common::{Queue, RingBuffer};
use crate::config;
use crate::debug;
use crate::errorcode::ErrorCode;
use crate::ipc;
use crate::mem::{ReadOnlyAppSlice, ReadWriteAppSlice};
use crate::platform::mpu::{self, MPU};
use crate::platform::Chip;
use crate::returncode::ReturnCode;
use crate::sched::Kernel;
use crate::syscall::{self, Syscall, SyscallReturn, UserspaceKernelBoundary};
use crate::upcall::{AppId, UpcallId};

// The completion code for a process if it faulted.
const COMPLETION_FAULT: u32 = 0xffffffff;

/// Errors that can occur when trying to load and create processes.
pub enum ProcessLoadError {
    /// The TBF header for the process could not be successfully parsed.
    TbfHeaderParseFailure(tock_tbf::types::TbfParseError),

    /// Not enough flash remaining to parse a process and its header.
    NotEnoughFlash,

    /// Not enough memory to meet the amount requested by a process. Modify the
    /// process to request less memory, flash fewer processes, or increase the
    /// size of the region your board reserves for process memory.
    NotEnoughMemory,

    /// A process was loaded with a length in flash that the MPU does not
    /// support. The fix is probably to correct the process size, but this could
    /// also be caused by a bad MPU implementation.
    MpuInvalidFlashLength,

    /// A process specified a fixed memory address that it needs its memory
    /// range to start at, and the kernel did not or could not give the process
    /// a memory region starting at that address.
    MemoryAddressMismatch {
        actual_address: u32,
        expected_address: u32,
    },

    /// A process specified that its binary must start at a particular address,
    /// and that is not the address the binary is actually placed at.
    IncorrectFlashAddress {
        actual_address: u32,
        expected_address: u32,
    },

    /// Process loading error due (likely) to a bug in the kernel. If you get
    /// this error please open a bug report.
    InternalError,
}

impl From<tock_tbf::types::TbfParseError> for ProcessLoadError {
    /// Convert between a TBF Header parse error and a process load error.
    ///
    /// We note that the process load error is because a TBF header failed to
    /// parse, and just pass through the parse error.
    fn from(error: tock_tbf::types::TbfParseError) -> Self {
        ProcessLoadError::TbfHeaderParseFailure(error)
    }
}

impl fmt::Debug for ProcessLoadError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ProcessLoadError::TbfHeaderParseFailure(tbf_parse_error) => {
                write!(f, "Error parsing TBF header\n")?;
                write!(f, "{:?}", tbf_parse_error)
            }

            ProcessLoadError::NotEnoughFlash => {
                write!(f, "Not enough flash available for app linked list")
            }

            ProcessLoadError::NotEnoughMemory => {
                write!(f, "Not able to meet memory requirements requested by apps")
            }

            ProcessLoadError::MpuInvalidFlashLength => {
                write!(f, "App flash length not supported by MPU")
            }

            ProcessLoadError::MemoryAddressMismatch {
                actual_address,
                expected_address,
            } => write!(
                f,
                "App memory does not match requested address Actual:{:#x}, Expected:{:#x}",
                actual_address, expected_address
            ),

            ProcessLoadError::IncorrectFlashAddress {
                actual_address,
                expected_address,
            } => write!(
                f,
                "App flash does not match requested address. Actual:{:#x}, Expected:{:#x}",
                actual_address, expected_address
            ),

            ProcessLoadError::InternalError => write!(f, "Error in kernel. Likely a bug."),
        }
    }
}

/// Helper function to load processes from flash into an array of active
/// processes. This is the default template for loading processes, but a board
/// is able to create its own `load_processes()` function and use that instead.
///
/// Processes are found in flash starting from the given address and iterating
/// through Tock Binary Format (TBF) headers. Processes are given memory out of
/// the `app_memory` buffer until either the memory is exhausted or the
/// allocated number of processes are created. A reference to each process is
/// stored in the provided `procs` array. How process faults are handled by the
/// kernel must be provided and is assigned to every created process.
///
/// This function is made `pub` so that board files can use it, but loading
/// processes from slices of flash an memory is fundamentally unsafe. Therefore,
/// we require the `ProcessManagementCapability` to call this function.
///
/// Returns `Ok(())` if process discovery went as expected. Returns a
/// `ProcessLoadError` if something goes wrong during TBF parsing or process
/// creation.
pub fn load_processes<C: Chip>(
    kernel: &'static Kernel,
    chip: &'static C,
    app_flash: &'static [u8],
    app_memory: &'static mut [u8],
    procs: &'static mut [Option<&'static dyn ProcessType>],
    fault_response: FaultResponse,
    _capability: &dyn ProcessManagementCapability,
) -> Result<(), ProcessLoadError> {
    if config::CONFIG.debug_load_processes {
        debug!(
            "Loading processes from flash={:#010X}-{:#010X} into sram={:#010X}-{:#010X}",
            app_flash.as_ptr() as usize,
            app_flash.as_ptr() as usize + app_flash.len() - 1,
            app_memory.as_ptr() as usize,
            app_memory.as_ptr() as usize + app_memory.len() - 1
        );
    }

    let mut remaining_flash = app_flash;
    let mut remaining_memory = app_memory;

    // Try to discover up to `procs.len()` processes in flash.
    for i in 0..procs.len() {
        // Get the first eight bytes of flash to check if there is another
        // app.
        let test_header_slice = match remaining_flash.get(0..8) {
            Some(s) => s,
            None => {
                // Not enough flash to test for another app. This just means
                // we are at the end of flash, and there are no more apps to
                // load.
                return Ok(());
            }
        };

        // Pass the first eight bytes to tbfheader to parse out the length of
        // the tbf header and app. We then use those values to see if we have
        // enough flash remaining to parse the remainder of the header.
        let (version, header_length, entry_length) = match tock_tbf::parse::parse_tbf_header_lengths(
            test_header_slice
                .try_into()
                .or(Err(ProcessLoadError::InternalError))?,
        ) {
            Ok((v, hl, el)) => (v, hl, el),
            Err(tock_tbf::types::InitialTbfParseError::InvalidHeader(entry_length)) => {
                // If we could not parse the header, then we want to skip over
                // this app and look for the next one.
                (0, 0, entry_length)
            }
            Err(tock_tbf::types::InitialTbfParseError::UnableToParse) => {
                // Since Tock apps use a linked list, it is very possible the
                // header we started to parse is intentionally invalid to signal
                // the end of apps. This is ok and just means we have finished
                // loading apps.
                return Ok(());
            }
        };

        // Now we can get a slice which only encompasses the length of flash
        // described by this tbf header.  We will either parse this as an actual
        // app, or skip over this region.
        let entry_flash = remaining_flash
            .get(0..entry_length as usize)
            .ok_or(ProcessLoadError::NotEnoughFlash)?;

        // Advance the flash slice for process discovery beyond this last entry.
        // This will be the start of where we look for a new process since Tock
        // processes are allocated back-to-back in flash.
        remaining_flash = remaining_flash
            .get(entry_flash.len()..)
            .ok_or(ProcessLoadError::NotEnoughFlash)?;

        // Need to reassign remaining_memory in every iteration so the compiler
        // knows it will not be re-borrowed.
        remaining_memory = if header_length > 0 {
            // If we found an actual app header, try to create a `Process`
            // object. We also need to shrink the amount of remaining memory
            // based on whatever is assigned to the new process if one is
            // created.

            // Try to create a process object from that app slice. If we don't
            // get a process and we didn't get a loading error (aka we got to
            // this point), then the app is a disabled process or just padding.
            let (process_option, unused_memory) = unsafe {
                Process::create(
                    kernel,
                    chip,
                    entry_flash,
                    header_length as usize,
                    version,
                    remaining_memory,
                    fault_response,
                    i,
                )?
            };
            process_option.map(|process| {
                if config::CONFIG.debug_load_processes {
                    debug!(
                        "Loaded process[{}] from flash={:#010X}-{:#010X} into sram={:#010X}-{:#010X} = {:?}",
                        i,
                        entry_flash.as_ptr() as usize,
                        entry_flash.as_ptr() as usize + entry_flash.len() - 1,
                        process.mem_start() as usize,
                        process.mem_end() as usize - 1,
                        process.get_process_name()
                    );
                }

                // Save the reference to this process in the processes array.
                procs[i] = Some(process);
            });
            unused_memory
        } else {
            // We are just skipping over this region of flash, so we have the
            // same amount of process memory to allocate from.
            remaining_memory
        };
    }

    Ok(())
}

/// Opaque identifier for dynamic grants allocated from a process's grant
/// region.
///
/// This type allows Process to provide a handle to a dynamic grant within a
/// process's memory that `DynamicGrant` can use to access the dynamic grant
/// memory later.
///
/// We use this type rather than a direct pointer so that any attempt to access
/// can ensure the process still exists and is valid, and that the dynamic grant
/// has not been freed.
///
/// The fields of this struct are private so only Process can create this
/// identifier.
#[derive(Copy, Clone)]
pub struct ProcessDynamicGrantIdentifer {
    offset: usize,
}

/// This trait is implemented by process structs.
pub trait ProcessType {
    /// Returns the process's identifier
    fn appid(&self) -> AppId;

    /// Queue a `Task` for the process. This will be added to a per-process
    /// buffer and executed by the scheduler. `Task`s are some function the app
    /// should run, for example a upcall or an IPC call.
    ///
    /// This function returns `true` if the `Task` was successfully enqueued,
    /// and `false` otherwise. This is represented as a simple `bool` because
    /// this is passed to the capsule that tried to schedule the `Task`.
    ///
    /// This will fail if the process is no longer active, and therefore cannot
    /// execute any new tasks.
    fn enqueue_task(&self, task: Task) -> bool;

    /// Returns whether this process is ready to execute.
    fn ready(&self) -> bool;

    /// Return if there are any Tasks (upcalls/IPC requests) enqueued
    /// for the process.
    fn has_tasks(&self) -> bool;

    /// Remove the scheduled operation from the front of the queue and return it
    /// to be handled by the scheduler.
    ///
    /// If there are no `Task`s in the queue for this process this will return
    /// `None`.
    fn dequeue_task(&self) -> Option<Task>;

    /// Remove all scheduled upcalls for a given upcall id from the task
    /// queue.
    fn remove_pending_upcalls(&self, upcall_id: UpcallId);

    /// Returns the current state the process is in. Common states are "running"
    /// or "yielded".
    fn get_state(&self) -> State;

    /// Move this process from the running state to the yielded state.
    ///
    /// This will fail (i.e. not do anything) if the process was not previously
    /// running.
    fn set_yielded_state(&self);

    /// Move this process from running or yielded state into the stopped state.
    ///
    /// This will fail (i.e. not do anything) if the process was not either
    /// running or yielded.
    fn stop(&self);

    /// Move this stopped process back into its original state.
    ///
    /// This transitions a process from `StoppedRunning` -> `Running` or
    /// `StoppedYielded` -> `Yielded`.
    fn resume(&self);

    /// Put this process in the fault state. This will trigger the
    /// `FaultResponse` for this process to occur.
    fn set_fault_state(&self);

    /// Returns how many times this process has been restarted.
    fn get_restart_count(&self) -> usize;

    /// Get the name of the process. Used for IPC.
    fn get_process_name(&self) -> &'static str;

    /// Stop and clear a process's state, putting it into the `Terminated`
    /// state.
    ///
    /// This will end the process, but does not reset it such that it could be
    /// restarted and run again. This function instead frees grants and any
    /// queued tasks for this process, but leaves the debug information about
    /// the process and other state intact.
    fn terminate(&self, completion_code: u32);

    /// Terminates and attempts to restart the process. The process and current
    /// application always terminate. The kernel may, based on its own policy,
    /// restart the application using the same process, reuse the process for
    /// another application, or simply terminate the process and application.
    ///
    /// This function can be called when the process is in any state. It
    /// attempts to reset all process state and re-initialize it so that it can
    /// be reused.
    ///
    /// Restarting an application can fail for two general reasons:
    ///
    /// 1. The kernel chooses not to restart the application, based on its
    ///    policy.
    ///
    /// 2. The kernel decides to restart the application but fails to do so
    ///    because Some state can no long be configured for the process. For
    ///    example, the syscall state for the process fails to initialize.
    ///
    /// After `restart()` runs the process will either be queued to run its the
    /// application's `_start` function, terminated, or queued to run a
    /// different application's `_start` function.
    fn try_restart(&self, completion_code: u32);

    // memop operations

    /// Change the location of the program break and reallocate the MPU region
    /// covering program memory.
    ///
    /// This will fail with an error if the process is no longer active. An
    /// inactive process will not run again without being reset, and changing
    /// the memory pointers is not valid at this point.
    fn brk(&self, new_break: *const u8) -> Result<*const u8, Error>;

    /// Change the location of the program break, reallocate the MPU region
    /// covering program memory, and return the previous break address.
    ///
    /// This will fail with an error if the process is no longer active. An
    /// inactive process will not run again without being reset, and changing
    /// the memory pointers is not valid at this point.
    fn sbrk(&self, increment: isize) -> Result<*const u8, Error>;

    /// The start address of allocated RAM for this process.
    fn mem_start(&self) -> *const u8;

    /// The first address after the end of the allocated RAM for this process.
    fn mem_end(&self) -> *const u8;

    /// The start address of the flash region allocated for this process.
    fn flash_start(&self) -> *const u8;

    /// The first address after the end of the flash region allocated for this
    /// process.
    fn flash_end(&self) -> *const u8;

    /// The lowest address of the grant region for the process.
    fn kernel_memory_break(&self) -> *const u8;

    /// How many writeable flash regions defined in the TBF header for this
    /// process.
    fn number_writeable_flash_regions(&self) -> usize;

    /// Get the offset from the beginning of flash and the size of the defined
    /// writeable flash region.
    fn get_writeable_flash_region(&self, region_index: usize) -> (u32, u32);

    /// Debug function to update the kernel on where the stack starts for this
    /// process. Processes are not required to call this through the memop
    /// system call, but it aids in debugging the process.
    fn update_stack_start_pointer(&self, stack_pointer: *const u8);

    /// Debug function to update the kernel on where the process heap starts.
    /// Also optional.
    fn update_heap_start_pointer(&self, heap_pointer: *const u8);

    // additional memop like functions

    /// Creates a `ReadWriteAppSlice` from the given offset and size
    /// in process memory.
    ///
    /// ## Returns
    ///
    /// In case of success, this method returns the created
    /// [`ReadWriteAppSlice`].
    ///
    /// In case of an error, an appropriate ErrorCode is returned:
    ///
    /// - if the memory is not contained in the process-accessible
    ///   memory space / `buf_start_addr` and `size` are not a valid
    ///   read-write buffer (any byte in the range is not read/write
    ///   accessible to the process), [`ErrorCode::INVAL`]
    /// - if the process is not active: [`ErrorCode::FAIL`]
    /// - for all other errors: [`ErrorCode::FAIL`]
    fn build_readwrite_appslice(
        &self,
        buf_start_addr: *mut u8,
        size: usize,
    ) -> Result<ReadWriteAppSlice, ErrorCode>;

    /// Creates a [`ReadOnlyAppSlice`] from the given offset and size
    /// in process memory.
    ///
    /// ## Returns
    ///
    /// In case of success, this method returns the created
    /// [`ReadOnlyAppSlice`].
    ///
    /// In case of an error, an appropriate ErrorCode is returned:
    ///
    /// - if the memory is not contained in the process-accessible
    ///   memory space / `buf_start_addr` and `size` are not a valid
    ///   read-only buffer (any byte in the range is not
    ///   read-accessible to the process), [`ErrorCode::INVAL`]
    /// - if the process is not active: [`ErrorCode::FAIL`]
    /// - for all other errors: [`ErrorCode::FAIL`]
    fn build_readonly_appslice(
        &self,
        buf_start_addr: *const u8,
        size: usize,
    ) -> Result<ReadOnlyAppSlice, ErrorCode>;

    /// Set a single byte within the process address space at
    /// `addr` to `value`. Return true if `addr` is within the RAM
    /// bounds currently exposed to the process (thereby writable
    /// by the process itself) and the value was set, false otherwise.
    ///
    /// ### Safety
    ///
    /// This function verifies that the byte to be written is in the process's
    /// accessible memory. However, to avoid undefined behavior the caller needs
    /// to ensure that no other references exist to the process's memory before
    /// calling this function.
    unsafe fn set_byte(&self, addr: *mut u8, value: u8) -> bool;

    /// Get the first address of process's flash that isn't protected by the
    /// kernel. The protected range of flash contains the TBF header and
    /// potentially other state the kernel is storing on behalf of the process,
    /// and cannot be edited by the process.
    fn flash_non_protected_start(&self) -> *const u8;

    // mpu

    /// Configure the MPU to use the process's allocated regions.
    ///
    /// It is not valid to call this function when the process is inactive (i.e.
    /// the process will not run again).
    fn setup_mpu(&self);

    /// Allocate a new MPU region for the process that is at least
    /// `min_region_size` bytes and lies within the specified stretch of
    /// unallocated memory.
    ///
    /// It is not valid to call this function when the process is inactive (i.e.
    /// the process will not run again).
    fn add_mpu_region(
        &self,
        unallocated_memory_start: *const u8,
        unallocated_memory_size: usize,
        min_region_size: usize,
    ) -> Option<mpu::Region>;

    // grants

    /// Allocate memory from the grant region and store the reference in the
    /// proper grant pointer index.
    ///
    /// This function must check that the doing the allocation does not cause
    /// the kernel memory break to go below the top of the process accessible
    /// memory region allowed by the MPU. Note, this can be different from the
    /// actual app_brk, as MPU alignment and size constraints may result in the
    /// PMP enforced region differing from the app_brk.
    ///
    /// This will return `None` and fail if:
    /// - The process is inactive, or
    /// - There is not enough available memory to do the allocation, or
    /// - The grant_num is invalid, or
    /// - The grant_num already has an allocated grant.
    fn allocate_grant(&self, grant_num: usize, size: usize, align: usize) -> Option<NonNull<u8>>;

    /// Check if a given grant for this process has been allocated.
    ///
    /// Returns `None` if the process is not active. Otherwise, returns `true`
    /// if the grant has been allocated, `false` otherwise.
    fn grant_is_allocated(&self, grant_num: usize) -> Option<bool>;

    /// Allocate memory from the grant region that is `size` bytes long and
    /// aligned to `align` bytes. This is used for creating dynamic grants which
    /// are not recorded in the grant pointer array, but are useful for capsules
    /// which need additional process-specific dynamically allocated memory.
    ///
    /// If successful, return a Some() with an identifier that can be used with
    /// `enter_dynamic_grant()` to get access to the memory and the pointer to
    /// the memory which must be used to initialize the memory.
    fn allocate_dynamic_grant(
        &self,
        size: usize,
        align: usize,
    ) -> Option<(ProcessDynamicGrantIdentifer, NonNull<u8>)>;

    /// Enter the grant based on `grant_num` for this process.
    ///
    /// Entering a grant means getting access to the actual memory for the
    /// object stored as the grant.
    ///
    /// This will return an `Err` if the process is inactive of the `grant_num`
    /// is invalid, if the grant has not been allocated, or if the grant is
    /// already entered. If this returns `Ok()` then the pointer points to the
    /// previously allocated memory for this grant.
    fn enter_grant(&self, grant_num: usize) -> Result<*mut u8, Error>;

    /// Enter a dynamic grant based on the `identifier`.
    ///
    /// This retrieves a pointer to the previously allocated dynamic grant based
    /// on the identifier returned when the dynamic grant was allocated.
    ///
    /// This returns an error if the dynamic grant is no longer accessible, or
    /// if the process is inactive.
    fn enter_dynamic_grant(
        &self,
        identifier: ProcessDynamicGrantIdentifer,
    ) -> Result<*mut u8, Error>;

    /// Opposite of `enter_grant()`. Used to signal that the grant is no longer
    /// entered.
    ///
    /// If `grant_num` is valid, this function cannot fail. If `grant_num` is
    /// invalid, this function will do nothing. If the process is inactive then
    /// grants are invalid and are not entered or not entered, and this function
    /// will do nothing.
    fn leave_grant(&self, grant_num: usize);

    /// Return the count of the number of allocated grants if the process is
    /// active.
    ///
    /// Useful for debugging/inspecting the system.
    fn grant_allocated_count(&self) -> Option<usize>;

    // functions for processes that are architecture specific

    /// Set the return value the process should see when it begins executing
    /// again after the syscall.
    ///
    /// It is not valid to call this function when the process is inactive (i.e.
    /// the process will not run again).
    ///
    /// This can fail, if the UKB implementation cannot correctly set the return value. An
    /// example of how this might occur:
    ///
    /// 1. The UKB implementation uses the process's stack to transfer values
    ///    between kernelspace and userspace.
    /// 2. The process calls memop.brk and reduces its accessible memory region
    ///    below its current stack.
    /// 3. The UKB implementation can no longer set the return value on the
    ///    stack since the process no longer has access to its stack.
    ///
    /// If it fails, the process will be put into the faulted state.
    fn set_syscall_return_value(&self, return_value: SyscallReturn);

    /// Set the function that is to be executed when the process is resumed.
    ///
    /// It is not valid to call this function when the process is inactive (i.e.
    /// the process will not run again).
    fn set_process_function(&self, callback: FunctionCall);

    /// Context switch to a specific process.
    ///
    /// This will return `None` if the process is inactive and cannot be
    /// switched to.
    fn switch_to(&self) -> Option<syscall::ContextSwitchReason>;

    /// Print out the memory map (Grant region, heap, stack, program
    /// memory, BSS, and data sections) of this process.
    fn print_memory_map(&self, writer: &mut dyn Write);

    /// Print out the full state of the process: its memory map, its
    /// context, and the state of the memory protection unit (MPU).
    fn print_full_process(&self, writer: &mut dyn Write);

    // debug

    /// Returns how many syscalls this app has called.
    fn debug_syscall_count(&self) -> usize;

    /// Returns how many upcalls for this process have been dropped.
    fn debug_dropped_upcall_count(&self) -> usize;

    /// Returns how many times this process has exceeded its timeslice.
    fn debug_timeslice_expiration_count(&self) -> usize;

    /// Increment the number of times the process has exceeded its timeslice.
    fn debug_timeslice_expired(&self);

    /// Increment the number of times the process called a syscall and record
    /// the last syscall that was called.
    fn debug_syscall_called(&self, last_syscall: Syscall);
}

/// Generic trait for implementing process restart policies.
///
/// This policy allows a board to specify how the kernel should decide whether
/// to restart an app after it crashes.
pub trait ProcessRestartPolicy {
    /// Decide whether to restart the `process` or not.
    ///
    /// Returns `true` if the process should be restarted, `false` otherwise.
    fn should_restart(&self, process: &dyn ProcessType) -> bool;
}

/// Implementation of `ProcessRestartPolicy` that uses a threshold to decide
/// whether to restart an app. If the app has been restarted more times than the
/// threshold then the app will no longer be restarted.
pub struct ThresholdRestart {
    threshold: usize,
}

impl ThresholdRestart {
    pub const fn new(threshold: usize) -> ThresholdRestart {
        ThresholdRestart { threshold }
    }
}

impl ProcessRestartPolicy for ThresholdRestart {
    fn should_restart(&self, process: &dyn ProcessType) -> bool {
        process.get_restart_count() <= self.threshold
    }
}

/// Implementation of `ProcessRestartPolicy` that uses a threshold to decide
/// whether to restart an app. If the app has been restarted more times than the
/// threshold then the system will panic.
pub struct ThresholdRestartThenPanic {
    threshold: usize,
}

impl ThresholdRestartThenPanic {
    pub const fn new(threshold: usize) -> ThresholdRestartThenPanic {
        ThresholdRestartThenPanic { threshold }
    }
}

impl ProcessRestartPolicy for ThresholdRestartThenPanic {
    fn should_restart(&self, process: &dyn ProcessType) -> bool {
        if process.get_restart_count() <= self.threshold {
            true
        } else {
            panic!("Restart threshold surpassed!");
        }
    }
}

/// Implementation of `ProcessRestartPolicy` that unconditionally restarts the
/// app.
pub struct AlwaysRestart {}

impl AlwaysRestart {
    pub const fn new() -> AlwaysRestart {
        AlwaysRestart {}
    }
}

impl ProcessRestartPolicy for AlwaysRestart {
    fn should_restart(&self, _process: &dyn ProcessType) -> bool {
        true
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Error {
    NoSuchApp,
    OutOfMemory,
    AddressOutOfBounds,
    /// The process is inactive (likely in a fault or exit state) and the
    /// attempted operation is therefore invalid.
    InactiveApp,
    /// This likely indicates a bug in the kernel and that some state is
    /// inconsistent in the kernel.
    KernelError,
    /// Indicates some process data, such as a Grant, is already borrowed.
    AlreadyInUse,
}

impl From<Error> for ReturnCode {
    fn from(err: Error) -> ReturnCode {
        match err {
            Error::OutOfMemory => ReturnCode::ENOMEM,
            Error::AddressOutOfBounds => ReturnCode::EINVAL,
            Error::NoSuchApp => ReturnCode::EINVAL,
            Error::InactiveApp => ReturnCode::FAIL,
            Error::KernelError => ReturnCode::FAIL,
            Error::AlreadyInUse => ReturnCode::FAIL,
        }
    }
}

impl From<Error> for ErrorCode {
    fn from(err: Error) -> ErrorCode {
        match err {
            Error::OutOfMemory => ErrorCode::NOMEM,
            Error::AddressOutOfBounds => ErrorCode::INVAL,
            Error::NoSuchApp => ErrorCode::INVAL,
            Error::InactiveApp => ErrorCode::FAIL,
            Error::KernelError => ErrorCode::FAIL,
            Error::AlreadyInUse => ErrorCode::FAIL,
        }
    }
}

/// Various states a process can be in.
///
/// This is made public in case external implementations of `ProcessType` want
/// to re-use these process states in the external implementation.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum State {
    /// Process expects to be running code. The process may not be currently
    /// scheduled by the scheduler, but the process has work to do if it is
    /// scheduled.
    Running,

    /// Process stopped executing and returned to the kernel because it called
    /// the `yield` syscall. This likely means it is waiting for some event to
    /// occur, but it could also mean it has finished and doesn't need to be
    /// scheduled again.
    Yielded,

    /// The process is stopped, and its previous state was Running. This is used
    /// if the kernel forcibly stops a process when it is in the `Running`
    /// state. This state indicates to the kernel not to schedule the process,
    /// but if the process is to be resumed later it should be put back in the
    /// running state so it will execute correctly.
    StoppedRunning,

    /// The process is stopped, and it was stopped while it was yielded. If this
    /// process needs to be resumed it should be put back in the `Yield` state.
    StoppedYielded,

    /// The process faulted and cannot be run.
    Faulted,

    /// The process exited with the `exit-terminate` system call and
    /// cannot be run.
    Terminated,

    /// The process has never actually been executed. This of course happens
    /// when the board first boots and the kernel has not switched to any
    /// processes yet. It can also happen if an process is terminated and all
    /// of its state is reset as if it has not been executed yet.
    Unstarted,
}

/// A wrapper around `Cell<State>` is used by `Process` to prevent bugs arising from
/// the state duplication in the kernel work tracking and process state tracking.
struct ProcessStateCell<'a> {
    state: Cell<State>,
    kernel: &'a Kernel,
}

impl<'a> ProcessStateCell<'a> {
    fn new(kernel: &'a Kernel) -> Self {
        Self {
            state: Cell::new(State::Unstarted),
            kernel,
        }
    }

    fn get(&self) -> State {
        self.state.get()
    }

    fn update(&self, new_state: State) {
        let old_state = self.state.get();

        if old_state == State::Running && new_state != State::Running {
            self.kernel.decrement_work();
        } else if new_state == State::Running && old_state != State::Running {
            self.kernel.increment_work()
        }
        self.state.set(new_state);
    }
}

/// The reaction the kernel should take when an app encounters a fault.
///
/// When an exception occurs during an app's execution (a common example is an
/// app trying to access memory outside of its allowed regions) the system will
/// trap back to the kernel, and the kernel has to decide what to do with the
/// app at that point.
#[derive(Copy, Clone)]
pub enum FaultResponse {
    /// Generate a `panic!()` call and crash the entire system. This is useful
    /// for debugging applications as the error is displayed immediately after
    /// it occurs.
    Panic,

    /// Attempt to cleanup and restart the app which caused the fault. This
    /// resets the app's memory to how it was when the app was started and
    /// schedules the app to run again from its init function.
    ///
    /// The provided restart policy is used to determine whether to reset the
    /// app, and can be specified on a per-app basis.
    Restart(&'static dyn ProcessRestartPolicy),

    /// Stop the app by no longer scheduling it to run.
    Stop,
}

/// Tasks that can be enqueued for a process.
///
/// This is public for external implementations of `ProcessType`.
#[derive(Copy, Clone)]
pub enum Task {
    /// Function pointer in the process to execute. Generally this is a upcall
    /// from a capsule.
    FunctionCall(FunctionCall),
    /// An IPC operation that needs additional setup to configure memory access.
    IPC((AppId, ipc::IPCUpcallType)),
}

/// Enumeration to identify whether a function call for a process comes directly
/// from the kernel or from a upcall subscribed through a `Driver`
/// implementation.
///
/// An example of a kernel function is the application entry point.
#[derive(Copy, Clone, Debug)]
pub enum FunctionCallSource {
    /// For functions coming directly from the kernel, such as `init_fn`.
    Kernel,
    /// For functions coming from capsules or any implementation of `Driver`.
    Driver(UpcallId),
}

/// Struct that defines a upcall that can be passed to a process. The upcall
/// takes four arguments that are `Driver` and upcall specific, so they are
/// represented generically here.
///
/// Likely these four arguments will get passed as the first four register
/// values, but this is architecture-dependent.
///
/// A `FunctionCall` also identifies the upcall that scheduled it, if any, so
/// that it can be unscheduled when the process unsubscribes from this upcall.
#[derive(Copy, Clone, Debug)]
pub struct FunctionCall {
    pub source: FunctionCallSource,
    pub argument0: usize,
    pub argument1: usize,
    pub argument2: usize,
    pub argument3: usize,
    pub pc: usize,
}

/// State for helping with debugging apps.
///
/// These pointers and counters are not strictly required for kernel operation,
/// but provide helpful information when an app crashes.
struct ProcessDebug {
    /// If this process was compiled for fixed addresses, save the address
    /// it must be at in flash. This is useful for debugging and saves having
    /// to re-parse the entire TBF header.
    fixed_address_flash: Option<u32>,

    /// If this process was compiled for fixed addresses, save the address
    /// it must be at in RAM. This is useful for debugging and saves having
    /// to re-parse the entire TBF header.
    fixed_address_ram: Option<u32>,

    /// Where the process has started its heap in RAM.
    app_heap_start_pointer: Option<*const u8>,

    /// Where the start of the stack is for the process. If the kernel does the
    /// PIC setup for this app then we know this, otherwise we need the app to
    /// tell us where it put its stack.
    app_stack_start_pointer: Option<*const u8>,

    /// How low have we ever seen the stack pointer.
    app_stack_min_pointer: Option<*const u8>,

    /// How many syscalls have occurred since the process started.
    syscall_count: usize,

    /// What was the most recent syscall.
    last_syscall: Option<Syscall>,

    /// How many upcalls were dropped because the queue was insufficiently
    /// long.
    dropped_upcall_count: usize,

    /// How many times this process has been paused because it exceeded its
    /// timeslice.
    timeslice_expiration_count: usize,
}

/// A type for userspace processes in Tock.
pub struct Process<'a, C: 'static + Chip> {
    /// Identifier of this process and the index of the process in the process
    /// table.
    app_id: Cell<AppId>,

    /// Pointer to the main Kernel struct.
    kernel: &'static Kernel,

    /// Pointer to the struct that defines the actual chip the kernel is running
    /// on. This is used because processes have subtle hardware-based
    /// differences. Specifically, the actual syscall interface and how
    /// processes are switched to is architecture-specific, and how memory must
    /// be allocated for memory protection units is also hardware-specific.
    chip: &'static C,

    /// Application memory layout:
    ///
    /// ```text
    ///     ╒════════ ← memory[memory.len()]
    ///  ╔═ │ Grant Pointers
    ///  ║  │ ──────
    ///     │ Process Control Block
    ///  D  │ ──────
    ///  Y  │ Grant Regions
    ///  N  │
    ///  A  │   ↓
    ///  M  │ ──────  ← kernel_memory_break
    ///  I  │
    ///  C  │ ──────  ← app_break               ═╗
    ///     │                                    ║
    ///  ║  │   ↑                                  A
    ///  ║  │  Heap                              P C
    ///  ╠═ │ ──────  ← app_heap_start           R C
    ///     │  Data                              O E
    ///  F  │ ──────  ← data_start_pointer       C S
    ///  I  │ Stack                              E S
    ///  X  │   ↓                                S I
    ///  E  │                                    S B
    ///  D  │ ──────  ← current_stack_pointer      L
    ///     │                                    ║ E
    ///  ╚═ ╘════════ ← memory[0]               ═╝
    /// ```
    ///
    /// The process's memory.
    memory: &'static mut [u8],

    /// Reference to the slice of pointers stored in the process's memory
    /// reserved for the kernel. These pointers are null if the grant region has
    /// not been allocated. When the grant region is allocated these pointers
    /// are updated to point to the allocated memory.
    grant_pointers: MapCell<&'static mut [*mut u8]>,

    /// Pointer to the end of the allocated (and MPU protected) grant region.
    kernel_memory_break: Cell<*const u8>,

    /// Pointer to the end of process RAM that has been sbrk'd to the process.
    app_break: Cell<*const u8>,

    /// Pointer to high water mark for process buffers shared through `allow`
    allow_high_water_mark: Cell<*const u8>,

    /// Process flash segment. This is the region of nonvolatile flash that
    /// the process occupies.
    flash: &'static [u8],

    /// Collection of pointers to the TBF header in flash.
    header: tock_tbf::types::TbfHeader,

    /// State saved on behalf of the process each time the app switches to the
    /// kernel.
    stored_state:
        MapCell<<<C as Chip>::UserspaceKernelBoundary as UserspaceKernelBoundary>::StoredState>,

    /// The current state of the app. The scheduler uses this to determine
    /// whether it can schedule this app to execute.
    ///
    /// The `state` is used both for bookkeeping for the scheduler as well as
    /// for enabling control by other parts of the system. The scheduler keeps
    /// track of if a process is ready to run or not by switching between the
    /// `Running` and `Yielded` states. The system can control the process by
    /// switching it to a "stopped" state to prevent the scheduler from
    /// scheduling it.
    state: ProcessStateCell<'static>,

    /// How to deal with Faults occurring in the process
    fault_response: FaultResponse,

    /// Configuration data for the MPU
    mpu_config: MapCell<<<C as Chip>::MPU as MPU>::MpuConfig>,

    /// MPU regions are saved as a pointer-size pair.
    mpu_regions: [Cell<Option<mpu::Region>>; 6],

    /// Essentially a list of upcalls that want to call functions in the
    /// process.
    tasks: MapCell<RingBuffer<'a, Task>>,

    /// Count of how many times this process has entered the fault condition and
    /// been restarted. This is used by some `ProcessRestartPolicy`s to
    /// determine if the process should be restarted or not.
    restart_count: Cell<usize>,

    /// Name of the app.
    process_name: &'static str,

    /// Values kept so that we can print useful debug messages when apps fault.
    debug: MapCell<ProcessDebug>,
}

impl<C: Chip> ProcessType for Process<'_, C> {
    fn appid(&self) -> AppId {
        self.app_id.get()
    }

    fn enqueue_task(&self, task: Task) -> bool {
        // If this app is in a `Fault` state then we shouldn't schedule
        // any work for it.
        if !self.is_active() {
            return false;
        }

        let ret = self.tasks.map_or(false, |tasks| tasks.enqueue(task));

        // Make a note that we lost this upcall if the enqueue function
        // fails.
        if ret == false {
            self.debug.map(|debug| {
                debug.dropped_upcall_count += 1;
            });
        } else {
            self.kernel.increment_work();
        }

        ret
    }

    fn ready(&self) -> bool {
        self.tasks.map_or(false, |ring_buf| ring_buf.has_elements())
            || self.state.get() == State::Running
    }

    fn remove_pending_upcalls(&self, upcall_id: UpcallId) {
        self.tasks.map(|tasks| {
            let count_before = tasks.len();
            tasks.retain(|task| match task {
                // Remove only tasks that are function calls with an id equal
                // to `upcall_id`.
                Task::FunctionCall(function_call) => match function_call.source {
                    FunctionCallSource::Kernel => true,
                    FunctionCallSource::Driver(id) => {
                        if id != upcall_id {
                            true
                        } else {
                            self.kernel.decrement_work();
                            false
                        }
                    }
                },
                _ => true,
            });
            if config::CONFIG.trace_syscalls {
                let count_after = tasks.len();
                debug!(
                    "[{:?}] remove_pending_upcalls[{:#x}:{}] = {} upcall(s) removed",
                    self.appid(),
                    upcall_id.driver_num,
                    upcall_id.subscribe_num,
                    count_before - count_after,
                );
            }
        });
    }

    fn get_state(&self) -> State {
        self.state.get()
    }

    fn set_yielded_state(&self) {
        if self.state.get() == State::Running {
            self.state.update(State::Yielded);
        }
    }

    fn stop(&self) {
        match self.state.get() {
            State::Running => self.state.update(State::StoppedRunning),
            State::Yielded => self.state.update(State::StoppedYielded),
            _ => {} // Do nothing
        }
    }

    fn resume(&self) {
        match self.state.get() {
            State::StoppedRunning => self.state.update(State::Running),
            State::StoppedYielded => self.state.update(State::Yielded),
            _ => {} // Do nothing
        }
    }

    fn set_fault_state(&self) {
        match self.fault_response {
            FaultResponse::Panic => {
                // process faulted. Panic and print status
                self.state.update(State::Faulted);
                panic!("Process {} had a fault", self.process_name);
            }
            FaultResponse::Restart(restart_policy) => {
                // Apply the process policy for whether to try to restart
                // on a fault. We sometimes don't want to (e.g., too
                // many faults). If we decide to try to restart, the
                // kernel applies its own policy for how to reuse the
                // process: it may or may not restart the application.
                if restart_policy.should_restart(self) {
                    self.try_restart(COMPLETION_FAULT);
                } else {
                    self.terminate(COMPLETION_FAULT);
                    self.state.update(State::Faulted);
                }
            }
            FaultResponse::Stop => {
                // This looks a lot like restart, except we just leave the app
                // how it faulted and mark it as `Faulted`. By clearing
                // all of the app's todo work it will not be scheduled, and
                // clearing all of the grant regions will cause capsules to drop
                // this app as well.
                self.terminate(COMPLETION_FAULT);
                self.state.update(State::Faulted);
            }
        }
    }

    fn try_restart(&self, completion_code: u32) {
        // Terminate the process, freeing its state and removing any
        // pending tasks from the scheduler's queue.
        self.terminate(completion_code);

        // If there is a kernel policy that controls restarts, it should be
        // implemented here. For now, always restart.
        let _res = self.restart();

        // Decide what to do with res later. E.g., if we can't restart
        // want to reclaim the process resources.
    }

    fn terminate(&self, _completion_code: u32) {
        // Remove the tasks that were scheduled for the app from the
        // amount of work queue.
        let tasks_len = self.tasks.map_or(0, |tasks| tasks.len());
        for _ in 0..tasks_len {
            self.kernel.decrement_work();
        }

        // And remove those tasks
        self.tasks.map(|tasks| {
            tasks.empty();
        });

        // Clear any grant regions this app has setup with any capsules.
        unsafe {
            self.grant_ptrs_reset();
        }

        // Mark the app as stopped so the scheduler won't try to run it.
        self.state.update(State::Terminated);
    }

    fn get_restart_count(&self) -> usize {
        self.restart_count.get()
    }

    fn has_tasks(&self) -> bool {
        self.tasks.map_or(false, |tasks| tasks.has_elements())
    }

    fn dequeue_task(&self) -> Option<Task> {
        self.tasks.map_or(None, |tasks| {
            tasks.dequeue().map(|cb| {
                self.kernel.decrement_work();
                cb
            })
        })
    }

    fn mem_start(&self) -> *const u8 {
        self.memory.as_ptr()
    }

    fn mem_end(&self) -> *const u8 {
        unsafe { self.memory.as_ptr().add(self.memory.len()) }
    }

    fn flash_start(&self) -> *const u8 {
        self.flash.as_ptr()
    }

    fn flash_non_protected_start(&self) -> *const u8 {
        ((self.flash.as_ptr() as usize) + self.header.get_protected_size() as usize) as *const u8
    }

    fn flash_end(&self) -> *const u8 {
        unsafe { self.flash.as_ptr().add(self.flash.len()) }
    }

    fn kernel_memory_break(&self) -> *const u8 {
        self.kernel_memory_break.get()
    }

    fn number_writeable_flash_regions(&self) -> usize {
        self.header.number_writeable_flash_regions()
    }

    fn get_writeable_flash_region(&self, region_index: usize) -> (u32, u32) {
        self.header.get_writeable_flash_region(region_index)
    }

    fn update_stack_start_pointer(&self, stack_pointer: *const u8) {
        if stack_pointer >= self.mem_start() && stack_pointer < self.mem_end() {
            self.debug.map(|debug| {
                debug.app_stack_start_pointer = Some(stack_pointer);

                // We also reset the minimum stack pointer because whatever value
                // we had could be entirely wrong by now.
                debug.app_stack_min_pointer = Some(stack_pointer);
            });
        }
    }

    fn update_heap_start_pointer(&self, heap_pointer: *const u8) {
        if heap_pointer >= self.mem_start() && heap_pointer < self.mem_end() {
            self.debug.map(|debug| {
                debug.app_heap_start_pointer = Some(heap_pointer);
            });
        }
    }

    fn setup_mpu(&self) {
        self.mpu_config.map(|config| {
            self.chip.mpu().configure_mpu(&config, &self.appid());
        });
    }

    fn add_mpu_region(
        &self,
        unallocated_memory_start: *const u8,
        unallocated_memory_size: usize,
        min_region_size: usize,
    ) -> Option<mpu::Region> {
        self.mpu_config.and_then(|mut config| {
            let new_region = self.chip.mpu().allocate_region(
                unallocated_memory_start,
                unallocated_memory_size,
                min_region_size,
                mpu::Permissions::ReadWriteOnly,
                &mut config,
            );

            if new_region.is_none() {
                return None;
            }

            for region in self.mpu_regions.iter() {
                if region.get().is_none() {
                    region.set(new_region);
                    return new_region;
                }
            }

            // Not enough room in Process struct to store the MPU region.
            None
        })
    }

    fn sbrk(&self, increment: isize) -> Result<*const u8, Error> {
        // Do not modify an inactive process.
        if !self.is_active() {
            return Err(Error::InactiveApp);
        }

        let new_break = unsafe { self.app_break.get().offset(increment) };
        self.brk(new_break)
    }

    fn brk(&self, new_break: *const u8) -> Result<*const u8, Error> {
        // Do not modify an inactive process.
        if !self.is_active() {
            return Err(Error::InactiveApp);
        }

        self.mpu_config
            .map_or(Err(Error::KernelError), |mut config| {
                if new_break < self.allow_high_water_mark.get() || new_break >= self.mem_end() {
                    Err(Error::AddressOutOfBounds)
                } else if new_break > self.kernel_memory_break.get() {
                    Err(Error::OutOfMemory)
                } else if let Err(_) = self.chip.mpu().update_app_memory_region(
                    new_break,
                    self.kernel_memory_break.get(),
                    mpu::Permissions::ReadWriteOnly,
                    &mut config,
                ) {
                    Err(Error::OutOfMemory)
                } else {
                    let old_break = self.app_break.get();
                    self.app_break.set(new_break);
                    self.chip.mpu().configure_mpu(&config, &self.appid());
                    Ok(old_break)
                }
            })
    }

    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn build_readwrite_appslice(
        &self,
        buf_start_addr: *mut u8,
        size: usize,
    ) -> Result<ReadWriteAppSlice, ErrorCode> {
        if !self.is_active() {
            // Do not operate on an inactive process
            return Err(ErrorCode::FAIL);
        }

        // A process is allowed to pass any pointer if the slice
        // length is 0, as to revoke kernel access to a memory region
        // without granting access to another one
        if size == 0 {
            // Clippy complains that we're deferencing a pointer in a
            // public and safe function here. While we are not
            // dereferencing the pointer here, we pass it along to an
            // unsafe function, which is as dangerous (as it is likely
            // to be dereferenced down the line).
            //
            // Relevant discussion:
            // https://github.com/rust-lang/rust-clippy/issues/3045
            //
            // It should be fine to ignore the lint here, as a slice
            // of length 0 will never allow dereferencing any memory
            // in a safe manner.
            //
            // ### Safety
            //
            // We specific a zero-length buffer, so the implementation of
            // `ReadWriteAppSlice` will handle any safety issues. Therefore, we
            // can encapsulate the unsafe.
            Ok(unsafe { ReadWriteAppSlice::new(buf_start_addr, 0, self.appid()) })
        } else if self.in_app_owned_memory(buf_start_addr, size) {
            // TODO: Check for buffer aliasing here

            // Valid slice, we need to adjust the app's watermark
            // note: in_app_owned_memory ensures this offset does not wrap
            let buf_end_addr = buf_start_addr.wrapping_add(size);
            let new_water_mark = cmp::max(self.allow_high_water_mark.get(), buf_end_addr);
            self.allow_high_water_mark.set(new_water_mark);

            // Clippy complains that we're deferencing a pointer in a
            // public and safe function here. While we are not
            // deferencing the pointer here, we pass it along to an
            // unsafe function, which is as dangerous (as it is likely
            // to be deferenced down the line).
            //
            // Relevant discussion:
            // https://github.com/rust-lang/rust-clippy/issues/3045
            //
            // It should be fine to ignore the lint here, as long as
            // we make sure that we're pointing towards userspace
            // memory (verified using `in_app_owned_memory`) and
            // respect alignment and other constraints of the Rust
            // references created by ReadWriteAppSlice.
            //
            // ### Safety
            //
            // We encapsulate the unsafe here on the condition in the TODO
            // above, as we must ensure that this `ReadWriteAppSlice` will be
            // the only reference to this memory.
            Ok(unsafe { ReadWriteAppSlice::new(buf_start_addr, size, self.appid()) })
        } else {
            Err(ErrorCode::INVAL)
        }
    }

    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn build_readonly_appslice(
        &self,
        buf_start_addr: *const u8,
        size: usize,
    ) -> Result<ReadOnlyAppSlice, ErrorCode> {
        if !self.is_active() {
            // Do not operate on an inactive process
            return Err(ErrorCode::FAIL);
        }

        // A process is allowed to pass any pointer if the slice
        // length is 0, as to revoke kernel access to a memory region
        // without granting access to another one
        if size == 0 {
            // Clippy complains that we're deferencing a pointer in a
            // public and safe function here. While we are not
            // deferencing the pointer here, we pass it along to an
            // unsafe function, which is as dangerous (as it is likely
            // to be deferenced down the line).
            //
            // Relevant discussion:
            // https://github.com/rust-lang/rust-clippy/issues/3045
            //
            // It should be fine to ignore the lint here, as a slice
            // of length 0 will never allow dereferencing any memory
            // in a safe manner.
            //
            // ### Safety
            //
            // We specific a zero-length buffer, so the implementation of
            // `ReadOnlyAppSlice` will handle any safety issues. Therefore, we
            // can encapsulate the unsafe.
            Ok(unsafe { ReadOnlyAppSlice::new(buf_start_addr, 0, self.appid()) })
        } else if self.in_app_owned_memory(buf_start_addr, size)
            || self.in_app_flash_memory(buf_start_addr, size)
        {
            // TODO: Check for buffer aliasing here

            // Valid slice, we need to adjust the app's watermark
            // note: in_app_owned_memory ensures this offset does not wrap
            let buf_end_addr = buf_start_addr.wrapping_add(size);
            let new_water_mark = cmp::max(self.allow_high_water_mark.get(), buf_end_addr);
            self.allow_high_water_mark.set(new_water_mark);

            // Clippy complains that we're deferencing a pointer in a
            // public and safe function here. While we are not
            // deferencing the pointer here, we pass it along to an
            // unsafe function, which is as dangerous (as it is likely
            // to be deferenced down the line).
            //
            // Relevant discussion:
            // https://github.com/rust-lang/rust-clippy/issues/3045
            //
            // It should be fine to ignore the lint here, as long as
            // we make sure that we're pointing towards userspace
            // memory (verified using `in_app_owned_memory` or
            // `in_app_flash_memory`) and respect alignment and other
            // constraints of the Rust references created by
            // ReadWriteAppSlice.
            //
            // ### Safety
            //
            // We encapsulate the unsafe here on the condition in the TODO
            // above, as we must ensure that this `ReadOnlyAppSlice` will be
            // the only reference to this memory.
            Ok(unsafe { ReadOnlyAppSlice::new(buf_start_addr, size, self.appid()) })
        } else {
            Err(ErrorCode::INVAL)
        }
    }

    unsafe fn set_byte(&self, addr: *mut u8, value: u8) -> bool {
        if self.in_app_owned_memory(addr, 1) {
            // We verify that this will only write process-accessible memory,
            // but this can still be undefined behavior if something else holds
            // a reference to this memory.
            *addr = value;
            true
        } else {
            false
        }
    }

    fn grant_is_allocated(&self, grant_num: usize) -> Option<bool> {
        // Do not modify an inactive process.
        if !self.is_active() {
            return None;
        }

        // Update the grant pointer to the address of the new allocation.
        self.grant_pointers.map_or(None, |grant_pointers| {
            // Implement `grant_pointers[grant_num]` without a
            // chance of a panic.
            grant_pointers
                .get(grant_num)
                .map_or(None, |grant_pointer_pointer| {
                    Some(!(*grant_pointer_pointer).is_null())
                })
        })
    }

    fn allocate_grant(&self, grant_num: usize, size: usize, align: usize) -> Option<NonNull<u8>> {
        // Do not modify an inactive process.
        if !self.is_active() {
            return None;
        }

        // Verify the grant_num is valid.
        if grant_num >= self.kernel.get_grant_count_and_finalize() {
            return None;
        }

        // Verify that the grant is not already allocated. If the pointer is not
        // null then the grant is already allocated.
        if let Some(is_allocated) = self.grant_is_allocated(grant_num) {
            if is_allocated {
                return None;
            }
        }

        // Use the shared grant allocator function to actually allocate memory.
        // Returns `None` if the allocation cannot be created.
        if let Some(grant_ptr) = self.allocate_in_grant_region_internal(size, align) {
            // Update the grant pointer to the address of the new allocation.
            self.grant_pointers.map_or(None, |grant_pointers| {
                // Implement `grant_pointers[grant_num] = grant_ptr` without a
                // chance of a panic.
                grant_pointers
                    .get_mut(grant_num)
                    .map_or(None, |grant_pointer_pointer| {
                        // Actually set the grant pointer.
                        *grant_pointer_pointer = grant_ptr.as_ptr() as *mut u8;

                        // If all of this worked, return the allocated pointer.
                        Some(grant_ptr)
                    })
            })
        } else {
            // Could not allocate the memory for the grant region.
            None
        }
    }

    fn allocate_dynamic_grant(
        &self,
        size: usize,
        align: usize,
    ) -> Option<(ProcessDynamicGrantIdentifer, NonNull<u8>)> {
        // Do not modify an inactive process.
        if !self.is_active() {
            return None;
        }

        // Use the shared grant allocator function to actually allocate memory.
        // Returns `None` if the allocation cannot be created.
        if let Some(ptr) = self.allocate_in_grant_region_internal(size, align) {
            // Create the identifier that the caller will use to get access to this
            // dynamic grant in the future.
            let identifier = self.create_dynamic_grant_identifier(ptr);

            Some((identifier, ptr))
        } else {
            // Could not allocate memory for the dynamic grant.
            None
        }
    }

    fn enter_grant(&self, grant_num: usize) -> Result<*mut u8, Error> {
        // Do not try to access the grant region of inactive process.
        if !self.is_active() {
            return Err(Error::InactiveApp);
        }

        // Retrieve the grant pointer from the `grant_pointers` slice. We use
        // `[slice].get()` so that if the grant number is invalid this will
        // return `Err` and not panic.
        self.grant_pointers
            .map_or(Err(Error::KernelError), |grant_pointers| {
                // Implement `grant_pointers[grant_num]` without a chance of a
                // panic.
                match grant_pointers.get_mut(grant_num) {
                    Some(grant_pointer_pointer) => {
                        // Get a copy of the actual grant pointer.
                        let grant_ptr = *grant_pointer_pointer;

                        // Check if the grant pointer is marked that the grant
                        // has already been entered. If so, return an error.
                        if (grant_ptr as usize) & 0x1 == 0x1 {
                            // Lowest bit is one, meaning this grant has been
                            // entered.
                            Err(Error::AlreadyInUse)
                        } else {
                            // Now, to mark that the grant has been entered, we
                            // set the lowest bit to one and save this as the
                            // grant pointer.
                            *grant_pointer_pointer = (grant_ptr as usize | 0x1) as *mut u8;

                            // And we return the grant pointer to the entered
                            // grant.
                            Ok(grant_ptr)
                        }
                    }
                    None => Err(Error::AddressOutOfBounds),
                }
            })
    }

    fn enter_dynamic_grant(
        &self,
        identifier: ProcessDynamicGrantIdentifer,
    ) -> Result<*mut u8, Error> {
        // Do not try to access the grant region of inactive process.
        if !self.is_active() {
            return Err(Error::InactiveApp);
        }

        // Get the address of the dynamic grant based on the identifier.
        let dynamic_grant_address = self.get_dynamic_grant_address(identifier);

        // We never deallocate dynamic grants and only we can change the
        // `identifier` so we know this is a valid address for the dynamic
        // grant.
        Ok(dynamic_grant_address as *mut u8)
    }

    fn leave_grant(&self, grant_num: usize) {
        // Do not modify an inactive process.
        if !self.is_active() {
            return;
        }

        self.grant_pointers.map(|grant_pointers| {
            // Implement `grant_pointers[grant_num]` without a chance of a
            // panic.
            match grant_pointers.get_mut(grant_num) {
                Some(grant_pointer_pointer) => {
                    // Get a copy of the actual grant pointer.
                    let grant_ptr = *grant_pointer_pointer;

                    // Now, to mark that the grant has been released, we set the
                    // lowest bit back to zero and save this as the grant
                    // pointer.
                    *grant_pointer_pointer = (grant_ptr as usize & !0x1) as *mut u8;
                }
                None => {}
            }
        });
    }

    fn grant_allocated_count(&self) -> Option<usize> {
        // Do not modify an inactive process.
        if !self.is_active() {
            return None;
        }

        self.grant_pointers.map(|grant_pointers| {
            // Filter our list of grant pointers into just the non null ones,
            // and count those. A grant is allocated if its grant pointer is non
            // null.
            grant_pointers
                .iter()
                .filter(|grant_ptr| !grant_ptr.is_null())
                .count()
        })
    }

    fn get_process_name(&self) -> &'static str {
        self.process_name
    }

    fn set_syscall_return_value(&self, return_value: SyscallReturn) {
        match self.stored_state.map(|stored_state| unsafe {
            // Actually set the return value for a particular process.
            //
            // The UKB implementation uses the bounds of process-accessible
            // memory to verify that any memory changes are valid. Here, the
            // unsafe promise we are making is that the bounds passed to the UKB
            // are correct.
            self.chip
                .userspace_kernel_boundary()
                .set_syscall_return_value(
                    self.memory.as_ptr(),
                    self.app_break.get(),
                    stored_state,
                    return_value,
                )
        }) {
            Some(Ok(())) => {
                // If we get an `Ok` we are all set.
            }

            Some(Err(())) => {
                // If we get an `Err`, then the UKB implementation could not set
                // the return value, likely because the process's stack is no
                // longer accessible to it. All we can do is fault.
                self.set_fault_state();
            }

            None => {
                // We should never be here since `stored_state` should always be
                // occupied.
                self.set_fault_state();
            }
        }
    }

    fn set_process_function(&self, callback: FunctionCall) {
        // See if we can actually enqueue this function for this process.
        // Architecture-specific code handles actually doing this since the
        // exact method is both architecture- and implementation-specific.
        //
        // This can fail, for example if the process does not have enough memory
        // remaining.
        match self.stored_state.map(|stored_state| {
            // Let the UKB implementation handle setting the process's PC so
            // that the process executes the upcall function. We encapsulate
            // unsafe here because we are guaranteeing that the memory bounds
            // passed to `set_process_function` are correct.
            unsafe {
                self.chip.userspace_kernel_boundary().set_process_function(
                    self.memory.as_ptr(),
                    self.app_break.get(),
                    stored_state,
                    callback,
                )
            }
        }) {
            Some(Ok(())) => {
                // If we got an `Ok` we are all set and should mark that this
                // process is ready to be scheduled.

                // Move this process to the "running" state so the scheduler
                // will schedule it.
                self.state.update(State::Running);
            }

            Some(Err(())) => {
                // If we got an Error, then there was likely not enough room on
                // the stack to allow the process to execute this function given
                // the details of the particular architecture this is running
                // on. This process has essentially faulted, so we mark it as
                // such.
                self.set_fault_state();
            }

            None => {
                // We should never be here since `stored_state` should always be
                // occupied.
                self.set_fault_state();
            }
        }
    }

    fn switch_to(&self) -> Option<syscall::ContextSwitchReason> {
        // Cannot switch to an invalid process
        if !self.is_active() {
            return None;
        }

        let (switch_reason, stack_pointer) =
            self.stored_state.map_or((None, None), |stored_state| {
                // Switch to the process. We guarantee that the memory pointers
                // we pass are valid, ensuring this context switch is safe.
                // Therefore we encapsulate the `unsafe`.
                unsafe {
                    let (switch_reason, optional_stack_pointer) =
                        self.chip.userspace_kernel_boundary().switch_to_process(
                            self.memory.as_ptr(),
                            self.app_break.get(),
                            stored_state,
                        );
                    (Some(switch_reason), optional_stack_pointer)
                }
            });

        // If the UKB implementation passed us a stack pointer, update our
        // debugging state. This is completely optional.
        stack_pointer.map(|sp| {
            self.debug.map(|debug| {
                match debug.app_stack_min_pointer {
                    None => debug.app_stack_min_pointer = Some(sp),
                    Some(asmp) => {
                        // Update max stack depth if needed.
                        if sp < asmp {
                            debug.app_stack_min_pointer = Some(sp);
                        }
                    }
                }
            });
        });

        switch_reason
    }

    fn debug_syscall_count(&self) -> usize {
        self.debug.map_or(0, |debug| debug.syscall_count)
    }

    fn debug_dropped_upcall_count(&self) -> usize {
        self.debug.map_or(0, |debug| debug.dropped_upcall_count)
    }

    fn debug_timeslice_expiration_count(&self) -> usize {
        self.debug
            .map_or(0, |debug| debug.timeslice_expiration_count)
    }

    fn debug_timeslice_expired(&self) {
        self.debug
            .map(|debug| debug.timeslice_expiration_count += 1);
    }

    fn debug_syscall_called(&self, last_syscall: Syscall) {
        self.debug.map(|debug| {
            debug.syscall_count += 1;
            debug.last_syscall = Some(last_syscall);
        });
    }

    fn print_memory_map(&self, writer: &mut dyn Write) {
        // Flash
        let flash_end = self.flash.as_ptr().wrapping_add(self.flash.len()) as usize;
        let flash_start = self.flash.as_ptr() as usize;
        let flash_protected_size = self.header.get_protected_size() as usize;
        let flash_app_start = flash_start + flash_protected_size;
        let flash_app_size = flash_end - flash_app_start;

        // SRAM addresses
        let sram_end = self.memory.as_ptr().wrapping_add(self.memory.len()) as usize;
        let sram_grant_start = self.kernel_memory_break.get() as usize;
        let sram_heap_end = self.app_break.get() as usize;
        let sram_heap_start: Option<usize> = self.debug.map_or(None, |debug| {
            debug.app_heap_start_pointer.map(|p| p as usize)
        });
        let sram_stack_start: Option<usize> = self.debug.map_or(None, |debug| {
            debug.app_stack_start_pointer.map(|p| p as usize)
        });
        let sram_stack_bottom: Option<usize> = self.debug.map_or(None, |debug| {
            debug.app_stack_min_pointer.map(|p| p as usize)
        });
        let sram_start = self.memory.as_ptr() as usize;

        // SRAM sizes
        let sram_grant_size = sram_end - sram_grant_start;
        let sram_grant_allocated = sram_end - sram_grant_start;

        // application statistics
        let events_queued = self.tasks.map_or(0, |tasks| tasks.len());
        let syscall_count = self.debug.map_or(0, |debug| debug.syscall_count);
        let last_syscall = self.debug.map(|debug| debug.last_syscall);
        let dropped_upcall_count = self.debug.map_or(0, |debug| debug.dropped_upcall_count);
        let restart_count = self.restart_count.get();

        let _ = writer.write_fmt(format_args!(
            "\
             𝐀𝐩𝐩: {}   -   [{:?}]\
             \r\n Events Queued: {}   Syscall Count: {}   Dropped Upcall Count: {}\
             \r\n Restart Count: {}\r\n",
            self.process_name,
            self.state.get(),
            events_queued,
            syscall_count,
            dropped_upcall_count,
            restart_count,
        ));

        let _ = match last_syscall {
            Some(syscall) => writer.write_fmt(format_args!(" Last Syscall: {:?}\r\n", syscall)),
            None => writer.write_str(" Last Syscall: None\r\n"),
        };

        let _ = writer.write_fmt(format_args!(
            "\
             \r\n\
             \r\n ╔═══════════╤══════════════════════════════════════════╗\
             \r\n ║  Address  │ Region Name    Used | Allocated (bytes)  ║\
             \r\n ╚{:#010X}═╪══════════════════════════════════════════╝\
             \r\n             │ ▼ Grant      {:6} | {:6}{}\
             \r\n  {:#010X} ┼───────────────────────────────────────────\
             \r\n             │ Unused\
             \r\n  {:#010X} ┼───────────────────────────────────────────",
            sram_end,
            sram_grant_size,
            sram_grant_allocated,
            exceeded_check(sram_grant_size, sram_grant_allocated),
            sram_grant_start,
            sram_heap_end,
        ));

        match sram_heap_start {
            Some(sram_heap_start) => {
                let sram_heap_size = sram_heap_end - sram_heap_start;
                let sram_heap_allocated = sram_grant_start - sram_heap_start;

                let _ = writer.write_fmt(format_args!(
                    "\
                     \r\n             │ ▲ Heap       {:6} | {:6}{}     S\
                     \r\n  {:#010X} ┼─────────────────────────────────────────── R",
                    sram_heap_size,
                    sram_heap_allocated,
                    exceeded_check(sram_heap_size, sram_heap_allocated),
                    sram_heap_start,
                ));
            }
            None => {
                let _ = writer.write_str(
                    "\
                     \r\n             │ ▲ Heap            ? |      ?               S\
                     \r\n  ?????????? ┼─────────────────────────────────────────── R",
                );
            }
        }

        match (sram_heap_start, sram_stack_start) {
            (Some(sram_heap_start), Some(sram_stack_start)) => {
                let sram_data_size = sram_heap_start - sram_stack_start;
                let sram_data_allocated = sram_data_size as usize;

                let _ = writer.write_fmt(format_args!(
                    "\
                     \r\n             │ Data         {:6} | {:6}               A",
                    sram_data_size, sram_data_allocated,
                ));
            }
            _ => {
                let _ = writer.write_str(
                    "\
                     \r\n             │ Data              ? |      ?               A",
                );
            }
        }

        match (sram_stack_start, sram_stack_bottom) {
            (Some(sram_stack_start), Some(sram_stack_bottom)) => {
                let sram_stack_size = sram_stack_start - sram_stack_bottom;
                let sram_stack_allocated = sram_stack_start - sram_start;

                let _ = writer.write_fmt(format_args!(
                    "\
                     \r\n  {:#010X} ┼─────────────────────────────────────────── M\
                     \r\n             │ ▼ Stack      {:6} | {:6}{}",
                    sram_stack_start,
                    sram_stack_size,
                    sram_stack_allocated,
                    exceeded_check(sram_stack_size, sram_stack_allocated),
                ));
            }
            _ => {
                let _ = writer.write_str(
                    "\
                     \r\n  ?????????? ┼─────────────────────────────────────────── M\
                     \r\n             │ ▼ Stack           ? |      ?",
                );
            }
        }

        let _ = writer.write_fmt(format_args!(
            "\
             \r\n  {:#010X} ┼───────────────────────────────────────────\
             \r\n             │ Unused\
             \r\n  {:#010X} ┴───────────────────────────────────────────\
             \r\n             .....\
             \r\n  {:#010X} ┬─────────────────────────────────────────── F\
             \r\n             │ App Flash    {:6}                        L\
             \r\n  {:#010X} ┼─────────────────────────────────────────── A\
             \r\n             │ Protected    {:6}                        S\
             \r\n  {:#010X} ┴─────────────────────────────────────────── H\
             \r\n",
            sram_stack_bottom.unwrap_or(0),
            sram_start,
            flash_end,
            flash_app_size,
            flash_app_start,
            flash_protected_size,
            flash_start
        ));
    }

    fn print_full_process(&self, writer: &mut dyn Write) {
        self.print_memory_map(writer);

        self.stored_state.map(|stored_state| {
            // We guarantee the memory bounds pointers provided to the UKB are
            // correct.
            unsafe {
                self.chip.userspace_kernel_boundary().print_context(
                    self.memory.as_ptr(),
                    self.app_break.get(),
                    stored_state,
                    writer,
                );
            }
        });

        // Display grant information.
        let number_grants = self.kernel.get_grant_count_and_finalize();
        let _ = writer.write_fmt(format_args!(
            "\
             \r\n Total number of grant regions defined: {}\r\n",
            self.kernel.get_grant_count_and_finalize()
        ));
        let rows = (number_grants + 2) / 3;

        // Access our array of grant pointers.
        self.grant_pointers.map(|grant_pointers| {
            // Iterate each grant and show its address.
            for i in 0..rows {
                for j in 0..3 {
                    let index = i + (rows * j);
                    if index >= number_grants {
                        break;
                    }

                    // Implement `grant_pointers[grant_num]` without a chance of
                    // a panic.
                    grant_pointers.get(index).map(|grant_pointer_pointer| {
                        if grant_pointer_pointer.is_null() {
                            let _ =
                                writer.write_fmt(format_args!("  Grant {:>2}: --        ", index));
                        } else {
                            let _ = writer.write_fmt(format_args!(
                                "  Grant {:>2}: {:p}",
                                index, grant_pointer_pointer
                            ));
                        }
                    });
                }
                let _ = writer.write_fmt(format_args!("\r\n"));
            }
        });

        // Display the current state of the MPU for this process.
        self.mpu_config.map(|config| {
            let _ = writer.write_fmt(format_args!("{}", config));
        });

        // Print a helpful message on how to re-compile a process to view the
        // listing file. If a process is PIC, then we also need to print the
        // actual addresses the process executed at so that the .lst file can be
        // generated for those addresses. If the process was already compiled
        // for a fixed address, then just generating a .lst file is fine.

        self.debug.map(|debug| {
            if debug.fixed_address_flash.is_some() {
                // Fixed addresses, can just run `make lst`.
                let _ = writer.write_fmt(format_args!(
                    "\
                     \r\nTo debug, run `make lst` in the app's folder\
                     \r\nand open the arch.{:#x}.{:#x}.lst file.\r\n\r\n",
                    debug.fixed_address_flash.unwrap_or(0),
                    debug.fixed_address_ram.unwrap_or(0)
                ));
            } else {
                // PIC, need to specify the addresses.
                let sram_start = self.memory.as_ptr() as usize;
                let flash_start = self.flash.as_ptr() as usize;
                let flash_init_fn = flash_start + self.header.get_init_function_offset() as usize;

                let _ = writer.write_fmt(format_args!(
                    "\
                     \r\nTo debug, run `make debug RAM_START={:#x} FLASH_INIT={:#x}`\
                     \r\nin the app's folder and open the .lst file.\r\n\r\n",
                    sram_start, flash_init_fn
                ));
            }
        });
    }
}

fn exceeded_check(size: usize, allocated: usize) -> &'static str {
    if size > allocated {
        " EXCEEDED!"
    } else {
        "          "
    }
}

impl<C: 'static + Chip> Process<'_, C> {
    // Memory offset for upcall ring buffer (10 element length).
    const CALLBACK_LEN: usize = 10;
    const CALLBACKS_OFFSET: usize = mem::size_of::<Task>() * Self::CALLBACK_LEN;

    // Memory offset to make room for this process's metadata.
    const PROCESS_STRUCT_OFFSET: usize = mem::size_of::<Process<C>>();

    pub(crate) unsafe fn create(
        kernel: &'static Kernel,
        chip: &'static C,
        app_flash: &'static [u8],
        header_length: usize,
        app_version: u16,
        remaining_memory: &'static mut [u8],
        fault_response: FaultResponse,
        index: usize,
    ) -> Result<(Option<&'static dyn ProcessType>, &'static mut [u8]), ProcessLoadError> {
        // Get a slice for just the app header.
        let header_flash = app_flash
            .get(0..header_length as usize)
            .ok_or(ProcessLoadError::NotEnoughFlash)?;

        // Parse the full TBF header to see if this is a valid app. If the
        // header can't parse, we will error right here.
        let tbf_header = tock_tbf::parse::parse_tbf_header(header_flash, app_version)?;

        // First thing: check that the process is at the correct location in
        // flash if the TBF header specified a fixed address. If there is a
        // mismatch we catch that early.
        if let Some(fixed_flash_start) = tbf_header.get_fixed_address_flash() {
            // The flash address in the header is based on the app binary,
            // so we need to take into account the header length.
            let actual_address = app_flash.as_ptr() as u32 + tbf_header.get_protected_size();
            let expected_address = fixed_flash_start;
            if actual_address != expected_address {
                return Err(ProcessLoadError::IncorrectFlashAddress {
                    actual_address,
                    expected_address,
                });
            }
        }

        let process_name = tbf_header.get_package_name();

        // If this isn't an app (i.e. it is padding) or it is an app but it
        // isn't enabled, then we can skip it and do not create a `Process`
        // object.
        if !tbf_header.is_app() || !tbf_header.enabled() {
            if config::CONFIG.debug_load_processes {
                if !tbf_header.is_app() {
                    debug!(
                        "Padding in flash={:#010X}-{:#010X}",
                        app_flash.as_ptr() as usize,
                        app_flash.as_ptr() as usize + app_flash.len() - 1
                    );
                }
                if !tbf_header.enabled() {
                    debug!(
                        "Process not enabled flash={:#010X}-{:#010X} process={:?}",
                        app_flash.as_ptr() as usize,
                        app_flash.as_ptr() as usize + app_flash.len() - 1,
                        process_name
                    );
                }
            }
            // Return no process and the full memory slice we were given.
            return Ok((None, remaining_memory));
        }

        // Otherwise, actually load the app.
        let process_ram_requested_size = tbf_header.get_minimum_app_ram_size() as usize;
        let init_fn = app_flash
            .as_ptr()
            .offset(tbf_header.get_init_function_offset() as isize) as usize;

        // Initialize MPU region configuration.
        let mut mpu_config: <<C as Chip>::MPU as MPU>::MpuConfig = Default::default();

        // Allocate MPU region for flash.
        if chip
            .mpu()
            .allocate_region(
                app_flash.as_ptr(),
                app_flash.len(),
                app_flash.len(),
                mpu::Permissions::ReadExecuteOnly,
                &mut mpu_config,
            )
            .is_none()
        {
            if config::CONFIG.debug_load_processes {
                debug!(
                    "[!] flash={:#010X}-{:#010X} process={:?} - couldn't allocate MPU region for flash",
                    app_flash.as_ptr() as usize,
                    app_flash.as_ptr() as usize + app_flash.len() - 1,
                    process_name
                );
            }
            return Err(ProcessLoadError::MpuInvalidFlashLength);
        }

        // Determine how much space we need in the application's
        // memory space just for kernel and grant state. We need to make
        // sure we allocate enough memory just for that.

        // Make room for grant pointers.
        let grant_ptr_size = mem::size_of::<*const usize>();
        let grant_ptrs_num = kernel.get_grant_count_and_finalize();
        let grant_ptrs_offset = grant_ptrs_num * grant_ptr_size;

        // Initial size of the kernel-owned part of process memory can be
        // calculated directly based on the initial size of all kernel-owned
        // data structures.
        let initial_kernel_memory_size =
            grant_ptrs_offset + Self::CALLBACKS_OFFSET + Self::PROCESS_STRUCT_OFFSET;

        // By default we start with the initial size of process-accessible
        // memory set to 0. This maximizes the flexibility that processes have
        // to allocate their memory as they see fit. If a process needs more
        // accessible memory it must use the `brk` memop syscalls to request more
        // memory.
        //
        // We must take into account any process-accessible memory required by
        // the context switching implementation and allocate at least that much
        // memory so that we can successfully switch to the process. This is
        // architecture and implementation specific, so we query that now.
        let min_process_memory_size = chip
            .userspace_kernel_boundary()
            .initial_process_app_brk_size();

        // We have to ensure that we at least ask the MPU for
        // `min_process_memory_size` so that we can be sure that `app_brk` is
        // not set inside the kernel-owned memory region. Now, in practice,
        // processes should not request 0 (or very few) bytes of memory in their
        // TBF header (i.e. `process_ram_requested_size` will almost always be
        // much larger than `min_process_memory_size`), as they are unlikely to
        // work with essentially no available memory. But, we still must protect
        // for that case.
        let min_process_ram_size = cmp::max(process_ram_requested_size, min_process_memory_size);

        // Minimum memory size for the process.
        let min_total_memory_size = min_process_ram_size + initial_kernel_memory_size;

        // Check if this process requires a fixed memory start address. If so,
        // try to adjust the memory region to work for this process.
        //
        // Right now, we only support skipping some RAM and leaving a chunk
        // unused so that the memory region starts where the process needs it
        // to.
        let remaining_memory = if let Some(fixed_memory_start) = tbf_header.get_fixed_address_ram()
        {
            // The process does have a fixed address.
            if fixed_memory_start == remaining_memory.as_ptr() as u32 {
                // Address already matches.
                remaining_memory
            } else if fixed_memory_start > remaining_memory.as_ptr() as u32 {
                // Process wants a memory address farther in memory. Try to
                // advance the memory region to make the address match.
                let diff = (fixed_memory_start - remaining_memory.as_ptr() as u32) as usize;
                if diff > remaining_memory.len() {
                    // We ran out of memory.
                    let actual_address =
                        remaining_memory.as_ptr() as u32 + remaining_memory.len() as u32 - 1;
                    let expected_address = fixed_memory_start;
                    return Err(ProcessLoadError::MemoryAddressMismatch {
                        actual_address,
                        expected_address,
                    });
                } else {
                    // Change the memory range to start where the process
                    // requested it.
                    remaining_memory
                        .get_mut(diff..)
                        .ok_or(ProcessLoadError::InternalError)?
                }
            } else {
                // Address is earlier in memory, nothing we can do.
                let actual_address = remaining_memory.as_ptr() as u32;
                let expected_address = fixed_memory_start;
                return Err(ProcessLoadError::MemoryAddressMismatch {
                    actual_address,
                    expected_address,
                });
            }
        } else {
            remaining_memory
        };

        // Determine where process memory will go and allocate MPU region for
        // app-owned memory.
        let (app_memory_start, app_memory_size) = match chip.mpu().allocate_app_memory_region(
            remaining_memory.as_ptr() as *const u8,
            remaining_memory.len(),
            min_total_memory_size,
            min_process_memory_size,
            initial_kernel_memory_size,
            mpu::Permissions::ReadWriteOnly,
            &mut mpu_config,
        ) {
            Some((memory_start, memory_size)) => (memory_start, memory_size),
            None => {
                // Failed to load process. Insufficient memory.
                if config::CONFIG.debug_load_processes {
                    debug!(
                        "[!] flash={:#010X}-{:#010X} process={:?} - couldn't allocate memory region of size >= {:#X}",
                        app_flash.as_ptr() as usize,
                        app_flash.as_ptr() as usize + app_flash.len() - 1,
                        process_name,
                        min_total_memory_size
                    );
                }
                return Err(ProcessLoadError::NotEnoughMemory);
            }
        };

        // Get a slice for the memory dedicated to the process. This can fail if
        // the MPU returns a region of memory that is not inside of the
        // `remaining_memory` slice passed to `create()` to allocate the
        // process's memory out of.
        let memory_start_offset = app_memory_start as usize - remaining_memory.as_ptr() as usize;
        // First split the remaining memory into a slice that contains the
        // process memory and a slice that will not be used by this process.
        let (app_memory_oversize, unused_memory) =
            remaining_memory.split_at_mut(memory_start_offset + app_memory_size);
        // Then since the process's memory need not start at the beginning of
        // the remaining slice given to create(), get a smaller slice as needed.
        let app_memory = app_memory_oversize
            .get_mut(memory_start_offset..)
            .ok_or(ProcessLoadError::InternalError)?;

        // Check if the memory region is valid for the process. If a process
        // included a fixed address for the start of RAM in its TBF header (this
        // field is optional, processes that are position independent do not
        // need a fixed address) then we check that we used the same address
        // when we allocated it in RAM.
        if let Some(fixed_memory_start) = tbf_header.get_fixed_address_ram() {
            let actual_address = app_memory.as_ptr() as u32;
            let expected_address = fixed_memory_start;
            if actual_address != expected_address {
                return Err(ProcessLoadError::MemoryAddressMismatch {
                    actual_address,
                    expected_address,
                });
            }
        }

        // Set the initial process-accessible memory to the amount specified by
        // the context switch implementation.
        let initial_app_brk = app_memory.as_ptr().add(min_process_memory_size);

        // Set the initial allow high water mark to the start of process memory
        // since no `allow` calls have been made yet.
        let initial_allow_high_water_mark = app_memory.as_ptr();

        // Set up initial grant region.
        let mut kernel_memory_break = app_memory.as_mut_ptr().add(app_memory.len());

        // Now that we know we have the space we can setup the grant
        // pointers.
        kernel_memory_break = kernel_memory_break.offset(-(grant_ptrs_offset as isize));

        // This is safe today, as MPU constraints ensure that `memory_start`
        // will always be aligned on at least a word boundary, and that
        // memory_size will be aligned on at least a word boundary, and
        // `grant_ptrs_offset` is a multiple of the word size. Thus,
        // `kernel_memory_break` must be word aligned. While this is unlikely to
        // change, it should be more proactively enforced.
        //
        // TODO: https://github.com/tock/tock/issues/1739
        #[allow(clippy::cast_ptr_alignment)]
        // Set all grant pointers to null.
        let opts = slice::from_raw_parts_mut(kernel_memory_break as *mut *mut u8, grant_ptrs_num);
        for opt in opts.iter_mut() {
            *opt = ptr::null_mut()
        }

        // Now that we know we have the space we can setup the memory for the
        // upcalls.
        kernel_memory_break = kernel_memory_break.offset(-(Self::CALLBACKS_OFFSET as isize));

        // This is safe today, as MPU constraints ensure that `memory_start`
        // will always be aligned on at least a word boundary, and that
        // memory_size will be aligned on at least a word boundary, and
        // `grant_ptrs_offset` is a multiple of the word size. Thus,
        // `kernel_memory_break` must be word aligned. While this is unlikely to
        // change, it should be more proactively enforced.
        //
        // TODO: https://github.com/tock/tock/issues/1739
        #[allow(clippy::cast_ptr_alignment)]
        // Set up ring buffer for upcalls to the process.
        let upcall_buf =
            slice::from_raw_parts_mut(kernel_memory_break as *mut Task, Self::CALLBACK_LEN);
        let tasks = RingBuffer::new(upcall_buf);

        // Last thing in the kernel region of process RAM is the process struct.
        kernel_memory_break = kernel_memory_break.offset(-(Self::PROCESS_STRUCT_OFFSET as isize));
        let process_struct_memory_location = kernel_memory_break;

        // Create the Process struct in the app grant region.
        let mut process: &mut Process<C> =
            &mut *(process_struct_memory_location as *mut Process<'static, C>);

        // Ask the kernel for a unique identifier for this process that is being
        // created.
        let unique_identifier = kernel.create_process_identifier();

        // Save copies of these in case the app was compiled for fixed addresses
        // for later debugging.
        let fixed_address_flash = tbf_header.get_fixed_address_flash();
        let fixed_address_ram = tbf_header.get_fixed_address_ram();

        process
            .app_id
            .set(AppId::new(kernel, unique_identifier, index));
        process.kernel = kernel;
        process.chip = chip;
        process.allow_high_water_mark = Cell::new(initial_allow_high_water_mark);
        process.memory = app_memory;
        process.header = tbf_header;
        process.kernel_memory_break = Cell::new(kernel_memory_break);
        process.app_break = Cell::new(initial_app_brk);
        process.grant_pointers = MapCell::new(opts);

        process.flash = app_flash;

        process.stored_state = MapCell::new(Default::default());
        // Mark this process as unstarted
        process.state = ProcessStateCell::new(process.kernel);
        process.fault_response = fault_response;
        process.restart_count = Cell::new(0);

        process.mpu_config = MapCell::new(mpu_config);
        process.mpu_regions = [
            Cell::new(None),
            Cell::new(None),
            Cell::new(None),
            Cell::new(None),
            Cell::new(None),
            Cell::new(None),
        ];
        process.tasks = MapCell::new(tasks);
        process.process_name = process_name.unwrap_or("");

        process.debug = MapCell::new(ProcessDebug {
            fixed_address_flash: fixed_address_flash,
            fixed_address_ram: fixed_address_ram,
            app_heap_start_pointer: None,
            app_stack_start_pointer: None,
            app_stack_min_pointer: None,
            syscall_count: 0,
            last_syscall: None,
            dropped_upcall_count: 0,
            timeslice_expiration_count: 0,
        });

        let flash_protected_size = process.header.get_protected_size() as usize;
        let flash_app_start_addr = app_flash.as_ptr() as usize + flash_protected_size;

        process.tasks.map(|tasks| {
            tasks.enqueue(Task::FunctionCall(FunctionCall {
                source: FunctionCallSource::Kernel,
                pc: init_fn,
                argument0: flash_app_start_addr,
                argument1: process.memory.as_ptr() as usize,
                argument2: process.memory.len() as usize,
                argument3: process.app_break.get() as usize,
            }));
        });

        // Handle any architecture-specific requirements for a new process.
        //
        // NOTE! We have to ensure that the start of process-accessible memory
        // (`app_memory_start`) is word-aligned. Since we currently start
        // process-accessible memory at the beginning of the allocated memory
        // region, we trust the MPU to give us a word-aligned starting address.
        //
        // TODO: https://github.com/tock/tock/issues/1739
        match process.stored_state.map(|stored_state| {
            chip.userspace_kernel_boundary().initialize_process(
                app_memory_start,
                initial_app_brk,
                stored_state,
            )
        }) {
            Some(Ok(())) => {}
            _ => {
                if config::CONFIG.debug_load_processes {
                    debug!(
                        "[!] flash={:#010X}-{:#010X} process={:?} - couldn't initialize process",
                        app_flash.as_ptr() as usize,
                        app_flash.as_ptr() as usize + app_flash.len() - 1,
                        process_name
                    );
                }
                return Err(ProcessLoadError::InternalError);
            }
        };

        kernel.increment_work();

        // Return the process object and a remaining memory for processes slice.
        Ok((Some(process), unused_memory))
    }

    /// Restart the process, resetting all of its state and re-initializing
    /// it to start running.  Assumes the process is not running but is still in flash
    /// and still has its memory region allocated to it. This implements
    /// the mechanism of restart.
    fn restart(&self) -> Result<(), ErrorCode> {
        // We need a new process identifier for this process since the restarted
        // version is in effect a new process. This is also necessary to
        // invalidate any stored `AppId`s that point to the old version of the
        // process. However, the process has not moved locations in the
        // processes array, so we copy the existing index.
        let old_index = self.app_id.get().index;
        let new_identifier = self.kernel.create_process_identifier();
        self.app_id
            .set(AppId::new(self.kernel, new_identifier, old_index));

        // Reset debug information that is per-execution and not per-process.
        self.debug.map(|debug| {
            debug.syscall_count = 0;
            debug.last_syscall = None;
            debug.dropped_upcall_count = 0;
            debug.timeslice_expiration_count = 0;
        });

        // FLASH

        // We are going to start this process over again, so need the init_fn
        // location.
        let app_flash_address = self.flash_start();
        let init_fn = unsafe {
            app_flash_address.offset(self.header.get_init_function_offset() as isize) as usize
        };

        // Reset MPU region configuration.
        // TODO: ideally, this would be moved into a helper function used by both
        // create() and reset(), but process load debugging complicates this.
        // We just want to create new config with only flash and memory regions.
        let mut mpu_config: <<C as Chip>::MPU as MPU>::MpuConfig = Default::default();
        // Allocate MPU region for flash.
        let app_mpu_flash = self.chip.mpu().allocate_region(
            self.flash.as_ptr(),
            self.flash.len(),
            self.flash.len(),
            mpu::Permissions::ReadExecuteOnly,
            &mut mpu_config,
        );
        if app_mpu_flash.is_none() {
            // We were unable to allocate an MPU region for flash. This is very
            // unexpected since we previously ran this process. However, we
            // return now and leave the process faulted and it will not be
            // scheduled.
            return Err(ErrorCode::FAIL);
        }

        // RAM

        // Re-determine the minimum amount of RAM the kernel must allocate to the process
        // based on the specific requirements of the syscall implementation.
        let min_process_memory_size = self
            .chip
            .userspace_kernel_boundary()
            .initial_process_app_brk_size();

        // Recalculate initial_kernel_memory_size as was done in create()
        let grant_ptr_size = mem::size_of::<*const usize>();
        let grant_ptrs_num = self.kernel.get_grant_count_and_finalize();
        let grant_ptrs_offset = grant_ptrs_num * grant_ptr_size;

        let initial_kernel_memory_size =
            grant_ptrs_offset + Self::CALLBACKS_OFFSET + Self::PROCESS_STRUCT_OFFSET;

        let app_mpu_mem = self.chip.mpu().allocate_app_memory_region(
            self.memory.as_ptr() as *const u8,
            self.memory.len(),
            self.memory.len(), //we want exactly as much as we had before restart
            min_process_memory_size,
            initial_kernel_memory_size,
            mpu::Permissions::ReadWriteOnly,
            &mut mpu_config,
        );
        let (app_mpu_mem_start, app_mpu_mem_len) = match app_mpu_mem {
            Some((start, len)) => (start, len),
            None => {
                // We couldn't configure the MPU for the process. This shouldn't
                // happen since we were able to start the process before, but at
                // this point it is better to leave the app faulted and not
                // schedule it.
                return Err(ErrorCode::NOMEM);
            }
        };

        // Reset memory pointers now that we know the layout of the process
        // memory and know that we can configure the MPU.

        // app_brk is set based on minimum syscall size above the start of
        // memory.
        let app_brk = app_mpu_mem_start.wrapping_add(min_process_memory_size);
        self.app_break.set(app_brk);
        // kernel_brk is calculated backwards from the end of memory the size of
        // the initial kernel data structures.
        let kernel_brk = app_mpu_mem_start
            .wrapping_add(app_mpu_mem_len)
            .wrapping_sub(initial_kernel_memory_size);
        self.kernel_memory_break.set(kernel_brk);
        // High water mark for `allow`ed memory is reset to the start of the
        // process's memory region.
        self.allow_high_water_mark.set(app_mpu_mem_start);

        // Drop the old config and use the clean one
        self.mpu_config.replace(mpu_config);

        // Handle any architecture-specific requirements for a process when it
        // first starts (as it would when it is new).
        let ukb_init_process = self.stored_state.map_or(Err(()), |stored_state| unsafe {
            self.chip.userspace_kernel_boundary().initialize_process(
                app_mpu_mem_start,
                app_brk,
                stored_state,
            )
        });
        match ukb_init_process {
            Ok(()) => {}
            Err(_) => {
                // We couldn't initialize the architecture-specific
                // state for this process. This shouldn't happen since
                // the app was able to be started before, but at this
                // point the app is no longer valid. The best thing we
                // can do now is leave the app as still faulted and not
                // schedule it.
                return Err(ErrorCode::RESERVE);
            }
        };

        // And queue up this app to be restarted.
        let flash_protected_size = self.header.get_protected_size() as usize;
        let flash_app_start = app_flash_address as usize + flash_protected_size;

        // Mark the state as `Unstarted` for the scheduler.
        self.state.update(State::Unstarted);

        // Mark that we restarted this process.
        self.restart_count.increment();

        // Enqueue the initial function.
        self.tasks.map(|tasks| {
            tasks.enqueue(Task::FunctionCall(FunctionCall {
                source: FunctionCallSource::Kernel,
                pc: init_fn,
                argument0: flash_app_start,
                argument1: self.memory.as_ptr() as usize,
                argument2: self.memory.len() as usize,
                argument3: self.app_break.get() as usize,
            }));
        });

        // Mark that the process is ready to run.
        self.kernel.increment_work();

        Ok(())
    }

    /// Checks if the buffer represented by the passed in base pointer and size
    /// is within the RAM bounds currently exposed to the processes (i.e.
    /// ending at `app_break`). If this method returns `true`, the buffer
    /// is guaranteed to be accessible to the process and to not overlap with
    /// the grant region.
    fn in_app_owned_memory(&self, buf_start_addr: *const u8, size: usize) -> bool {
        let buf_end_addr = buf_start_addr.wrapping_add(size);

        buf_end_addr >= buf_start_addr
            && buf_start_addr >= self.mem_start()
            && buf_end_addr <= self.app_break.get()
    }

    /// Checks if the buffer represented by the passed in base pointer and size
    /// are within the readable region of an application's flash
    /// memory.  If this method returns true, the buffer
    /// is guaranteed to be readable to the process.
    fn in_app_flash_memory(&self, buf_start_addr: *const u8, size: usize) -> bool {
        let buf_end_addr = buf_start_addr.wrapping_add(size);

        buf_end_addr >= buf_start_addr
            && buf_start_addr >= self.flash_non_protected_start()
            && buf_end_addr <= self.flash_end()
    }

    /// Reset all `grant_ptr`s to NULL.
    // This is safe today, as MPU constraints ensure that `mem_end` will always
    // be aligned on at least a word boundary. While this is unlikely to
    // change, it should be more proactively enforced.
    //
    // TODO: https://github.com/tock/tock/issues/1739
    #[allow(clippy::cast_ptr_alignment)]
    unsafe fn grant_ptrs_reset(&self) {
        let grant_ptrs_num = self.kernel.get_grant_count_and_finalize();
        for grant_num in 0..grant_ptrs_num {
            let grant_num = grant_num as isize;
            let ctr_ptr = (self.mem_end() as *mut *mut usize).offset(-(grant_num + 1));
            write_volatile(ctr_ptr, ptr::null_mut());
        }
    }

    /// Allocate memory in a process's grant region.
    ///
    /// Ensures that the allocation is of `size` bytes and aligned to `align`
    /// bytes.
    ///
    /// If there is not enough memory, or the MPU cannot isolate the process
    /// accessible region from the new kernel memory break after doing the
    /// allocation, then this will return `None`.
    fn allocate_in_grant_region_internal(&self, size: usize, align: usize) -> Option<NonNull<u8>> {
        self.mpu_config.and_then(|mut config| {
            // First, compute the candidate new pointer. Note that at this
            // point we have not yet checked whether there is space for
            // this allocation or that it meets alignment requirements.
            let new_break_unaligned = self.kernel_memory_break.get().wrapping_sub(size);

            // Our minimum alignment requirement is two bytes, so that the
            // lowest bit of the address will always be zero and we can use it
            // as a flag. It doesn't hurt to increase the alignment (except for
            // potentially a wasted byte) so we make sure `align` is at least
            // two.
            let align = cmp::max(align, 2);

            // The alignment must be a power of two, 2^a. The expression
            // `!(align - 1)` then returns a mask with leading ones,
            // followed by `a` trailing zeros.
            let alignment_mask = !(align - 1);
            let new_break = (new_break_unaligned as usize & alignment_mask) as *const u8;

            // Verify there is space for this allocation
            if new_break < self.app_break.get() {
                None
            // Verify it didn't wrap around
            } else if new_break > self.kernel_memory_break.get() {
                None
            // Verify this is compatible with the MPU.
            } else if let Err(_) = self.chip.mpu().update_app_memory_region(
                self.app_break.get(),
                new_break,
                mpu::Permissions::ReadWriteOnly,
                &mut config,
            ) {
                None
            } else {
                // Allocation is valid.

                // We always allocate down, so we must lower the
                // kernel_memory_break.
                self.kernel_memory_break.set(new_break);

                // We need `grant_ptr` as a mutable pointer.
                let grant_ptr = new_break as *mut u8;

                // ### Safety
                //
                // Here we are guaranteeing that `grant_ptr` is not null. We can
                // ensure this because we just created `grant_ptr` based on the
                // process's allocated memory, and we know it cannot be null.
                unsafe { Some(NonNull::new_unchecked(grant_ptr)) }
            }
        })
    }

    /// Create the identifier for a dynamic grant that grant.rs uses to access
    /// the dynamic grant.
    ///
    /// We create this identifier by calculating the number of bytes between
    /// where the dynamic grant starts and the end of the process memory.
    fn create_dynamic_grant_identifier(&self, ptr: NonNull<u8>) -> ProcessDynamicGrantIdentifer {
        let dynamic_grant_address = ptr.as_ptr() as usize;
        let process_memory_end = self.mem_end() as usize;

        ProcessDynamicGrantIdentifer {
            offset: process_memory_end - dynamic_grant_address,
        }
    }

    /// Use a ProcessDynamicGrantIdentifer to find the address of the dynamic
    /// grant.
    ///
    /// This reverses `create_dynamic_grant_identifier()`.
    fn get_dynamic_grant_address(&self, identifier: ProcessDynamicGrantIdentifer) -> usize {
        let process_memory_end = self.mem_end() as usize;

        // Subtract the offset in the identifier from the end of the process
        // memory to get the address of the dynamic grant.
        process_memory_end - identifier.offset
    }

    /// Check if the process is active.
    ///
    /// "Active" is defined as the process can resume executing in the future.
    /// This means its state in the `Process` struct is still valid, and that
    /// the kernel could resume its execution without completely restarting and
    /// resetting its state.
    ///
    /// A process is inactive if the kernel cannot resume its execution, such as
    /// if the process faults and is in an invalid state, or if the process
    /// explicitly exits.
    fn is_active(&self) -> bool {
        let current_state = self.state.get();
        current_state != State::Terminated && current_state != State::Faulted
    }
}
