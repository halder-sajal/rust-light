//! WLED UDP realtime protocol — see project `CLAUDE.md` for the verified
//! spec this implements. Only WARLS / DRGB / DRGBW / DNRGB are supported;
//! protocol 0 (Notifier) is out of scope.

use std::io;
use std::net::{SocketAddr, UdpSocket};

pub const DEFAULT_PORT: u16 = 21324;

/// Stay in realtime mode indefinitely (no timeout).
pub const TIMEOUT_INDEFINITE: u8 = 255;

pub const WARLS_MAX_LEDS: usize = 255;
pub const DRGB_MAX_LEDS: usize = 490;
pub const DRGBW_MAX_LEDS: usize = 367;
pub const DNRGB_MAX_LEDS_PER_PACKET: usize = 489;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgbw {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub w: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Warls,
    Drgb,
    Drgbw,
    Dnrgb,
}

impl Protocol {
    fn id(self) -> u8 {
        match self {
            Protocol::Warls => 1,
            Protocol::Drgb => 2,
            Protocol::Drgbw => 3,
            Protocol::Dnrgb => 4,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WledError {
    #[error("too many LEDs for this protocol/packet: got {got}, max {max}")]
    TooManyLeds { got: usize, max: usize },
    #[error("network error: {0}")]
    Io(#[from] io::Error),
}

/// A UDP connection to a single WLED device, speaking the realtime protocol.
pub struct WledSender {
    socket: UdpSocket,
    target: SocketAddr,
    timeout: u8,
}

impl WledSender {
    pub fn new(target: SocketAddr) -> io::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.connect(target)?;
        Ok(Self {
            socket,
            target,
            timeout: TIMEOUT_INDEFINITE,
        })
    }

    pub fn with_timeout(mut self, timeout: u8) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn target(&self) -> SocketAddr {
        self.target
    }

    fn send_packet(&self, protocol_id: u8, payload: &[u8]) -> io::Result<()> {
        let mut buf = Vec::with_capacity(2 + payload.len());
        buf.push(protocol_id);
        buf.push(self.timeout);
        buf.extend_from_slice(payload);
        self.socket.send(&buf)?;
        Ok(())
    }

    /// WARLS: indexed per-LED updates. Only changed LEDs need to be included.
    pub fn send_warls(&self, leds: &[(u8, Rgb)]) -> Result<(), WledError> {
        if leds.len() > WARLS_MAX_LEDS {
            return Err(WledError::TooManyLeds {
                got: leds.len(),
                max: WARLS_MAX_LEDS,
            });
        }
        let mut payload = Vec::with_capacity(leds.len() * 4);
        for (index, rgb) in leds {
            payload.push(*index);
            payload.push(rgb.r);
            payload.push(rgb.g);
            payload.push(rgb.b);
        }
        self.send_packet(Protocol::Warls.id(), &payload)?;
        Ok(())
    }

    /// DRGB: flat RGB stream for every LED starting at index 0.
    pub fn send_drgb(&self, pixels: &[Rgb]) -> Result<(), WledError> {
        if pixels.len() > DRGB_MAX_LEDS {
            return Err(WledError::TooManyLeds {
                got: pixels.len(),
                max: DRGB_MAX_LEDS,
            });
        }
        let mut payload = Vec::with_capacity(pixels.len() * 3);
        for p in pixels {
            payload.extend_from_slice(&[p.r, p.g, p.b]);
        }
        self.send_packet(Protocol::Drgb.id(), &payload)?;
        Ok(())
    }

    /// DRGBW: flat RGBW stream for every LED starting at index 0.
    pub fn send_drgbw(&self, pixels: &[Rgbw]) -> Result<(), WledError> {
        if pixels.len() > DRGBW_MAX_LEDS {
            return Err(WledError::TooManyLeds {
                got: pixels.len(),
                max: DRGBW_MAX_LEDS,
            });
        }
        let mut payload = Vec::with_capacity(pixels.len() * 4);
        for p in pixels {
            payload.extend_from_slice(&[p.r, p.g, p.b, p.w]);
        }
        self.send_packet(Protocol::Drgbw.id(), &payload)?;
        Ok(())
    }

    /// DNRGB: one chunk starting at `start_index`. High byte of the index is
    /// sent first, then the low byte.
    pub fn send_dnrgb_chunk(&self, start_index: u16, pixels: &[Rgb]) -> Result<(), WledError> {
        if pixels.len() > DNRGB_MAX_LEDS_PER_PACKET {
            return Err(WledError::TooManyLeds {
                got: pixels.len(),
                max: DNRGB_MAX_LEDS_PER_PACKET,
            });
        }
        let mut payload = Vec::with_capacity(2 + pixels.len() * 3);
        payload.push((start_index >> 8) as u8);
        payload.push((start_index & 0xFF) as u8);
        for p in pixels {
            payload.extend_from_slice(&[p.r, p.g, p.b]);
        }
        self.send_packet(Protocol::Dnrgb.id(), &payload)?;
        Ok(())
    }

    /// DNRGB: send a full frame of any length, automatically split across as
    /// many packets as needed.
    pub fn send_dnrgb_frame(&self, pixels: &[Rgb]) -> Result<(), WledError> {
        for (chunk_idx, chunk) in pixels.chunks(DNRGB_MAX_LEDS_PER_PACKET).enumerate() {
            let start_index = (chunk_idx * DNRGB_MAX_LEDS_PER_PACKET) as u16;
            self.send_dnrgb_chunk(start_index, chunk)?;
        }
        Ok(())
    }

    /// Header-only keepalive so WLED doesn't fall out of realtime mode when
    /// nothing has changed and the timeout isn't indefinite.
    pub fn send_keepalive(&self, protocol: Protocol) -> Result<(), WledError> {
        match protocol {
            Protocol::Dnrgb => self.send_dnrgb_chunk(0, &[]),
            other => {
                self.send_packet(other.id(), &[])?;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture_payload(f: impl FnOnce(&WledSender) -> Result<(), WledError>) -> Vec<u8> {
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let sender = WledSender::new(addr).unwrap().with_timeout(5);
        f(&sender).unwrap();
        let mut buf = [0u8; 1500];
        let (n, _) = listener.recv_from(&mut buf).unwrap();
        buf[..n].to_vec()
    }

    #[test]
    fn warls_layout() {
        let leds = [(0u8, Rgb { r: 1, g: 2, b: 3 }), (5u8, Rgb { r: 4, g: 5, b: 6 })];
        let packet = capture_payload(|s| s.send_warls(&leds));
        assert_eq!(packet, vec![1, 5, 0, 1, 2, 3, 5, 4, 5, 6]);
    }

    #[test]
    fn drgb_layout() {
        let pixels = [Rgb { r: 10, g: 20, b: 30 }, Rgb { r: 40, g: 50, b: 60 }];
        let packet = capture_payload(|s| s.send_drgb(&pixels));
        assert_eq!(packet, vec![2, 5, 10, 20, 30, 40, 50, 60]);
    }

    #[test]
    fn dnrgb_start_index_is_high_byte_first() {
        let pixels = [Rgb { r: 7, g: 8, b: 9 }];
        let packet = capture_payload(|s| s.send_dnrgb_chunk(0x0102, &pixels));
        assert_eq!(packet, vec![4, 5, 0x01, 0x02, 7, 8, 9]);
    }

    #[test]
    fn dnrgb_frame_splits_across_packets() {
        let pixels = vec![Rgb { r: 1, g: 1, b: 1 }; DNRGB_MAX_LEDS_PER_PACKET + 1];

        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let sender = WledSender::new(addr).unwrap();
        sender.send_dnrgb_frame(&pixels).unwrap();

        let mut buf = [0u8; 1500];
        let (n1, _) = listener.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[2..4], &[0x00, 0x00]); // first chunk starts at 0
        assert_eq!((n1 - 4) / 3, DNRGB_MAX_LEDS_PER_PACKET);

        let (n2, _) = listener.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[2..4], &[0x01, 0xE9]); // 489 = 0x01E9
        assert_eq!((n2 - 4) / 3, 1);
    }

    #[test]
    fn warls_rejects_too_many_leds() {
        let leds = vec![(0u8, Rgb { r: 0, g: 0, b: 0 }); WARLS_MAX_LEDS + 1];
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender = WledSender::new(socket.local_addr().unwrap()).unwrap();
        assert!(matches!(
            sender.send_warls(&leds),
            Err(WledError::TooManyLeds { .. })
        ));
    }
}
