//! Constant-product bonding curve with virtual reserves, byte for byte the
//! math of `programs/launchpad/src/curve.rs`. Every rounding goes in the
//! pool's favour so k never shrinks.

pub const BPS: u128 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurveState {
    pub virtual_sol: u64,
    pub virtual_tokens: u64,
    pub reserve: u64,
    pub sold: u64,
}

impl CurveState {
    fn xy(&self) -> Option<(u128, u128)> {
        let x = (self.virtual_sol as u128).checked_add(self.reserve as u128)?;
        let y = (self.virtual_tokens as u128).checked_sub(self.sold as u128)?;
        Some((x, y))
    }
}

/// Tokens received for `sol_net` lamports (fee already removed).
pub fn tokens_out(s: &CurveState, sol_net: u64) -> Option<u64> {
    let (x, y) = s.xy()?;
    let k = x.checked_mul(y)?;
    let x2 = x.checked_add(sol_net as u128)?;
    let y2 = k.div_ceil(x2);
    let out = y.checked_sub(y2)?;
    u64::try_from(out).ok()
}

/// Lamports returned (before fee) for selling `tokens` base units.
pub fn sol_out(s: &CurveState, tokens: u64) -> Option<u64> {
    let (x, y) = s.xy()?;
    let k = x.checked_mul(y)?;
    let y2 = y.checked_add(tokens as u128)?;
    let x2 = k.div_ceil(y2);
    let out = x.checked_sub(x2)?;
    u64::try_from(out).ok()
}

pub fn fee_of(amount: u64, fee_bps: u16) -> u64 {
    ((amount as u128) * (fee_bps as u128) / BPS) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pump() -> CurveState {
        CurveState { virtual_sol: 30_000_000_000, virtual_tokens: 1_073_000_000_000_000_000, reserve: 0, sold: 0 }
    }

    #[test]
    fn roundtrip_never_pays_more_than_in() {
        let mut s = pump();
        let sol = 1_000_000_000;
        let out = tokens_out(&s, sol).unwrap();
        s.reserve += sol;
        s.sold += out;
        let back = sol_out(&s, out).unwrap();
        assert!(back <= sol);
        assert!(sol - back < 10);
    }

    #[test]
    fn first_sol_buys_about_35m_tokens() {
        // 30 SOL virtual, 1.073B virtual tokens: 1 SOL -> ~34.6M tokens
        let out = tokens_out(&pump(), 1_000_000_000).unwrap();
        assert!(out > 34_000_000_000_000_000 && out < 35_000_000_000_000_000, "{out}");
    }
}
