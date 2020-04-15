//! Component for a cooperative scheduler.
//!
//! This provides one Component, CooperativeComponent.
//!
//! Usage
//! -----
//! (Must be final lines in main.rs)
//! ```rust
//! let scheduler = components::cooperative::CooperativeComponent::new(board_kernel, &PROCESSES)
//!     .finalize(components::coop_component_helper!(NUM_PROCS));
//! scheduler.kernel_loop(&imix, chip, Some(&imix.ipc), &main_cap);
//! ```

// Author: Hudson Ayers <hayers@stanford.edu>

use kernel::component::Component;
use kernel::procs::ProcessType;
use kernel::static_init;
use kernel::{CoopProcessNode, CooperativeSched};

#[macro_export]
macro_rules! coop_component_helper {
    ($N:expr) => {{
        use kernel::static_init;
        use kernel::CoopProcessNode;
        static_init!([Option<CoopProcessNode<'static>>; $N], [None; $N])
    };};
}

pub struct CooperativeComponent {
    board_kernel: &'static kernel::Kernel,
    processes: &'static [Option<&'static dyn ProcessType>],
}

impl CooperativeComponent {
    pub fn new(
        board_kernel: &'static kernel::Kernel,
        processes: &'static [Option<&'static dyn ProcessType>],
    ) -> CooperativeComponent {
        CooperativeComponent {
            board_kernel: board_kernel,
            processes: processes,
        }
    }
}

impl Component for CooperativeComponent {
    type StaticInput = &'static mut [Option<CoopProcessNode<'static>>];
    type Output = &'static mut CooperativeSched<'static>;

    unsafe fn finalize(self, proc_nodes: Self::StaticInput) -> Self::Output {
        let scheduler = static_init!(
            CooperativeSched<'static>,
            CooperativeSched::new(self.board_kernel)
        );
        let num_procs = proc_nodes.len();

        for i in 0..num_procs {
            if self.processes[i].is_some() {
                proc_nodes[i] = Some(CoopProcessNode::new(self.processes[i].unwrap().appid()));
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
