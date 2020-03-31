//! Component for a round robin scheduler.
//!
//! This provides one Component, RoundRobinComponent.
//!
//! Usage
//! -----
//! (Must be final lines in main.rs)
//! ```rust
//! let scheduler = components::round_robin::RoundRobinComponent::new(board_kernel, &PROCESSES)
//!     .finalize(components::rr_component_helper!(NUM_PROCS));
//! scheduler.kernel_loop(&imix, chip, Some(&imix.ipc), &main_cap);
//! ```

// Author: Hudson Ayers <hayers@stanford.edu>
// Last modified: 03/31/2020

use kernel::component::Component;
use kernel::procs::ProcessType;
use kernel::static_init;
use kernel::{RRProcessNode, RoundRobinSched};

#[macro_export]
macro_rules! rr_component_helper {
    ($N:expr) => {{
        use kernel::static_init;
        use kernel::RRProcessNode;
        static_init!([Option<RRProcessNode<'static>>; $N], [None; $N])
    };};
}

pub struct RoundRobinComponent {
    board_kernel: &'static kernel::Kernel,
    processes: &'static [Option<&'static dyn ProcessType>],
}

impl RoundRobinComponent {
    pub fn new(
        board_kernel: &'static kernel::Kernel,
        processes: &'static [Option<&'static dyn ProcessType>],
    ) -> RoundRobinComponent {
        RoundRobinComponent {
            board_kernel: board_kernel,
            processes: processes,
        }
    }
}

impl Component for RoundRobinComponent {
    type StaticInput = &'static mut [Option<RRProcessNode<'static>>];
    type Output = &'static mut RoundRobinSched<'static>;

    unsafe fn finalize(self, proc_nodes: Self::StaticInput) -> Self::Output {
        let scheduler = static_init!(
            RoundRobinSched<'static>,
            RoundRobinSched::new(self.board_kernel)
        );
        let num_procs = proc_nodes.len();

        for i in 0..num_procs {
            if self.processes[i].is_some() {
                proc_nodes[i] = Some(RRProcessNode::new(self.processes[i].unwrap().appid()));
            }
        }
        for i in 0..num_procs {
            if self.processes[i].is_some() {
                scheduler
                    .processes
                    .push_head(proc_nodes[i].as_ref().unwrap());
            }
        }
        scheduler
    }
}
