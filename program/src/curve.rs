//! Constant-product bonding curve with virtual reserves (the pump.fun shape).
//!
//! x = virtual_sol + reserve (lamports), y = virtual_tokens - sold (base units),
//! k = x * y is invariant. Every rounding goes in the pool's favour so k never
//! shrinks. The same functions exist in TypeScript (`generator/../sdk/curve.ts`)
//! and both are checked against the vectors in `tests/vectors/curve.json`.

pub const BPS: u128 = 10_000;

#[derive(Clone, Copy, Debug)]
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

/// Net lamports needed to receive exactly `tokens` (quote helper).
pub fn sol_in_for(s: &CurveState, tokens: u64) -> Option<u64> {
    let (x, y) = s.xy()?;
    let k = x.checked_mul(y)?;
    let y2 = y.checked_sub(tokens as u128)?;
    if y2 == 0 {
        return None;
    }
    let x2 = k.div_ceil(y2);
    u64::try_from(x2.checked_sub(x)?).ok()
}

/// Spot price in lamports per whole token (1e9 base units), scaled by 1e9.
pub fn price_e9(s: &CurveState, decimals: u8) -> Option<u128> {
    let (x, y) = s.xy()?;
    let one = 10u128.pow(decimals as u32);
    x.checked_mul(one)?.checked_mul(1_000_000_000)?.checked_div(y)
}

pub fn fee_of(amount: u64, fee_bps: u16) -> u64 {
    ((amount as u128) * (fee_bps as u128) / BPS) as u64
}

/// Splits a fee into (platform, owner, creator). Platform takes the dust.
pub fn split(fee: u64, fee_bps: u16, owner_bps: u16, creator_bps: u16) -> (u64, u64, u64) {
    if fee_bps == 0 {
        return (fee, 0, 0);
    }
    let creator = ((fee as u128) * (creator_bps as u128) / (fee_bps as u128)) as u64;
    let owner = ((fee as u128) * (owner_bps as u128) / (fee_bps as u128)) as u64;
    (fee - creator - owner, owner, creator)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pump() -> CurveState {
        CurveState {
            virtual_sol: 30_000_000_000,
            virtual_tokens: 1_073_000_000_000_000_000,
            reserve: 0,
            sold: 0,
        }
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
    fn quote_matches_execution() {
        let s = pump();
        let out = tokens_out(&s, 2_500_000_000).unwrap();
        let need = sol_in_for(&s, out).unwrap();
        assert!(need <= 2_500_000_000);
        assert!(tokens_out(&s, need).unwrap() >= out);
    }

    #[test]
    fn whole_supply_costs_about_411_sol() {
        let s = pump();
        let need = sol_in_for(&s, 1_000_000_000_000_000_000).unwrap();
        assert!(need > 410_000_000_000 && need < 412_000_000_000, "{need}");
    }

    #[test]
    fn split_sums() {
        let (p, o, c) = split(1_000_003, 100, 30, 30);
        assert_eq!(p + o + c, 1_000_003);
        assert_eq!(o, 300_000);
        assert_eq!(c, 300_000);
    }
}
