use core::cell::Cell;
use core::slice;

pub unsafe trait ReadableDMABuffer {
    fn lock(&self) -> Option<(*const u8, usize)>;
    unsafe fn unlock(&self) -> bool;
    fn map(&self, fun: &mut dyn FnMut(&[u8])) -> bool;
    fn locked(&self) -> bool;
}

unsafe impl ReadableDMABuffer for () {
    fn lock(&self) -> Option<(*const u8, usize)> {
        None
    }

    unsafe fn unlock(&self) -> bool {
        false
    }

    fn map(&self, _fun: &mut dyn FnMut(&[u8])) -> bool {
        false
    }

    fn locked(&self) -> bool {
        false
    }
}

pub struct ImmutableDMABuffer {
    buffer: Cell<Option<&'static [u8]>>,
    locked: Cell<bool>,
}

impl ImmutableDMABuffer {
    pub const fn new() -> Self {
        ImmutableDMABuffer {
            buffer: Cell::new(None),
            locked: Cell::new(false),
        }
    }

    pub fn set(&self, buf: &'static [u8]) -> bool {
        if self.locked.get() {
            false
        } else {
            self.buffer.set(Some(buf));
            true
        }
    }

    pub fn get(&self) -> Option<&'static [u8]> {
        self.buffer.get()
    }
}

unsafe impl ReadableDMABuffer for ImmutableDMABuffer {
    fn lock(&self) -> Option<(*const u8, usize)> {
        if self.locked.get() {
            // Don't allow double-locking, to prevent this type from being used
            // by two peripheral drivers at the same time (we aren't
            // implementing a semaphore-like lock counter to disambiguate
            // multiple usages and tracking corresponding unlock calls
            // appropriately).
            None
        } else if self.buffer.get().is_none() {
            // There is nothing to lock
            None
        } else {
            self.locked.set(true);
            self.buffer.get().map(|buf| (buf.as_ptr(), buf.len()))
        }
    }

    unsafe fn unlock(&self) -> bool {
        // To provide an API consistent with that of the MutableDMABuffer,
        // prevent it from being unlocked when the buffer is currently
        // `map`ped. While this would be memory safe, there really isn't any
        // good reason to allow exchanging the underlying buffer when something
        // is accessing it through the `ReadableDMABuffer` trait.
        if self.buffer.get().is_none() {
            false
        } else {
            self.locked.replace(false)
        }
    }

    fn map(&self, fun: &mut dyn FnMut(&[u8])) -> bool {
        // Check whether the buffer is locked first. `map` is only supposed to
        // work when the buffer is locked. Then take the buffer to prevent it
        // from being `map`ped multiple times. This makes it consistent with the
        // behavior of `MutableDMABuffer`.
        if self.locked.get() {
            if let Some(buffer) = self.buffer.replace(None) {
                fun(buffer);
                self.buffer.replace(Some(buffer));
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    fn locked(&self) -> bool {
        self.locked.get()
    }
}

pub struct MutableDMABuffer {
    buffer: Cell<Option<(*mut u8, usize)>>,
    locked: Cell<bool>,
}

impl MutableDMABuffer {
    pub const fn new() -> Self {
        MutableDMABuffer {
            buffer: Cell::new(None),
            locked: Cell::new(false),
        }
    }

    pub fn set(&self, buf: &'static mut [u8]) -> bool {
        if self.locked.get() {
            false
        } else {
            self.buffer.set(Some((buf.as_mut_ptr(), buf.len())));
            true
        }
    }

    pub fn take(&self) -> Option<&'static mut [u8]> {
        if self.locked.get() {
            None
        } else {
            self.locked.set(false);
            self.buffer
                .replace(None)
                .map(|(ptr, len)| unsafe { slice::from_raw_parts_mut(ptr, len) })
        }
    }
}

unsafe impl ReadableDMABuffer for MutableDMABuffer {
    fn lock(&self) -> Option<(*const u8, usize)> {
        if self.locked.get() {
            // Don't allow double-locking, to prevent this type from being used
            // by two peripheral drivers at the same time.
            None
        } else if self.buffer.get().is_none() {
            // There is nothing to lock
            None
        } else {
            self.locked.set(true);
            self.buffer.get().map(|(ptr, len)| (ptr as *const u8, len))
        }
    }

    unsafe fn unlock(&self) -> bool {
        // To ensure we don't have any mutable aliasing, prevent the buffer from
        // being unlocked when it is currently `map`ped.
        if self.buffer.get().is_none() {
            false
        } else {
            self.locked.replace(false)
        }
    }

    fn map(&self, fun: &mut dyn FnMut(&[u8])) -> bool {
        // Check whether the buffer is locked first. `map` must only work when
        // the buffer is locked. Then take the buffer to prevent it from being
        // `map`ped multiple times.
        if self.locked.get() {
            if let Some((ptr, len)) = self.buffer.replace(None) {
                fun(unsafe { slice::from_raw_parts(ptr, len) });
                self.buffer.replace(Some((ptr, len)));
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    fn locked(&self) -> bool {
        self.locked.get()
    }
}
