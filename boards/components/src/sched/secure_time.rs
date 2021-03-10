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
use kernel::static_init_half;
use kernel::{KernelTask, STProcessNode, SecureTimeSched};

// Note: The below macro is a non-traditional use of components, as the secure time scheduler
// is stack allocated. This is neccessary because Rust does not provide a mechanism for
// specifying the exact type of a function item, which means that it is impossible to create
// a global static with a function item as a generic parameter, as global statics require
// types be declared explicitly. Tracking issue: https://github.com/rust-lang/rfcs/issues/1349
#[macro_export]
macro_rules! st_component_helper {
    ($N:expr, $F:expr $(,)?) => {{
        use core::mem::MaybeUninit;
        use kernel::static_buf;
        use kernel::STProcessNode;
        use kernel::SecureTimeSched;
        const UNINIT: MaybeUninit<STProcessNode<'static>> = MaybeUninit::uninit();
        static mut BUF: [MaybeUninit<STProcessNode<'static>>; $N] = [UNINIT; $N];
        let mut buf2 = SecureTimeSched::new($F);
        (&mut BUF, buf2)
    };};
}

pub struct SecureTimeComponent<F>
where
    F: 'static + FnOnce(KernelTask) -> u32,
{
    processes: &'static [Option<&'static dyn ProcessType>],
    _wcet_func: F,
}

impl<F: 'static + FnOnce(KernelTask) -> u32> SecureTimeComponent<F> {
    pub fn new(processes: &'static [Option<&'static dyn ProcessType>], wcet_func: F) -> Self {
        Self {
            processes,
            _wcet_func: wcet_func,
        }
    }
}

impl<F: 'static + FnOnce(KernelTask) -> u32> Component for SecureTimeComponent<F> {
    type StaticInput = (
        &'static mut [MaybeUninit<STProcessNode<'static>>],
        SecureTimeSched<'static, F>,
    );
    type Output = SecureTimeSched<'static, F>;

    unsafe fn finalize(self, buf: Self::StaticInput) -> Self::Output {
        let scheduler = buf.1;

        for (i, node) in buf.0.iter_mut().enumerate() {
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
