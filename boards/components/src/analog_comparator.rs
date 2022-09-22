//! Component for initializing an Analog Comparator.
//!
//! This provides one Component, AcComponent, which implements
//! a userspace syscall interface to a passed analog comparator driver.
//!
//! Usage
//! -----
//! ```rust
//! let analog_comparator = components::analog_comparator::AcComponent::new(
//!     &sam4l::acifc::ACIFC,
//!     components::acomp_component_helper!(
//!         <sam4l::acifc::Acifc as kernel::hil::analog_comparator::AnalogComparator>::Channel,
//!         &sam4l::acifc::CHANNEL_AC0,
//!         &sam4l::acifc::CHANNEL_AC1,
//!         &sam4l::acifc::CHANNEL_AC2,
//!         &sam4l::acifc::CHANNEL_AC3
//!     ),
//! )
//! .finalize(components::acomp_component_buf!(sam4l::acifc::Acifc));
//! ```

use core::mem::MaybeUninit;

use kernel;
use kernel::capabilities;
use kernel::component::Component;
use kernel::create_capability;

use capsules::analog_comparator;

#[macro_export]
macro_rules! acomp_component_helper {
    ($Channel:ty, $($P:expr),+ $(,)?) => {{
        use kernel::count_expressions;
        use kernel::static_init;
        const NUM_CHANNELS: usize = count_expressions!($($P),+);

        static_init!(
            [&'static $Channel; NUM_CHANNELS],
            [
                $($P,)*
            ]
        )
    };};
}

#[macro_export]
macro_rules! acomp_component_static {
    ($Comp:ty $(,)?) => {{
        kernel::static_buf!(capsules::analog_comparator::AnalogComparator<'static, $Comp>)
    };};
}

pub struct AcComponent<AC: 'static + kernel::hil::analog_comparator::AnalogComparator<'static>> {
    comp: &'static AC,
    ac_channels: &'static [&'static AC::Channel],
    board_kernel: &'static kernel::Kernel,
    driver_num: usize,
}

impl<AC: 'static + kernel::hil::analog_comparator::AnalogComparator<'static>> AcComponent<AC> {
    pub fn new(
        comp: &'static AC,
        ac_channels: &'static [&'static AC::Channel],
        board_kernel: &'static kernel::Kernel,
        driver_num: usize,
    ) -> Self {
        Self {
            comp,
            ac_channels,
            board_kernel,
            driver_num,
        }
    }
}

impl<AC: 'static + kernel::hil::analog_comparator::AnalogComparator<'static>> Component
    for AcComponent<AC>
{
    type StaticInput = &'static mut MaybeUninit<analog_comparator::AnalogComparator<'static, AC>>;
    type Output = &'static analog_comparator::AnalogComparator<'static, AC>;

    unsafe fn finalize(self, static_buffer: Self::StaticInput) -> Self::Output {
        let grant_cap = create_capability!(capabilities::MemoryAllocationCapability);
        let grant_ac = self.board_kernel.create_grant(self.driver_num, &grant_cap);

        let analog_comparator = static_buffer.write(analog_comparator::AnalogComparator::new(
            self.comp,
            self.ac_channels,
            grant_ac,
        ));
        self.comp.set_client(analog_comparator);

        analog_comparator
    }
}
