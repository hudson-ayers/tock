//! Component for a secure time scheduler.
//!
//! This provides one Component, SecureTimeComponent.
//!
//! Usage
//! -----
//! ```rust
//! let scheduler = components::secure_time::SecureTimeComponent::new(&PROCESSES)
//!     .finalize(components::st_component_helper!(NUM_PROCS));
//! ```

// Author: Hudson Ayers <hayers@stanford.edu>
// Last modified: 03/31/2020

use core::mem::MaybeUninit;
use kernel::component::Component;
use kernel::procs::ProcessType;
use kernel::{static_init, static_init_half};
use kernel::{STProcessNode, SecureTimeSched};

#[macro_export]
macro_rules! st_component_helper {
    ($N:expr $(,)?) => {{
        use core::mem::MaybeUninit;
        use kernel::static_buf;
        use kernel::STProcessNode;
        const UNINIT: MaybeUninit<STProcessNode<'static>> = MaybeUninit::uninit();
        static mut BUF: [MaybeUninit<STProcessNode<'static>>; $N] = [UNINIT; $N];
        &mut BUF
    };};
}

pub struct SecureTimeComponent {
    processes: &'static [Option<&'static dyn ProcessType>],
}

impl SecureTimeComponent {
    pub fn new(processes: &'static [Option<&'static dyn ProcessType>]) -> SecureTimeComponent {
        SecureTimeComponent { processes }
    }
}

impl Component for SecureTimeComponent {
    type StaticInput = &'static mut [MaybeUninit<STProcessNode<'static>>];
    type Output = &'static mut SecureTimeSched<'static>;

    unsafe fn finalize(self, buf: Self::StaticInput) -> Self::Output {
        let scheduler = static_init!(SecureTimeSched<'static>, SecureTimeSched::new());

        for (i, node) in buf.iter_mut().enumerate() {
            let init_node = static_init_half!(
                node,
                STProcessNode<'static>,
                STProcessNode::new(&self.processes[i])
            );
            scheduler.processes.push_head(init_node);
        }
        scheduler
    }
}
