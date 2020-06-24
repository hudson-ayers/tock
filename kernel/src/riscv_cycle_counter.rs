// Move risc-v cycle counter code into the kernel so I can use it in sched.rs
//
use riscv_csr::csr::ReadWriteRiscvCsr;
use tock_registers::register_bitfields;

// myclce is the lower 32 bits of the number of elapsed cycles
register_bitfields![u32,
    pub(crate) mcycle [
        mcycle OFFSET(0) NUMBITS(32) []
    ]
];

// myclceh is the higher 32 bits of the number of elapsed cycles
register_bitfields![u32,
    pub(crate) mcycleh [
        mcycleh OFFSET(0) NUMBITS(32) []
    ]
];

#[repr(C)]
pub(crate) struct CSR {
    pub(crate) minstreth: ReadWriteRiscvCsr<u32>,
    pub(crate) minstret: ReadWriteRiscvCsr<u32>,
    pub(crate) mcycleh: ReadWriteRiscvCsr<u32, mcycleh::Register>,
    pub(crate) mcycle: ReadWriteRiscvCsr<u32, mcycle::Register>,
}

// Define the "addresses" of each CSR register.
pub(crate) const CSR: &CSR = &CSR {
    minstreth: ReadWriteRiscvCsr::new(riscv_csr::csr::MINSTRETH),
    minstret: ReadWriteRiscvCsr::new(riscv_csr::csr::MINSTRET),
    mcycleh: ReadWriteRiscvCsr::new(riscv_csr::csr::MCYCLEH),
    mcycle: ReadWriteRiscvCsr::new(riscv_csr::csr::MCYCLE),
};

impl CSR {
    // resets the cycle counter to 0
    pub(crate) fn reset_cycle_counter(&self) {
        CSR.mcycleh.write(mcycleh::mcycleh.val(0));
        CSR.mcycle.write(mcycle::mcycle.val(0));
    }

    // reads the cycle counter
    pub(crate) fn read_cycle_counter(&self) -> u64 {
        let top = CSR.mcycleh.read(mcycleh::mcycleh);
        let bot = CSR.mcycle.read(mcycle::mcycle);

        u64::from(top).checked_shl(32).unwrap() + u64::from(bot)
    }
    pub(crate) unsafe fn bench<F: FnOnce()>(&self, f: F) -> u64 {
        self.reset_cycle_counter();
        //f();
        self.read_cycle_counter()
    }
}
