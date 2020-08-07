//! Chip trait setup.

use core::fmt::Write;
use cortexm4;
use kernel::Chip;
use kernel::Platform;

pub struct Apollo3<P: Platform + 'static> {
    mpu: cortexm4::mpu::MPU,
    userspace_kernel_boundary: cortexm4::syscall::SysCall,
    scheduler_timer: cortexm4::systick::SysTick,
    platform: &'static P,
}

impl<P: Platform + 'static> Apollo3<P> {
    pub unsafe fn new(platform: &'static P) -> Self {
        Self {
            mpu: cortexm4::mpu::MPU::new(),
            userspace_kernel_boundary: cortexm4::syscall::SysCall::new(),
            scheduler_timer: cortexm4::systick::SysTick::new_with_calibration(48_000_000),
            platform,
        }
    }
}

/// This macro defines a struct that, when initialized,
/// instantiates all drivers for the apollo3. If a board
/// wishes to use only a subset of these drivers, this
/// macro cannot be used, and this struct should be
/// redefined.
#[macro_export]
macro_rules! apollo3_driver_definitions {
    () => {
        struct Apollo3Drivers {
            stimer: apollo3::stimer::STimer<'static>,
            uart0: apollo3::uart::Uart<'static>,
            uart1: apollo3::uart::Uart<'static>,
            gpio_port: apollo3::gpio::Port<'static>,
            iom0: apollo3::iom::Iom<'static>,
            iom1: apollo3::iom::Iom<'static>,
            iom2: apollo3::iom::Iom<'static>,
            iom3: apollo3::iom::Iom<'static>,
            iom4: apollo3::iom::Iom<'static>,
            iom5: apollo3::iom::Iom<'static>,
            ble: apollo3::ble::Ble<'static>,
        }
        impl Apollo3Drivers {
            unsafe fn new() -> Self {
                Self {
                    stimer: apollo3::stimer::STimer::new(),
                    uart0: apollo3::uart::Uart::new_uart_0(),
                    uart1: apollo3::uart::Uart::new_uart_1(),
                    gpio_port: apollo3::gpio::Port::new(),
                    iom0: apollo3::iom::Iom::new0(),
                    iom1: apollo3::iom::Iom::new1(),
                    iom2: apollo3::iom::Iom::new2(),
                    iom3: apollo3::iom::Iom::new3(),
                    iom4: apollo3::iom::Iom::new4(),
                    iom5: apollo3::iom::Iom::new5(),
                    ble: apollo3::ble::Ble::new(),
                }
            }
        }
    };
}

/// This macro defines the interrupt mapping for all drivers in the
/// Apollo3 chip. If a board wishes to use only a subset of these drivers,
/// the mapping must be manually defined.
#[macro_export]
macro_rules! apollo3_interrupt_mapping {
    ($P:expr, $I: expr) => {
        use apollo3::nvic;
        match $I {
            nvic::STIMER..=nvic::STIMER_CMPR7 => $P.drivers.stimer.handle_interrupt(),
            nvic::UART0 => $P.drivers.uart0.handle_interrupt(),
            nvic::UART1 => $P.drivers.uart1.handle_interrupt(),
            nvic::GPIO => $P.drivers.gpio_port.handle_interrupt(),
            nvic::IOMSTR0 => $P.drivers.iom0.handle_interrupt(),
            nvic::IOMSTR1 => $P.drivers.iom1.handle_interrupt(),
            nvic::IOMSTR2 => $P.drivers.iom2.handle_interrupt(),
            nvic::IOMSTR3 => $P.drivers.iom3.handle_interrupt(),
            nvic::IOMSTR4 => $P.drivers.iom4.handle_interrupt(),
            nvic::IOMSTR5 => $P.drivers.iom5.handle_interrupt(),
            nvic::BLE => $P.drivers.ble.handle_interrupt(),
            _ => panic!("unhandled interrupt {}", $I),
        }
    };
}

impl<P: Platform + 'static> Chip for Apollo3<P> {
    type MPU = cortexm4::mpu::MPU;
    type UserspaceKernelBoundary = cortexm4::syscall::SysCall;
    type SchedulerTimer = cortexm4::systick::SysTick;
    type WatchDog = ();

    fn service_pending_interrupts(&self) {
        unsafe {
            loop {
                if let Some(interrupt) = cortexm4::nvic::next_pending() {
                    self.platform.handle_interrupt(interrupt);

                    let n = cortexm4::nvic::Nvic::new(interrupt);
                    n.clear_pending();
                    n.enable();
                } else {
                    break;
                }
            }
        }
    }

    fn has_pending_interrupts(&self) -> bool {
        unsafe { cortexm4::nvic::has_pending() }
    }

    fn mpu(&self) -> &cortexm4::mpu::MPU {
        &self.mpu
    }

    fn scheduler_timer(&self) -> &cortexm4::systick::SysTick {
        &self.scheduler_timer
    }

    fn watchdog(&self) -> &Self::WatchDog {
        &()
    }

    fn userspace_kernel_boundary(&self) -> &cortexm4::syscall::SysCall {
        &self.userspace_kernel_boundary
    }

    fn sleep(&self) {
        unsafe {
            cortexm4::scb::unset_sleepdeep();
            cortexm4::support::wfi();
        }
    }

    unsafe fn atomic<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        cortexm4::support::atomic(f)
    }

    unsafe fn print_state(&self, write: &mut dyn Write) {
        cortexm4::print_cortexm4_state(write);
    }
}
