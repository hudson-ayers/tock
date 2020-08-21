use core::fmt::Write;
use core::panic::PanicInfo;

use cortexm4;
use kernel::debug;
use kernel::debug::IoWrite;
use kernel::hil::led;
use kernel::hil::uart::{self, Configure};
use nrf52840::gpio::Pin;

use crate::CHIP;
use crate::PROCESSES;

struct Writer {
    initialized: bool,
}

static mut WRITER: Writer = Writer { initialized: false };

impl Write for Writer {
    fn write_str(&mut self, s: &str) -> ::core::fmt::Result {
        self.write(s.as_bytes());
        Ok(())
    }
}

impl IoWrite for Writer {
    fn write(&mut self, buf: &[u8]) {
        let uart = unsafe { &mut nrf52840::uart::UARTE0 };
        if !self.initialized {
            self.initialized = true;
            uart.configure(uart::Parameters {
                baud_rate: 115200,
                stop_bits: uart::StopBits::One,
                parity: uart::Parity::None,
                hw_flow_control: false,
                width: uart::Width::Eight,
            });
        }
        for &c in buf {
            unsafe {
                uart.send_byte(c);
            }
<<<<<<< HEAD
            while !uart.tx_ready() {}
=======
        }

        // copy_from_slice() requires equal length slices
        // This will truncate any writes longer than BUF_LEN, but simplifies the
        // code. In practice, BUF_LEN=512 always seems sufficient for the size of
        // individual calls to write made by the panic handler.
        let mut max = BUF_LEN;
        if buf.len() < BUF_LEN {
            max = buf.len();
        }

        unsafe {
            // If CDC_REF_FOR_PANIC is not yet set we panicked very early,
            // and not much we can do. Don't want to double fault,
            // so just return.
            super::CDC_REF_FOR_PANIC.map(|cdc| {
                // Lots of unsafe dereferencing of global static mut objects here.
                // However, this should be okay, because it all happens within
                // a single thread, and:
                // - This is the only place the global CDC_REF_FOR_PANIC is used, the logic is the same
                //   as applies for the global CHIP variable used in the panic handler.
                // - We do create multiple mutable references to the STATIC_PANIC_BUF, but we never
                //   access the STATIC_PANIC_BUF after a slice of it is passed to transmit_buffer
                //   until the slice has been returned in the uart callback.
                // - Similarly, only this function uses the global DUMMY variable, and we do not
                //   mutate it.
                let usb = &mut cdc.controller();
                STATIC_PANIC_BUF[..max].copy_from_slice(&buf[..max]);
                let static_buf = &mut STATIC_PANIC_BUF;
                cdc.set_transmit_client(&DUMMY);
                cdc.transmit_buffer(static_buf, max);
                loop {
                    if let Some(interrupt) = cortexm4::nvic::next_pending() {
                        if interrupt == 39 {
                            usb.handle_interrupt();
                        }
                        let n = cortexm4::nvic::Nvic::new(interrupt);
                        n.clear_pending();
                        n.enable();
                    }
                    if DUMMY.fired.get() == true {
                        // buffer finished transmitting, return so we can output additional
                        // messages when requested by the panic handler.
                        break;
                    }
                }
                DUMMY.fired.set(false);
            });
>>>>>>> a98e7d3af... new peripheral instantiation model for nrf52840
        }
    }
}

/// Default panic handler for the Nano 33 Board.
///
/// We just use the standard default provided by the debug module in the kernel.
#[cfg(not(test))]
#[no_mangle]
#[panic_handler]
pub unsafe extern "C" fn panic_fmt(pi: &PanicInfo) -> ! {
    const LED_KERNEL_PIN: Pin = Pin::P0_13;
    let led = &mut led::LedLow::new(&mut nrf52840::gpio::PORT[LED_KERNEL_PIN]);
    let writer = &mut WRITER;
    debug::panic(
        &mut [led],
        writer,
        pi,
        &cortexm4::support::nop,
        &PROCESSES,
        &CHIP,
    )
}
