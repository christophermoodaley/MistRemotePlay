//! Per-frame timestamps carried in H.264 SEI "user data unregistered" NAL
//! units (payload type 5), so the client can measure capture-to-display
//! latency for each frame and split it into host and network time.
//! Decoders ignore unknown SEI.

/// Random UUID identifying Mist timing SEI payloads.
pub const MIST_SEI_UUID: [u8; 16] =
    [0x6d, 0x69, 0x73, 0x74, 0x2d, 0x74, 0x73, 0x00, 0x8f, 0x3a, 0x1c, 0x52, 0xb4, 0x07, 0x9e, 0x61];

const NAL_SEI: u8 = 6;
const NAL_AUD: u8 = 9;
const SEI_USER_DATA_UNREGISTERED: u8 = 5;
/// UUID + capture time (older hosts).
const PAYLOAD_V1: usize = 24;
/// UUID + capture time + encode-complete time.
const PAYLOAD_V2: usize = 32;

/// Host-clock timestamps (µs) of one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameTimes {
    /// Frame left the capture source.
    pub capture_us: u64,
    /// Encoded access unit left the encoder/parser, about to be packetized.
    pub encoded_us: Option<u64>,
}

/// Build an Annex-B SEI NAL (with 4-byte start code) carrying `times`.
pub fn build_sei_nal(times: FrameTimes) -> Vec<u8> {
    let mut rbsp = Vec::with_capacity(40);
    rbsp.push(SEI_USER_DATA_UNREGISTERED);
    let encoded = times.encoded_us;
    rbsp.push(if encoded.is_some() { PAYLOAD_V2 } else { PAYLOAD_V1 } as u8);
    rbsp.extend_from_slice(&MIST_SEI_UUID);
    rbsp.extend_from_slice(&times.capture_us.to_be_bytes());
    if let Some(e) = encoded {
        rbsp.extend_from_slice(&e.to_be_bytes());
    }
    rbsp.push(0x80); // rbsp_trailing_bits
    let mut out = vec![0, 0, 0, 1, NAL_SEI];
    out.extend(escape_rbsp(&rbsp));
    out
}

/// Insert emulation-prevention bytes (00 00 0x -> 00 00 03 0x for x <= 3).
fn escape_rbsp(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + 4);
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

fn unescape_rbsp(ebsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ebsp.len());
    let mut zeros = 0;
    for &b in ebsp {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Position of the next 3-byte start code at or after `from`.
fn next_start_code(au: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 3 <= au.len() {
        // Skip ahead quickly: a start code needs au[i + 2] == 1.
        match au[i + 2] {
            1 if au[i] == 0 && au[i + 1] == 0 => return Some(i),
            0 | 1 => i += 1,
            _ => i += 3,
        }
    }
    None
}

/// NAL units of an Annex-B access unit, in order, as
/// `(start code offset, header offset, end)`. Lazy: callers that only need
/// the leading non-slice NALs never scan the (large) slice data.
fn nal_units(au: &[u8]) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
    let mut next = next_start_code(au, 0);
    std::iter::from_fn(move || loop {
        let sc3 = next?;
        let begin = sc3 + 3;
        let sc = if sc3 > 0 && au[sc3 - 1] == 0 { sc3 - 1 } else { sc3 };
        next = next_start_code(au, begin);
        let end = match next {
            Some(n) if n > 0 && au[n - 1] == 0 => n - 1,
            Some(n) => n,
            None => au.len(),
        };
        if begin < end {
            return Some((sc, begin, end));
        }
    })
}

fn is_slice(nal_header: u8) -> bool {
    matches!(nal_header & 0x1f, 1..=5)
}

/// Insert `sei` (from [`build_sei_nal`]) into an Annex-B access unit, after
/// an access unit delimiter if present, otherwise at the start.
pub fn insert_sei(au: &[u8], sei: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(au.len() + sei.len());
    let insert_at = match nal_units(au).next() {
        Some((_, begin, end)) if au[begin] & 0x1f == NAL_AUD => end,
        Some((sc, _, _)) => sc,
        None => 0,
    };
    out.extend_from_slice(&au[..insert_at]);
    out.extend_from_slice(sei);
    out.extend_from_slice(&au[insert_at..]);
    out
}

/// Extract Mist frame timestamps from an Annex-B access unit. SEI precedes
/// the slices of its access unit, so scanning stops at the first slice.
pub fn extract_times(au: &[u8]) -> Option<FrameTimes> {
    for (_, begin, end) in nal_units(au) {
        if is_slice(au[begin]) {
            return None;
        }
        if au[begin] & 0x1f != NAL_SEI {
            continue;
        }
        let rbsp = unescape_rbsp(&au[begin + 1..end]);
        let mut p = 0;
        // Iterate SEI messages until rbsp trailing bits.
        while p < rbsp.len() && rbsp[p] != 0x80 {
            let mut ptype = 0usize;
            while p < rbsp.len() && rbsp[p] == 0xff {
                ptype += 255;
                p += 1;
            }
            ptype += *rbsp.get(p)? as usize;
            p += 1;
            let mut size = 0usize;
            while p < rbsp.len() && rbsp[p] == 0xff {
                size += 255;
                p += 1;
            }
            size += *rbsp.get(p)? as usize;
            p += 1;
            let payload = rbsp.get(p..p + size)?;
            if ptype == SEI_USER_DATA_UNREGISTERED as usize
                && (size == PAYLOAD_V1 || size == PAYLOAD_V2)
                && payload[..16] == MIST_SEI_UUID
            {
                let word = |at: usize| u64::from_be_bytes(payload[at..at + 8].try_into().expect("8 bytes"));
                return Some(FrameTimes { capture_us: word(16), encoded_us: (size == PAYLOAD_V2).then(|| word(24)) });
            }
            p += size;
        }
    }
    None
}

/// Capture timestamp only (see [`extract_times`]).
pub fn extract_timestamp(au: &[u8]) -> Option<u64> {
    extract_times(au).map(|t| t.capture_us)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDR: &[u8] = &[0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 0, 1, 0x68, 4, 0, 0, 1, 0x65, 0x88, 0x84, 0x00];

    #[test]
    fn roundtrip_with_emulation_prevention() {
        for ts in [0u64, 1, 0x0000_0000_0001_0000, 0x0000_0300_0000_0002, u64::MAX, 123_456_789_012] {
            let au = insert_sei(IDR, &build_sei_nal(FrameTimes { capture_us: ts, encoded_us: None }));
            assert_eq!(extract_timestamp(&au), Some(ts), "ts={ts:#x}");
            // Original NALs are untouched.
            assert!(au.ends_with(&IDR[8..]));
        }
    }

    #[test]
    fn sei_never_contains_start_code() {
        let nal = build_sei_nal(FrameTimes { capture_us: 1, encoded_us: Some(0x0000_0100_0000_0000) });
        assert!(!nal[4..].windows(3).any(|w| w == [0, 0, 1] || w == [0, 0, 0]));
    }

    #[test]
    fn inserted_after_aud() {
        let mut au = vec![0, 0, 0, 1, 0x09, 0xf0];
        au.extend_from_slice(IDR);
        let out = insert_sei(&au, &build_sei_nal(FrameTimes { capture_us: 7, encoded_us: None }));
        assert_eq!(&out[..6], &[0, 0, 0, 1, 0x09, 0xf0]);
        assert_eq!(out[10], 0x06);
        assert_eq!(extract_timestamp(&out), Some(7));
    }

    #[test]
    fn foreign_sei_ignored() {
        // x264's own unregistered SEI with a different UUID.
        let mut au = vec![0, 0, 0, 1, 0x06, 5, 17];
        au.extend_from_slice(&[0xdc; 16]);
        au.extend_from_slice(&[b'x', 0x80]);
        au.extend_from_slice(IDR);
        assert_eq!(extract_timestamp(&au), None);
        let out = insert_sei(&au, &build_sei_nal(FrameTimes { capture_us: 42, encoded_us: None }));
        assert_eq!(extract_timestamp(&out), Some(42));
    }

    #[test]
    fn carries_encode_time() {
        for (c, e) in [(0u64, 0u64), (1, 0x0300_0000), (123_456_789, 123_461_000), (u64::MAX - 1, u64::MAX)] {
            let t = FrameTimes { capture_us: c, encoded_us: Some(e) };
            assert_eq!(extract_times(&insert_sei(IDR, &build_sei_nal(t))), Some(t));
        }
    }

    #[test]
    fn sei_after_first_slice_is_ignored() {
        let mut au = IDR.to_vec();
        au.extend_from_slice(&build_sei_nal(FrameTimes { capture_us: 5, encoded_us: None }));
        assert_eq!(extract_times(&au), None);
    }

    #[test]
    fn three_byte_start_codes() {
        let au = [0, 0, 1, 0x67, 1, 0, 0, 1, 0x68, 2, 0, 0, 1, 0x65, 0x88];
        let out = insert_sei(&au, &build_sei_nal(FrameTimes { capture_us: 9, encoded_us: Some(10) }));
        assert!(out.ends_with(&au));
        assert_eq!(extract_timestamp(&out), Some(9));
    }

    #[test]
    fn garbage_does_not_panic() {
        assert_eq!(extract_timestamp(&[]), None);
        assert_eq!(extract_timestamp(&[0, 0, 1, 6, 5, 0xff, 0xff]), None);
        assert_eq!(extract_timestamp(&[0, 0, 1, 6, 5, 200, 1, 2]), None);
    }
}
