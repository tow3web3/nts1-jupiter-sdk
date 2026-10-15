//! Jupiter AMM adapter for NTS-1 bonding curves.
//!
//! Every NTS-1 coin is a Token-2022 mint with its own permanent constant-product
//! curve (program `EvH7PZ34bGKi7SyiUP37G4o3B36XXhPpkWCxhfiMu9AN`). One `Curve`
//! account per coin is the pool: it holds the SOL reserve as native lamports and
//! the unsold supply sits in a `stock` token account owned by the program's
//! `config` PDA. Swaps are `buy(sol_in, min_tokens_out)` and
//! `sell(tokens_in, min_sol_out)`, both exact-in, 1 % fee on the SOL side.
//!
//! Native SOL: the program debits lamports from the signer on `buy` and credits
//! lamports to the signer on `sell`; it does not read a wrapped-SOL token account.
//! Jupiter integrates such programs with a "wrapped" swap variant on its side
//! (as for Pump, Moonshot, Boop). Until a variant exists this adapter uses
//! `Swap::Placeholder { data: [op] }` with op 0 = buy, 1 = sell.
//!
//! No network calls here: the curve, the config (treasury) and the stock account
//! are delivered through `update`.

pub mod curve;
pub mod encode;

use anyhow::{anyhow, Result};
use solana_account::ReadableAccount;
use jupiter_amm_interface::{
    AccountProvider, Amm, AmmContext, AmmError, AmmLabel, KeyedAccount, Quote, QuoteParams, Swap, SwapAndAccountMetas, SwapMode, SwapParams,
};
use rust_decimal::Decimal;
use solana_instruction::AccountMeta;
use solana_pubkey::{pubkey, Pubkey};

use curve::{fee_of, sol_out, tokens_out, CurveState};
use encode::{OP_BUY, OP_SELL};

pub const PROGRAM_ID: Pubkey = pubkey!("EvH7PZ34bGKi7SyiUP37G4o3B36XXhPpkWCxhfiMu9AN");
pub const WSOL: Pubkey = pubkey!("So11111111111111111111111111111111111111112");
pub const TOKEN_2022: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
pub const ASSOCIATED_TOKEN: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
pub const SYSTEM_PROGRAM: Pubkey = pubkey!("11111111111111111111111111111111");

pub const LABEL: AmmLabel = "NTS-1";

/// Anchor discriminator of the `Curve` account.
pub const CURVE_DISCRIMINATOR: [u8; 8] = [191, 180, 249, 66, 180, 71, 51, 182];
/// Anchor discriminator of the `Config` account.
pub const CONFIG_DISCRIMINATOR: [u8; 8] = [155, 12, 170, 224, 30, 250, 204, 130];

/// Token decimals of every NTS-1 coin.
pub const DECIMALS: u8 = 9;

/// One bonding curve = one Jupiter market.
#[derive(Clone, Debug)]
pub struct Nts1Amm {
    key: Pubkey,
    mint: Pubkey,
    group_mint: Pubkey,
    group: Pubkey,
    config: Pubkey,
    stock: Pubkey,
    state: CurveState,
    fee_bps: u16,
    /// read from the config account on update
    treasury: Pubkey,
    /// unsold supply in the stock account, caps a buy
    stock_amount: u64,
}

fn u64_at(data: &[u8], off: usize) -> Result<u64> {
    let b: [u8; 8] = data.get(off..off + 8).ok_or_else(|| anyhow!("short account data"))?.try_into()?;
    Ok(u64::from_le_bytes(b))
}

fn u16_at(data: &[u8], off: usize) -> Result<u16> {
    let b: [u8; 2] = data.get(off..off + 2).ok_or_else(|| anyhow!("short account data"))?.try_into()?;
    Ok(u16::from_le_bytes(b))
}

fn pubkey_at(data: &[u8], off: usize) -> Result<Pubkey> {
    let b: [u8; 32] = data.get(off..off + 32).ok_or_else(|| anyhow!("short account data"))?.try_into()?;
    Ok(Pubkey::new_from_array(b))
}

/// Parsed `Curve` account (Anchor, see `programs/launchpad/src/state.rs`).
#[derive(Clone, Debug)]
pub struct CurveAccount {
    pub mint: Pubkey,
    pub group: Pubkey,
    pub state: CurveState,
    pub fee_bps: u16,
    pub owner_bps: u16,
    pub creator_bps: u16,
}

impl CurveAccount {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 8 || data[..8] != CURVE_DISCRIMINATOR {
            return Err(anyhow!("not an NTS-1 Curve account"));
        }
        // 8 disc | mint 32 | group 32 | virtual_sol 8 | virtual_tokens 8 | reserve 8 | sold 8 | trades 8 | volume 8 | fee_bps 2 | owner_bps 2 | creator_bps 2 | creator_fees 8 | bump 1
        Ok(Self {
            mint: pubkey_at(data, 8)?,
            group: pubkey_at(data, 40)?,
            state: CurveState {
                virtual_sol: u64_at(data, 72)?,
                virtual_tokens: u64_at(data, 80)?,
                reserve: u64_at(data, 88)?,
                sold: u64_at(data, 96)?,
            },
            fee_bps: u16_at(data, 120)?,
            owner_bps: u16_at(data, 122)?,
            creator_bps: u16_at(data, 124)?,
        })
    }
}

/// `treasury` field of the `Config` account: 8 disc | admin 32 | payer 32 | treasury 32 ...
pub fn config_treasury(data: &[u8]) -> Result<Pubkey> {
    if data.len() < 8 || data[..8] != CONFIG_DISCRIMINATOR {
        return Err(anyhow!("not an NTS-1 Config account"));
    }
    pubkey_at(data, 72)
}

/// `amount` of an SPL / Token-2022 token account (offset 64).
pub fn token_amount(data: &[u8]) -> Result<u64> {
    u64_at(data, 64)
}

fn custom(e: anyhow::Error) -> AmmError {
    AmmError::from(e.to_string())
}

pub fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"config"], &PROGRAM_ID).0
}
pub fn curve_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"curve", mint.as_ref()], &PROGRAM_ID).0
}
pub fn stock_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"stock", mint.as_ref()], &PROGRAM_ID).0
}
pub fn group_pda(group_mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"group", group_mint.as_ref()], &PROGRAM_ID).0
}

impl Nts1Amm {
    pub fn mint(&self) -> Pubkey {
        self.mint
    }
    /// The collection (group) mint this coin belongs to.
    pub fn group_mint(&self) -> Pubkey {
        self.group_mint
    }
    pub fn state(&self) -> CurveState {
        self.state
    }
    pub fn fee_bps(&self) -> u16 {
        self.fee_bps
    }
    pub fn treasury(&self) -> Pubkey {
        self.treasury
    }
    pub fn stock_amount(&self) -> u64 {
        self.stock_amount
    }

    /// Quote a buy: `sol_in` lamports (fee included) -> tokens.
    pub fn quote_buy(&self, sol_in: u64) -> Result<Quote> {
        if sol_in == 0 {
            return Err(anyhow!("amount must be > 0"));
        }
        let fee = fee_of(sol_in, self.fee_bps);
        let net = sol_in - fee;
        let out = tokens_out(&self.state, net).ok_or_else(|| anyhow!("math overflow"))?;
        if out == 0 {
            return Err(anyhow!("amount too small"));
        }
        if out > self.stock_amount {
            return Err(anyhow!("sold out: {} tokens left", self.stock_amount));
        }
        Ok(Quote { in_amount: sol_in, out_amount: out, fee_amount: fee, fee_mint: WSOL, fee_pct: self.fee_pct() })
    }

    /// Quote a sell: `tokens_in` base units -> lamports (fee deducted).
    pub fn quote_sell(&self, tokens_in: u64) -> Result<Quote> {
        if tokens_in == 0 {
            return Err(anyhow!("amount must be > 0"));
        }
        let gross = sol_out(&self.state, tokens_in).ok_or_else(|| anyhow!("math overflow"))?;
        if gross > self.state.reserve {
            return Err(anyhow!("reserve too small"));
        }
        let fee = fee_of(gross, self.fee_bps);
        let out = gross - fee;
        Ok(Quote { in_amount: tokens_in, out_amount: out, fee_amount: fee, fee_mint: WSOL, fee_pct: self.fee_pct() })
    }

    fn fee_pct(&self) -> Decimal {
        Decimal::new(self.fee_bps as i64, 4)
    }
}

impl Amm for Nts1Amm {
    fn from_keyed_account(keyed_account: &KeyedAccount, _amm_context: &AmmContext) -> Result<Self, AmmError>
    where
        Self: Sized,
    {
        let c = CurveAccount::parse(&keyed_account.account.data).map_err(custom)?;
        Ok(Self {
            key: keyed_account.key,
            mint: c.mint,
            group_mint: c.group,
            group: group_pda(&c.group),
            config: config_pda(),
            stock: stock_pda(&c.mint),
            state: c.state,
            fee_bps: c.fee_bps,
            treasury: Pubkey::default(),
            stock_amount: 0,
        })
    }

    fn label(&self) -> AmmLabel {
        LABEL
    }

    fn program_id(&self) -> Pubkey {
        PROGRAM_ID
    }

    fn key(&self) -> Pubkey {
        self.key
    }

    fn get_reserve_mints(&self) -> Vec<Pubkey> {
        vec![WSOL, self.mint]
    }

    fn get_accounts_to_update(&self) -> Vec<Pubkey> {
        vec![self.key, self.config, self.stock]
    }

    fn update(&mut self, account_provider: impl AccountProvider) -> Result<(), AmmError> {
        let curve = account_provider.try_get(&self.key)?;
        let c = CurveAccount::parse(curve.data()).map_err(custom)?;
        self.state = c.state;
        self.fee_bps = c.fee_bps;
        if let Some(cfg) = account_provider.get(&self.config) {
            self.treasury = config_treasury(cfg.data()).map_err(custom)?;
        }
        if let Some(stock) = account_provider.get(&self.stock) {
            self.stock_amount = token_amount(stock.data()).map_err(custom)?;
        }
        Ok(())
    }

    fn quote(&self, quote_params: &QuoteParams) -> Result<Quote, AmmError> {
        if quote_params.swap_mode != SwapMode::ExactIn {
            return Err(AmmError::from("ExactOut not supported"));
        }
        let q = if quote_params.input_mint == WSOL && quote_params.output_mint == self.mint {
            self.quote_buy(quote_params.amount).map_err(custom)?
        } else if quote_params.input_mint == self.mint && quote_params.output_mint == WSOL {
            self.quote_sell(quote_params.amount).map_err(custom)?
        } else {
            return Err(AmmError::from("unknown mint pair"));
        };
        Ok(q)
    }

    fn get_swap_and_account_metas(&self, swap_params: &SwapParams) -> Result<SwapAndAccountMetas, AmmError> {
        let (op, trader_ata) = if swap_params.source_mint == WSOL {
            (OP_BUY, swap_params.destination_token_account)
        } else if swap_params.destination_mint == WSOL {
            (OP_SELL, swap_params.source_token_account)
        } else {
            return Err(AmmError::from("unknown mint pair"));
        };
        if self.treasury == Pubkey::default() {
            return Err(AmmError::from("treasury unknown, update first"));
        }
        // on-chain order of the `Trade` accounts
        let account_metas = vec![
            AccountMeta::new(swap_params.token_transfer_authority, true), // buyer (signer, pays / receives lamports)
            AccountMeta::new_readonly(self.config, false),
            AccountMeta::new(self.treasury, false),
            AccountMeta::new(self.group, false),
            AccountMeta::new(self.key, false),
            AccountMeta::new_readonly(self.mint, false),
            AccountMeta::new(self.stock, false),
            AccountMeta::new(trader_ata, false),
            AccountMeta::new_readonly(TOKEN_2022, false),
            AccountMeta::new_readonly(ASSOCIATED_TOKEN, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ];
        Ok(SwapAndAccountMetas { swap: Swap::Placeholder { data: Some(vec![op]) }, account_metas })
    }

    fn program_dependencies(&self) -> Vec<(Pubkey, String)> {
        vec![(TOKEN_2022, "token_2022".to_string()), (ASSOCIATED_TOKEN, "associated_token".to_string())]
    }

    fn get_accounts_len(&self) -> usize {
        11
    }

    fn supports_exact_out(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_account::Account;
    use std::collections::HashMap;

    fn curve_bytes(reserve: u64, sold: u64) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&CURVE_DISCRIMINATOR);
        d.extend_from_slice(Pubkey::new_unique().as_ref()); // mint
        d.extend_from_slice(Pubkey::new_unique().as_ref()); // group
        d.extend_from_slice(&30_000_000_000u64.to_le_bytes());
        d.extend_from_slice(&1_073_000_000_000_000_000u64.to_le_bytes());
        d.extend_from_slice(&reserve.to_le_bytes());
        d.extend_from_slice(&sold.to_le_bytes());
        d.extend_from_slice(&0u64.to_le_bytes()); // trades
        d.extend_from_slice(&0u64.to_le_bytes()); // volume
        d.extend_from_slice(&100u16.to_le_bytes()); // fee 1 %
        d.extend_from_slice(&30u16.to_le_bytes());
        d.extend_from_slice(&30u16.to_le_bytes());
        d.extend_from_slice(&0u64.to_le_bytes()); // creator_fees
        d.push(255);
        d
    }

    fn amm(reserve: u64, sold: u64, stock: u64) -> Nts1Amm {
        let key = Pubkey::new_unique();
        let ka = KeyedAccount { key, account: Account { lamports: 1, data: curve_bytes(reserve, sold), owner: PROGRAM_ID, executable: false, rent_epoch: 0 }, params: None };
        let mut a = Nts1Amm::from_keyed_account(&ka, &AmmContext::default()).unwrap();
        let mut cfg = Vec::new();
        cfg.extend_from_slice(&CONFIG_DISCRIMINATOR);
        cfg.extend_from_slice(Pubkey::new_unique().as_ref());
        cfg.extend_from_slice(Pubkey::new_unique().as_ref());
        cfg.extend_from_slice(Pubkey::new_unique().as_ref()); // treasury
        let mut st = vec![0u8; 165];
        st[64..72].copy_from_slice(&stock.to_le_bytes());
        let mut map: HashMap<Pubkey, Box<Account>> = HashMap::new();
        map.insert(key, Box::new(Account { lamports: 1, data: curve_bytes(reserve, sold), owner: PROGRAM_ID, executable: false, rent_epoch: 0 }));
        map.insert(a.config, Box::new(Account { lamports: 1, data: cfg, owner: PROGRAM_ID, executable: false, rent_epoch: 0 }));
        map.insert(a.stock, Box::new(Account { lamports: 1, data: st, owner: TOKEN_2022, executable: false, rent_epoch: 0 }));
        a.update(&map).unwrap();
        a
    }

    #[test]
    fn buy_quote_matches_program_math() {
        let a = amm(0, 0, 1_000_000_000_000_000_000);
        let q = a.quote(&QuoteParams { amount: 1_000_000_000, input_mint: WSOL, output_mint: a.mint, swap_mode: SwapMode::ExactIn, fee_mode: Default::default() }).unwrap();
        assert_eq!(q.fee_amount, 10_000_000);
        let expect = tokens_out(&a.state, 990_000_000).unwrap();
        assert_eq!(q.out_amount, expect);
        assert_eq!(q.fee_mint, WSOL);
    }

    #[test]
    fn sell_quote_deducts_fee_from_sol() {
        let mut a = amm(0, 0, 1_000_000_000_000_000_000);
        // simulate a prior buy of 1 SOL net
        let bought = tokens_out(&a.state, 1_000_000_000).unwrap();
        a.state.reserve = 1_000_000_000;
        a.state.sold = bought;
        let q = a.quote(&QuoteParams { amount: bought, input_mint: a.mint, output_mint: WSOL, swap_mode: SwapMode::ExactIn, fee_mode: Default::default() }).unwrap();
        let gross = sol_out(&a.state, bought).unwrap();
        assert_eq!(q.out_amount, gross - fee_of(gross, 100));
        assert!(q.out_amount < 1_000_000_000);
    }

    #[test]
    fn buy_is_capped_by_stock() {
        let a = amm(0, 0, 1_000);
        let r = a.quote(&QuoteParams { amount: 1_000_000_000, input_mint: WSOL, output_mint: a.mint, swap_mode: SwapMode::ExactIn, fee_mode: Default::default() });
        assert!(r.is_err());
    }

    #[test]
    fn account_metas_follow_the_program_order() {
        let a = amm(0, 0, 1_000_000_000_000_000_000);
        let user = Pubkey::new_unique();
        let src = Pubkey::new_unique();
        let dst = Pubkey::new_unique();
        let jup = Pubkey::new_unique();
        let p = SwapParams {
            swap_mode: SwapMode::ExactIn,
            in_amount: 1,
            out_amount: 1,
            source_mint: WSOL,
            destination_mint: a.mint,
            source_token_account: src,
            destination_token_account: dst,
            token_transfer_authority: user,
            user,
            payer: user,
            quote_mint_to_referrer: None,
            jupiter_program_id: &jup,
            missing_dynamic_accounts_as_default: false,
        };
        let s = a.get_swap_and_account_metas(&p).unwrap();
        assert_eq!(s.account_metas.len(), 11);
        assert_eq!(s.account_metas[0].pubkey, user);
        assert!(s.account_metas[0].is_signer);
        assert_eq!(s.account_metas[7].pubkey, dst);
        assert!(matches!(s.swap, Swap::Placeholder { data: Some(ref d) } if d[0] == OP_BUY));
        let d = encode::encode_swap(&s.swap, 5);
        assert_eq!(&d[..8], &encode::BUY_DISCRIMINATOR);
        assert_eq!(u64::from_le_bytes(d[8..16].try_into().unwrap()), 5);
    }
}
