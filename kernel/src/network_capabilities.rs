use core::cell::Cell;
use core::marker::PhantomData;

const MAX_ADDR_SET_SIZE: usize = 16;
const MAX_PORT_SET_SIZE: usize = 16;
const MAX_NUM_CAPAB: usize = 16;
const MAX_NUM_CAPSULES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AddrRange { // TODO: change u32 to IPAddr type (inclusion weirdness?)
    Any, // Any address
    NoAddrs,
    AddrSet([u32; MAX_ADDR_SET_SIZE]),
    Range(u32, u32),
    Addr(u32),
}

impl AddrRange {
    pub fn is_addr_valid(&self, addr: u32) -> bool {
        match self {
            AddrRange::Any => true,
            AddrRange::NoAddrs => false,
            AddrRange::AddrSet(allowed_addrs) =>
                allowed_addrs.iter().any(|&a| a == addr),
            AddrRange::Range(low, high) => (*low <= addr && addr <= *high),
            AddrRange::Addr(allowed_addr) => addr == *allowed_addr,
        }
    }
}

// An opaque descriptor that allows the holder to send to a set of IP Addresses
#[derive(Clone, Copy, PartialEq)]
pub struct IpCapability {
    remote_addrs: AddrRange, //Not visible outside this module!
}

impl IpCapability {
    pub unsafe fn new(remote_addrs: AddrRange) -> IpCapability {
        IpCapability {
            remote_addrs: remote_addrs,
        }
    }
    pub fn get_range(&self) -> AddrRange {
        self.remote_addrs
    }

    pub fn remote_addr_valid(&self, remote_addr: u32) -> bool {
        self.remote_addrs.is_addr_valid(remote_addr)
    }

}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PortRange {
    Any,
    NoPorts,
    PortSet([u16; MAX_PORT_SET_SIZE]),
    Range(u16, u16),
    Port(u16),
}

#[derive(Clone, Copy, PartialEq)]
pub struct UdpCapability {
    remote_ports: PortRange, // dst
    local_ports: PortRange, // src
}

impl PortRange {
    pub fn is_port_valid(&self, port: u16) -> bool {
        match self {
            PortRange::Any => true,
            PortRange::NoPorts => false,
            PortRange::PortSet(allowed_ports) =>
                allowed_ports.iter().any(|&p| p == port), // TODO: check refs
            PortRange::Range(low, high) => (*low <= port && port <= *high),
            PortRange::Port(allowed_port) => port == *allowed_port,
        }
    }
}

impl UdpCapability {
    pub unsafe fn new(remote_ports: PortRange, local_ports: PortRange) -> UdpCapability {
        UdpCapability {
            remote_ports: remote_ports,
            local_ports: local_ports,
        }
    }

    pub fn get_remote_ports(&self) -> PortRange {
        self.remote_ports
    }

    pub fn get_local_ports(&self) -> PortRange {
        self.local_ports
    }

    pub fn remote_port_valid(&self, remote_port: u16) -> bool {
        self.remote_ports.is_port_valid(remote_port)
    }

    pub fn local_port_valid(&self, local_port: u16) -> bool {
        self.local_ports.is_port_valid(local_port)
    }
}

// Modes allow us to enforce layer separation here.
#[derive(Clone, Copy, PartialEq)] // TODO: remove copy eventually
pub struct UdpMode;
#[derive(Clone, Copy, PartialEq)] // TODO: remove copy eventually
pub struct IpMode;
#[derive(Clone, Copy, PartialEq)] // TODO: remove copy eventually
pub struct NeutralMode;
// Make the structs below implement an unsafe trait to make them only
// constructable in trusted code.
pub unsafe trait UdpVisCap {}

pub unsafe trait IpVisCap {}
// TODO: remove copy eventually!!!!
#[derive(PartialEq)]
pub struct NetworkCapability<M> {
    // can potentially add more
    udp_cap: UdpCapability,
    ip_cap: IpCapability,
    _mode: PhantomData<M>
}

impl NetworkCapability<NeutralMode> {
    pub unsafe fn new(udp_cap: UdpCapability, ip_cap: IpCapability)
        -> NetworkCapability<NeutralMode> {
            NetworkCapability {
                udp_cap: udp_cap,
                ip_cap: ip_cap,
                _mode: PhantomData,
            }
    }
}

impl<M> NetworkCapability<M> {
    pub fn into_udp_cap(self, udp_visibility_cap: &UdpVisCap)
        -> NetworkCapability<UdpMode> { // Call with map?
            NetworkCapability {
                udp_cap: self.udp_cap,
                ip_cap: self.ip_cap,
                _mode: PhantomData,
            }
    }
    
    pub fn into_ip_cap(self, ip_visibility_cap: &IpVisCap)
        -> NetworkCapability<IpMode> {
            NetworkCapability {
                udp_cap: self.udp_cap,
                ip_cap: self.ip_cap,
                _mode: PhantomData,
            }
    }
    
    // convert back into neutral -- maybe this should always be called when
    // switching layers. We could also have functions that check the capabilities
    // always switch things back into neutral mode to ensure that clients
    // always have to re-prove possession of the relevant visibility capability.
    pub fn into_neutral_mode(self) -> NetworkCapability<NeutralMode> {
            NetworkCapability {
                udp_cap: self.udp_cap,
                ip_cap: self.ip_cap,
                _mode: PhantomData,
            }
    }
}

impl NetworkCapability<UdpMode> {
    pub fn get_remote_ports(&self) -> PortRange {
        self.udp_cap.get_remote_ports()
    }

    pub fn get_local_ports(&self) -> PortRange {
        self.udp_cap.get_local_ports()
    }
}

impl NetworkCapability<IpMode> {
    pub fn get_range(&self) -> AddrRange {
        self.ip_cap.get_range()
    }
}