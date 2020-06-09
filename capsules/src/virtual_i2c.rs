//! Virtualize an I2C master bus.
//!
//! `MuxI2C` provides shared access to a single I2C Master Bus for multiple
//! users. `I2CDevice` provides access to a specific I2C address.

use core::cell::Cell;
use kernel::common::cells::{OptionalCell, TakeCell};
use kernel::common::dynamic_deferred_call::{
    DeferredCallHandle, DynamicDeferredCall, DynamicDeferredCallClient,
};
use kernel::common::{List, ListLink, ListNode};
use kernel::hil::i2c::{self, Error, I2CClient, I2CHwMasterClient};

pub struct MuxI2C<'a> {
    i2c: &'a dyn i2c::I2CMaster,
    smbus: Option<&'a dyn i2c::SMBusMaster>,
    i2c_devices: List<'a, I2CDevice<'a>>,
    enabled: Cell<usize>,
    i2c_inflight: OptionalCell<&'a I2CDevice<'a>>,
    deferred_caller: &'a DynamicDeferredCall,
    handle: OptionalCell<DeferredCallHandle>,
}

impl I2CHwMasterClient for MuxI2C<'_> {
    fn command_complete(&self, buffer: &'static mut [u8], error: Error) {
        if self.i2c_inflight.is_some() {
            self.i2c_inflight.take().map(move |device| {
                device.command_complete(buffer, error);
            });
        }
        self.do_next_op();
    }
}

impl<'a> MuxI2C<'a> {
    pub const fn new(
        i2c: &'a dyn i2c::I2CMaster,
        smbus: Option<&'a dyn i2c::SMBusMaster>,
        deferred_caller: &'a DynamicDeferredCall,
    ) -> MuxI2C<'a> {
        MuxI2C {
            i2c: i2c,
            smbus: smbus,
            i2c_devices: List::new(),
            enabled: Cell::new(0),
            i2c_inflight: OptionalCell::empty(),
            deferred_caller: deferred_caller,
            handle: OptionalCell::empty(),
        }
    }

    pub fn initialize_callback_handle(&self, handle: DeferredCallHandle) {
        self.handle.replace(handle);
    }

    fn enable(&self) {
        let enabled = self.enabled.get();
        self.enabled.set(enabled + 1);
        if enabled == 0 {
            self.i2c.enable();
        }
    }

    fn disable(&self) {
        let enabled = self.enabled.get();
        self.enabled.set(enabled - 1);
        if enabled == 1 {
            self.i2c.disable();
        }
    }

    fn do_next_op(&self) {
        if self.i2c_inflight.is_none() {
            // Nothing is currently in flight

            // Try to do the next I2C operation
            let mnode = self
                .i2c_devices
                .iter()
                .find(|node| node.operation.get() != Op::Idle);
            mnode.map(|node| {
                use kernel::hil::i2c::I2CDevice; //import here to avoid collision with trait name
                node.buffer.take().map(|buf| {
                    if !node.is_smbus() {
                        // This is an i2c operation
                        match node.operation.get() {
                            Op::Write(len) => self.i2c.write(node.addr, buf, len),
                            Op::Read(len) => self.i2c.read(node.addr, buf, len),
                            Op::WriteRead(wlen, rlen) => {
                                self.i2c.write_read(node.addr, buf, wlen, rlen)
                            }
                            Op::CommandComplete(err) => {
                                self.command_complete(buf, err);
                            }
                            Op::Idle => {} // Can't get here...
                        }
                    } else {
                        // this is an smbus operation
                        match node.operation.get() {
                            Op::Write(len) => {
                                match self.smbus.unwrap().smbus_write(node.addr, buf, len) {
                                    Ok(_) => {}
                                    Err(e) => {
                                        node.buffer.replace(e.1);
                                        node.operation.set(Op::CommandComplete(e.0));
                                        node.mux.do_next_op_async();
                                    }
                                };
                            }
                            Op::Read(len) => {
                                match self.smbus.unwrap().smbus_read(node.addr, buf, len) {
                                    Ok(_) => {}
                                    Err(e) => {
                                        node.buffer.replace(e.1);
                                        node.operation.set(Op::CommandComplete(e.0));
                                        node.mux.do_next_op_async();
                                    }
                                };
                            }
                            Op::WriteRead(wlen, rlen) => {
                                match self
                                    .smbus
                                    .unwrap()
                                    .smbus_write_read(node.addr, buf, wlen, rlen)
                                {
                                    Ok(_) => {}
                                    Err(e) => {
                                        node.buffer.replace(e.1);
                                        node.operation.set(Op::CommandComplete(e.0));
                                        node.mux.do_next_op_async();
                                    }
                                };
                            }
                            Op::CommandComplete(err) => {
                                self.command_complete(buf, err);
                            }
                            Op::Idle => unreachable!(),
                        }
                    }
                });
                node.operation.set(Op::Idle);
                self.i2c_inflight.set(node);
            });
        }
    }

    /// Asynchronously executes the next operation, if any. Used by calls
    /// to trigger do_next_op such that it will execute after the call
    /// returns. This is important in case the operation triggers an error,
    /// requiring a callback with an error condition; if the operation
    /// is executed synchronously, the callback may be reentrant (executed
    /// during the downcall). Please see
    ///
    /// https://github.com/tock/tock/issues/1496
    fn do_next_op_async(&self) {
        self.handle.map(|handle| self.deferred_caller.set(*handle));
    }
}

impl<'a> DynamicDeferredCallClient for MuxI2C<'a> {
    fn call(&self, _handle: DeferredCallHandle) {
        self.do_next_op();
    }
}

#[derive(Copy, Clone, PartialEq)]
enum Op {
    Idle,
    Write(u8),
    Read(u8),
    WriteRead(u8, u8),
    CommandComplete(i2c::Error),
}

pub struct I2CDevice<'a> {
    mux: &'a MuxI2C<'a>,
    addr: u8,
    enabled: Cell<bool>,
    buffer: TakeCell<'static, [u8]>,
    operation: Cell<Op>,
    next: ListLink<'a, I2CDevice<'a>>,
    client: OptionalCell<&'a dyn I2CClient>,
    is_smbus: bool,
}

impl<'a> I2CDevice<'a> {
    pub const fn new(mux: &'a MuxI2C<'a>, addr: u8, is_smbus: bool) -> I2CDevice<'a> {
        I2CDevice {
            mux: mux,
            addr: addr,
            enabled: Cell::new(false),
            buffer: TakeCell::empty(),
            operation: Cell::new(Op::Idle),
            next: ListLink::empty(),
            client: OptionalCell::empty(),
            is_smbus: is_smbus,
        }
    }

    pub fn set_client(&'a self, client: &'a dyn I2CClient) {
        self.mux.i2c_devices.push_head(self);
        self.client.set(client);
    }
}

impl I2CClient for I2CDevice<'_> {
    fn command_complete(&self, buffer: &'static mut [u8], error: Error) {
        self.client.map(move |client| {
            client.command_complete(buffer, error);
        });
    }
}

impl<'a> ListNode<'a, I2CDevice<'a>> for I2CDevice<'a> {
    fn next(&'a self) -> &'a ListLink<'a, I2CDevice<'a>> {
        &self.next
    }
}

impl i2c::I2CDevice for I2CDevice<'_> {
    fn is_smbus(&self) -> bool {
        self.is_smbus
    }

    fn enable(&self) {
        if !self.enabled.get() {
            self.enabled.set(true);
            self.mux.enable();
        }
    }

    fn disable(&self) {
        if self.enabled.get() {
            self.enabled.set(false);
            self.mux.disable();
        }
    }

    fn write_read(&self, data: &'static mut [u8], write_len: u8, read_len: u8) {
        self.buffer.replace(data);
        self.operation.set(Op::WriteRead(write_len, read_len));
        self.mux.do_next_op();
    }

    fn write(&self, data: &'static mut [u8], len: u8) {
        self.buffer.replace(data);
        self.operation.set(Op::Write(len));
        self.mux.do_next_op();
    }

    fn read(&self, buffer: &'static mut [u8], len: u8) {
        self.buffer.replace(buffer);
        self.operation.set(Op::Read(len));
        self.mux.do_next_op();
    }
}
