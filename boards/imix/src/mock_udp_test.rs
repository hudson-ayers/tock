//! `mock_udp_test.rs`: Test kernel space sending of
//! UDP Packets with the UdpPortTable.
use super::components::mock_udp::MockUDPComponent;
use capsules::ieee802154::device::MacDevice;
use capsules::mock_udp1::MockUdp1;
use capsules::mock_udp2::MockUdp2;
use super::components::mock_udp2::{MockUDPComponent2};
use capsules::net::ieee802154::MacAddress;
use capsules::net::ipv6::ip_utils::{ip6_nh, IPAddr};
use capsules::net::ipv6::ipv6::{IP6Header, IP6Packet, IPPayload, TransportHeader};
use capsules::net::ipv6::ipv6_send::{IP6SendStruct, IP6Sender};
use capsules::net::sixlowpan::sixlowpan_compression;
use capsules::net::sixlowpan::sixlowpan_state::{Sixlowpan, SixlowpanState, TxState};
use capsules::net::udp::udp::UDPHeader;
use capsules::net::udp::udp_send::{UDPSendStruct, UDPSender, MuxUdpSender};
use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use core::cell::Cell;
use kernel::component::Component;
use kernel::debug;
use kernel::hil::radio;
use kernel::hil::time;
use kernel::hil::time::Frequency;
use kernel::static_init;
use kernel::udp_port_table::{UdpPortTable, UdpPortSocket};
use kernel::ReturnCode;
use kernel::capabilities;
use kernel::net_permissions::EncryptionMode;

const DST_MAC_ADDR: MacAddress = MacAddress::Short(49138);
const DEFAULT_CTX_PREFIX_LEN: u8 = 8; //Length of context for 6LoWPAN compression
const DEFAULT_CTX_PREFIX: [u8; 16] = [0x0 as u8; 16]; //Context for 6LoWPAN Compression

pub struct MockUdpTest<'a, A: time::Alarm> {
    alarm: A,
    test_counter: Cell<usize>,
    // TODO: change the bottom two to 'a references
    mock1: &'a MockUdp1<'a, A>,
    mock2: &'a MockUdp2<'a, A>,
    port_table: &'static UdpPortTable,
}

// TODO: git add this file
// based on udp_lowpan_test
pub unsafe fn initialize_all(
    mux_mac: &'static capsules::ieee802154::virtual_mac::MuxMac<'static>,
    mux_alarm: &'static MuxAlarm<'static, sam4l::ast::Ast>,
    port_table: &'static UdpPortTable) -> &'static MockUdpTest<'static,
        VirtualMuxAlarm<'static, sam4l::ast::Ast<'static>>> {
    let serial_num: sam4l::serial_num::SerialNum = sam4l::serial_num::SerialNum::new();
    let serial_num_bottom_16 = (serial_num.get_lower_64() & 0x0000_0000_0000_ffff) as u16;
    let src_mac_from_serial_num: MacAddress = MacAddress::Short(serial_num_bottom_16);
    let local_ip_ifaces = static_init!(
        [IPAddr; 3],
        [
            IPAddr([
                0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
                0x0e, 0x0f,
            ]),
            IPAddr([
                0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
                0x1e, 0x1f,
            ]),
            IPAddr::generate_from_mac(src_mac_from_serial_num),
        ]
    );

    struct ucap;
    unsafe impl capabilities::UnencryptedDataCapability for ucap {}
    static unencr_mode: EncryptionMode = EncryptionMode::Unencrypted(&ucap);

    let mock_udp1 = MockUDPComponent::new(
        mux_mac,
        DEFAULT_CTX_PREFIX_LEN,
        DEFAULT_CTX_PREFIX,
        DST_MAC_ADDR,
        src_mac_from_serial_num,
        local_ip_ifaces,
        mux_alarm,
        &unencr_mode,
    ).finalize();

    let mock_udp2 = MockUDPComponent2::new(
        mux_mac,
        DEFAULT_CTX_PREFIX_LEN,
        DEFAULT_CTX_PREFIX,
        DST_MAC_ADDR,
        src_mac_from_serial_num,
        local_ip_ifaces,
        mux_alarm,
        &unencr_mode,
    ).finalize();
    let mock_udp_test_alarm = VirtualMuxAlarm::new(mux_alarm);
    let mock_udp_test = static_init!(
        MockUdpTest<'static,
            VirtualMuxAlarm<'static, sam4l::ast::Ast<'static>>
            >,
        MockUdpTest::new(
            mock_udp_test_alarm,
            mock_udp1,
            mock_udp2,
            port_table,
        )
    );


    // TODO: initialize stuff/set clients etc.
    mock_udp_test



}

impl<'a, A: time::Alarm> MockUdpTest<'a, A> {
    pub fn new(alarm: A, mock1: &'a MockUdp1<'a, A>, mock2: &'a MockUdp2<'a, A>,
        port_table: &'static UdpPortTable) -> MockUdpTest<'a, A> {
        MockUdpTest {
            alarm: alarm,
            test_counter: Cell::new(0),
            mock1: mock1,
            mock2: mock2,
            port_table: port_table,
        }
    }

    pub fn bind_to_same_port() {
        // TODO: fill this in.
    }

    pub fn bind_to_different_ports() {
        // TODO: fill this in.
    }

    pub fn bind_then_unbind() {
        // TODO: fill this in
    }

    // TODO: any more tests to add here?
}

// impl<'a, A: time::Alarm> time::Client for MockUdpTest<'a, A> {
//     fn fired(&self) {
//         //self.run_test_and_increment();
//     }
// }

