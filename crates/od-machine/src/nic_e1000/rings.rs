//! Transmit and receive rings of legacy descriptors (NIC_DEVICE.md sections 7 and 8).
//!
//! A legacy descriptor is 16 bytes: the buffer address (0..8), the length (8..10), then on
//! transmit CSO, CMD, STA, CSS and the special field, on receive the checksum, STA, the error
//! byte and the special field. Everything is served from `service`, never from a register write.

use super::regs::*;
use super::{NicState, RAH_VALID, RX_BUDGET, TX_ASSEMBLY_LIMIT, TX_BUDGET};
use crate::nvme::GuestDma;

const DESCRIPTOR_BYTES: u64 = 16;
/// The shortest frame on the wire without FCS: the receive side pads shorter ones with zeros.
const MIN_WIRE_FRAME: usize = 60;
const FCS_BYTES: usize = 4;

/// How one received frame lies in the ring: the padded length, the length with FCS (unless
/// SECRC strips it), the buffer size and the number of descriptors it takes.
#[derive(Clone, Copy)]
struct RxLayout {
    data_len: usize,
    total: usize,
    buffer_bytes: usize,
    needed: u32,
}

/// Address of descriptor `index` of the ring at `base`; `None` when it would wrap the address
/// space.
fn descriptor_address(base: u64, index: u32) -> Option<u64> {
    (base & !0xF).checked_add(u64::from(index) * DESCRIPTOR_BYTES)
}

impl NicState {
    /// Consumes transmit descriptors from TDH to TDT (at most [`TX_BUDGET`]) and returns how
    /// many. A chain of descriptors up to the one with EOP is one frame.
    pub(super) fn service_tx(&mut self, dma: &mut dyn GuestDma) -> u32 {
        let hw = &mut self.hw;
        if hw.tctl & TCTL_EN == 0 || hw.tx_fault {
            return 0;
        }
        let count = hw.tdlen / DESCRIPTOR_BYTES as u32;
        if count == 0 || hw.tdh == hw.tdt {
            return 0;
        }
        if hw.tdh >= count || hw.tdt >= count {
            self.counters.ring_errors += 1;
            return 0;
        }
        let mut done = 0;
        while hw.tdh != hw.tdt && done < TX_BUDGET {
            let Some(address) = descriptor_address(hw.tdba, hw.tdh) else {
                self.counters.dma_errors += 1;
                hw.tx_fault = true;
                break;
            };
            let mut descriptor = [0u8; DESCRIPTOR_BYTES as usize];
            if dma.read(address, &mut descriptor).is_err() {
                self.counters.dma_errors += 1;
                hw.tx_fault = true;
                break;
            }
            let buffer = u64::from_le_bytes(descriptor[0..8].try_into().expect("8 bytes"));
            let length = usize::from(u16::from_le_bytes([descriptor[8], descriptor[9]]));
            let command = descriptor[11];
            self.counters.tx_descriptors += 1;
            if command & CMD_DEXT != 0 {
                // Context and extended data descriptors are not implemented: the packet they
                // belong to is dropped at its last descriptor.
                hw.tx_discard = true;
            } else if length > 0 && !hw.tx_discard {
                let start = hw.tx_frame.len();
                if start + length > TX_ASSEMBLY_LIMIT {
                    hw.tx_discard = true;
                } else {
                    hw.tx_frame.resize(start + length, 0);
                    if dma.read(buffer, &mut hw.tx_frame[start..]).is_err() {
                        self.counters.dma_errors += 1;
                        hw.tx_discard = true;
                    }
                }
            }
            if command & CMD_EOP != 0 {
                if hw.tx_discard {
                    self.counters.tx_discarded += 1;
                } else if !hw.tx_frame.is_empty() {
                    match self.backend.as_mut() {
                        Some(backend) => match backend.transmit(&hw.tx_frame) {
                            Ok(()) => {
                                self.counters.tx_frames += 1;
                                self.counters.tx_bytes += hw.tx_frame.len() as u64;
                            }
                            // A lost packet, not a device error (NETWORK.md section 4).
                            Err(_) => self.counters.tx_backend_dropped += 1,
                        },
                        None => self.counters.tx_no_backend += 1,
                    }
                }
                hw.tx_frame.clear();
                hw.tx_discard = false;
            }
            if command & CMD_RS != 0 {
                if dma.write(address + 12, &[STA_DD]).is_err() {
                    self.counters.dma_errors += 1;
                }
                hw.icr |= INT_TXDW;
            }
            hw.tdh = (hw.tdh + 1) % count;
            done += 1;
        }
        if done == TX_BUDGET && hw.tdh != hw.tdt {
            // More work than one call does: the next quantum ends at once and comes back.
            self.host_dirty
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if done > 0 && hw.tdh == hw.tdt {
            hw.icr |= INT_TXQE;
        }
        done
    }

    /// Size of one receive buffer: RCTL.BSIZE with BSEX.
    fn rx_buffer_bytes(&self) -> usize {
        let size = (self.hw.rctl >> RCTL_BSIZE_SHIFT) & 0b11;
        if self.hw.rctl & RCTL_BSEX != 0 {
            // BSEX with BSIZE 00 is reserved: the model treats it as 2048.
            match size {
                0b01 => 16384,
                0b10 => 8192,
                0b11 => 4096,
                _ => 2048,
            }
        } else {
            2048 >> size
        }
    }

    /// Descriptors the hardware may use: from RDH up to, not including, RDT.
    fn rx_free(&self, count: u32) -> u32 {
        (self.hw.rdt + count - self.hw.rdh) % count
    }

    /// Whether the frame passes the receive filter (RCTL, RAL/RAH, MTA).
    fn rx_accepts(&self, frame: &[u8]) -> bool {
        let Some(destination) = frame.get(0..6) else {
            return false;
        };
        let hw = &self.hw;
        let exact = hw.ra.iter().any(|[low, high]| {
            high & RAH_VALID != 0
                && low.to_le_bytes() == destination[0..4]
                && (high & 0xFFFF).to_le_bytes()[0..2] == destination[4..6]
        });
        if exact {
            return true;
        }
        if destination == [0xFF; 6] {
            return hw.rctl & RCTL_BAM != 0;
        }
        if destination[0] & 1 != 0 {
            if hw.rctl & RCTL_MPE != 0 {
                return true;
            }
            // Multicast hash: 12 bits of the address picked by RCTL.MO select one bit of MTA.
            let shift = [4, 3, 2, 0][((hw.rctl >> RCTL_MO_SHIFT) & 0b11) as usize];
            let hash = (u32::from(destination[4]) >> shift
                | u32::from(destination[5]) << (8 - shift))
                & 0xFFF;
            return hw.mta[(hash >> 5) as usize] >> (hash & 31) & 1 != 0;
        }
        hw.rctl & RCTL_UPE != 0
    }

    /// Takes frames from the backend and writes them into receive descriptors (at most
    /// [`RX_BUDGET`] frames) and returns how many were delivered. The backend is polled only
    /// while a descriptor is free; a frame that does not fit yet is kept until it does.
    pub(super) fn service_rx(&mut self, dma: &mut dyn GuestDma) -> u32 {
        if self.hw.rctl & RCTL_EN == 0 || self.hw.rx_fault {
            return 0;
        }
        let count = self.hw.rdlen / DESCRIPTOR_BYTES as u32;
        if count == 0 {
            return 0;
        }
        if self.hw.rdh >= count || self.hw.rdt >= count {
            if self.hw.rdh != self.hw.rdt {
                self.counters.ring_errors += 1;
            }
            return 0;
        }
        let mut delivered = 0;
        for _ in 0..RX_BUDGET {
            let len = match self.rx_staged {
                Some(len) => len,
                None => {
                    if self.rx_free(count) == 0 {
                        break;
                    }
                    let Some(backend) = self.backend.as_mut() else {
                        break;
                    };
                    let Some(len) = backend.poll_receive(&mut self.rx_buffer) else {
                        break;
                    };
                    // The trait promises a length within the buffer; a backend that breaks it
                    // loses the frame, it does not make the device panic.
                    if len > self.rx_buffer.len() {
                        self.counters.rx_bad_backend_length += 1;
                        continue;
                    }
                    self.rx_staged = Some(len);
                    len
                }
            };
            if !self.rx_accepts(&self.rx_buffer[..len]) {
                self.counters.rx_filtered += 1;
                self.rx_staged = None;
                continue;
            }
            // The wire carries at least 60 bytes before the FCS; SECRC strips the FCS.
            let data_len = len.max(MIN_WIRE_FRAME);
            let total = if self.hw.rctl & RCTL_SECRC != 0 {
                data_len
            } else {
                data_len + FCS_BYTES
            };
            let buffer_bytes = self.rx_buffer_bytes();
            // A ring has at least 8 descriptors and a frame needs at most 6 of the smallest
            // (256-byte) buffers, so a staged frame always fits once the driver has returned
            // enough descriptors.
            let layout = RxLayout {
                data_len,
                total,
                buffer_bytes,
                needed: total.div_ceil(buffer_bytes) as u32,
            };
            if self.rx_free(count) < layout.needed {
                // Wait for the driver to hand back descriptors.
                break;
            }
            self.rx_staged = None;
            if self.deliver_frame(dma, len, layout, count) {
                delivered += 1;
            }
        }
        delivered
    }

    /// Writes one frame (padded, with or without FCS) into `needed` descriptors from RDH.
    /// Returns false, with no descriptor completed, when guest memory refuses an access.
    fn deliver_frame(
        &mut self,
        dma: &mut dyn GuestDma,
        len: usize,
        layout: RxLayout,
        count: u32,
    ) -> bool {
        let RxLayout {
            data_len,
            total,
            buffer_bytes,
            needed,
        } = layout;
        if len > self.rx_buffer.len() {
            // Unreachable through `service_rx`; the check keeps the slice below total.
            self.counters.rx_bad_backend_length += 1;
            return false;
        }
        // The padded frame, then the FCS of the frame as the wire would carry it.
        let mut image = vec![0u8; total];
        image[..len].copy_from_slice(&self.rx_buffer[..len]);
        if total > data_len {
            let fcs = crate::crc32::crc32_ieee(&[&image[..data_len]]);
            image[data_len..].copy_from_slice(&fcs.to_le_bytes());
        }
        // Fetch every descriptor first and write every buffer before any status.
        let mut slots = Vec::with_capacity(needed as usize);
        for step in 0..needed {
            let index = (self.hw.rdh + step) % count;
            let Some(address) = descriptor_address(self.hw.rdba, index) else {
                self.counters.dma_errors += 1;
                self.hw.rx_fault = true;
                return false;
            };
            let mut descriptor = [0u8; DESCRIPTOR_BYTES as usize];
            if dma.read(address, &mut descriptor).is_err() {
                self.counters.dma_errors += 1;
                self.hw.rx_fault = true;
                return false;
            }
            slots.push((
                address,
                u64::from_le_bytes(descriptor[0..8].try_into().expect("8 bytes")),
            ));
        }
        for (step, chunk) in image.chunks(buffer_bytes).enumerate() {
            if dma.write(slots[step].1, chunk).is_err() {
                self.counters.dma_errors += 1;
                return false;
            }
        }
        for (step, chunk) in image.chunks(buffer_bytes).enumerate() {
            let last = step + 1 == needed as usize;
            let mut status = STA_DD | STA_IXSM;
            if last {
                status |= STA_EOP;
            }
            let mut tail = [0u8; 8];
            tail[0..2].copy_from_slice(&(chunk.len() as u16).to_le_bytes());
            tail[4] = status;
            if dma.write(slots[step].0 + 8, &tail).is_err() {
                self.counters.dma_errors += 1;
            }
        }
        self.counters.rx_descriptors += u64::from(needed);
        self.counters.rx_frames += 1;
        self.counters.rx_bytes += len as u64;
        self.hw.rdh = (self.hw.rdh + needed) % count;
        self.hw.icr |= INT_RXT0;
        let threshold = count >> (1 + ((self.hw.rctl >> RCTL_RDMTS_SHIFT) & 0b11).min(2));
        if self.rx_free(count) <= threshold {
            self.hw.icr |= INT_RXDMT0;
        }
        true
    }
}
