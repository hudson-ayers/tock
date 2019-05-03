//! Component to test in kernel udp
//!

// Author: Hudson Ayers <hayers@stanford.edu>

#![allow(dead_code)] // Components are intended to be conditionally included

use capsules::ieee802154::device::MacDevice;
use capsules::net::ieee802154::MacAddress;
use capsules::net::ipv6::ip_utils::IPAddr;
use capsules::net::ipv6::ipv6::{IP6Packet, IPPayload, TransportHeader};
use capsules::net::ipv6::ipv6_recv::IP6Receiver;
use capsules::net::ipv6::ipv6_send::IP6Sender;
use capsules::net::sixlowpan::{sixlowpan_compression, sixlowpan_state};
use capsules::net::udp::udp::UDPHeader;
use capsules::net::udp::udp_recv::UDPReceiver;
use capsules::net::udp::udp_send::{UDPSendStruct, UDPSender, MuxUdpSender};
use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};

use kernel::capabilities;
use kernel::udp_port_table::{UdpPortTable};

use kernel::component::Component;
use kernel::create_capability;
use kernel::hil::radio;
use kernel::static_init;

const PAYLOAD_LEN: usize = 200;

// The UDP stack requires several packet buffers:
//
//   1. RF233_BUF: buffer the IP6_Sender uses to pass frames to the radio after fragmentation
//   2. SIXLOWPAN_RX_BUF: Buffer to hold full IP packets after they are decompressed by 6LoWPAN
//   3. UDP_DGRAM: The payload of the IP6_Packet, which holds full IP Packets before they are tx'd

const UDP_HDR_SIZE: usize = 8;

static mut RF233_BUF: [u8; radio::MAX_BUF_SIZE] = [0x00; radio::MAX_BUF_SIZE];
static mut SIXLOWPAN_RX_BUF: [u8; 1280] = [0x00; 1280];
static mut UDP_DGRAM: [u8; PAYLOAD_LEN - UDP_HDR_SIZE] = [0; PAYLOAD_LEN - UDP_HDR_SIZE];


pub struct MockUDPComponent2 {
    mux_mac: &'static capsules::ieee802154::virtual_mac::MuxMac<'static>,
    ctx_pfix_len: u8,
    ctx_pfix: [u8; 16],
    // TODO: consider putting bound_port_table in a TakeCell
    bound_port_table: &'static UdpPortTable,
    dst_mac_addr: MacAddress,
    src_mac_addr: MacAddress,
    interface_list: &'static [IPAddr],
    alarm_mux: &'static MuxAlarm<'static, sam4l::ast::Ast<'static>>,
}



impl MockUDPComponent2 {
    pub fn new(
        mux_mac: &'static capsules::ieee802154::virtual_mac::MuxMac<'static>,
        ctx_pfix_len: u8,
        ctx_pfix: [u8; 16],
        dst_mac_addr: MacAddress,
        src_mac_addr: MacAddress,
        interface_list: &'static [IPAddr],
        alarm: &'static MuxAlarm<'static, sam4l::ast::Ast<'static>>,
    ) -> MockUDPComponent2 {
        MockUDPComponent2 {
            mux_mac: mux_mac,
            ctx_pfix_len: ctx_pfix_len,
            ctx_pfix: ctx_pfix,
            bound_port_table: unsafe {static_init!(UdpPortTable, UdpPortTable::new())},
            dst_mac_addr: dst_mac_addr,
            src_mac_addr: src_mac_addr,
            interface_list: interface_list,
            alarm_mux: alarm,
        }
    }
}

impl Component for MockUDPComponent2 {
    type Output = &'static capsules::mock_udp2::MockUdp2<'static,
        VirtualMuxAlarm<'static, sam4l::ast::Ast<'static>>>;

    unsafe fn finalize(&mut self) -> Self::Output {
        let ipsender_virtual_alarm = static_init!(
            VirtualMuxAlarm<'static, sam4l::ast::Ast>,
            VirtualMuxAlarm::new(self.alarm_mux)
        );

        let udp_mac = static_init!(
            capsules::ieee802154::virtual_mac::MacUser<'static>,
            capsules::ieee802154::virtual_mac::MacUser::new(self.mux_mac)
        );
        self.mux_mac.add_user(udp_mac);

        let sixlowpan = static_init!(
            sixlowpan_state::Sixlowpan<
                'static,
                sam4l::ast::Ast<'static>,
                sixlowpan_compression::Context,
            >,
            sixlowpan_state::Sixlowpan::new(
                sixlowpan_compression::Context {
                    prefix: self.ctx_pfix,
                    prefix_len: self.ctx_pfix_len,
                    id: 0,
                    compress: false,
                },
                &sam4l::ast::AST
            )
        );

        let sixlowpan_state = sixlowpan as &sixlowpan_state::SixlowpanState;
        let sixlowpan_tx = sixlowpan_state::TxState::new(sixlowpan_state);
        /*
        let default_rx_state = static_init!(
            sixlowpan_state::RxState<'static>,
            sixlowpan_state::RxState::new(&mut SIXLOWPAN_RX_BUF)
        );
        sixlowpan_state.add_rx_state(default_rx_state);
        udp_mac.set_receive_client(sixlowpan);
        */

        let tr_hdr = TransportHeader::UDP(UDPHeader::new());
        let ip_pyld: IPPayload = IPPayload {
            header: tr_hdr,
            payload: &mut UDP_DGRAM,
        };
        let ip6_dg = static_init!(IP6Packet<'static>, IP6Packet::new(ip_pyld));

        let ip_send = static_init!(
            capsules::net::ipv6::ipv6_send::IP6SendStruct<
                'static,
                VirtualMuxAlarm<'static, sam4l::ast::Ast>,
            >,
            capsules::net::ipv6::ipv6_send::IP6SendStruct::new(
                ip6_dg,
                ipsender_virtual_alarm,
                &mut RF233_BUF,
                sixlowpan_tx,
                udp_mac,
                self.dst_mac_addr,
                self.src_mac_addr
            )
        );

        // Set src IP of the sender to be the address configured via the sam4l.
        // Userland apps can change this if they so choose.
        ip_send.set_addr(self.interface_list[2]);
        udp_mac.set_transmit_client(ip_send);

        // TODO: probably eventually change 'udp_mux' to 'udp_send_mux'
        let udp_mux = static_init!(
            MuxUdpSender<
                    'static,
                    capsules::net::ipv6::ipv6_send::IP6SendStruct<
                    'static,
                    VirtualMuxAlarm<'static, sam4l::ast::Ast<'static>>,
                >,
            >,
            MuxUdpSender::new(ip_send)
        );

        let udp_send = static_init!(
            UDPSendStruct<
                'static,
                capsules::net::ipv6::ipv6_send::IP6SendStruct<
                    'static,
                    VirtualMuxAlarm<'static, sam4l::ast::Ast<'static>>,
                >,
            >,
            UDPSendStruct::new(udp_mux)
        );

        /*
        let ip_receive = static_init!(
            capsules::net::ipv6::ipv6_recv::IP6RecvStruct<'static>,
            capsules::net::ipv6::ipv6_recv::IP6RecvStruct::new()
        );
        sixlowpan_state.set_rx_client(ip_receive);
        // TODO: use seperate bound_port_table for receiver? Is this necessary?
        let udp_recv = static_init!(UDPReceiver<'static>, UDPReceiver::new());
        ip_receive.set_client(udp_recv);
        */

        let mock_udp = static_init!(
            capsules::mock_udp2::MockUdp2<'static, VirtualMuxAlarm<'static, sam4l::ast::Ast>>,
            capsules::mock_udp2::MockUdp2::new(
                5,
                VirtualMuxAlarm::new(self.alarm_mux),
                udp_send,
                self.bound_port_table,
            )
        );
        ip_send.set_client(udp_mux);
        udp_send.set_client(mock_udp);
        mock_udp.alarm.set_client(mock_udp);
        ipsender_virtual_alarm.set_client(ip_send);
        //udp_recv.set_client(mock_udp);
        mock_udp
    }
}
