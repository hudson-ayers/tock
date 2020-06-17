//! RISC-V CSR Library
//!
//! Uses the Tock Register Interface to control RISC-V CSRs.

#![feature(llvm_asm)]
#![feature(const_fn)]
#![feature(const_generics)]
#![no_std]

pub mod csr;
