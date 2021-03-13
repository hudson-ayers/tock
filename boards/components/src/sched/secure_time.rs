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

use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use core::mem::MaybeUninit;
use kernel::component::Component;
use kernel::hil::time;
use kernel::procs::ProcessType;
use kernel::static_init_half;
use kernel::{KernelTask, STTaskNode, SecureTimeSched};

// Note: The below macro is a non-traditional use of components, as the secure time scheduler
// is stack allocated. This is neccessary because Rust does not provide a mechanism for
// specifying the exact type of a function item, which means that it is impossible to create
// a global static with a function item as a generic parameter, as global statics require
// types be declared explicitly. Tracking issue: https://github.com/rust-lang/rfcs/issues/1349
#[macro_export]
macro_rules! st_component_helper {
    ($F:expr, $A:ty, $MA:expr $(,)?) => {{
        use core::mem::MaybeUninit;
        use kernel::static_buf;
        use kernel::STTaskNode;
        use kernel::SecureTimeSched;
        use kernel::MAX_TASKS;
        const UNINIT: MaybeUninit<STTaskNode<'static>> = MaybeUninit::uninit();
        static mut BUF0: [MaybeUninit<STTaskNode<'static>>; MAX_TASKS] = [UNINIT; MAX_TASKS];
        static mut BUF1: [MaybeUninit<STTaskNode<'static>>; MAX_TASKS] = [UNINIT; MAX_TASKS];
        let scheduler_alarm = static_init!(VirtualMuxAlarm<'static, $A>, VirtualMuxAlarm::new($MA));
        let mut buf2 = SecureTimeSched::new(scheduler_alarm, $F);

        (&mut BUF0, &mut BUF1, buf2)
    };};
}

pub struct SecureTimeComponent<F, A>
where
    F: 'static + FnOnce(KernelTask) -> u32,
    A: 'static + time::Alarm<'static>,
{
    processes: &'static [Option<&'static dyn ProcessType>],
    _wcet_func: F, //we just pass it here so the types work, not actually used.
    _alarm_mux: &'static MuxAlarm<'static, A>, //same
}

impl<F: 'static + FnOnce(KernelTask) -> u32, A: 'static + time::Alarm<'static>>
    SecureTimeComponent<F, A>
{
    pub fn new(
        alarm_mux: &'static MuxAlarm<'static, A>,
        processes: &'static [Option<&'static dyn ProcessType>],
        wcet_func: F,
    ) -> Self {
        Self {
            _alarm_mux: alarm_mux,
            processes,
            _wcet_func: wcet_func,
        }
    }
}

impl<F: 'static + FnOnce(KernelTask) -> u32, A: 'static + time::Alarm<'static>> Component
    for SecureTimeComponent<F, A>
{
    type StaticInput = (
        &'static mut [MaybeUninit<STTaskNode<'static>>],
        &'static mut [MaybeUninit<STTaskNode<'static>>],
        SecureTimeSched<'static, F, VirtualMuxAlarm<'static, A>>,
    );
    type Output = SecureTimeSched<'static, F, VirtualMuxAlarm<'static, A>>;

    unsafe fn finalize(self, buf: Self::StaticInput) -> Self::Output {
        let scheduler = buf.2;
        for (i, node) in buf.0.iter_mut().enumerate() {
            let init_node = static_init_half!(node, STTaskNode<'static>, STTaskNode::new(None));
            scheduler.ready_tasks.push_head(init_node);
        }
        for (i, node) in buf.1.iter_mut().enumerate() {
            let init_node = static_init_half!(node, STTaskNode<'static>, STTaskNode::new(None));
            scheduler.potential_tasks.push_head(init_node);
        }
        scheduler
    }
}
