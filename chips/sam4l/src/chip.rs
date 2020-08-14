//! Interrupt mapping and DMA channel setup.

use crate::deferred_call_tasks::Task;
use crate::pm;

use core::fmt::Write;
use cortexm4;
use kernel::common::deferred_call;
use kernel::{Chip, InterruptService};

pub struct Sam4l<I: InterruptService<Task> + 'static> {
    mpu: cortexm4::mpu::MPU,
    userspace_kernel_boundary: cortexm4::syscall::SysCall,
    scheduler_timer: cortexm4::systick::SysTick,
    pub pm: &'static crate::pm::PowerManager,
    interrupt_service: &'static I,
}

impl<I: InterruptService<Task> + 'static> Sam4l<I> {
    pub unsafe fn new(pm: &'static crate::pm::PowerManager, interrupt_service: &'static I) -> Self {
        Self {
            mpu: cortexm4::mpu::MPU::new(),
            userspace_kernel_boundary: cortexm4::syscall::SysCall::new(),
            scheduler_timer: cortexm4::systick::SysTick::new(),
            pm,
            interrupt_service,
        }
    }
}

/// This macro defines a struct that, when initialized,
/// instantiates all peripheral drivers for the sam4l. If a board
/// wishes to use only a subset of these peripherals, this
/// macro cannot be used, and this struct should be
/// constructed manually in main.rs. The input to the macro is the name of the struct
/// that will hold the peripherals, which can be chosen by the board.
#[macro_export]
macro_rules! create_default_sam4l_peripherals {
    ($N:ident) => {
        use sam4l::deferred_call_tasks::Task;
        struct $N {
            acifc: sam4l::acifc::Acifc<'static>,
            adc: sam4l::adc::Adc,
            aes: sam4l::aes::Aes<'static>,
            ast: sam4l::ast::Ast<'static>,
            crccu: sam4l::crccu::Crccu<'static>,
            dac: sam4l::dac::Dac,
            dma_channels: [sam4l::dma::DMAChannel; 16],
            eic: sam4l::eic::Eic<'static>,
            flash_controller: sam4l::flashcalw::FLASHCALW,
            gloc: sam4l::gloc::Gloc,
            pa: sam4l::gpio::Port<'static>,
            pb: sam4l::gpio::Port<'static>,
            pc: sam4l::gpio::Port<'static>,
            i2c0: sam4l::i2c::I2CHw,
            i2c1: sam4l::i2c::I2CHw,
            i2c2: sam4l::i2c::I2CHw,
            i2c3: sam4l::i2c::I2CHw,
            spi: sam4l::spi::SpiHw,
            trng: sam4l::trng::Trng<'static>,
            usart0: sam4l::usart::USART<'static>,
            usart1: sam4l::usart::USART<'static>,
            usart2: sam4l::usart::USART<'static>,
            usart3: sam4l::usart::USART<'static>,
            usbc: sam4l::usbc::Usbc<'static>,
        }
        impl $N {
            fn new(pm: &'static sam4l::pm::PowerManager) -> Self {
                use sam4l::dma::{DMAChannel, DMAChannelNum};
                Self {
                    acifc: sam4l::acifc::Acifc::new(),
                    adc: sam4l::adc::Adc::new(sam4l::dma::DMAPeripheral::ADCIFE_RX, pm),
                    aes: sam4l::aes::Aes::new(),
                    ast: sam4l::ast::Ast::new(),
                    crccu: sam4l::crccu::Crccu::new(),
                    dac: sam4l::dac::Dac::new(),
                    dma_channels: [
                        DMAChannel::new(DMAChannelNum::DMAChannel00),
                        DMAChannel::new(DMAChannelNum::DMAChannel01),
                        DMAChannel::new(DMAChannelNum::DMAChannel02),
                        DMAChannel::new(DMAChannelNum::DMAChannel03),
                        DMAChannel::new(DMAChannelNum::DMAChannel04),
                        DMAChannel::new(DMAChannelNum::DMAChannel05),
                        DMAChannel::new(DMAChannelNum::DMAChannel06),
                        DMAChannel::new(DMAChannelNum::DMAChannel07),
                        DMAChannel::new(DMAChannelNum::DMAChannel08),
                        DMAChannel::new(DMAChannelNum::DMAChannel09),
                        DMAChannel::new(DMAChannelNum::DMAChannel10),
                        DMAChannel::new(DMAChannelNum::DMAChannel11),
                        DMAChannel::new(DMAChannelNum::DMAChannel12),
                        DMAChannel::new(DMAChannelNum::DMAChannel13),
                        DMAChannel::new(DMAChannelNum::DMAChannel14),
                        DMAChannel::new(DMAChannelNum::DMAChannel15),
                    ],
                    eic: sam4l::eic::Eic::new(),
                    flash_controller: sam4l::flashcalw::FLASHCALW::new(
                        sam4l::pm::HSBClock::FLASHCALW,
                        sam4l::pm::HSBClock::FLASHCALWP,
                        sam4l::pm::PBBClock::FLASHCALW,
                    ),
                    gloc: sam4l::gloc::Gloc::new(),
                    pa: sam4l::gpio::Port::new_port_a(),
                    pb: sam4l::gpio::Port::new_port_b(),
                    pc: sam4l::gpio::Port::new_port_c(),
                    i2c0: sam4l::i2c::I2CHw::new_i2c0(pm),
                    i2c1: sam4l::i2c::I2CHw::new_i2c1(pm),
                    i2c2: sam4l::i2c::I2CHw::new_i2c2(pm),
                    i2c3: sam4l::i2c::I2CHw::new_i2c3(pm),
                    spi: sam4l::spi::SpiHw::new(pm),
                    trng: sam4l::trng::Trng::new(),
                    usart0: sam4l::usart::USART::new_usart0(pm),
                    usart1: sam4l::usart::USART::new_usart1(pm),
                    usart2: sam4l::usart::USART::new_usart2(pm),
                    usart3: sam4l::usart::USART::new_usart3(pm),
                    usbc: sam4l::usbc::Usbc::new(pm),
                }
            }

            // Sam4l was the only chip that partially initialized some drivers in new, I
            // have moved that initialization to this helper function.
            // TODO: Delete explanation
            pub fn setup_dma(&'static self) {
                use sam4l::dma;
                self.usart0
                    .set_dma(&self.dma_channels[0], &self.dma_channels[1]);
                self.dma_channels[0].initialize(&self.usart0, dma::DMAWidth::Width8Bit);
                self.dma_channels[1].initialize(&self.usart0, dma::DMAWidth::Width8Bit);

                self.usart1
                    .set_dma(&self.dma_channels[2], &self.dma_channels[3]);
                self.dma_channels[2].initialize(&self.usart1, dma::DMAWidth::Width8Bit);
                self.dma_channels[3].initialize(&self.usart1, dma::DMAWidth::Width8Bit);

                self.usart2
                    .set_dma(&self.dma_channels[4], &self.dma_channels[5]);
                self.dma_channels[4].initialize(&self.usart2, dma::DMAWidth::Width8Bit);
                self.dma_channels[5].initialize(&self.usart2, dma::DMAWidth::Width8Bit);

                self.usart3
                    .set_dma(&self.dma_channels[6], &self.dma_channels[7]);
                self.dma_channels[6].initialize(&self.usart3, dma::DMAWidth::Width8Bit);
                self.dma_channels[7].initialize(&self.usart3, dma::DMAWidth::Width8Bit);

                self.spi
                    .set_dma(&self.dma_channels[8], &self.dma_channels[9]);
                self.dma_channels[8].initialize(&self.spi, dma::DMAWidth::Width8Bit);
                self.dma_channels[9].initialize(&self.spi, dma::DMAWidth::Width8Bit);

                self.i2c0.set_dma(&self.dma_channels[10]);
                self.dma_channels[10].initialize(&self.i2c0, dma::DMAWidth::Width8Bit);

                self.i2c1.set_dma(&self.dma_channels[11]);
                self.dma_channels[11].initialize(&self.i2c1, dma::DMAWidth::Width8Bit);

                self.i2c2.set_dma(&self.dma_channels[12]);
                self.dma_channels[12].initialize(&self.i2c2, dma::DMAWidth::Width8Bit);

                self.adc.set_dma(&self.dma_channels[13]);
                self.dma_channels[13].initialize(&self.adc, dma::DMAWidth::Width16Bit);
            }
        }
        impl kernel::InterruptService<Task> for $N {
            unsafe fn service_interrupt(&self, interrupt: u32) -> bool {
                use sam4l::nvic;
                match interrupt {
                    nvic::ASTALARM => self.ast.handle_interrupt(),

                    nvic::USART0 => self.usart0.handle_interrupt(),
                    nvic::USART1 => self.usart1.handle_interrupt(),
                    nvic::USART2 => self.usart2.handle_interrupt(),
                    nvic::USART3 => self.usart3.handle_interrupt(),

                    nvic::PDCA0 => self.dma_channels[0].handle_interrupt(),
                    nvic::PDCA1 => self.dma_channels[1].handle_interrupt(),
                    nvic::PDCA2 => self.dma_channels[2].handle_interrupt(),
                    nvic::PDCA3 => self.dma_channels[3].handle_interrupt(),
                    nvic::PDCA4 => self.dma_channels[4].handle_interrupt(),
                    nvic::PDCA5 => self.dma_channels[5].handle_interrupt(),
                    nvic::PDCA6 => self.dma_channels[6].handle_interrupt(),
                    nvic::PDCA7 => self.dma_channels[7].handle_interrupt(),
                    nvic::PDCA8 => self.dma_channels[8].handle_interrupt(),
                    nvic::PDCA9 => self.dma_channels[9].handle_interrupt(),
                    nvic::PDCA10 => self.dma_channels[10].handle_interrupt(),
                    nvic::PDCA11 => self.dma_channels[11].handle_interrupt(),
                    nvic::PDCA12 => self.dma_channels[12].handle_interrupt(),
                    nvic::PDCA13 => self.dma_channels[13].handle_interrupt(),
                    nvic::PDCA14 => self.dma_channels[14].handle_interrupt(),
                    nvic::PDCA15 => self.dma_channels[15].handle_interrupt(),

                    nvic::CRCCU => self.crccu.handle_interrupt(),
                    nvic::USBC => self.usbc.handle_interrupt(),

                    nvic::GPIO0 => self.pa.handle_interrupt(),
                    nvic::GPIO1 => self.pa.handle_interrupt(),
                    nvic::GPIO2 => self.pa.handle_interrupt(),
                    nvic::GPIO3 => self.pa.handle_interrupt(),
                    nvic::GPIO4 => self.pb.handle_interrupt(),
                    nvic::GPIO5 => self.pb.handle_interrupt(),
                    nvic::GPIO6 => self.pb.handle_interrupt(),
                    nvic::GPIO7 => self.pb.handle_interrupt(),
                    nvic::GPIO8 => self.pc.handle_interrupt(),
                    nvic::GPIO9 => self.pc.handle_interrupt(),
                    nvic::GPIO10 => self.pc.handle_interrupt(),
                    nvic::GPIO11 => self.pc.handle_interrupt(),

                    nvic::SPI => self.spi.handle_interrupt(),

                    nvic::TWIM0 => self.i2c0.handle_interrupt(),
                    nvic::TWIM1 => self.i2c1.handle_interrupt(),
                    nvic::TWIM2 => self.i2c2.handle_interrupt(),
                    nvic::TWIM3 => self.i2c3.handle_interrupt(),
                    nvic::TWIS0 => self.i2c0.handle_slave_interrupt(),
                    nvic::TWIS1 => self.i2c1.handle_slave_interrupt(),

                    nvic::HFLASHC => self.flash_controller.handle_interrupt(),
                    nvic::ADCIFE => self.adc.handle_interrupt(),
                    nvic::DACC => self.dac.handle_interrupt(),
                    nvic::ACIFC => self.acifc.handle_interrupt(),

                    nvic::TRNG => self.trng.handle_interrupt(),
                    nvic::AESA => self.aes.handle_interrupt(),

                    nvic::EIC1 => self.eic.handle_interrupt(&sam4l::eic::Line::Ext1),
                    nvic::EIC2 => self.eic.handle_interrupt(&sam4l::eic::Line::Ext2),
                    nvic::EIC3 => self.eic.handle_interrupt(&sam4l::eic::Line::Ext3),
                    nvic::EIC4 => self.eic.handle_interrupt(&sam4l::eic::Line::Ext4),
                    nvic::EIC5 => self.eic.handle_interrupt(&sam4l::eic::Line::Ext5),
                    nvic::EIC6 => self.eic.handle_interrupt(&sam4l::eic::Line::Ext6),
                    nvic::EIC7 => self.eic.handle_interrupt(&sam4l::eic::Line::Ext7),
                    nvic::EIC8 => self.eic.handle_interrupt(&sam4l::eic::Line::Ext8),
                    _ => return false,
                }
                true
            }
            unsafe fn service_deferred_call(&self, task: Task) -> bool {
                match task {
                    sam4l::deferred_call_tasks::Task::Flashcalw => {
                        self.flash_controller.handle_interrupt()
                    }
                    _ => return false,
                }
                true
            }
        }
    };
}

impl<I: InterruptService<Task> + 'static> Chip for Sam4l<I> {
    type MPU = cortexm4::mpu::MPU;
    type UserspaceKernelBoundary = cortexm4::syscall::SysCall;
    type SchedulerTimer = cortexm4::systick::SysTick;
    type WatchDog = ();

    fn service_pending_interrupts(&self) {
        unsafe {
            loop {
                if let Some(task) = deferred_call::DeferredCall::next_pending() {
                    match self.interrupt_service.service_deferred_call(task) {
                        true => {}
                        false => panic!("unhandled deferred call task"),
                    }
                } else if let Some(interrupt) = cortexm4::nvic::next_pending() {
                    match self.interrupt_service.service_interrupt(interrupt) {
                        true => {}
                        false => panic!("unhandled interrupt"),
                    }
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
        unsafe { cortexm4::nvic::has_pending() || deferred_call::has_tasks() }
    }

    fn mpu(&self) -> &cortexm4::mpu::MPU {
        &self.mpu
    }

    fn scheduler_timer(&self) -> &Self::SchedulerTimer {
        &self.scheduler_timer
    }

    fn watchdog(&self) -> &Self::WatchDog {
        &()
    }

    fn userspace_kernel_boundary(&self) -> &cortexm4::syscall::SysCall {
        &self.userspace_kernel_boundary
    }

    fn sleep(&self) {
        if pm::deep_sleep_ready() {
            unsafe {
                cortexm4::scb::set_sleepdeep();
            }
        } else {
            unsafe {
                cortexm4::scb::unset_sleepdeep();
            }
        }

        unsafe {
            cortexm4::support::wfi();
        }
    }

    unsafe fn atomic<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        cortexm4::support::atomic(f)
    }

    unsafe fn print_state(&self, writer: &mut dyn Write) {
        cortexm4::print_cortexm4_state(writer);
    }
}
