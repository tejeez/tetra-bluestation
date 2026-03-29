use std::os::unix::net::UnixDatagram;

use super::demodulator;
use super::dsp_types::*;
use super::modulator;

use tetra_config::bluestation::SharedConfig;
use tetra_pdus::phy::traits::rxtx_dev::RxSlotBits;
use tetra_pdus::phy::traits::rxtx_dev::RxTxDev;
use tetra_pdus::phy::traits::rxtx_dev::RxTxDevError;
use tetra_pdus::phy::traits::rxtx_dev::TxSlotBits;

const PACKET_MAX_BYTES_RX: usize = 0x8000;
const PACKET_MAX_BYTES_TX: usize = 0x8000;

pub struct RxTxDevIqSocket {
    tx_count: SampleCount,
    rx_socket: UnixDatagram,
    tx_socket: UnixDatagram,
    rx_buf: Vec<u8>,
    tx_buf: Vec<u8>,
    demodulator: demodulator::Demodulator,
    modulator: modulator::Modulator,
}

impl RxTxDevIqSocket {
    pub fn new(cfg: &SharedConfig) -> Self {
        let config_guard = cfg.config();
        let iqsocket_cfg = &config_guard
            .as_ref()
            .phy_io
            .iqsocket;

        Self {
            tx_count: 0,
            rx_socket: {
                // There may be a leftover socket from a previous run, so first remove it.
                // Ignore return value since it fails if there was no leftover socket.
                let _ = std::fs::remove_file(&iqsocket_cfg.rx_path);
                UnixDatagram::bind(&iqsocket_cfg.rx_path).unwrap()
            },
            tx_socket: {
                let socket = UnixDatagram::unbound().unwrap();
                // Re-try connecting until Sdrglue has started and the socket exists
                loop {
                    match socket.connect(&iqsocket_cfg.tx_path) {
                        Ok(()) => break socket,
                        Err(err) => {
                            tracing::debug!("Could not open TX socket yet: {}", err);
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                    }
                }
            },
            rx_buf: vec![0; PACKET_MAX_BYTES_RX],
            tx_buf: Vec::with_capacity(PACKET_MAX_BYTES_TX),
            demodulator: demodulator::Demodulator::new(demodulator::Mode::Ul),
            modulator: modulator::Modulator::new(modulator::Mode::Dl),
        }
    }

    fn transmit_all(&mut self, tx_slot: &[TxSlotBits]) -> Result<(), RxTxDevError> {
        loop {
            self.tx_buf.clear();

            let header_value: u64 = 0; // placeholder, not decided yet
            self.tx_buf.extend_from_slice(&header_value.to_le_bytes());
            self.tx_buf.extend_from_slice(&self.tx_count.to_le_bytes());

            let mut samples_added = false;
            loop {
                if self.tx_buf.len() + 8 >= PACKET_MAX_BYTES_TX {
                    // Additional sample would exceed maximum packet size.
                    // Send a packet and continue in next packet.
                    break;
                }
                match self.modulator.sample(self.tx_count, &tx_slot[0]) {
                    Ok(sample) => {
                        samples_added = true;
                        self.tx_buf.extend_from_slice(&sample.re.to_le_bytes());
                        self.tx_buf.extend_from_slice(&sample.im.to_le_bytes());
                        self.tx_count = self.tx_count.wrapping_add(1);
                    },
                    Err(modulator::Error::NeedMoreData) => break,
                }
            }
            if !samples_added {
                // Nothing to send anymore
                break;
            }
            match self.tx_socket.send(&self.tx_buf) {
                Ok(len) => {
                    // If not all bytes were written,
                    // we should probably send the rest of the samples in another packet,
                    // but hope we do not really need to implement that for now.
                    // On the other hand it might mean the TX socket buffer is full
                    // and sending these samples would be too late anyway,
                    // so maybe it does not need to be handled better than this.
                    // Print a warning in case it happens.
                    if len != self.tx_buf.len() {
                        tracing::warn!("Wrote {} out of {} bytes", len, self.tx_buf.len());
                    }
                }
                Err(err) => {
                    tracing::error!("Error writing TX socket: {}", err);
                    return Err(RxTxDevError::TxWriteError)
                }
            }
        }
        Ok(())
    }

    fn receive_packet(&mut self) -> Result<(), RxTxDevError> {
        match self.rx_socket.recv(&mut self.rx_buf[..]) {
            Ok(len) => {
                if len >= 16 {
                    let packet = &self.rx_buf[0..len];
                    let mut rx_count = SampleCount::from_le_bytes(packet[8..16].try_into().unwrap());
                    for sample_u8 in packet[16..].chunks_exact(8) {
                        // unwrap is OK because chunks_exact always returns the right size
                        self.demodulator.sample(ComplexSample {
                            re: RealSample::from_le_bytes(sample_u8[0..4].try_into().unwrap()),
                            im: RealSample::from_le_bytes(sample_u8[4..8].try_into().unwrap()),
                        }, rx_count);

                        rx_count = rx_count.wrapping_add(1);
                    }

                    // To avoid generating TX samples in the past,
                    // ensure tx_count is ahead of rx_count by some minimum number of samples.
                    let tx_ahead_min = 0;
                    if rx_count.wrapping_sub(self.tx_count) > tx_ahead_min {
                        self.tx_count = rx_count.wrapping_add(tx_ahead_min);
                    }
                    Ok(())
                } else {
                    tracing::warn!("Too short RX packet of {} bytes", len);
                    Ok(())
                }
            },
            Err(err) => {
                tracing::error!("Error reading RX socket: {}", err);
                Err(RxTxDevError::RxReadError)
            }
        }
    }
}

impl RxTxDev for RxTxDevIqSocket {
    fn rxtx_timeslot<'a>(
        &'a mut self,
        tx_slot: &[TxSlotBits],
    ) -> Result<Vec<Option<RxSlotBits<'a>>>, RxTxDevError> {
        self.transmit_all(tx_slot)?;
        loop {
            self.receive_packet()?;
            if self.demodulator.demodulated_slot_available() {
                return Ok(vec![self.demodulator.take_demodulated_slot()]);
            }
        }
    }
}
