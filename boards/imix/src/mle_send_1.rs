//! `mle_send_1.rs`: Test Sending the first message of the MLE sequence
//! ...
//! // Radio initialization code
//! ...
//! let mle_test = mle_send_1::initialize_all(radio_mac as &'static Mac,
//!                                                          mux_alarm as &'static
//!                                                             MuxAlarm<'static,
//!                                                                 sam4l::ast::Ast>);
//! ...
//! // Imix initialization
//! ...
//! mle_test.start(); // Assumes flashing the Imix that is transmitting the first msg

use capsules;
extern crate sam4l;
use capsules::ieee802154::mac;
use capsules::ieee802154::mac::Mac;
use capsules::net::ieee802154::MacAddress;
use capsules::net::ip::{IP6Header, IPAddr, ip6_nh};
use capsules::net::lowpan;
use capsules::net::lowpan::{ContextStore, Context};
use capsules::net::lowpan_fragment::{FragState, TxState, TransmitClient, ReceiveClient};
use capsules::net::util;
use capsules::net::udp::{send_udp_packet, udp_ipv6_prepare_packet, TF, SAC, DAC}; //Temp change for udp testing
use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use core::cell::Cell;

use core::mem;
use kernel::ReturnCode;

use kernel::hil::radio;
use kernel::hil::time;
use kernel::hil::time::Frequency;

pub struct DummyStore {
    context0: Context,
}

impl DummyStore {
    pub fn new(context0: Context) -> DummyStore {
        DummyStore { context0: context0 }
    }
}

impl ContextStore for DummyStore {
    fn get_context_from_addr(&self, ip_addr: IPAddr) -> Option<Context> {
        if util::matches_prefix(&ip_addr.0, &self.context0.prefix, self.context0.prefix_len) {
            Some(self.context0)
        } else {
            None
        }
    }

    fn get_context_from_id(&self, ctx_id: u8) -> Option<Context> {
        if ctx_id == 0 {
            Some(self.context0)
        } else {
            None
        }
    }

    fn get_context_from_prefix(&self, prefix: &[u8], prefix_len: u8) -> Option<Context> {
        if prefix_len == self.context0.prefix_len &&
           util::matches_prefix(prefix, &self.context0.prefix, prefix_len) {
            Some(self.context0)
        } else {
            None
        }
    }
}

pub const MLP: [u8; 8] = [0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7];
pub const SRC_ADDR: IPAddr = IPAddr([0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
                                     0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f]);
pub const DST_ADDR: IPAddr = IPAddr([0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29,
                                     0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f]);
pub const SRC_MAC_ADDR: MacAddress = MacAddress::Long([0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16,
                                                       0x17]);
pub const DST_MAC_ADDR: MacAddress = MacAddress::Long([0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
                                                       0x1f]);

pub const IP6_HDR_SIZE: usize = 40;
pub const PAYLOAD_LEN: usize = 42;
pub static mut RF233_BUF: [u8; radio::MAX_BUF_SIZE] = [0 as u8; radio::MAX_BUF_SIZE];

/* 6LoWPAN Constants */
const DEFAULT_CTX_PREFIX_LEN: usize = 8;
static DEFAULT_CTX_PREFIX: [u8; 16] = [0x0 as u8; 16];
static mut RX_STATE_BUF: [u8; 1280] = [0x0; 1280];
static mut RADIO_BUF_TMP: [u8; radio::MAX_BUF_SIZE] = [0x0; radio::MAX_BUF_SIZE];

//Note: Replaced TF, SAC, DAC Enums with import of these enums from the udp capsule

pub struct LowpanTest<'a, A: time::Alarm + 'a> {
    radio: &'a mac::Mac<'a>,
    alarm: &'a A,
    frag_state: &'a FragState<'a, A>,
    tx_state: &'a TxState<'a>,
    test_counter: Cell<usize>,
}

pub unsafe fn initialize_all(radio_mac: &'static Mac,
                      mux_alarm: &'static MuxAlarm<'static, sam4l::ast::Ast>)
        -> &'static LowpanTest<'static,
        capsules::virtual_alarm::VirtualMuxAlarm<'static, sam4l::ast::Ast<'static>>> {
    let dummy_ctx_store = static_init!(DummyStore,
                                       DummyStore::new(capsules::net::lowpan::Context {
                                           prefix: DEFAULT_CTX_PREFIX,
                                           prefix_len: DEFAULT_CTX_PREFIX_LEN as u8,
                                           id: 0,
                                           compress: false,
                                       }));

    let default_tx_state = static_init!(
        capsules::net::lowpan_fragment::TxState<'static>,
        capsules::net::lowpan_fragment::TxState::new()
        );

    let default_rx_state = static_init!(
        capsules::net::lowpan_fragment::RxState<'static>,
        capsules::net::lowpan_fragment::RxState::new(&mut RX_STATE_BUF)
        );

    let frag_state_alarm = static_init!(
        VirtualMuxAlarm<'static, sam4l::ast::Ast>,
        VirtualMuxAlarm::new(mux_alarm)
        );

    let frag_dummy_alarm = static_init!(
        VirtualMuxAlarm<'static, sam4l::ast::Ast>,
        VirtualMuxAlarm::new(mux_alarm)
        );

    let frag_state = static_init!(
        capsules::net::lowpan_fragment::FragState<'static,
        VirtualMuxAlarm<'static, sam4l::ast::Ast>>,
        capsules::net::lowpan_fragment::FragState::new(
            radio_mac,
            dummy_ctx_store as &'static capsules::net::lowpan::ContextStore,
            &mut RADIO_BUF_TMP,
            frag_state_alarm)
        );

    frag_state.add_rx_state(default_rx_state);
    radio_mac.set_transmit_client(frag_state);
    radio_mac.set_receive_client(frag_state);

    let lowpan_frag_test = static_init!(
        LowpanTest<'static,
        VirtualMuxAlarm<'static, sam4l::ast::Ast>>,
        LowpanTest::new(radio_mac as &'static Mac,
                        frag_state,
                        default_tx_state,
                        frag_dummy_alarm)
    );

    frag_state.set_receive_client(lowpan_frag_test);
    default_tx_state.set_transmit_client(lowpan_frag_test);
    frag_state_alarm.set_client(frag_state);
    frag_dummy_alarm.set_client(lowpan_frag_test);
    frag_state.schedule_next_timer();

    lowpan_frag_test
}

impl<'a, A: time::Alarm + 'a> LowpanTest<'a, A> {
    pub fn new(radio: &'a mac::Mac<'a>,
               frag_state: &'a FragState<'a, A>,
               tx_state: &'a TxState<'a>,
               alarm: &'a A)
               -> LowpanTest<'a, A> {
        LowpanTest {
            radio: radio,
            alarm: alarm,
            frag_state: frag_state,
            tx_state: tx_state,
            test_counter: Cell::new(0),
        }
    }

    pub fn start(&self) {
        let delta = A::Frequency::frequency() * 10;
        let next = self.alarm.now().wrapping_add(delta);
        self.alarm.set_alarm(next); 
    }

    fn num_tests(&self) -> usize {
        1
    }

    fn run_test(&self) {
        debug!("Sending First MLE Message:");
        //Send single IP packet
        //ipv6_prepare_packet(TF::Inline, 255, SAC::Inline, DAC::Inline);
//Begin Temp Changes for UDP testing

        //Set UDP Payload within IP6_DGRAM
        {
            //Only edit UDP portion of the payload (begins 8 bytes after IPV6 Header)
            let payload = unsafe { &mut IP6_DGRAM[(IP6_HDR_SIZE+8)..] };
            for i in 0..(PAYLOAD_LEN - 8) {
                payload[i] = 100 as u8;
            }
        }
   

        unsafe {
            udp_ipv6_prepare_packet(TF::Inline, 255, SAC::Inline, DAC::Inline, &mut IP6_DGRAM, IP6_DGRAM.len(), IP6_HDR_SIZE, SRC_ADDR, DST_ADDR, SRC_MAC_ADDR, DST_MAC_ADDR, MLP, 19788, 19788); 
        }
        unsafe {
            self.send_ipv6_packet(&MLP, SRC_MAC_ADDR, DST_MAC_ADDR);
        }

    }

    unsafe fn send_ipv6_packet(&self,
                               _: &[u8],
                               src_mac_addr: MacAddress,
                               dst_mac_addr: MacAddress) {
        let frag_state = self.frag_state;
        let tx_state = self.tx_state;
        
        //frag_state.radio.config_set_pan(0xABCD);
/*
        let ret_code = frag_state.transmit_packet(src_mac_addr,
                                                  dst_mac_addr,
                                                  &mut IP6_DGRAM,
                                                  IP6_DGRAM.len(),
                                                  None,
                                                  tx_state,
                                                  true,
                                                  true);
        debug!("Ret code: {:?}", ret_code);
*/
        send_udp_packet(frag_state, tx_state, src_mac_addr, dst_mac_addr, &mut IP6_DGRAM, IP6_DGRAM.len()
          //, TF::Inline, 255, SAC::Inline, DAC::Inline, SRC_ADDR, DST_ADDR, MLP
          );
    }
}

impl<'a, A: time::Alarm + 'a> time::Client for LowpanTest<'a, A> {
    fn fired(&self) {
        self.run_test();
    }
}

impl<'a, A: time::Alarm + 'a> TransmitClient for LowpanTest<'a, A> {
    fn send_done(&self, _: &'static mut [u8], _: &TxState, _: bool, _: ReturnCode) {
        debug!("Send completed");
        //self.schedule_next(); //Now, code pauses after single transmit bc no more alarms fired.
    }
}

impl<'a, A: time::Alarm + 'a> ReceiveClient for LowpanTest<'a, A> {
    fn receive<'b>(&self, buf: &'b [u8], len: u16, retcode: ReturnCode) {
        debug!("Receive completed: {:?}", retcode);
        let test_num = self.test_counter.get();
        self.test_counter.set((test_num + 1) % self.num_tests());
        ipv6_prepare_packet(TF::Inline, 255, SAC::Inline, DAC::Inline); 
//Modified to just print first character of a received packet
	debug!("Received Packet: first char: {}", buf[0]);
	
   }
}

static mut IP6_DGRAM: [u8; IP6_HDR_SIZE + PAYLOAD_LEN] = [0; IP6_HDR_SIZE + PAYLOAD_LEN];


fn ipv6_prepare_packet(tf: TF, hop_limit: u8, sac: SAC, dac: DAC) {
    {
        let payload = unsafe { &mut IP6_DGRAM[IP6_HDR_SIZE..] };
        for i in 0..PAYLOAD_LEN {
            payload[i] = i as u8;
        }
        //payload = ;
    }
    {
        let ip6_header: &mut IP6Header = unsafe { mem::transmute(IP6_DGRAM.as_mut_ptr()) };
        *ip6_header = IP6Header::new();
        ip6_header.set_payload_len(PAYLOAD_LEN as u16);

        if tf != TF::TrafficFlow {
            ip6_header.set_ecn(0b01);
        }
        if (tf as u8) & (TF::Traffic as u8) != 0 {
            ip6_header.set_dscp(0b000000);
        } else {
            ip6_header.set_dscp(0b101010);
        }

        if (tf as u8) & (TF::Flow as u8) != 0 {
            ip6_header.set_flow_label(0);
        } else {
            ip6_header.set_flow_label(0xABCDE);
        }

        ip6_header.set_next_header(ip6_nh::NO_NEXT);

        ip6_header.set_hop_limit(hop_limit);

        match sac {
            SAC::Inline => {
                ip6_header.src_addr = SRC_ADDR;
            }
            SAC::LLP64 => {
                // LLP::xxxx:xxxx:xxxx:xxxx
                ip6_header.src_addr.set_unicast_link_local();
                ip6_header.src_addr.0[8..16].copy_from_slice(&SRC_ADDR.0[8..16]);
            }
            SAC::LLP16 => {
                // LLP::ff:fe00:xxxx
                ip6_header.src_addr.set_unicast_link_local();
                // Distinct from compute_iid because the U/L bit is not flipped
                ip6_header.src_addr.0[11] = 0xff;
                ip6_header.src_addr.0[12] = 0xfe;
                ip6_header.src_addr.0[14..16].copy_from_slice(&SRC_ADDR.0[14..16]);
            }
            SAC::LLPIID => {
                // LLP::IID
                ip6_header.src_addr.set_unicast_link_local();
                ip6_header.src_addr.0[8..16].copy_from_slice(&lowpan::compute_iid(&SRC_MAC_ADDR));
            }
            SAC::Unspecified => {}
            SAC::Ctx64 => {
                // MLP::xxxx:xxxx:xxxx:xxxx
                ip6_header.src_addr.set_prefix(&MLP, 64);
                ip6_header.src_addr.0[8..16].copy_from_slice(&SRC_ADDR.0[8..16]);
            }
            SAC::Ctx16 => {
                // MLP::ff:fe00:xxxx
                ip6_header.src_addr.set_prefix(&MLP, 64);
                // Distinct from compute_iid because the U/L bit is not flipped
                ip6_header.src_addr.0[11] = 0xff;
                ip6_header.src_addr.0[12] = 0xfe;
                ip6_header.src_addr.0[14..16].copy_from_slice(&SRC_ADDR.0[14..16]);
            }
            SAC::CtxIID => {
                // MLP::IID
                ip6_header.src_addr.set_prefix(&MLP, 64);
                ip6_header.src_addr.0[8..16].copy_from_slice(&lowpan::compute_iid(&SRC_MAC_ADDR));
            }
        }

        match dac {
            DAC::Inline => {
                ip6_header.dst_addr = DST_ADDR;
            }
            DAC::LLP64 => {
                // LLP::xxxx:xxxx:xxxx:xxxx
                ip6_header.dst_addr.set_unicast_link_local();
                ip6_header.dst_addr.0[8..16].copy_from_slice(&DST_ADDR.0[8..16]);
            }
            DAC::LLP16 => {
                // LLP::ff:fe00:xxxx
                ip6_header.dst_addr.set_unicast_link_local();
                // Distinct from compute_iid because the U/L bit is not flipped
                ip6_header.dst_addr.0[11] = 0xff;
                ip6_header.dst_addr.0[12] = 0xfe;
                ip6_header.dst_addr.0[14..16].copy_from_slice(&SRC_ADDR.0[14..16]);
            }
            DAC::LLPIID => {
                // LLP::IID
                ip6_header.dst_addr.set_unicast_link_local();
                ip6_header.dst_addr.0[8..16].copy_from_slice(&lowpan::compute_iid(&DST_MAC_ADDR));
            }
            DAC::Ctx64 => {
                // MLP::xxxx:xxxx:xxxx:xxxx
                ip6_header.dst_addr.set_prefix(&MLP, 64);
                ip6_header.dst_addr.0[8..16].copy_from_slice(&SRC_ADDR.0[8..16]);
            }
            DAC::Ctx16 => {
                // MLP::ff:fe00:xxxx
                ip6_header.dst_addr.set_prefix(&MLP, 64);
                // Distinct from compute_iid because the U/L bit is not flipped
                ip6_header.dst_addr.0[11] = 0xff;
                ip6_header.dst_addr.0[12] = 0xfe;
                ip6_header.dst_addr.0[14..16].copy_from_slice(&SRC_ADDR.0[14..16]);
            }
            DAC::CtxIID => {
                // MLP::IID
                ip6_header.dst_addr.set_prefix(&MLP, 64);
                ip6_header.dst_addr.0[8..16].copy_from_slice(&lowpan::compute_iid(&DST_MAC_ADDR));
            }
            DAC::McastInline => {
                // first byte is ff, that's all we know
                ip6_header.dst_addr = DST_ADDR;
                ip6_header.dst_addr.0[0] = 0xff;
            }
            DAC::Mcast48 => {
                // ffXX::00XX:XXXX:XXXX
                ip6_header.dst_addr.0[0] = 0xff;
                ip6_header.dst_addr.0[1] = DST_ADDR.0[1];
                ip6_header.dst_addr.0[11..16].copy_from_slice(&DST_ADDR.0[11..16]);
            }
            DAC::Mcast32 => {
                // ffXX::00XX:XXXX
                ip6_header.dst_addr.0[0] = 0xff;
                ip6_header.dst_addr.0[1] = DST_ADDR.0[1];
                ip6_header.dst_addr.0[13..16].copy_from_slice(&DST_ADDR.0[13..16]);
            }
            DAC::Mcast8 => {
                // ff02::00XX
                ip6_header.dst_addr.0[0] = 0xff;
                ip6_header.dst_addr.0[1] = DST_ADDR.0[1];
                ip6_header.dst_addr.0[15] = DST_ADDR.0[15];
            }
            DAC::McastCtx => {
                // ffXX:XX + plen + pfx64 + XXXX:XXXX
                ip6_header.dst_addr.0[0] = 0xff;
                ip6_header.dst_addr.0[1] = DST_ADDR.0[1];
                ip6_header.dst_addr.0[2] = DST_ADDR.0[2];
                ip6_header.dst_addr.0[3] = 64 as u8;
                ip6_header.dst_addr.0[4..12].copy_from_slice(&MLP);
                ip6_header.dst_addr.0[12..16].copy_from_slice(&DST_ADDR.0[12..16]);
            }
        }
    }
    debug!("Packet with tf={:?} hl={} sac={:?} dac={:?}",
           tf,
           hop_limit,
           sac,
           dac);
}
