//! Native instruction data of the NTS-1 program for a Jupiter swap.
//! Anchor layout: 8-byte discriminator, then the arguments little-endian.

use jupiter_amm_interface::Swap;

/// Anchor discriminator of `buy(sol_in, min_tokens_out)`.
pub const BUY_DISCRIMINATOR: [u8; 8] = [102, 6, 61, 18, 1, 218, 235, 234];
/// Anchor discriminator of `sell(tokens_in, min_sol_out)`.
pub const SELL_DISCRIMINATOR: [u8; 8] = [51, 230, 133, 164, 1, 127, 131, 173];

/// Decision bit carried in `Swap::Placeholder { data }`: 0 = buy (SOL in), 1 = sell (tokens in).
pub const OP_BUY: u8 = 0;
pub const OP_SELL: u8 = 1;

/// Instruction data for one swap. `min_out` is the slippage floor the caller wants
/// (Jupiter enforces its own minimum at the route level, so 0 is acceptable here).
pub fn encode_swap_with_min(swap: &Swap, in_amount: u64, min_out: u64) -> Vec<u8> {
    let op = match swap {
        Swap::Placeholder { data: Some(d) } if !d.is_empty() => d[0],
        _ => OP_BUY,
    };
    let disc = if op == OP_SELL { SELL_DISCRIMINATOR } else { BUY_DISCRIMINATOR };
    let mut v = Vec::with_capacity(24);
    v.extend_from_slice(&disc);
    v.extend_from_slice(&in_amount.to_le_bytes());
    v.extend_from_slice(&min_out.to_le_bytes());
    v
}

/// The `fn(&Swap, u64) -> Vec<u8>` shape the Jupiter test kit expects.
pub fn encode_swap(swap: &Swap, in_amount: u64) -> Vec<u8> {
    encode_swap_with_min(swap, in_amount, 0)
}
