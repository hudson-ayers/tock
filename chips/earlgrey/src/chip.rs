//! High-level setup and interrupt mapping for the chip.

use core::fmt::Write;
use core::hint::unreachable_unchecked;
use kernel;
use kernel::common::registers::FieldValue;
use kernel::debug;
use kernel::hil::time::Alarm;
use kernel::{Chip, InterruptService};
use rv32i::csr::{mcause, mie::mie, mip::mip, mtvec::mtvec, CSR};
use rv32i::syscall::SysCall;
use rv32i::PMPConfigMacro;

use crate::chip_config::CONFIG;
use crate::interrupts;
use crate::plic;

PMPConfigMacro!(4);

pub const CHIP_FREQ: u32 = CONFIG.chip_freq;

pub struct EarlGrey<'a, A: 'static + Alarm<'static>, I: InterruptService<()> + 'a> {
    userspace_kernel_boundary: SysCall,
    pmp: PMPConfig<[Option<PMPRegion>; 2]>,
    scheduler_timer: kernel::VirtualSchedulerTimer<A>,
    timer: &'static crate::timer::RvTimer<'static>,
    pwrmgr: lowrisc::pwrmgr::PwrMgr,
    plic_interrupt_service: &'a I,
}

pub struct EarlGreyDefaultPeripherals<'a> {
    pub aes: crate::aes::Aes<'a>,
    pub hmac: lowrisc::hmac::Hmac<'a>,
    pub usb: lowrisc::usbdev::Usb<'a>,
    pub uart0: lowrisc::uart::Uart<'a>,
    pub gpio_port: crate::gpio::Port<'a>,
    pub i2c: lowrisc::i2c::I2c<'a>,
}

impl<'a> EarlGreyDefaultPeripherals<'a> {
    pub fn new() -> Self {
        Self {
            aes: crate::aes::Aes::new(),
            hmac: lowrisc::hmac::Hmac::new(crate::hmac::HMAC0_BASE),
            usb: lowrisc::usbdev::Usb::new(crate::usbdev::USB0_BASE),
            uart0: lowrisc::uart::Uart::new(crate::uart::UART0_BASE, CHIP_FREQ),
            gpio_port: crate::gpio::Port::new(),
            i2c: lowrisc::i2c::I2c::new(crate::i2c::I2C_BASE, (1 / CHIP_FREQ) * 1000 * 1000),
        }
    }
}

impl<'a> InterruptService<()> for EarlGreyDefaultPeripherals<'a> {
    unsafe fn service_interrupt(&self, interrupt: u32) -> bool {
        match interrupt {
            interrupts::UART_TX_WATERMARK..=interrupts::UART_RX_PARITY_ERR => {
                self.uart0.handle_interrupt();
            }
            int_pin @ interrupts::GPIO_PIN0..=interrupts::GPIO_PIN31 => {
                let pin = &self.gpio_port[(int_pin - interrupts::GPIO_PIN0) as usize];
                pin.handle_interrupt();
            }
            interrupts::HMAC_HMAC_DONE..=interrupts::HMAC_HMAC_ERR => {
                self.hmac.handle_interrupt();
            }
            interrupts::USBDEV_PKT_RECEIVED..=interrupts::USBDEV_CONNECTED => {
                self.usb.handle_interrupt();
            }
            _ => return false,
        }
        true
    }

    unsafe fn service_deferred_call(&self, _: ()) -> bool {
        false
    }
}

impl<'a, A: 'static + Alarm<'static>, I: InterruptService<()> + 'a> EarlGrey<'a, A, I> {
    pub unsafe fn new(
        virtual_alarm: &'static A,
        plic_interrupt_service: &'a I,
        timer: &'static crate::timer::RvTimer,
    ) -> Self {
        Self {
            userspace_kernel_boundary: SysCall::new(),
            pmp: PMPConfig::default(),
            scheduler_timer: kernel::VirtualSchedulerTimer::new(virtual_alarm),
            pwrmgr: lowrisc::pwrmgr::PwrMgr::new(crate::pwrmgr::PWRMGR_BASE),
            timer,
            plic_interrupt_service,
        }
    }

    pub unsafe fn enable_plic_interrupts(&self) {
        plic::disable_all();
        plic::clear_all_pending();
        plic::enable_all();
    }

    unsafe fn handle_plic_interrupts(&self) {
        while let Some(interrupt) = plic::next_pending() {
            if interrupt == interrupts::PWRMGRWAKEUP {
                self.pwrmgr.handle_interrupt();
                self.check_until_true_or_interrupt(|| self.pwrmgr.check_clock_propagation(), None);
            } else if !self.plic_interrupt_service.service_interrupt(interrupt) {
                debug!("Pidx {}", interrupt);
            }
            plic::complete(interrupt);
        }
    }

    /// Run a function in an interruptable loop.
    ///
    /// The function will run until it returns true, an interrupt occurs or if
    /// `max_tries` is not `None` and that limit is reached.
    /// If the function returns true this call will also return true. If an
    /// interrupt occurs or `max_tries` is reached this call will return false.
    fn check_until_true_or_interrupt<F>(&self, f: F, max_tries: Option<usize>) -> bool
    where
        F: Fn() -> bool,
    {
        match max_tries {
            Some(t) => {
                for _i in 0..t {
                    if self.has_pending_interrupts() {
                        return false;
                    }
                    if f() {
                        return true;
                    }
                }
            }
            None => {
                while !self.has_pending_interrupts() {
                    if f() {
                        return true;
                    }
                }
            }
        }

        false
    }
}

impl<'a, A: 'static + Alarm<'static>, I: InterruptService<()> + 'a> kernel::Chip
    for EarlGrey<'a, A, I>
{
    type MPU = PMPConfig<[Option<PMPRegion>; 2]>;
    type UserspaceKernelBoundary = SysCall;
    type SchedulerTimer = kernel::VirtualSchedulerTimer<A>;
    type WatchDog = ();

    fn mpu(&self) -> &Self::MPU {
        &self.pmp
    }

    fn scheduler_timer(&self) -> &Self::SchedulerTimer {
        &self.scheduler_timer
    }

    fn watchdog(&self) -> &Self::WatchDog {
        &()
    }

    fn userspace_kernel_boundary(&self) -> &SysCall {
        &self.userspace_kernel_boundary
    }

    fn service_pending_interrupts(&self) {
        let mut reenable_intr = FieldValue::<u32, mie::Register>::new(0, 0, 0);

        loop {
            let mip = CSR.mip.extract();

            if mip.is_set(mip::mtimer) {
                self.timer.service_interrupt();
                reenable_intr += mie::mtimer::SET;
            }
            if mip.is_set(mip::mext) {
                unsafe {
                    self.handle_plic_interrupts();
                }
                reenable_intr += mie::mext::SET;
            }

            if !mip.matches_any(mip::mext::SET + mip::mtimer::SET) {
                break;
            }
        }

        // re-enable any interrupt classes which we handled
        CSR.mie.modify(reenable_intr);
    }

    fn has_pending_interrupts(&self) -> bool {
        let mip = CSR.mip.extract();
        mip.matches_any(mip::mext::SET + mip::mtimer::SET)
    }

    fn sleep(&self) {
        unsafe {
            self.pwrmgr.enable_low_power();
            self.check_until_true_or_interrupt(|| self.pwrmgr.check_clock_propagation(), None);
            rv32i::support::wfi();
        }
    }

    unsafe fn atomic<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        rv32i::support::atomic(f)
    }

    unsafe fn print_state(&self, writer: &mut dyn Write) {
        let _ = writer.write_fmt(format_args!(
            "\r\n---| EarlGrey configuration for {} |---",
            CONFIG.name
        ));
        rv32i::print_riscv_state(writer);
    }
}

fn handle_exception(exception: mcause::Exception) {
    match exception {
        mcause::Exception::UserEnvCall | mcause::Exception::SupervisorEnvCall => (),

        mcause::Exception::InstructionMisaligned
        | mcause::Exception::InstructionFault
        | mcause::Exception::IllegalInstruction
        | mcause::Exception::Breakpoint
        | mcause::Exception::LoadMisaligned
        | mcause::Exception::LoadFault
        | mcause::Exception::StoreMisaligned
        | mcause::Exception::StoreFault
        | mcause::Exception::MachineEnvCall
        | mcause::Exception::InstructionPageFault
        | mcause::Exception::LoadPageFault
        | mcause::Exception::StorePageFault
        | mcause::Exception::Unknown => {
            panic!("fatal exception");
        }
    }
}

unsafe fn handle_interrupt(intr: mcause::Interrupt) {
    match intr {
        mcause::Interrupt::UserSoft
        | mcause::Interrupt::UserTimer
        | mcause::Interrupt::UserExternal => {
            debug!("unexpected user-mode interrupt");
        }
        mcause::Interrupt::SupervisorExternal
        | mcause::Interrupt::SupervisorTimer
        | mcause::Interrupt::SupervisorSoft => {
            debug!("unexpected supervisor-mode interrupt");
        }

        mcause::Interrupt::MachineSoft => {
            CSR.mie.modify(mie::msoft::CLEAR);
        }
        mcause::Interrupt::MachineTimer => {
            CSR.mie.modify(mie::mtimer::CLEAR);
        }
        mcause::Interrupt::MachineExternal => {
            CSR.mie.modify(mie::mext::CLEAR);
        }

        mcause::Interrupt::Unknown => {
            debug!("interrupt of unknown cause");
        }
    }
}

/// Trap handler for board/chip specific code.
///
/// For the Ibex this gets called when an interrupt occurs while the chip is
/// in kernel mode. All we need to do is check which interrupt occurred and
/// disable it.
#[export_name = "_start_trap_rust"]
pub unsafe extern "C" fn start_trap_rust() {
    match mcause::Trap::from(CSR.mcause.extract()) {
        mcause::Trap::Interrupt(interrupt) => {
            handle_interrupt(interrupt);
        }
        mcause::Trap::Exception(exception) => {
            handle_exception(exception);
        }
    }
}

/// Function that gets called if an interrupt occurs while an app was running.
/// mcause is passed in, and this function should correctly handle disabling the
/// interrupt that fired so that it does not trigger again.
#[export_name = "_disable_interrupt_trap_handler"]
pub unsafe extern "C" fn disable_interrupt_trap_handler(mcause_val: u32) {
    match mcause::Trap::from(mcause_val) {
        mcause::Trap::Interrupt(interrupt) => {
            handle_interrupt(interrupt);
        }
        _ => {
            panic!("unexpected non-interrupt\n");
        }
    }
}

pub unsafe fn configure_trap_handler() {
    // The Ibex CPU does not support non-vectored trap entries.
    CSR.mtvec
        .write(mtvec::trap_addr.val(_start_trap_vectored as u32 >> 2) + mtvec::mode::Vectored)
}

// Mock implementation for crate tests that does not include the section
// specifier, as the test will not use our linker script, and the host
// compilation environment may not allow the section name.
#[cfg(not(any(target_arch = "riscv32", target_os = "none")))]
pub extern "C" fn _start_trap_vectored() {
    unsafe {
        unreachable_unchecked();
    }
}

#[cfg(all(target_arch = "riscv32", target_os = "none"))]
#[link_section = ".riscv.trap_vectored"]
#[export_name = "_start_trap_vectored"]
#[naked]
pub extern "C" fn _start_trap_vectored() -> ! {
    unsafe {
        // According to the Ibex user manual:
        // [NMI] has interrupt ID 31, i.e., it has the highest priority of all
        // interrupts and the core jumps to the trap-handler base address (in
        // mtvec) plus 0x7C to handle the NMI.
        //
        // Below are 32 (non-compressed) jumps to cover the entire possible
        // range of vectored traps.
        #[cfg(all(target_arch = "riscv32", target_os = "none"))]
        llvm_asm!("
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
            j _start_trap
        "
        :
        :
        :
        : "volatile");
        unreachable_unchecked()
    }
}
