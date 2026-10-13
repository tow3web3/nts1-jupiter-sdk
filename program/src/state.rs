use anchor_lang::prelude::*;

pub const MAX_PREFIX: usize = 20;
pub const MAX_NAME: usize = 32;
pub const MAX_SYMBOL: usize = 10;
pub const MAX_URI_BASE: usize = 96;
pub const MAX_NICKNAME: usize = 24;

/// One per deployment. Also the signing authority (mint + update authority)
/// of every mint the program creates: seeds ["config"].
#[account]
#[derive(InitSpace)]
pub struct Config {
    pub admin: Pubkey,
    /// Backend hot wallet allowed to launch on a group's budget.
    pub payer: Pubkey,
    /// Receives the platform share of every fee.
    pub treasury: Pubkey,
    pub root_mint: Pubkey,
    pub fee_bps: u16,
    pub owner_bps: u16,
    pub creator_bps: u16,
    pub virtual_sol: u64,
    pub virtual_tokens: u64,
    pub supply: u64,
    pub claim_ttl: i64,
    pub groups: u64,
    pub launches: u64,
    #[max_len(MAX_URI_BASE)]
    pub uri_base: String,
    pub bump: u8,
}

/// Mirror of a Token-2022 group mint (root or sub-launchpad) plus what the
/// token program cannot hold: owner, budget, fees, theme.
#[account]
#[derive(InitSpace)]
pub struct Group {
    pub mint: Pubkey,
    pub owner: Pubkey,
    /// Pubkey::default() for the root.
    pub parent: Pubkey,
    /// member_number of this group inside its parent (0 for the root).
    pub number: u64,
    pub max_size: u64,
    /// Mirror of TokenGroup.size: members ever created (launches + sub-groups).
    pub size: u64,
    pub expired: u64,
    /// Lamports held here to reimburse the backend payer's rent on launches.
    pub budget: u64,
    /// Lamports owed to the owner (their share of trading fees).
    pub owner_fees: u64,
    pub theme: u8,
    /// Anyone may launch here (self-paid or through the backend).
    pub open_launch: bool,
    #[max_len(MAX_NAME)]
    pub name: String,
    /// Member names are "{prefix} #{number}".
    #[max_len(MAX_PREFIX)]
    pub prefix: String,
    #[max_len(MAX_SYMBOL)]
    pub symbol: String,
    pub bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum LaunchStatus {
    Pending,
    Claimed,
    Expired,
}

#[account]
#[derive(InitSpace)]
pub struct Launch {
    pub group: Pubkey,
    pub mint: Pubkey,
    pub number: u64,
    pub seed: [u8; 32],
    pub claim_hash: [u8; 32],
    pub creator: Pubkey,
    pub created_at: i64,
    pub claimed_at: i64,
    pub status: LaunchStatus,
    /// The creator may set a nickname once (at claim or later).
    pub nickname_set: bool,
    pub bump: u8,
}

/// Holds the SOL reserve (as its own lamports) and the curve parameters,
/// copied from Config at launch so a curve never changes after birth.
#[account]
#[derive(InitSpace)]
pub struct Curve {
    pub mint: Pubkey,
    pub group: Pubkey,
    pub virtual_sol: u64,
    pub virtual_tokens: u64,
    pub reserve: u64,
    pub sold: u64,
    pub trades: u64,
    pub volume: u64,
    pub fee_bps: u16,
    pub owner_bps: u16,
    pub creator_bps: u16,
    /// Creator share of fees, kept here until the creator withdraws (after claim).
    pub creator_fees: u64,
    pub bump: u8,
}
