//! Interface system tick timer.

use crate::hil::time::{self, Frequency};

/// Interface for the system tick timer.
///
/// A system tick timer provides a countdown timer to enforce process scheduling
/// quantums.  Implementations should have consistent timing while the CPU is
/// active, but need not operate during sleep.
///
/// On most chips, this will be implemented by the core (e.g. the ARM core), but
/// some chips lack this optional peripheral, in which case it might be
/// implemented by another timer or alarm controller.
pub trait SysTick {
    /// Sets the timer as close as possible to the given interval in
    /// microseconds, and starts counting down. Interrupts are not enabled.
    ///
    /// Callers can assume at least a 24-bit wide clock. Specific timing is
    /// dependent on the driving clock. In practice, increments of 10ms are most
    /// accurate and values up to 400ms are valid.
    fn start_timer(&self, us: u32);

    /// Returns if there is at least `us` microseconds left
    fn greater_than(&self, us: u32) -> bool;

    /// Returns true if the timer has expired since the last time this or set_timer()
    /// was called. If called a second time without an intermittent call to set_timer(),
    /// the return value is unspecified (implementations can return whatever they like)
    fn overflowed(&self) -> bool;

    /// Resets the timer
    ///
    /// Resets the timer to 0 and disables it
    fn reset(&self);

    /// Enable or disable interrupts
    ///
    fn config_interrupts(&self, enabled: bool);
}

/// A dummy `SysTick` implementation in which the timer never expires.
///
/// Using this implementation is functional, but will mean the scheduler cannot
/// interrupt non-yielding processes.
impl SysTick for () {
    fn reset(&self) {}

    fn start_timer(&self, _: u32) {}

    fn config_interrupts(&self, _: bool) {}

    fn overflowed(&self) -> bool {
        false
    }

    fn greater_than(&self, _: u32) -> bool {
        true
    }
}

pub struct VirtualSystick<A: 'static + time::Alarm<'static>> {
    alarm: &'static A,
}

impl<A: 'static + time::Alarm<'static>> VirtualSystick<A> {
    pub fn new(alarm: &'static A) -> Self {
        Self { alarm }
    }
}

// Difference between virtual systick and a normal virtual alarm
// is that systick requires ability to track passed time while selectively
// enabling and disabling only interrupts

impl<A: 'static + time::Alarm<'static>> SysTick for VirtualSystick<A> {
    fn reset(&self) {
        // This doesn't techinically set the timer to 0, but I a not convinced it matters
        self.alarm.disable(); //seems pretty inefficient
    }

    fn start_timer(&self, us: u32) {
        let tics = {
            // We need to convert from microseconds to native tics, which could overflow in 32-bit
            // arithmetic. So we convert to 64-bit. 64-bit division is an expensive subroutine, but
            // if `us` is a power of 10 the compiler will simplify it with the 1_000_000 divisor
            // instead.
            let us = us as u64;
            let hertz = A::Frequency::frequency() as u64;

            (hertz * us / 1_000_000) as u32
        };
        let fire_at = self.alarm.now().wrapping_add(tics);
        self.alarm.set_alarm(fire_at);
        self.alarm.disable(); //interrupts are off, but value is saved
    }

    fn config_interrupts(&self, enabled: bool) {
        if enabled {
            self.alarm.enable();
        } else {
            self.alarm.disable();
        }
    }

    fn overflowed(&self) -> bool {
        self.alarm.now() > self.alarm.get_alarm()
    }

    fn greater_than(&self, us: u32) -> bool {
        let tics = {
            // We need to convert from microseconds to native tics, which could overflow in 32-bit
            // arithmetic. So we convert to 64-bit. 64-bit division is an expensive subroutine, but
            // if `us` is a power of 10 the compiler will simplify it with the 1_000_000 divisor
            // instead.
            let us = us as u64;
            let hertz = A::Frequency::frequency() as u64;

            (hertz * us / 1_000_000) as u32
        };
        self.alarm.now() + tics < self.alarm.get_alarm()
    }
}

// No need to handle the interrupt, or even register as a client! The entire purpose
// of the interrupt is to cause a transition to userspace, which
// already happens for any mtimer interrupt, and the overflow check is sufficient
// to determine that it was an mtimer interrupt.
