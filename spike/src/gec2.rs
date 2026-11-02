//! Edge's "v2" `Sec-MS-GEC` token (log entries 12 and 15; the user chose this route as Q13 option 2).
//!
//! Format: `<CS64 as 16 hex>-<SHA-256 as 64 hex>`, both over the same text as v1: the Windows tick count
//! floored to 5 minutes, then the public client token.
//!
//! The SHA-256 is standard except that the message is preceded by a 57-byte secret. Its effect is baked
//! in as constants: the hash state after 14 rounds of the first block (`MID_STATE`), the first block's
//! schedule words that depend only on the secret (`MID_SCHEDULE`), and three adjustments. CS64 is a
//! keyed 64-bit checksum seeded from the digest.
//!
//! The protocol facts and constants come from the `edge-tts` project (rany2/edge-tts, LGPL-3.0), which
//! recovered them from Edge. This is our own implementation, written from the algorithm and checked
//! against the output of `edge-tts` 7.3.1 in the tests below. Expect Microsoft to change it again.

const H0: [u32; 8] = [
    0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
];

const K: [u32; 64] = [
    0x428A2F98, 0x71374491, 0xB5C0FBCF, 0xE9B5DBA5, 0x3956C25B, 0x59F111F1, 0x923F82A4, 0xAB1C5ED5,
    0xD807AA98, 0x12835B01, 0x243185BE, 0x550C7DC3, 0x72BE5D74, 0x80DEB1FE, 0x9BDC06A7, 0xC19BF174,
    0xE49B69C1, 0xEFBE4786, 0x0FC19DC6, 0x240CA1CC, 0x2DE92C6F, 0x4A7484AA, 0x5CB0A9DC, 0x76F988DA,
    0x983E5152, 0xA831C66D, 0xB00327C8, 0xBF597FC7, 0xC6E00BF3, 0xD5A79147, 0x06CA6351, 0x14292967,
    0x27B70A85, 0x2E1B2138, 0x4D2C6DFC, 0x53380D13, 0x650A7354, 0x766A0ABB, 0x81C2C92E, 0x92722C85,
    0xA2BFE8A1, 0xA81A664B, 0xC24B8B70, 0xC76C51A3, 0xD192E819, 0xD6990624, 0xF40E3585, 0x106AA070,
    0x19A4C116, 0x1E376C08, 0x2748774C, 0x34B0BCB5, 0x391C0CB3, 0x4ED8AA4A, 0x5B9CCA4F, 0x682E6FF3,
    0x748F82EE, 0x78A5636F, 0x84C87814, 0x8CC70208, 0x90BEFFFA, 0xA4506CEB, 0xBEF9A3F7, 0xC67178F2,
];

/// State after rounds 0 to 13 of the first block, which hold only secret bytes.
const MID_STATE: [u32; 8] = [
    0xFD9FBEA2, 0x91243CE7, 0xBE499094, 0xCD2E1E0A, 0x6C006F49, 0x04BD35AB, 0xFEB0A7E4, 0x03EFFBD2,
];
/// The secret-only part of schedule words 16 to 29 of the first block.
const MID_SCHEDULE: [u32; 14] = [
    0xEB24BF9E, 0x1558691F, 0xA0C09EE2, 0x0CFA32BA, 0xA622EE8C, 0xD1DD9491, 0xC0DD25A0, 0xF9E70AB6,
    0xF0C31573, 0x7058CE4E, 0x7188E606, 0x00E5AB33, 0xD258B060, 0x72766564,
];
/// Replaces K[14] in the first block, folding in the secret's share of round 14.
const ROUND14: u32 = 0xC9DC06A7;
const SIGMA0_ADJ: u32 = 0x059C0B80;
const SIGMA1_ADJ: u32 = 0x000B92C0;
const SECRET_LEN: usize = 57;

const CS64_COEFFICIENTS: [(u32, u32); 14] = [
    (0xE82E91B1, 0xB32700D0),
    (0x44DE24C6, 0x88DFF51D),
    (0x3BEB65BC, 0x29514038),
    (0xC89C5933, 0xB328CBDF),
    (0x8D1EC211, 0xD405B4D5),
    (0x314EF5D5, 0x26E55F3C),
    (0xE92491B1, 0x607801C6),
    (0xF0A5B9D5, 0x82B2C1FB),
    (0x06064CF3, 0xFA51A3DA),
    (0xDA79FA60, 0x207CC464),
    (0xBED20560, 0x3C3C74F3),
    (0x89F9C9F8, 0xD2D14B61),
    (0x35FA38C6, 0xBB04A2C1),
    (0xCB49E3C6, 0xC5626167),
];
const CS64_FACTOR: u32 = 0x82893933;
const CS64_ADJ0: u32 = 0xF2F90B1A;
const CS64_ADJ1: u32 = 0x0B1A0000;

fn s0(x: u32) -> u32 {
    x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3)
}

fn s1(x: u32) -> u32 {
    x.rotate_right(17) ^ x.rotate_right(19) ^ (x >> 10)
}

fn expand(w: &mut [u32; 64], from: usize) {
    for i in from..64 {
        w[i] = s1(w[i - 2])
            .wrapping_add(w[i - 7])
            .wrapping_add(s0(w[i - 15]))
            .wrapping_add(w[i - 16]);
    }
}

/// SHA-256 rounds `first..64` from `state`, returning the working variables (not yet added in).
fn rounds(state: [u32; 8], w: &[u32; 64], first: usize, k: &[u32; 64]) -> [u32; 8] {
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
    for i in first..64 {
        let t1 = h
            .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
            .wrapping_add(g ^ (e & (f ^ g)))
            .wrapping_add(k[i])
            .wrapping_add(w[i]);
        let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
            .wrapping_add((a & b) | (c & (a | b)));
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    [a, b, c, d, e, f, g, h]
}

fn add(x: [u32; 8], y: [u32; 8]) -> [u32; 8] {
    std::array::from_fn(|i| x[i].wrapping_add(y[i]))
}

/// The first block: the secret's 57 bytes then the message's first 7, resumed at round 14.
fn first_block(m: &[u8]) -> [u32; 8] {
    let mut w = [0u32; 64];
    w[14] = u32::from_be_bytes([0, m[0], m[1], m[2]]);
    w[15] = u32::from_be_bytes([m[3], m[4], m[5], m[6]]);
    for i in 16..30 {
        let mut v = MID_SCHEDULE[i - 16];
        // Terms that touch words 14 and 15 (the message's), with the secret's share adjusted away.
        if i - 2 >= 14 {
            v = v.wrapping_add(s1(w[i - 2]) ^ if i == 16 { SIGMA1_ADJ } else { 0 });
        }
        if i - 7 >= 14 {
            v = v.wrapping_add(w[i - 7]);
        }
        if i >= 29 {
            v = v.wrapping_add(s0(w[i - 15]) ^ if i == 29 { SIGMA0_ADJ } else { 0 });
        }
        w[i] = v;
    }
    w[30] = s1(w[28])
        .wrapping_add(w[23])
        .wrapping_add(s0(w[15]))
        .wrapping_add(w[14])
        .wrapping_add(ROUND14)
        .wrapping_sub(K[14]);
    expand(&mut w, 31);
    let mut k = K;
    k[14] = ROUND14;
    add(H0, rounds(MID_STATE, &w, 14, &k))
}

/// SHA-256 of the secret followed by the message. The message must be at least 7 bytes.
pub fn digest(m: &[u8]) -> [u8; 32] {
    assert!(m.len() >= 7);
    let total = SECRET_LEN + m.len();
    let mut rest = m[7..].to_vec();
    rest.push(0x80);
    while (rest.len() + 8) % 64 != 0 {
        rest.push(0);
    }
    rest.extend_from_slice(&((total as u64) * 8).to_be_bytes());
    let mut state = first_block(m);
    for block in rest.chunks(64) {
        let mut w = [0u32; 64];
        for (i, c) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        expand(&mut w, 16);
        state = add(state, rounds(state, &w, 0, &K));
    }
    let mut out = [0u8; 32];
    for (i, v) in state.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn rol16(x: u32) -> u32 {
    x.rotate_left(16)
}

/// The keyed 64-bit checksum, keyed by the first 8 bytes of the digest.
pub fn cs64(m: &[u8], digest: &[u8; 32]) -> (u32, u32) {
    let key_a = u32::from_le_bytes(digest[0..4].try_into().unwrap()) | 1;
    let key_b = u32::from_le_bytes(digest[4..8].try_into().unwrap()) | 1;
    let factors = [
        key_a.wrapping_mul(CS64_FACTOR),
        key_b.wrapping_mul(CS64_FACTOR),
    ];
    let mut terms: Vec<u32> = CS64_COEFFICIENTS
        .iter()
        .enumerate()
        .map(|(i, c)| c.0.wrapping_mul(factors[i % 2]))
        .collect();
    let mut offsets: Vec<u32> = CS64_COEFFICIENTS.iter().map(|c| c.1).collect();
    // Little-endian words of a zero byte followed by the message, zero-padded to an even word count.
    let mut bytes = vec![0u8];
    bytes.extend_from_slice(m);
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
    let mut words: Vec<u32> = bytes
        .chunks(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    if words.len() % 2 == 1 {
        words.push(0);
    }
    for (i, &word) in words.iter().enumerate() {
        let even = i % 2 == 0;
        let mut term = word.wrapping_mul(if even { key_a } else { key_b });
        let mut offset = rol16(word).wrapping_mul(if even { 0xEF0569FB } else { 0xC31713DB });
        if i == 0 {
            term = term.wrapping_add(CS64_ADJ0.wrapping_mul(factors[0]));
            offset = offset.wrapping_add(CS64_ADJ1);
        }
        terms.push(term);
        offsets.push(offset);
    }
    let (mut swap_acc, mut swap_last, mut state, mut acc) = (0u32, 0u32, 0u32, 0u32);
    for i in (0..terms.len()).step_by(2) {
        let (x, y) = (terms[i], terms[i + 1]);
        let q = rol16(offsets[i].wrapping_add(x)).wrapping_mul(0x76F50F34);
        swap_acc = swap_acc.wrapping_add(rol16(q).wrapping_mul(0xC3EFEA97).wrapping_add(q));
        let q = rol16(offsets[i + 1].wrapping_add(y)).wrapping_mul(0x1881CE3C);
        swap_last = rol16(q).wrapping_mul(0x35BD1EC9).wrapping_add(q);
        swap_acc = swap_acc.wrapping_add(swap_last);
        state = state.wrapping_mul(key_a).wrapping_add(x);
        for c in [0xCF98B111u32, 0x87085B9F, 0x12CEB96D, 0x257E1D83] {
            state = rol16(state).wrapping_mul(c);
        }
        acc = acc.wrapping_add(state);
        let mut t = state.wrapping_mul(key_b).wrapping_add(y);
        t = rol16(t).wrapping_mul(0xA27416F5);
        t = rol16(t).wrapping_mul(0xD38396FF);
        state = rol16(rol16(t).wrapping_mul(0x7C932B89)).wrapping_mul(0xBFA49F69);
        acc = acc.wrapping_add(state);
    }
    (swap_last ^ state, swap_acc ^ acc)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// The token for a message (ticks then client token).
pub fn token_for(m: &[u8]) -> String {
    let d = digest(m);
    let (a, b) = cs64(m, &d);
    let mut c = a.to_le_bytes().to_vec();
    c.extend_from_slice(&b.to_le_bytes());
    format!("{}-{}", hex(&c), hex(&d))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected values: edge-tts 7.3.1, generate_sec_ms_gec_v2 and _token_hash (log entry 15).
    #[test]
    fn matches_edge_tts_for_raw_messages() {
        assert_eq!(
            token_for(b"1234567"),
            "CF8264F204E37578-4B4B1B27BCB09F84AE904B387373DA006A64E1B4EFCC1BAF1D6F289D44E6DAC8"
        );
        let mut long = b"1330000000000000006A5AA1D4EAFF4E9FB37E23D68491D6F4".to_vec();
        long.extend_from_slice(&[b'x'; 40]);
        assert_eq!(
            token_for(&long),
            "0488575F99AF2917-4F42B1034BA5B3F927E48D2C834B3E65AB7A347B306F1CABEE2BEE243B803B05"
        );
    }

    #[test]
    fn matches_edge_tts_for_times() {
        for (t, want) in [
            (
                0u64,
                "52BC7A17B50DFEC9-28C4394775C8751CD2F6859C030C06DB1489ACAB3B3A7651831FC26895177ED6",
            ),
            (
                1_700_000_000,
                "2CD23A2CA0B2595A-F3C846C370E05F9CC83ACC5F995C5F5D87A4D0F14CE8936A0F902E64099E38A2",
            ),
            (
                1_792_933_509,
                "59CB024CA1278E90-0239A1E0C179AF97506ABB4918DF8CC67C153C5FBCF07EE5551BA639031AB551",
            ),
            (
                1_800_000_123,
                "99E54519B490BE5C-E162DB0977FBDCB9B86DD8148BF1BCDC3C0303176CE39852A42B415935C20654",
            ),
        ] {
            assert_eq!(crate::edge::gec_v2(t), want, "at {t}");
        }
    }
}
