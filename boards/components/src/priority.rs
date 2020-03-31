//! Component for a priority scheduler.
//!
//! This provides one Component, PriorityComponent.
//!
//! Usage
//! -----
//! (Must be final lines in main.rs)
//! ```rust
//! let scheduler =
//!     components::priority::PriorityComponent::new(board_kernel, &PROCESSES).finalize(());
//! scheduler.kernel_loop(&imix, chip, Some(&imix.ipc), &main_cap);
//! ```

// Author: Hudson Ayers <hayers@stanford.edu>
// Last modified: 03/31/2020

use kernel::component::Component;
use kernel::procs::ProcessType;
use kernel::static_init;
use kernel::PrioritySched;

pub struct PriorityComponent {
    board_kernel: &'static kernel::Kernel,
    processes: &'static [Option<&'static dyn ProcessType>],
}

impl PriorityComponent {
    pub fn new(
        board_kernel: &'static kernel::Kernel,
        processes: &'static [Option<&'static dyn ProcessType>],
    ) -> PriorityComponent {
        PriorityComponent {
            board_kernel: board_kernel,
            processes: processes,
        }
    }
}

impl Component for PriorityComponent {
    type StaticInput = ();
    type Output = &'static mut PrioritySched;

    unsafe fn finalize(self, _static_buffer: Self::StaticInput) -> Self::Output {
        let scheduler = static_init!(
            PrioritySched,
            PrioritySched::new(self.board_kernel, self.processes)
        );
        scheduler
    }
}
