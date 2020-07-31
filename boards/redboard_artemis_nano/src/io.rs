use core::fmt::Write;
use core::panic::PanicInfo;

use crate::CHIP;
use crate::PROCESSES;
use apollo3;
use kernel::debug;
use kernel::debug::IoWrite;
use kernel::hil::led;

/// Writer is used by kernel::debug to panic message to the serial port.
pub struct Writer {
    initialized: bool,
}

/// Global static for debug writer
pub static mut WRITER: Writer = Writer { initialized: false };

impl Writer {
    /// Indicate that USART has already been initialized.
    pub fn set_initialized(&mut self) {
        self.initialized = true;
    }
}

impl Write for Writer {
    fn write_str(&mut self, s: &str) -> ::core::fmt::Result {
        self.write(s.as_bytes());
        Ok(())
    }
}

impl IoWrite for Writer {
    fn write(&mut self, buf: &[u8]) {
        // Okay, this probably doesn't work, maybe we need a global for the uart?
        // Or maybe it does idk
        unsafe {
            let uart = apollo3::uart::Uart::new(apollo3::uart::UART0_BASE);
            uart.transmit_sync(buf);
        }
    }
}

/// Panic handler.
#[no_mangle]
#[panic_handler]
pub unsafe extern "C" fn panic_fmt(info: &PanicInfo) -> ! {
    // just create a new pin reference here instead of using global
    let led_pin =
        &mut apollo3::gpio::GpioPin::new(apollo3::gpio::GPIO_BASE, apollo3::gpio::Pin::Pin19);
    let led = &mut led::LedLow::new(led_pin);
    let writer = &mut WRITER;

    debug::panic(
        &mut [led],
        writer,
        info,
        &cortexm4::support::nop,
        &PROCESSES,
        &CHIP,
    )
}
