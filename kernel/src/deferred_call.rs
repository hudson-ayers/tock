//! Deferred call mechanism.
//!
//! This is a tool to allow chip peripherals to schedule "interrupts"
//! in the chip scheduler when hardware doesn't support interrupts where
//! they are needed, or to allow capsules to schedule "interrupts" in the
//! same way.

use crate::utilities::cells::OptionalCell;
use core::cell::Cell;
use core::convert::Into;
use core::convert::TryFrom;
use core::convert::TryInto;
use core::marker::Copy;
use core::marker::PhantomData;

pub trait DeferredCallMapper<T: Into<usize> + TryFrom<usize> + Copy> {
    fn handle_deferred_call(&self, task: T) -> bool;
}

/// Any chip with peripherals which require deferred calls should
/// instantiate exactly one of these, and a reference to that manager should be
/// passed to all created `DeferredCall`s.
pub struct DeferredCallManager<
    T: Into<usize> + TryFrom<usize> + Copy,
    M: DeferredCallMapper<T> + 'static,
> {
    v: Cell<usize>,
    _p: PhantomData<T>,
    mapping: OptionalCell<&'static M>,
}

impl<T: Into<usize> + TryFrom<usize> + Copy, M: DeferredCallMapper<T> + 'static>
    DeferredCallManager<T, M>
{
    pub fn new() -> Self {
        Self {
            v: Cell::new(0),
            _p: PhantomData,
            mapping: OptionalCell::empty(),
        }
    }

    /// Are there any pending `DeferredCall`s?
    pub fn has_tasks(&self) -> bool {
        self.v.get() != 0
    }

    /// Gets and clears the next pending `DeferredCall`
    pub fn next_pending(&self) -> Option<T> {
        let val = self.v.get();
        if val == 0 {
            None
        } else {
            let bit = val.trailing_zeros() as usize;
            let new_val = val & !(1 << bit);
            self.v.set(new_val);
            bit.try_into().ok()
        }
    }

    pub fn set_mapping(&self, mapping: &'static M) {
        self.mapping.set(mapping)
    }

    pub fn service_deferred_call(&self, task: T) -> bool {
        self.mapping.map_or(false, |m| m.handle_deferred_call(task))
    }
}

/// Represents a way to generate an asynchronous call without a hardware
/// interrupt. Supports up to 32 possible deferrable tasks.
pub struct DeferredCall<
    PT: PeripheralTask,
    CT: CapsuleTask,
    M: DeferredCallMapper<DeferredCallTask<PT, CT>> + 'static,
> {
    task: DeferredCallTask<PT, CT>,
    mgr: &'static DeferredCallManager<DeferredCallTask<PT, CT>, M>,
}

impl<
        PT: PeripheralTask,
        CT: CapsuleTask,
        M: DeferredCallMapper<DeferredCallTask<PT, CT>> + 'static,
    > DeferredCall<PT, CT, M>
{
    /// Creates a new DeferredCall
    ///
    /// Only create one per task, preferably in the module that it will be used
    /// in. Creating more than 32 tasks on a given manager will lead to
    /// incorrect behavior.
    pub const fn new(
        task: DeferredCallTask<PT, CT>,
        mgr: &'static DeferredCallManager<DeferredCallTask<PT, CT>, M>,
    ) -> Self {
        Self { task, mgr }
    }

    /// Set the `DeferredCall` as pending
    pub fn set(&self) {
        self.mgr.v.set(
            (1 << <DeferredCallTask<PT, CT> as Into<usize>>::into(self.task) as usize)
                | self.mgr.v.get(),
        );
    }
}

pub unsafe trait PeripheralTask: TryFrom<usize> + Into<usize> + Copy + 'static {}
pub trait CapsuleTask: TryFrom<usize> + Into<usize> + Copy + 'static {}

#[derive(Copy, Clone)]
pub enum DeferredCallTask<PT: PeripheralTask, CT: CapsuleTask> {
    Peripheral(PT),
    Capsule(CT),
}

impl<PT: PeripheralTask, CT: CapsuleTask> TryFrom<usize> for DeferredCallTask<PT, CT> {
    type Error = ();

    fn try_from(value: usize) -> Result<Self, ()> {
        if let Ok(v) = <PT as TryFrom<usize>>::try_from(value) {
            return Ok(Self::Peripheral(v));
        } else if let Ok(v) = <CT as TryFrom<usize>>::try_from(value) {
            return Ok(Self::Capsule(v));
        }
        Err(())
    }
}

impl<PT: PeripheralTask, CT: CapsuleTask> Into<usize> for DeferredCallTask<PT, CT> {
    fn into(self) -> usize {
        match self {
            Self::Peripheral(t) => <PT as Into<usize>>::into(t),
            Self::Capsule(t) => <CT as Into<usize>>::into(t),
        }
    }
}
