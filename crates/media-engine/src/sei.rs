//! Per-frame capture timestamps carried in H.264 SEI "user data
//! unregistered" NAL units (payload type 5), so the client can measure
//! capture-to-display latency for each frame. Decoders ignore unknown SEI.

/// Random UUID identifying Mist timing SEI payloads.
pub const MIST_SEI_UUID: [u8; 16] =
    [0x6d, 0x69, 0x73, 0x74, 0x2d, 0x74, 0x73, 0x00, 0x8f, 0x3a, 0x1c, 0x52, 0xb4, 0x07, 0x9e, 0x61];

const NAL_SEI: u8 = 6;
const NAL_AUD: u8 = 9;
const SEI_USER_DATA_UNREGISTERED: u8 = 5;

/// Build an Annex-B SEI NAL (with 4-byte start code) carrying `capture_us`.
pub fn build_sei_nal(capture_us: u64) -> Vec<u8> {
    let mut rbsp = Vec::with_capacity(32);
    rbsp.push(SEI_USER_DATA_UNREGISTERED);
    rbsp.push(24); // 16 byte UUID + 8 byte timestamp
    rbsp.extend_from_slice(&MIST_SEI_UUID);
    rbsp.extend_from_slice(&capture_us.to_be_bytes());
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

/// Byte ranges of NAL units (header included, start code excluded) in an
/// Annex-B access unit, along with the offset where each start code begins.
fn nal_units(au: &[u8]) -> Vec<(usize, usize, usize)> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            let sc_start = if i > 0 && au[i - 1] == 0 { i - 1 } else { i };
            starts.push((sc_start, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (n, &(sc, begin)) in starts.iter().enumerate() {
        let end = starts.get(n + 1).map(|(next_sc, _)| *next_sc).unwrap_or(au.len());
        if begin < end {
            out.push((sc, begin, end));
        }
    }
    out
}

/// Insert `sei` (from [`build_sei_nal`]) into an Annex-B access unit, after
/// an access unit delimiter if present, otherwise at the start.
pub fn insert_sei(au: &[u8], sei: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(au.len() + sei.len());
    let nals = nal_units(au);
    let insert_at = match nals.first() {
        Some(&(_, begin, end)) if au[begin] & 0x1f == NAL_AUD => end,
        Some(&(sc, _, _)) => sc,
        None => 0,
    };
    out.extend_from_slice(&au[..insert_at]);
    out.extend_from_slice(sei);
    out.extend_from_slice(&au[insert_at..]);
    out
}

/// Extract the Mist capture timestamp from an Annex-B access unit.
pub fn extract_timestamp(au: &[u8]) -> Option<u64> {
    for (_, begin, end) in nal_units(au) {
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
            if ptype == SEI_USER_DATA_UNREGISTERED as usize && size == 24 && payload[..16] == MIST_SEI_UUID {
                return Some(u64::from_be_bytes(payload[16..24].try_into().ok()?));
            }
            p += size;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDR: &[u8] = &[0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 0, 1, 0x68, 4, 0, 0, 1, 0x65, 0x88, 0x84, 0x00];

    #[test]
    fn roundtrip_with_emulation_prevention() {
        for ts in [0u64, 1, 0x0000_0000_0001_0000, 0x0000_0300_0000_0002, u64::MAX, 123_456_789_012] {
            let au = insert_sei(IDR, &build_sei_nal(ts));
            assert_eq!(extract_timestamp(&au), Some(ts), "ts={ts:#x}");
            // Original NALs are untouched.
            assert!(au.ends_with(&IDR[8..]));
        }
    }

    #[test]
    fn sei_never_contains_start_code() {
        let nal = build_sei_nal(0x0000_0000_0000_0001);
        assert!(!nal[4..].windows(3).any(|w| w == [0, 0, 1] || w == [0, 0, 0]));
    }

    #[test]
    fn inserted_after_aud() {
        let mut au = vec![0, 0, 0, 1, 0x09, 0xf0];
        au.extend_from_slice(IDR);
        let out = insert_sei(&au, &build_sei_nal(7));
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
        let out = insert_sei(&au, &build_sei_nal(42));
        assert_eq!(extract_timestamp(&out), Some(42));
    }

    #[test]
    fn garbage_does_not_panic() {
        assert_eq!(extract_timestamp(&[]), None);
        assert_eq!(extract_timestamp(&[0, 0, 1, 6, 5, 0xff, 0xff]), None);
        assert_eq!(extract_timestamp(&[0, 0, 1, 6, 5, 200, 1, 2]), None);
    }
}
