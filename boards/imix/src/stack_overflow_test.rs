//! To test that detection of kernel stack overflows is working properly
//!
//! To test, add the following line to the imix reset handler, after initialization complete:
//! ```
//!     stack_overflow_test::fib(100);
//! ```
//! You should see the following output:
//! ```
//!     Kernel panic at tock/arch/cortex-m4/src/lib.rs:337:
//!             "kernel stack overflow."
//!     ...
//! ```
//!

use kernel::debug;

#[inline(never)]
#[no_mangle]
pub fn fib(n: u32) -> u32 {
    let mut _use_stack = [0u8; 1024];
    _use_stack[1000] = 1;
    if _use_stack[n as usize] == 1 {
        debug!("hi");
    }
    if n < 2 {
        1
    } else {
        fib(n - 1) + fib(n - 2)
    }
}
