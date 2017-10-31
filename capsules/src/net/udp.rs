//! Implements a basic UDP layer to exist between the application layer and
//! the 6lowpan layer. Eventually, this design will have to be modified once
//! an actual IP layer exists to handle multiplexing of packets by address
//! and protocol. Such a layer does not exist yet, and so for now this layer
//! will effectively serve to take in raw data, construct a UDP datagram
//! and construct an IP packet into which this UDP datagram will be
//! inserted. This layer also receives IP packets from the 6lowpan layer,
//! checks if the packet carries a UDP datagram, and passes the data 
//! contained in the UDP datagram up to the application layer if this is the 
//! case. This file also includes several private functions which are used 
//! internally to enable the public interface functions. Finally, this file
//! adds a structure which can be used to call on these UDP functions. This
//! is very minimal given that UDP is a stateless protocol, but helps to make
//! function calls cleaner by allowing one to simply pass a struct rather
//! than all of the variables individually.

//!  Author: Hudson Ayers, hayers@stanford.edu

use net::lowpan_fragment::{FragState, TxState};
use net::ieee802154::MacAddress;
use kernel::hil::time;
use net::ip::{IP6Header, IPAddr, ip6_nh};
use net::lowpan;
use core::mem;
// Define some commong sixlowpan enums. These are copied from lowpan_frag_test.
// Paul says that SAC and DAC are only needed for the lowpan frag testing, which
// I don't entirely understand bc it seems users would still need to classify
// the different possible compression types any time they are choosing how to
// send 6lowpan packets. Pending a discussion with him on this, I am choosing
// to modify his prepare_ipv6_packet code as little as possible

#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct UDPHeader {
    pub source_port: u16,
    pub dest_port: u16,
    pub len: u16,
    pub cksum: u16,
}

#[derive(Copy,Clone,Debug,PartialEq)]
pub enum TF {
    Inline = 0b00,
    Traffic = 0b01,
    Flow = 0b10,
    TrafficFlow = 0b11,
}

#[derive(Copy,Clone,Debug)]
pub enum SAC {
    Inline,
    LLP64,
    LLP16,
    LLPIID,
    Unspecified,
    Ctx64,
    Ctx16,
    CtxIID,
}

#[derive(Copy,Clone,Debug)]
pub enum DAC {
    Inline,
    LLP64,
    LLP16,
    LLPIID,
    Ctx64,
    Ctx16,
    CtxIID,
    McastInline,
    Mcast48,
    Mcast32,
    Mcast8,
    McastCtx,
}


// Function that computes the UDP checksum of a UDP packet
fn compute_udp_checksum(src_addr: IPAddr, dst_addr: IPAddr, udp_len: usize, 
                        udp_packet: &'static [u8]) -> u16 {
  500 as u16
}



// Function that simply calls on lowpan_fragments transmit_packet() function
pub fn send_udp_packet<'a, A: time::Alarm>(frag_state: &'a FragState<'a, A>,
                       tx_state: &'a TxState<'a>,
                       src_mac_addr: MacAddress,
                       dst_mac_addr: MacAddress,
                       ip6_packet: &'static mut [u8],
                       ip6_packet_len: usize
                       ) {

  let ret_code = frag_state.transmit_packet(src_mac_addr,
                                              dst_mac_addr,
                                              ip6_packet,
                                              ip6_packet_len,
                                              None,
                                              tx_state,
                                              true,
                                              true);

  debug!("Ret code: {:?}", ret_code);
}

//Function copied from lowpan_frag_test to create an ipv6 packet - credit to Paul
//Modifications were made so that it taken in additional fields and now creates a UDP 
//datagram inside an IPv6 packet which is ready to be sent by the 6lowpan layer
//Request help on removing unsafe calls
pub fn udp_ipv6_prepare_packet(tf: TF, hop_limit: u8, sac: SAC, dac: DAC, ip6_packet: &'static mut [u8], ip6_packet_len: usize, ip6_hdr_size: usize, src_addr: IPAddr, dst_addr: IPAddr, src_mac_addr: MacAddress, dst_mac_addr: MacAddress, mlp: [u8; 8], src_port: u16, dst_port: u16) {

    // No need to change payload, as it is passed in. Just need to construct IPv6 and UDP headers
    /*{
        for i in 8..ip6_packet_len { //TODO: Change hardcoded 8
            ip6_packet[i] = 0 as u8;
        }
    }*/
    
    //Set UDP Headers Here

    //change hardcoded IPV6 Header length, functionalize
    ip6_packet[40] = (src_port >> 8) as u8;
    ip6_packet[41] = (src_port & 255) as u8;
    ip6_packet[42] = (dst_port >> 8) as u8;
    ip6_packet[43] = (dst_port & 255) as u8;
    ip6_packet[44] = ((ip6_packet_len - ip6_hdr_size - 8) >> 8) as u8; //edit for cariable header length
    ip6_packet[45] = (((ip6_packet_len - ip6_hdr_size - 8) & 255)) as u8; //edit for variable header length
    //set cksum to 0 in preparation for cksum calculation
    ip6_packet[46] = 0 as u8;
    ip6_packet[47] = 0 as u8;

// For now calculate checksum inline bc I am bad at Rust and it makes functions annoying.

  //***** start checksum calc here

  let sum: u32 = 0;
  //sum += (src_addr[1] + (src_addr[0] << 4)) as u16;
  //debug!("Sum: {:?}", sum);
  sum += src_port as u32;
  sum += dst_port as u32;

  //***** end checksum calc here

/*
    let udp_packet: &[u8; ip6_packet_len - ip6_hdr_size];
    udp_packet.copy_from_slice(ip6_packet[ip6_hdr_size..ip6_packet_len]); 
    let cksum = compute_udp_checksum(src_addr, dst_addr, ip6_packet_len - 8, udp_packet); //edit for variable header length

    ip6_packet[46] = (cksum >> 8) as u8;
    ip6_packet[47] = (cksum & 255) as u8;
*/
    {
        let ip6_header: &mut IP6Header = unsafe { mem::transmute(ip6_packet.as_mut_ptr()) }; //TODO: Remove unsafe block
        *ip6_header = IP6Header::new();
        ip6_header.set_payload_len((ip6_packet_len - ip6_hdr_size) as u16);//TODO: Change assumption that ip6_packet_len is actually the length of the payload?

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

        ip6_header.set_next_header(ip6_nh::UDP);//Hudson Edit

        ip6_header.set_hop_limit(hop_limit);

        match sac {
            SAC::Inline => {
                ip6_header.src_addr = src_addr;
            }
            SAC::LLP64 => {
                // LLP::xxxx:xxxx:xxxx:xxxx
                ip6_header.src_addr.set_unicast_link_local();
                ip6_header.src_addr.0[8..16].copy_from_slice(&src_addr.0[8..16]);
            }
            SAC::LLP16 => {
                // LLP::ff:fe00:xxxx
                ip6_header.src_addr.set_unicast_link_local();
                // Distinct from compute_iid because the U/L bit is not flipped
                ip6_header.src_addr.0[11] = 0xff;
                ip6_header.src_addr.0[12] = 0xfe;
                ip6_header.src_addr.0[14..16].copy_from_slice(&src_addr.0[14..16]);
            }
            SAC::LLPIID => {
                // LLP::IID
                ip6_header.src_addr.set_unicast_link_local();
                ip6_header.src_addr.0[8..16].copy_from_slice(&lowpan::compute_iid(&src_mac_addr));
            }
            SAC::Unspecified => {}
            SAC::Ctx64 => {
                // MLP::xxxx:xxxx:xxxx:xxxx
                ip6_header.src_addr.set_prefix(&mlp, 64);
                ip6_header.src_addr.0[8..16].copy_from_slice(&src_addr.0[8..16]);
            }
            SAC::Ctx16 => {
                // MLP::ff:fe00:xxxx
                ip6_header.src_addr.set_prefix(&mlp, 64);
                // Distinct from compute_iid because the U/L bit is not flipped
                ip6_header.src_addr.0[11] = 0xff;
                ip6_header.src_addr.0[12] = 0xfe;
                ip6_header.src_addr.0[14..16].copy_from_slice(&src_addr.0[14..16]);
            }
            SAC::CtxIID => {
                // MLP::IID
                ip6_header.src_addr.set_prefix(&mlp, 64);
                ip6_header.src_addr.0[8..16].copy_from_slice(&lowpan::compute_iid(&src_mac_addr));
            }
        }

        match dac {
            DAC::Inline => {
                ip6_header.dst_addr = dst_addr;
            }
            DAC::LLP64 => {
                // LLP::xxxx:xxxx:xxxx:xxxx
                ip6_header.dst_addr.set_unicast_link_local();
                ip6_header.dst_addr.0[8..16].copy_from_slice(&dst_addr.0[8..16]);
            }
            DAC::LLP16 => {
                // LLP::ff:fe00:xxxx
                ip6_header.dst_addr.set_unicast_link_local();
                // Distinct from compute_iid because the U/L bit is not flipped
                ip6_header.dst_addr.0[11] = 0xff;
                ip6_header.dst_addr.0[12] = 0xfe;
                ip6_header.dst_addr.0[14..16].copy_from_slice(&src_addr.0[14..16]);
            }
            DAC::LLPIID => {
                // LLP::IID
                ip6_header.dst_addr.set_unicast_link_local();
                ip6_header.dst_addr.0[8..16].copy_from_slice(&lowpan::compute_iid(&dst_mac_addr));
            }
            DAC::Ctx64 => {
                // MLP::xxxx:xxxx:xxxx:xxxx
                ip6_header.dst_addr.set_prefix(&mlp, 64);
                ip6_header.dst_addr.0[8..16].copy_from_slice(&src_addr.0[8..16]);
            }
            DAC::Ctx16 => {
                // MLP::ff:fe00:xxxx
                ip6_header.dst_addr.set_prefix(&mlp, 64);
                // Distinct from compute_iid because the U/L bit is not flipped
                ip6_header.dst_addr.0[11] = 0xff;
                ip6_header.dst_addr.0[12] = 0xfe;
                ip6_header.dst_addr.0[14..16].copy_from_slice(&src_addr.0[14..16]);
            }
            DAC::CtxIID => {
                // MLP::IID
                ip6_header.dst_addr.set_prefix(&mlp, 64);
                ip6_header.dst_addr.0[8..16].copy_from_slice(&lowpan::compute_iid(&dst_mac_addr));
            }
            DAC::McastInline => {
                // first byte is ff, that's all we know
                ip6_header.dst_addr = dst_addr;
                ip6_header.dst_addr.0[0] = 0xff;
            }
            DAC::Mcast48 => {
                // ffXX::00XX:XXXX:XXXX
                ip6_header.dst_addr.0[0] = 0xff;
                ip6_header.dst_addr.0[1] = dst_addr.0[1];
                ip6_header.dst_addr.0[11..16].copy_from_slice(&dst_addr.0[11..16]);
            }
            DAC::Mcast32 => {
                // ffXX::00XX:XXXX
                ip6_header.dst_addr.0[0] = 0xff;
                ip6_header.dst_addr.0[1] = dst_addr.0[1];
                ip6_header.dst_addr.0[13..16].copy_from_slice(&dst_addr.0[13..16]);
            }
            DAC::Mcast8 => {
                // ff02::00XX
                ip6_header.dst_addr.0[0] = 0xff;
                ip6_header.dst_addr.0[1] = dst_addr.0[1];
                ip6_header.dst_addr.0[15] = dst_addr.0[15];
            }
            DAC::McastCtx => {
                // ffXX:XX + plen + pfx64 + XXXX:XXXX
                ip6_header.dst_addr.0[0] = 0xff;
                ip6_header.dst_addr.0[1] = dst_addr.0[1];
                ip6_header.dst_addr.0[2] = dst_addr.0[2];
                ip6_header.dst_addr.0[3] = 64 as u8;
                ip6_header.dst_addr.0[4..12].copy_from_slice(&mlp);
                ip6_header.dst_addr.0[12..16].copy_from_slice(&dst_addr.0[12..16]);
            }
        }
    }
    debug!("Packet with tf={:?} hl={} sac={:?} dac={:?}",
           tf,
           hop_limit,
           sac,
           dac);
}

