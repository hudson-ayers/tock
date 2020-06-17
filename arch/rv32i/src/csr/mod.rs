//! Tock Register interface for using CSR registers.

use riscv_csr::csr::ReadWriteRiscvCsr;

pub mod mcause;
pub mod mcycle;
pub mod mepc;
pub mod mie;
pub mod minstret;
pub mod mip;
pub mod mscratch;
pub mod mstatus;
pub mod mtval;
pub mod mtvec;
pub mod pmpaddr;
pub mod pmpconfig;
pub mod stvec;
pub mod utvec;

#[repr(C)]
pub struct CSR {
    pub minstreth: ReadWriteRiscvCsr<u32, minstret::minstreth::Register, 0xB82>,
    pub minstret: ReadWriteRiscvCsr<u32, minstret::minstret::Register, 0xB02>,
    pub mcycleh: ReadWriteRiscvCsr<u32, mcycle::mcycleh::Register, 0xB80>,
    pub mcycle: ReadWriteRiscvCsr<u32, mcycle::mcycle::Register, 0xB00>,
    pub pmpcfg0: ReadWriteRiscvCsr<u32, pmpconfig::pmpcfg::Register, 0x304>,
    pub pmpcfg1: ReadWriteRiscvCsr<u32, pmpconfig::pmpcfg::Register, 0x305>,
    pub pmpcfg2: ReadWriteRiscvCsr<u32, pmpconfig::pmpcfg::Register, 0x300>,
    pub pmpcfg3: ReadWriteRiscvCsr<u32, pmpconfig::pmpcfg::Register, 0x005>,
    pub pmpaddr0: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x105>,
    pub pmpaddr1: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x340>,
    pub pmpaddr2: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x341>,
    pub pmpaddr3: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x342>,
    pub pmpaddr4: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x343>,
    pub pmpaddr5: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x344>,
    pub pmpaddr6: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x3A0>,
    pub pmpaddr7: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x3A1>,
    pub pmpaddr8: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x3A2>,
    pub pmpaddr9: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x3A3>,
    pub pmpaddr10: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x3B0>,
    pub pmpaddr11: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x3B1>,
    pub pmpaddr12: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x3B2>,
    pub pmpaddr13: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x3B3>,
    pub pmpaddr14: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x3B4>,
    pub pmpaddr15: ReadWriteRiscvCsr<u32, pmpaddr::pmpaddr::Register, 0x3B5>,
    pub mie: ReadWriteRiscvCsr<u32, mie::mie::Register, 0x3B6>,
    pub mscratch: ReadWriteRiscvCsr<u32, mscratch::mscratch::Register, 0x3B7>,
    pub mepc: ReadWriteRiscvCsr<u32, mepc::mepc::Register, 0x3B8>,
    pub mcause: ReadWriteRiscvCsr<u32, mcause::mcause::Register, 0x3B9>,
    pub mtval: ReadWriteRiscvCsr<u32, mtval::mtval::Register, 0x3BA>,
    pub mip: ReadWriteRiscvCsr<u32, mip::mip::Register, 0x3BB>,
    pub mtvec: ReadWriteRiscvCsr<u32, mtvec::mtvec::Register, 0x3BC>,
    pub stvec: ReadWriteRiscvCsr<u32, stvec::stvec::Register, 0x3BD>,
    pub utvec: ReadWriteRiscvCsr<u32, utvec::utvec::Register, 0x3BE>,
    pub mstatus: ReadWriteRiscvCsr<u32, mstatus::mstatus::Register, 0x3BF>,
}

// Define the "addresses" of each CSR register.
pub const CSR: &CSR = &CSR {
    minstreth: ReadWriteRiscvCsr::new(0xB82),
    minstret: ReadWriteRiscvCsr::new(0xB02),
    mcycleh: ReadWriteRiscvCsr::new(0xB80),
    mcycle: ReadWriteRiscvCsr::new(0xB00),
    mie: ReadWriteRiscvCsr::new(0x304),
    mtvec: ReadWriteRiscvCsr::new(0x305),
    mstatus: ReadWriteRiscvCsr::new(0x300),
    utvec: ReadWriteRiscvCsr::new(0x005),
    stvec: ReadWriteRiscvCsr::new(0x105),
    mscratch: ReadWriteRiscvCsr::new(0x340),
    mepc: ReadWriteRiscvCsr::new(0x341),
    mcause: ReadWriteRiscvCsr::new(0x342),
    mtval: ReadWriteRiscvCsr::new(0x343),
    mip: ReadWriteRiscvCsr::new(0x344),
    pmpcfg0: ReadWriteRiscvCsr::new(0x3A0),
    pmpcfg1: ReadWriteRiscvCsr::new(0x3A1),
    pmpcfg2: ReadWriteRiscvCsr::new(0x3A2),
    pmpcfg3: ReadWriteRiscvCsr::new(0x3A3),
    pmpaddr0: ReadWriteRiscvCsr::new(0x3B0),
    pmpaddr1: ReadWriteRiscvCsr::new(0x3B1),
    pmpaddr2: ReadWriteRiscvCsr::new(0x3B2),
    pmpaddr3: ReadWriteRiscvCsr::new(0x3B3),
    pmpaddr4: ReadWriteRiscvCsr::new(0x3B4),
    pmpaddr5: ReadWriteRiscvCsr::new(0x3B5),
    pmpaddr6: ReadWriteRiscvCsr::new(0x3B6),
    pmpaddr7: ReadWriteRiscvCsr::new(0x3B7),
    pmpaddr8: ReadWriteRiscvCsr::new(0x3B8),
    pmpaddr9: ReadWriteRiscvCsr::new(0x3B9),
    pmpaddr10: ReadWriteRiscvCsr::new(0x3BA),
    pmpaddr11: ReadWriteRiscvCsr::new(0x3BB),
    pmpaddr12: ReadWriteRiscvCsr::new(0x3BC),
    pmpaddr13: ReadWriteRiscvCsr::new(0x3BD),
    pmpaddr14: ReadWriteRiscvCsr::new(0x3BE),
    pmpaddr15: ReadWriteRiscvCsr::new(0x3BF),
};

impl CSR {
    // resets the cycle counter to 0
    pub fn reset_cycle_counter(&self) {
        CSR.mcycleh.write(mcycle::mcycleh::mcycleh.val(0));
        CSR.mcycle.write(mcycle::mcycle::mcycle.val(0));
    }

    // reads the cycle counter
    pub fn read_cycle_counter(&self) -> u64 {
        let top = CSR.mcycleh.read(mcycle::mcycleh::mcycleh);
        let bot = CSR.mcycle.read(mcycle::mcycle::mcycle);

        u64::from(top).checked_shl(32).unwrap() + u64::from(bot)
    }
}
