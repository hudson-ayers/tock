//! These structs define  for network related 
// use capsules::net::udp;
// use capsules::net::ipv6;

use crate::capabilities::{IpCapabilityAccess, UdpCapabilityAccess,
    UnencryptedDataCapability};

const MAX_ADDR_SET_SIZE: usize = 16;
const MAX_PORT_SET_SIZE: usize = 16;
const MAX_DATA_SEGMENT_SIZE: usize = 1024;
const KEY_BYTES: usize = 32;

#[derive(Clone, Copy)]
pub enum AddrRange {
    // TODO: provide netmask option?
    Any, // Any address
    NoAddrs, // Is this one necessary?
    AddrSet([u32; MAX_ADDR_SET_SIZE]),
    Range(u32, u32),
    Addr(u32),
}

#[derive(Clone, Copy)]
pub enum PortRange {
    Any,
    NoPorts,
    PortSet([u16; MAX_PORT_SET_SIZE]),
    Range(u16, u16),
    Port(u16),
}

pub struct IpCapability {
    remote_addrs: AddrRange, // local vs. remote
    //recv_addrs: AddrRange, // AddrRange is for remote
}

pub struct UdpCapability {
    remote_ports: PortRange,
    local_ports: PortRange,
}

impl IpCapability {
    pub unsafe fn new(remote_addrs: AddrRange, cap: &IpCapabilityAccess)
        -> IpCapability {
        IpCapability {
            remote_addrs: remote_addrs
        }
    }

    pub fn get_range(&self, cap: &IpCapabilityAccess) -> AddrRange {
        self.remote_addrs
    }
}

// TODO: more considerations here for layer separation?
impl UdpCapability {
    pub unsafe fn new(remote_ports: PortRange, local_ports: PortRange,
        cap: &UdpCapabilityAccess) -> UdpCapability {
        UdpCapability {
            remote_ports: remote_ports,
            local_ports: local_ports,
        }
    }

    pub fn get_remote_ports(&self, cap: &UdpCapabilityAccess)
        -> PortRange {
        self.remote_ports
    }

    pub fn get_local_ports(&self, cap: &UdpCapabilityAccess)
        -> PortRange {
        self.local_ports
    }
}

// Empty definition to have it compile
pub unsafe trait Encryptor {}


pub enum EncryptionMode<'a> {
    Unencrypted(&'a UnencryptedDataCapability),
    Encrypted(&'a Encryptor)
}

// Needed to allow us to alias the same EncryptionMode object for the
// unencrypted data variant.
unsafe impl<'a> Sync for EncryptionMode<'a> {}