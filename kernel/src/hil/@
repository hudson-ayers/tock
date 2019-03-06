//! COMP driver, nRF5X-family
//!
//! COMP compares an input voltage (VIN+) against a second input voltage (VIN-)
//!
//! Authors
//! ----------------
//! * Colleen Dai <colleend21@gmail.com>
//! * Date: January 31, 2019
//! boards/src/main.rs

use kernel::common::cells::OptionalCell;
use kernel::common::registers::{register_bitfields, ReadOnly, WriteOnly, ReadWrite};
use kernel::common::StaticRef;
use kernel::hil::comp::{self, Pin, return_pin_num, return_pin_enum, RefPin, return_ref_pin_num, return_ref_pin_enum, OpModes, return_op_mode};
use kernel::ReturnCode;

const COMP_BASE: StaticRef<CompRegisters> = 
    unsafe { StaticRef::new(0x40013000 as *const CompRegisters) };

#[repr(C)]
// Routine to print register mem addreses
pub struct CompRegisters {
    /// Task starting the comparator
    /// Address: 0x000 - 0x004
    pub task_start: WriteOnly<u32, Task::Register>,
    /// Task stopping the comparator
    /// Address: 0x004 - 0x008
    pub task_stop: WriteOnly<u32, Task::Register>,
    /// Task that samples comparator value
    /// Address: 0x008 - 0x01C
    pub task_sample: ReadOnly<u32, Task::Register>,
    /// Reserved
    pub _reserved1: [u32; 61],
    /// Event generated when comparator is ready and output is valid
    /// Address: 0x100 - 0x104
    pub event_ready: ReadWrite<u32, Event::Register>,
    /// Event generated during downward crossing
    /// Address: 0x104 - 0x108
    pub event_down: ReadWrite<u32, Event::Register>,
    /// Event generated during upward crossing
    /// Address: 0x108 - 0x10C
    pub event_up: ReadWrite<u32, Event::Register>,
    /// Event generated during an upward or downward crossing
    /// Address: 0x108 - 0x10C
    pub event_cross: ReadWrite<u32, Event::Register>,
    /// Reserved
    pub _reserved2: [u32; 60],
    /// Shortcut register
    /// Address: 0x200 - 0x204
    pub shorts: ReadWrite<u32, Shorts::Register>,
    /// Reserved
    pub _reserved3: [u32; 63],
    /// Enable or disable interrupt
    /// Address: 0x300 - 0x304
    pub inten: ReadWrite<u32, Interrupt::Register>,
    /// Enable interrupt
    /// Address: 0x304 - 0x308
    pub intenset: ReadWrite<u32, Interrupt::Register>,
    /// Disable interrupt
    /// Address: 0x308 - 0x30C
    pub intenclr: ReadWrite<u32, Interrupt::Register>,
    /// Reserved
    pub _reserved4: [u32; 61],
    /// Compare result
    /// Address: 0x400 - 0x404
    pub result: ReadOnly<u32, Res::Register>,
    pub _reserved5: [u32; 63],
    /// enable COMP
    /// Address: 0x500 - 0x504
    pub enable: ReadWrite<u32, Enable::Register>,
    /// pin select
    /// Address: 0x504 - 0x508
    pub psel: ReadWrite<u32, Psel::Register>,
    /// reference select
    /// Address: 0x508-0x50C
    pub refsel: ReadWrite<u32, Refsel::Register>,
    /// External reference select
    /// Address: 0x50C - 0x510
    pub extrefsel: ReadWrite<u32, Extrefsel::Register>,
    pub _reserved7: [u32; 5],
    /// Threshold configuration for hysteresis unit
    /// Address: 0x530 - 0x534
    pub th: ReadWrite<u32, Th::Register>,
    /// Mode configuration
    /// Address: 0x534 - 0x538
    pub mode: ReadWrite<u32, Mode::Register>,
    /// Enable comparator hysteresis
    /// Address: 0x538 - 0x53C
    pub hyst: ReadWrite<u32, Hyst::Register>,
}

register_bitfields! [u32,
    /// Start task
    Task [
        ENABLE OFFSET(0) NUMBITS(1)
    ],

    /// Ready event
    Event [
        READY OFFSET(0) NUMBITS(1)
    ],

    /// Shortcut register
    Shorts [
        /// Shortcut between READY event and SAMPLE task
        READY_SAMPLE OFFSET(0) NUMBITS(1),
        READY_STOP OFFSET(1) NUMBITS(1),
        DOWN_STOP OFFSET(2) NUMBITS(1),
        UP_STOP OFFSET(3) NUMBITS(1),
        CROSS_STOP OFFSET(4) NUMBITS(1)
    ],

    /// Enable or disable interrupt
    Interrupt [
        READY OFFSET(0) NUMBITS(1),
        DOWN OFFSET(1) NUMBITS(1),
        UP OFFSET(2) NUMBITS(1),
        CROSS OFFSET(3) NUMBITS(1)
    ],

    /// Result of last compare
    Res [
        RESULT OFFSET(0) NUMBITS(1)
    ],

    /// Enable or disable comp
    Enable [
        ENABLE OFFSET(0) NUMBITS(1)
    ],

    /// Pin select
    Psel [
        PSEL OFFSET(0) NUMBITS(3)
    ],

    /// Reference select
    Refsel [
        REFSEL OFFSET(0) NUMBITS(3)
    ],

    /// External reference select
    Extrefsel [
        EXTREFSEL OFFSET(0) NUMBITS(3)
    ],

    /// Threshold configuration
    Th [
        THDOWN OFFSET(0) NUMBITS(6),
        THUP OFFSET(1) NUMBITS(6)
    ],

    /// Mode configuration
    Mode [
        SP OFFSET(0) NUMBITS(2),
        MAIN OFFSET(1) NUMBITS(1)
    ],

    /// Enable comparator hysteresis
    Hyst [
        HYST OFFSET(0) NUMBITS(1)
    ]
];

pub struct Comp<'a> {
    registers: StaticRef<CompRegisters>,
    client: OptionalCell<&'a comp::Client<Pin, RefPin>>,
}

pub static mut COMP: Comp = Comp::new();

impl Comp<'a> {
    pub const fn new() -> Comp<'a> {
        Comp {
            registers: COMP_BASE, 
            client: OptionalCell::empty(),
        }
    }

    pub fn handle_interrupt(&self) {
        // Check which client generated the interrupt, callback to client accordingly: only
        // handling the case where temp goes up
        let regs = &*self.registers;
        if regs.intenset.is_set(Interrupt::UP) {
            // Disable interrupts for UP
            regs.intenset.write(Interrupt::UP::SET);

            // If V of input pin > V of reference pin, throw an interrupt to the client
            if regs.result.is_set(Res::RESULT) {
                self.client.map(|client| {
                    let input = regs.psel.read(Psel::PSEL);
                    let reference = regs.extrefsel.read(Extrefsel::EXTREFSEL);
                    client.event(regs.result.is_set(Res::RESULT), return_pin_enum(input), return_ref_pin_enum(reference));// call event, pass actual stuff
                });
            }
        }
    }
}

impl comp::AnalogComparator<'a, Pin, RefPin> for Comp<'a> {
    fn set_input(&self, input_pin: Pin) {
        let regs = &*self.registers;
        regs.psel.write(Psel::PSEL.val(return_pin_num(input_pin)));
    }

    fn set_reference(&self, ref_pin: RefPin) {
        let regs = &*self.registers;
        if regs.mode.is_set(Mode::MAIN) {
            regs.extrefsel.write(Extrefsel::EXTREFSEL.val(return_ref_pin_num(ref_pin)));
        }
        else {
            regs.refsel.write(Refsel::REFSEL.val(return_ref_pin_num(ref_pin)));
        }
    }

    fn set_both(&self, input_pin:Pin, ref_pin:RefPin) {
        let regs = &*self.registers;
        self.set_input(input_pin);
        self.set_reference(ref_pin);
    }

    // rising or not needs to be passed in
    // kernel::RETURNCODE if wrong pins
    fn start_comparing(&self, input_pin: Pin, ref_pin: RefPin, rising: bool, mode: OpModes) {
        let regs = &*self.registers;
        regs.mode.write(Mode::MAIN.val(return_op_mode(mode)));
        self.set_both(input_pin, ref_pin);
        regs.enable.write(Enable::ENABLE::SET);
        regs.task_start.write(Task::ENABLE::SET);
    }

    fn set_client(&self, client: &'a comp::Client<Pin, RefPin>) {
        self.client.set(client);
    }

    // RETURNCODE
    fn stop(&self) {
        let regs = &*self.registers;
        regs.enable.write(Enable::ENABLE::CLEAR);
        regs.task_stop.write(Task::ENABLE::SET);
    }
}
