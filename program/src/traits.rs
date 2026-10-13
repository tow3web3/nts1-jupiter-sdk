//! seed -> traits. Pure, no I/O, mirrored byte for byte in
//! `generator/src/traits.ts`. The seed is the on-chain truth; the image is
//! only a rendering of these indices.
//!
//! Layers (in order): b body, e eyes, m mouth, a accessory, f background.
//! Byte `i` of the seed picks the rarity tier of layer `i` (80 / 17 / 3 %),
//! byte `5 + i` picks the variant inside that tier. Indices are flat per
//! layer: commons first, then rares, then epics, so `b7` is always the second
//! rare body whatever the tier bytes said.

pub const LAYERS: usize = 5;
pub const KEYS: [u8; LAYERS] = *b"bemaf";
/// variants per layer: [common, rare, epic]
pub const COUNTS: [[u8; 3]; LAYERS] = [[6, 3, 1], [5, 2, 1], [5, 2, 1], [6, 3, 1], [4, 1, 1]];
pub const RARE_AT: u8 = 204; // 204..=246 -> rare   (43/256 = 16.8 %)
pub const EPIC_AT: u8 = 247; // 247..=255 -> epic   ( 9/256 =  3.5 %)

pub fn tier(byte: u8) -> usize {
    if byte >= EPIC_AT {
        2
    } else if byte >= RARE_AT {
        1
    } else {
        0
    }
}

pub fn index(layer: usize, seed: &[u8; 32]) -> u8 {
    let t = tier(seed[layer]);
    let counts = COUNTS[layer];
    let offset: u8 = counts[..t].iter().sum();
    offset + seed[LAYERS + layer] % counts[t]
}

/// "b3;e7;m1;a0;f2"
pub fn traits_string(seed: &[u8; 32]) -> String {
    let mut s = String::with_capacity(20);
    for layer in 0..LAYERS {
        if layer > 0 {
            s.push(';');
        }
        s.push(KEYS[layer] as char);
        s.push_str(&index(layer, seed).to_string());
    }
    s
}

/// Special numbers get a readable extra trait. Precedence: first, repdigit,
/// palindrome, round (multiple of 100).
pub fn special(number: u64) -> Option<&'static str> {
    if number == 1 {
        return Some("first");
    }
    let digits: Vec<u8> = number.to_string().bytes().collect();
    if digits.len() >= 2 && digits.iter().all(|d| *d == digits[0]) {
        return Some("repdigit");
    }
    if digits.len() >= 2 && digits.iter().eq(digits.iter().rev()) {
        return Some("palindrome");
    }
    if number % 100 == 0 {
        return Some("round");
    }
    None
}

pub fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 15) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_stay_in_range() {
        for i in 0..=255u8 {
            let mut seed = [i; 32];
            seed[7] = i.wrapping_mul(31);
            for layer in 0..LAYERS {
                let total: u8 = COUNTS[layer].iter().sum();
                assert!(index(layer, &seed) < total);
            }
        }
    }

    #[test]
    fn vector() {
        let mut seed = [0u8; 32];
        seed[0] = 250; // epic body -> index 9
        seed[1] = 210; // rare eyes
        seed[6] = 1; //   second rare eye -> 5 + 1 = 6
        seed[4] = 3; // common background
        seed[9] = 7; //   7 % 4 = 3
        assert_eq!(traits_string(&seed), "b9;e6;m0;a0;f3");
    }

    #[test]
    fn specials() {
        assert_eq!(special(1), Some("first"));
        assert_eq!(special(7), None);
        assert_eq!(special(77), Some("repdigit"));
        assert_eq!(special(121), Some("palindrome"));
        assert_eq!(special(100), Some("round"));
        assert_eq!(special(1001), Some("palindrome"));
        assert_eq!(special(1000), Some("round"));
        assert_eq!(special(412), None);
    }
}
