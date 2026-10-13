//! PADPADPAD
//!
//! A launchpad of numbered monsters. Every monster is a fungible Token-2022
//! mint with its own bonding curve, and a member of a capped collection
//! (a Token-2022 group). The number is the `member_number` the token program
//! assigns when the member joins the group: no counter of our own, no race,
//! and the cap is refused by Token-2022 itself (`size == max_size`).
//!
//! The platform is the root group. Anyone can create a sub-launchpad: a group
//! mint that is itself a member of the root, with its own cap, theme and
//! owner. Depth is 2 (root -> group -> monsters).
//!
//! Authorities: the `Config` PDA is mint authority and update authority of
//! every mint and group the program creates. The backend never holds any; it
//! only signs `launch` with the payer wallet and gets its rent back from the
//! group's budget.
//!
//! No transfer hook, no graduation: the curve is permanent and fees are taken
//! on the curve only (see docs/adr).

use anchor_lang::prelude::*;
use solana_sha256_hasher::hashv;
use anchor_lang::system_program;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token_2022::spl_token_2022::extension::{BaseStateWithExtensions, StateWithExtensions};
use anchor_spl::token_2022::spl_token_2022::state::Mint as MintState;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_2022_extensions::spl_pod::optional_keys::OptionalNonZeroPubkey;
use spl_token_group_interface::state::TokenGroupMember;
use anchor_spl::token_2022_extensions::spl_token_metadata_interface::state::{Field, TokenMetadata};
use anchor_spl::token_2022_extensions::{
    token_group_initialize, token_member_initialize, token_metadata_initialize,
    token_metadata_update_field, TokenGroupInitialize, TokenMemberInitialize,
    TokenMetadataInitialize, TokenMetadataUpdateField,
};
use anchor_spl::token_interface::{
    self, Burn, CloseAccount, Mint, MintTo, TokenAccount, TransferChecked,
};

pub mod curve;
pub mod errors;
pub mod state;
pub mod traits;

use errors::PadError;
use state::*;

declare_id!("5ZmEp1vQj45R3itxEo6hgH8aX4E8UNCkRsTw13gjchco");

pub const TOKEN_DECIMALS: u8 = 9;
pub const MIN_GROUP_SIZE: u64 = 10;
pub const MAX_GROUP_SIZE: u64 = 10_000;
pub const MAX_FEE_BPS: u16 = 500;
/// Encoded custom style (see generator/src/style.ts); must fit in one create_group transaction.
pub const MAX_STYLE: usize = 640;
/// The standard every mint declares in its metadata ("standard" field).
pub const STANDARD: &str = "nts-1";
pub const MAX_DESCRIPTION: usize = 200;
pub const MAX_URL: usize = 96;
pub const MAX_PROMPT: usize = 64;
/// GroupArgs.naming: members are "Prefix #042" and nothing else.
pub const NAMING_FIXED: u8 = 0;
/// GroupArgs.naming: the claimer may engrave one nickname.
pub const NAMING_NICKNAME: u8 = 1;
/// TLV bytes Token-2022 appends to a mint for each extension (4-byte header).
const MEMBER_TLV: usize = 4 + 32 + 32 + 8;
const GROUP_TLV: usize = 4 + 32 + 32 + 8 + 8;

#[program]
pub mod launchpad {
    use super::*;

    /// Creates Config and the root group mint. The root is a group like any
    /// other (owner = admin); sub-launchpads and direct monsters are its members.
    pub fn init_config(ctx: Context<InitConfig>, p: ConfigParams, root: GroupArgs) -> Result<()> {
        p.check()?;
        root.check()?;
        let c = &mut ctx.accounts.config;
        c.admin = ctx.accounts.admin.key();
        c.root_mint = ctx.accounts.root_mint.key();
        c.bump = ctx.bumps.config;
        p.apply(c);
        let bump = [c.bump];
        let seeds: &[&[u8]] = &[b"config", &bump];
        let signer = &[seeds];

        let tp = ctx.accounts.token_program.to_account_info();
        let mint = ctx.accounts.root_mint.to_account_info();
        let config = ctx.accounts.config.to_account_info();
        let payer = ctx.accounts.admin.to_account_info();
        let sys = ctx.accounts.system_program.to_account_info();

        let uri = member_uri(&ctx.accounts.config.uri_base, &mint.key());
        let fields = vec![
            ("standard".to_string(), STANDARD.to_string()),
            ("kind".to_string(), "root".to_string()),
            ("max".to_string(), root.max_size.to_string()),
            ("theme".to_string(), root.theme.to_string()),
            ("naming".to_string(), naming_label(root.naming).to_string()),
            ("prompt".to_string(), if root.minter_prompt { "1" } else { "0" }.to_string()),
        ];
        let meta_len = metadata_len(&config.key(), &mint.key(), &root.name, &root.symbol, &uri, &fields)?;
        top_up(&payer, &mint, &sys, mint.data_len() + GROUP_TLV + meta_len)?;

        token_group_initialize(
            CpiContext::new_with_signer(
                tp.clone(),
                TokenGroupInitialize { program_id: tp.clone(), group: mint.clone(), mint: mint.clone(), mint_authority: config.clone() },
                signer,
            ),
            Some(config.key()),
            root.max_size,
        )?;
        init_metadata(&tp, &mint, &config, signer, &root.name, &root.symbol, &uri, &fields)?;

        let g = &mut ctx.accounts.root_group;
        g.mint = mint.key();
        g.owner = ctx.accounts.admin.key();
        g.parent = Pubkey::default();
        g.number = 0;
        g.max_size = root.max_size;
        g.theme = root.theme;
        g.open_launch = root.open_launch;
        g.name = root.name.clone();
        g.prefix = root.prefix.clone();
        g.symbol = root.symbol.clone();
        g.bump = ctx.bumps.root_group;
        Ok(())
    }

    pub fn update_config(ctx: Context<UpdateConfig>, p: ConfigParams, new_admin: Pubkey) -> Result<()> {
        p.check()?;
        let c = &mut ctx.accounts.config;
        p.apply(c);
        c.admin = new_admin;
        Ok(())
    }

    /// A sub-launchpad: a group mint (cap, theme, owner) that is itself a
    /// numbered member of the root. The creator pays the rent and may prefund
    /// the launch budget in the same transaction.
    pub fn create_group(ctx: Context<CreateGroup>, a: GroupArgs, budget: u64) -> Result<()> {
        a.check()?;
        let config = &ctx.accounts.config;
        let bump = [config.bump];
        let seeds: &[&[u8]] = &[b"config", &bump];
        let signer = &[seeds];

        let tp = ctx.accounts.token_program.to_account_info();
        let mint = ctx.accounts.mint.to_account_info();
        let root_mint = ctx.accounts.root_mint.to_account_info();
        let cfg = config.to_account_info();
        let payer = ctx.accounts.creator.to_account_info();
        let sys = ctx.accounts.system_program.to_account_info();

        // Join the root first: the number decides the metadata.
        top_up(&payer, &mint, &sys, mint.data_len() + MEMBER_TLV)?;
        token_member_initialize(CpiContext::new_with_signer(
            tp.clone(),
            TokenMemberInitialize {
                program_id: tp.clone(),
                member: mint.clone(),
                member_mint: mint.clone(),
                member_mint_authority: cfg.clone(),
                group: root_mint.clone(),
                group_update_authority: cfg.clone(),
            },
            signer,
        ))?;
        let number = member_number(&mint)?;

        let uri = member_uri(&config.uri_base, &mint.key());
        let mut fields = vec![
            ("standard".to_string(), STANDARD.to_string()),
            ("kind".to_string(), "launchpad".to_string()),
            ("number".to_string(), number.to_string()),
            ("max".to_string(), a.max_size.to_string()),
            ("theme".to_string(), a.theme.to_string()),
            ("creator".to_string(), ctx.accounts.creator.key().to_string()),
            ("naming".to_string(), naming_label(a.naming).to_string()),
            ("prompt".to_string(), if a.minter_prompt { "1" } else { "0" }.to_string()),
        ];
        if !a.style.is_empty() {
            fields.push(("style".to_string(), a.style.clone()));
        }
        let meta_len = metadata_len(&cfg.key(), &mint.key(), &a.name, &a.symbol, &uri, &fields)?;
        top_up(&payer, &mint, &sys, mint.data_len() + GROUP_TLV + meta_len)?;

        token_group_initialize(
            CpiContext::new_with_signer(
                tp.clone(),
                TokenGroupInitialize { program_id: tp.clone(), group: mint.clone(), mint: mint.clone(), mint_authority: cfg.clone() },
                signer,
            ),
            Some(cfg.key()),
            a.max_size,
        )?;
        init_metadata(&tp, &mint, &cfg, signer, &a.name, &a.symbol, &uri, &fields)?;

        let g = &mut ctx.accounts.group;
        g.mint = mint.key();
        g.owner = ctx.accounts.creator.key();
        g.parent = root_mint.key();
        g.number = number;
        g.max_size = a.max_size;
        g.theme = a.theme;
        g.open_launch = a.open_launch;
        g.name = a.name.clone();
        g.prefix = a.prefix.clone();
        g.symbol = a.symbol.clone();
        g.bump = ctx.bumps.group;

        if budget > 0 {
            system_program::transfer(
                CpiContext::new(sys.clone(), system_program::Transfer { from: payer.clone(), to: g.to_account_info() }),
                budget,
            )?;
            g.budget = budget;
        }

        let root = &mut ctx.accounts.root_group;
        root.size = number;
        let c = &mut ctx.accounts.config;
        c.groups += 1;

        emit!(GroupCreated { mint: mint.key(), number, max_size: a.max_size, owner: ctx.accounts.creator.key(), theme: a.theme });
        Ok(())
    }

    /// Anyone can add lamports to a group's launch budget.
    pub fn fund_group(ctx: Context<FundGroup>, lamports: u64) -> Result<()> {
        require!(lamports > 0, PadError::BadArgs);
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer { from: ctx.accounts.funder.to_account_info(), to: ctx.accounts.group.to_account_info() },
            ),
            lamports,
        )?;
        let g = &mut ctx.accounts.group;
        g.budget = g.budget.checked_add(lamports).ok_or(PadError::Math)?;
        Ok(())
    }

    /// Owner-written description and link, inherited by every member's JSON.
    /// Only before the first member exists: a collection's story is fixed once it starts.
    pub fn set_group_info(ctx: Context<SetGroupInfo>, description: String, url: String) -> Result<()> {
        require!(ctx.accounts.group.size == 0, PadError::NotEmpty);
        require!(description.len() <= MAX_DESCRIPTION && url.len() <= MAX_URL, PadError::TooLong);
        let config = &ctx.accounts.config;
        let bump = [config.bump];
        let seeds: &[&[u8]] = &[b"config", &bump];
        let signer = &[seeds];
        let tp = ctx.accounts.token_program.to_account_info();
        let mint = ctx.accounts.mint.to_account_info();
        let cfg = config.to_account_info();
        let owner = ctx.accounts.owner.to_account_info();
        let sys = ctx.accounts.system_program.to_account_info();
        // room for both fields (key + value + 2 borsh length prefixes each)
        let extra = 4 + 11 + 4 + description.len() + 4 + 3 + 4 + url.len();
        top_up(&owner, &mint, &sys, mint.data_len() + extra)?;
        if !description.is_empty() {
            set_field(&tp, &mint, &cfg, signer, "description", &description)?;
        }
        if !url.is_empty() {
            set_field(&tp, &mint, &cfg, signer, "url", &url)?;
        }
        Ok(())
    }

    pub fn set_group(ctx: Context<OwnerGroup>, new_owner: Option<Pubkey>, open_launch: Option<bool>) -> Result<()> {
        let g = &mut ctx.accounts.group;
        if let Some(o) = open_launch {
            g.open_launch = o;
        }
        if let Some(o) = new_owner {
            g.owner = o;
        }
        Ok(())
    }

    /// One click, one monster. Signed by the backend payer (rent reimbursed
    /// from the group budget, creator pending until `claim`) or by any wallet
    /// paying its own rent (then it is the creator right away).
    pub fn launch(ctx: Context<LaunchMonster>, a: LaunchArgs) -> Result<()> {
        let config = &ctx.accounts.config;
        let group = &ctx.accounts.group;
        require!(group.open_launch, PadError::LaunchClosed);
        let backend = ctx.accounts.payer.key() == config.payer;
        let creator = if backend { a.creator } else { Some(ctx.accounts.payer.key()) };

        let bump = [config.bump];
        let seeds: &[&[u8]] = &[b"config", &bump];
        let signer = &[seeds];
        let tp = ctx.accounts.token_program.to_account_info();
        let mint = ctx.accounts.mint.to_account_info();
        let group_mint = ctx.accounts.group_mint.to_account_info();
        let cfg = config.to_account_info();
        let payer = ctx.accounts.payer.to_account_info();
        let sys = ctx.accounts.system_program.to_account_info();

        // 1. Join the group. Token-2022 hands out the number and refuses the cap.
        top_up(&payer, &mint, &sys, mint.data_len() + MEMBER_TLV)?;
        token_member_initialize(CpiContext::new_with_signer(
            tp.clone(),
            TokenMemberInitialize {
                program_id: tp.clone(),
                member: mint.clone(),
                member_mint: mint.clone(),
                member_mint_authority: cfg.clone(),
                group: group_mint.clone(),
                group_update_authority: cfg.clone(),
            },
            signer,
        ))?;
        let number = member_number(&mint)?;

        // 2. Seed and traits, both on-chain truth.
        let clock = Clock::get()?;
        let prompt = a.prompt.clone().filter(|p| !p.is_empty());
        if let Some(p) = &prompt {
            require!(p.len() <= MAX_PROMPT, PadError::TooLong);
            require!(meta_field(&group_mint, "prompt")?.as_deref() == Some("1"), PadError::PromptNotAllowed);
        }
        let prompt_bytes = prompt.clone().unwrap_or_default();
        let seed = hashv(&[group_mint.key().as_ref(), &number.to_le_bytes(), &clock.slot.to_le_bytes(), prompt_bytes.as_bytes()]).to_bytes();
        let traits = traits::traits_string(&seed);
        let special = traits::special(number);

        // 3. Metadata, with the number in the name.
        let name = member_name(&group.prefix, number, group.max_size);
        let uri = member_uri(&config.uri_base, &mint.key());
        let mut fields = vec![
            ("standard".to_string(), STANDARD.to_string()),
            ("number".to_string(), number.to_string()),
            ("max".to_string(), group.max_size.to_string()),
            ("seed".to_string(), traits::hex(&seed)),
            ("traits".to_string(), traits.clone()),
        ];
        if let Some(s) = special {
            fields.push(("special".to_string(), s.to_string()));
        }
        if let Some(p) = &prompt {
            fields.push(("prompt".to_string(), p.clone()));
        }
        // Reserve room for the claim fields so claim never has to top up.
        let mut sized = fields.clone();
        sized.push(("creator".to_string(), Pubkey::default().to_string()));
        sized.push(("claimed_at".to_string(), i64::MAX.to_string()));
        sized.push(("nickname".to_string(), "x".repeat(MAX_NICKNAME)));
        let meta_len = metadata_len(&cfg.key(), &mint.key(), &name, &group.symbol, &uri, &sized)?;
        top_up(&payer, &mint, &sys, mint.data_len() + meta_len)?;
        init_metadata(&tp, &mint, &cfg, signer, &name, &group.symbol, &uri, &fields)?;

        // 4. Whole supply into the curve's stock.
        token_interface::mint_to(
            CpiContext::new_with_signer(
                tp.clone(),
                MintTo { mint: mint.clone(), to: ctx.accounts.stock.to_account_info(), authority: cfg.clone() },
                signer,
            ),
            config.supply,
        )?;

        let cv = &mut ctx.accounts.curve;
        cv.mint = mint.key();
        cv.group = group_mint.key();
        cv.virtual_sol = config.virtual_sol;
        cv.virtual_tokens = config.virtual_tokens;
        cv.fee_bps = config.fee_bps;
        cv.owner_bps = config.owner_bps;
        cv.creator_bps = config.creator_bps;
        cv.bump = ctx.bumps.curve;

        let l = &mut ctx.accounts.launch;
        l.group = group_mint.key();
        l.mint = mint.key();
        l.number = number;
        l.seed = seed;
        l.claim_hash = a.claim_hash;
        l.created_at = clock.unix_timestamp;
        l.bump = ctx.bumps.launch;
        if let Some(c) = creator {
            l.creator = c;
            l.claimed_at = clock.unix_timestamp;
            l.status = LaunchStatus::Claimed;
            write_claim_fields(&tp, &mint, &cfg, signer, &c, clock.unix_timestamp, None)?;
        } else {
            l.status = LaunchStatus::Pending;
        }

        // 5. Budget: the backend gets every lamport of rent back from the group.
        let g = &mut ctx.accounts.group;
        g.size = number;
        if backend {
            let spent = mint.lamports()
                + ctx.accounts.launch.to_account_info().lamports()
                + ctx.accounts.curve.to_account_info().lamports()
                + ctx.accounts.stock.to_account_info().lamports();
            require!(g.budget >= spent, PadError::BudgetExhausted);
            g.budget -= spent;
            move_lamports(&g.to_account_info(), &payer, spent)?;
        }
        let c = &mut ctx.accounts.config;
        c.launches += 1;

        emit!(Launched {
            group: group_mint.key(),
            mint: mint.key(),
            number,
            max_size: ctx.accounts.group.max_size,
            seed,
            traits,
            creator: creator.unwrap_or_default(),
        });
        Ok(())
    }

    /// The visitor comes back with a wallet and the secret from their session.
    pub fn claim(ctx: Context<Claim>, secret: [u8; 32], nickname: Option<String>) -> Result<()> {
        let l = &mut ctx.accounts.launch;
        require!(l.status == LaunchStatus::Pending, PadError::NotPending);
        require!(hashv(&[&secret]).to_bytes() == l.claim_hash, PadError::BadSecret);
        if let Some(n) = &nickname {
            require!(!n.is_empty() && n.len() <= MAX_NICKNAME, PadError::TooLong);
            require!(meta_field(&ctx.accounts.group_mint.to_account_info(), "naming")?.as_deref() != Some("fixed"), PadError::NamingFixed);
        }
        let now = Clock::get()?.unix_timestamp;
        let creator = ctx.accounts.claimer.key();
        l.creator = creator;
        l.claimed_at = now;
        l.status = LaunchStatus::Claimed;
        l.nickname_set = nickname.is_some();

        let config = &ctx.accounts.config;
        let bump = [config.bump];
        let seeds: &[&[u8]] = &[b"config", &bump];
        let signer = &[seeds];
        let tp = ctx.accounts.token_program.to_account_info();
        write_claim_fields(&tp, &ctx.accounts.mint.to_account_info(), &config.to_account_info(), signer, &creator, now, nickname.as_deref())?;
        emit!(Claimed { mint: ctx.accounts.mint.key(), creator, number: l.number });
        Ok(())
    }

    /// Once, by the creator, if no nickname was given at claim.
    pub fn set_nickname(ctx: Context<SetNickname>, nickname: String) -> Result<()> {
        let l = &mut ctx.accounts.launch;
        require!(l.status == LaunchStatus::Claimed, PadError::NotClaimed);
        require!(!l.nickname_set, PadError::NicknameSet);
        require!(!nickname.is_empty() && nickname.len() <= MAX_NICKNAME, PadError::TooLong);
        require!(meta_field(&ctx.accounts.group_mint.to_account_info(), "naming")?.as_deref() != Some("fixed"), PadError::NamingFixed);
        l.nickname_set = true;
        let config = &ctx.accounts.config;
        let bump = [config.bump];
        let seeds: &[&[u8]] = &[b"config", &bump];
        let signer = &[seeds];
        let tp = ctx.accounts.token_program.to_account_info();
        set_field(&tp, &ctx.accounts.mint.to_account_info(), &config.to_account_info(), signer, "nickname", &nickname)
    }

    /// Unclaimed after the TTL and never traded: the curve closes, its rent and
    /// the stock's go back to the group budget. The number stays consumed and
    /// the mint keeps its metadata with a supply of zero.
    pub fn expire(ctx: Context<Expire>) -> Result<()> {
        let l = &mut ctx.accounts.launch;
        require!(l.status == LaunchStatus::Pending, PadError::NotPending);
        let now = Clock::get()?.unix_timestamp;
        require!(now >= l.created_at + ctx.accounts.config.claim_ttl, PadError::NotExpired);
        require!(ctx.accounts.curve.trades == 0, PadError::HasTrades);
        l.status = LaunchStatus::Expired;

        let config = &ctx.accounts.config;
        let bump = [config.bump];
        let seeds: &[&[u8]] = &[b"config", &bump];
        let signer = &[seeds];
        let tp = ctx.accounts.token_program.to_account_info();
        let cfg = config.to_account_info();
        let stock = ctx.accounts.stock.to_account_info();
        let group = ctx.accounts.group.to_account_info();

        let recovered = stock.lamports() + ctx.accounts.curve.to_account_info().lamports();
        token_interface::burn(
            CpiContext::new_with_signer(
                tp.clone(),
                Burn { mint: ctx.accounts.mint.to_account_info(), from: stock.clone(), authority: cfg.clone() },
                signer,
            ),
            ctx.accounts.stock.amount,
        )?;
        token_interface::close_account(CpiContext::new_with_signer(
            tp.clone(),
            CloseAccount { account: stock.clone(), destination: group.clone(), authority: cfg.clone() },
            signer,
        ))?;
        let g = &mut ctx.accounts.group;
        g.budget = g.budget.checked_add(recovered).ok_or(PadError::Math)?;
        g.expired += 1;
        emit!(Expired { mint: ctx.accounts.mint.key(), number: l.number, recovered });
        Ok(())
    }

    /// SOL in, tokens out. `sol_in` is gross: the fee comes out of it.
    pub fn buy(ctx: Context<Trade>, sol_in: u64, min_tokens_out: u64) -> Result<()> {
        require!(sol_in > 0, PadError::BadArgs);
        let cv = &ctx.accounts.curve;
        let fee = curve::fee_of(sol_in, cv.fee_bps);
        let net = sol_in - fee;
        let st = curve::CurveState { virtual_sol: cv.virtual_sol, virtual_tokens: cv.virtual_tokens, reserve: cv.reserve, sold: cv.sold };
        let out = curve::tokens_out(&st, net).ok_or(PadError::Math)?;
        require!(out >= min_tokens_out, PadError::Slippage);
        require!(out > 0 && out <= ctx.accounts.stock.amount, PadError::SoldOut);
        let (platform, owner, creator) = curve::split(fee, cv.fee_bps, cv.owner_bps, cv.creator_bps);

        let sys = ctx.accounts.system_program.to_account_info();
        let buyer = ctx.accounts.buyer.to_account_info();
        let curve_ai = ctx.accounts.curve.to_account_info();
        let root = ctx.accounts.group.parent == Pubkey::default();
        pay(&sys, &buyer, &curve_ai, net + creator)?;
        if root {
            pay(&sys, &buyer, &ctx.accounts.treasury.to_account_info(), platform + owner)?;
        } else {
            pay(&sys, &buyer, &ctx.accounts.treasury.to_account_info(), platform)?;
            pay(&sys, &buyer, &ctx.accounts.group.to_account_info(), owner)?;
            ctx.accounts.group.owner_fees += owner;
        }

        let config = &ctx.accounts.config;
        let bump = [config.bump];
        let seeds: &[&[u8]] = &[b"config", &bump];
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.stock.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.trader_ata.to_account_info(),
                    authority: config.to_account_info(),
                },
                &[seeds],
            ),
            out,
            TOKEN_DECIMALS,
        )?;

        let cv = &mut ctx.accounts.curve;
        cv.reserve += net;
        cv.sold += out;
        cv.trades += 1;
        cv.volume = cv.volume.saturating_add(sol_in);
        cv.creator_fees += creator;
        emit!(Traded { mint: cv.mint, trader: buyer.key(), is_buy: true, sol: sol_in, tokens: out, reserve: cv.reserve, sold: cv.sold });
        Ok(())
    }

    /// Tokens in, SOL out. The fee comes out of the SOL returned.
    pub fn sell(ctx: Context<Trade>, tokens_in: u64, min_sol_out: u64) -> Result<()> {
        require!(tokens_in > 0, PadError::BadArgs);
        let cv = &ctx.accounts.curve;
        let st = curve::CurveState { virtual_sol: cv.virtual_sol, virtual_tokens: cv.virtual_tokens, reserve: cv.reserve, sold: cv.sold };
        let gross = curve::sol_out(&st, tokens_in).ok_or(PadError::Math)?;
        require!(gross <= cv.reserve, PadError::Reserve);
        let fee = curve::fee_of(gross, cv.fee_bps);
        let out = gross - fee;
        require!(out >= min_sol_out, PadError::Slippage);
        let (platform, owner, creator) = curve::split(fee, cv.fee_bps, cv.owner_bps, cv.creator_bps);

        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.trader_ata.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.stock.to_account_info(),
                    authority: ctx.accounts.buyer.to_account_info(),
                },
            ),
            tokens_in,
            TOKEN_DECIMALS,
        )?;

        let curve_ai = ctx.accounts.curve.to_account_info();
        let root = ctx.accounts.group.parent == Pubkey::default();
        move_lamports(&curve_ai, &ctx.accounts.buyer.to_account_info(), out)?;
        if root {
            move_lamports(&curve_ai, &ctx.accounts.treasury.to_account_info(), platform + owner)?;
        } else {
            move_lamports(&curve_ai, &ctx.accounts.treasury.to_account_info(), platform)?;
            move_lamports(&curve_ai, &ctx.accounts.group.to_account_info(), owner)?;
            ctx.accounts.group.owner_fees += owner;
        }

        let cv = &mut ctx.accounts.curve;
        cv.reserve -= gross;
        cv.sold -= tokens_in;
        cv.trades += 1;
        cv.volume = cv.volume.saturating_add(gross);
        cv.creator_fees += creator;
        emit!(Traded { mint: cv.mint, trader: ctx.accounts.buyer.key(), is_buy: false, sol: out, tokens: tokens_in, reserve: cv.reserve, sold: cv.sold });
        Ok(())
    }

    pub fn withdraw_owner_fees(ctx: Context<OwnerGroup>) -> Result<()> {
        let g = &mut ctx.accounts.group;
        let amount = g.owner_fees;
        require!(amount > 0, PadError::Nothing);
        g.owner_fees = 0;
        move_lamports(&g.to_account_info(), &ctx.accounts.owner.to_account_info(), amount)
    }

    pub fn withdraw_creator_fees(ctx: Context<WithdrawCreator>) -> Result<()> {
        require!(ctx.accounts.launch.status == LaunchStatus::Claimed, PadError::NotClaimed);
        let cv = &mut ctx.accounts.curve;
        let amount = cv.creator_fees;
        require!(amount > 0, PadError::Nothing);
        cv.creator_fees = 0;
        move_lamports(&cv.to_account_info(), &ctx.accounts.creator.to_account_info(), amount)
    }
}

// ---------------------------------------------------------------- helpers

fn digits(n: u64) -> usize {
    n.to_string().len()
}

/// "Monstre #042": zero-padded to the width of the last number of the group.
pub fn member_name(prefix: &str, number: u64, max_size: u64) -> String {
    let width = digits(max_size.saturating_sub(1)).max(2);
    format!("{} #{:0width$}", prefix, number, width = width)
}

pub fn member_uri(base: &str, mint: &Pubkey) -> String {
    format!("{}{}", base, mint)
}

fn naming_label(naming: u8) -> &'static str {
    if naming == NAMING_FIXED { "fixed" } else { "nickname" }
}

/// A field of the mint's TokenMetadata, read on-chain. The policy of a
/// collection lives there, so everything about a collection is in its mint.
fn meta_field(mint: &AccountInfo, key: &str) -> Result<Option<String>> {
    let data = mint.try_borrow_data()?;
    let st = StateWithExtensions::<MintState>::unpack(&data)?;
    let meta = st.get_variable_len_extension::<TokenMetadata>()?;
    Ok(meta.additional_metadata.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
}

fn member_number(mint: &AccountInfo) -> Result<u64> {
    let data = mint.try_borrow_data()?;
    let st = StateWithExtensions::<MintState>::unpack(&data)?;
    let m = st.get_extension::<TokenGroupMember>()?;
    Ok(u64::from(m.member_number))
}

fn metadata_len(authority: &Pubkey, mint: &Pubkey, name: &str, symbol: &str, uri: &str, fields: &[(String, String)]) -> Result<usize> {
    let meta = TokenMetadata {
        update_authority: OptionalNonZeroPubkey::try_from(Some(*authority))?,
        mint: *mint,
        name: name.to_string(),
        symbol: symbol.to_string(),
        uri: uri.to_string(),
        additional_metadata: fields.to_vec(),
    };
    Ok(meta.tlv_size_of()?)
}

/// Makes `target` rent-exempt for `new_len` bytes, paid by `payer`.
fn top_up<'info>(payer: &AccountInfo<'info>, target: &AccountInfo<'info>, sys: &AccountInfo<'info>, new_len: usize) -> Result<()> {
    let need = Rent::get()?.minimum_balance(new_len).saturating_sub(target.lamports());
    if need > 0 {
        system_program::transfer(
            CpiContext::new(sys.clone(), system_program::Transfer { from: payer.clone(), to: target.clone() }),
            need,
        )?;
    }
    Ok(())
}

fn pay<'info>(sys: &AccountInfo<'info>, from: &AccountInfo<'info>, to: &AccountInfo<'info>, lamports: u64) -> Result<()> {
    if lamports == 0 {
        return Ok(());
    }
    system_program::transfer(
        CpiContext::new(sys.clone(), system_program::Transfer { from: from.clone(), to: to.clone() }),
        lamports,
    )
}

/// Lamports out of a program-owned account.
fn move_lamports(from: &AccountInfo, to: &AccountInfo, lamports: u64) -> Result<()> {
    if lamports == 0 {
        return Ok(());
    }
    let new_from = from.lamports().checked_sub(lamports).ok_or(PadError::Math)?;
    let min = Rent::get()?.minimum_balance(from.data_len());
    require!(new_from >= min, PadError::Reserve);
    **from.try_borrow_mut_lamports()? = new_from;
    **to.try_borrow_mut_lamports()? = to.lamports().checked_add(lamports).ok_or(PadError::Math)?;
    Ok(())
}

fn init_metadata<'info>(
    tp: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    cfg: &AccountInfo<'info>,
    signer: &[&[&[u8]]],
    name: &str,
    symbol: &str,
    uri: &str,
    fields: &[(String, String)],
) -> Result<()> {
    token_metadata_initialize(
        CpiContext::new_with_signer(
            tp.clone(),
            TokenMetadataInitialize {
                program_id: tp.clone(),
                metadata: mint.clone(),
                update_authority: cfg.clone(),
                mint_authority: cfg.clone(),
                mint: mint.clone(),
            },
            signer,
        ),
        name.to_string(),
        symbol.to_string(),
        uri.to_string(),
    )?;
    for (k, v) in fields {
        set_field(tp, mint, cfg, signer, k, v)?;
    }
    Ok(())
}

fn set_field<'info>(tp: &AccountInfo<'info>, mint: &AccountInfo<'info>, cfg: &AccountInfo<'info>, signer: &[&[&[u8]]], key: &str, value: &str) -> Result<()> {
    token_metadata_update_field(
        CpiContext::new_with_signer(
            tp.clone(),
            TokenMetadataUpdateField { program_id: tp.clone(), metadata: mint.clone(), update_authority: cfg.clone() },
            signer,
        ),
        Field::Key(key.to_string()),
        value.to_string(),
    )
}

fn write_claim_fields<'info>(
    tp: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    cfg: &AccountInfo<'info>,
    signer: &[&[&[u8]]],
    creator: &Pubkey,
    at: i64,
    nickname: Option<&str>,
) -> Result<()> {
    set_field(tp, mint, cfg, signer, "creator", &creator.to_string())?;
    set_field(tp, mint, cfg, signer, "claimed_at", &at.to_string())?;
    if let Some(n) = nickname {
        set_field(tp, mint, cfg, signer, "nickname", n)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- args

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ConfigParams {
    pub payer: Pubkey,
    pub treasury: Pubkey,
    pub fee_bps: u16,
    pub owner_bps: u16,
    pub creator_bps: u16,
    pub virtual_sol: u64,
    pub virtual_tokens: u64,
    pub supply: u64,
    pub claim_ttl: i64,
    pub uri_base: String,
}

impl ConfigParams {
    fn check(&self) -> Result<()> {
        require!(self.fee_bps <= MAX_FEE_BPS, PadError::BadFees);
        require!(self.owner_bps + self.creator_bps <= self.fee_bps, PadError::BadFees);
        require!(self.virtual_sol > 0 && self.virtual_tokens > self.supply && self.supply > 0, PadError::BadArgs);
        require!(self.claim_ttl > 0, PadError::BadArgs);
        require!(self.uri_base.len() <= MAX_URI_BASE, PadError::TooLong);
        Ok(())
    }
    fn apply(&self, c: &mut Config) {
        c.payer = self.payer;
        c.treasury = self.treasury;
        c.fee_bps = self.fee_bps;
        c.owner_bps = self.owner_bps;
        c.creator_bps = self.creator_bps;
        c.virtual_sol = self.virtual_sol;
        c.virtual_tokens = self.virtual_tokens;
        c.supply = self.supply;
        c.claim_ttl = self.claim_ttl;
        c.uri_base = self.uri_base.clone();
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct GroupArgs {
    pub name: String,
    pub prefix: String,
    pub symbol: String,
    pub theme: u8,
    pub max_size: u64,
    pub open_launch: bool,
    /// Optional encoded style from the generator; empty = the base theme.
    pub style: String,
    /// NAMING_FIXED or NAMING_NICKNAME.
    pub naming: u8,
    /// Minters may type a few words that shape the seed and are stored as `prompt`.
    pub minter_prompt: bool,
}

impl GroupArgs {
    fn check(&self) -> Result<()> {
        require!(!self.name.is_empty() && self.name.len() <= MAX_NAME, PadError::TooLong);
        require!(!self.prefix.is_empty() && self.prefix.len() <= MAX_PREFIX, PadError::TooLong);
        require!(!self.symbol.is_empty() && self.symbol.len() <= MAX_SYMBOL, PadError::TooLong);
        require!(self.max_size >= MIN_GROUP_SIZE && self.max_size <= MAX_GROUP_SIZE, PadError::BadArgs);
        require!(self.style.len() <= MAX_STYLE, PadError::TooLong);
        require!(self.naming <= NAMING_NICKNAME, PadError::BadArgs);
        Ok(())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct LaunchArgs {
    /// sha256 of the secret the visitor will present at claim.
    pub claim_hash: [u8; 32],
    /// Backend only: a wallet already known, claimed on the spot.
    pub creator: Option<Pubkey>,
    /// Minter's words, only when the collection allows it; mixed into the seed.
    pub prompt: Option<String>,
}

// ---------------------------------------------------------------- events

#[event]
pub struct GroupCreated {
    pub mint: Pubkey,
    pub number: u64,
    pub max_size: u64,
    pub owner: Pubkey,
    pub theme: u8,
}

#[event]
pub struct Launched {
    pub group: Pubkey,
    pub mint: Pubkey,
    pub number: u64,
    pub max_size: u64,
    pub seed: [u8; 32],
    pub traits: String,
    pub creator: Pubkey,
}

#[event]
pub struct Claimed {
    pub mint: Pubkey,
    pub creator: Pubkey,
    pub number: u64,
}

#[event]
pub struct Expired {
    pub mint: Pubkey,
    pub number: u64,
    pub recovered: u64,
}

#[event]
pub struct Traded {
    pub mint: Pubkey,
    pub trader: Pubkey,
    pub is_buy: bool,
    pub sol: u64,
    pub tokens: u64,
    pub reserve: u64,
    pub sold: u64,
}

// ---------------------------------------------------------------- accounts

#[derive(Accounts)]
pub struct InitConfig<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(init, payer = admin, space = 8 + Config::INIT_SPACE, seeds = [b"config"], bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(
        init,
        payer = admin,
        seeds = [b"root"],
        bump,
        mint::decimals = 0,
        mint::authority = config,
        mint::token_program = token_program,
        extensions::group_pointer::authority = config,
        extensions::group_pointer::group_address = root_mint,
        extensions::metadata_pointer::authority = config,
        extensions::metadata_pointer::metadata_address = root_mint,
    )]
    pub root_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(init, payer = admin, space = 8 + Group::INIT_SPACE, seeds = [b"group", root_mint.key().as_ref()], bump)]
    pub root_group: Box<Account<'info, Group>>,
    pub token_program: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateConfig<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [b"config"], bump = config.bump, has_one = admin)]
    pub config: Box<Account<'info, Config>>,
}

#[derive(Accounts)]
pub struct CreateGroup<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    #[account(mut, seeds = [b"config"], bump = config.bump, has_one = root_mint)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut)]
    pub root_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut, seeds = [b"group", root_mint.key().as_ref()], bump = root_group.bump)]
    pub root_group: Box<Account<'info, Group>>,
    #[account(
        init,
        payer = creator,
        mint::decimals = 0,
        mint::authority = config,
        mint::token_program = token_program,
        extensions::group_pointer::authority = config,
        extensions::group_pointer::group_address = mint,
        extensions::group_member_pointer::authority = config,
        extensions::group_member_pointer::member_address = mint,
        extensions::metadata_pointer::authority = config,
        extensions::metadata_pointer::metadata_address = mint,
    )]
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(init, payer = creator, space = 8 + Group::INIT_SPACE, seeds = [b"group", mint.key().as_ref()], bump)]
    pub group: Box<Account<'info, Group>>,
    pub token_program: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct FundGroup<'info> {
    #[account(mut)]
    pub funder: Signer<'info>,
    #[account(mut, seeds = [b"group", group.mint.as_ref()], bump = group.bump)]
    pub group: Box<Account<'info, Group>>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct OwnerGroup<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(mut, seeds = [b"group", group.mint.as_ref()], bump = group.bump, has_one = owner)]
    pub group: Box<Account<'info, Group>>,
}

#[derive(Accounts)]
pub struct LaunchMonster<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut, seeds = [b"config"], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [b"group", group_mint.key().as_ref()], bump = group.bump)]
    pub group: Box<Account<'info, Group>>,
    #[account(mut, address = group.mint @ PadError::WrongGroup)]
    pub group_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        init,
        payer = payer,
        mint::decimals = TOKEN_DECIMALS,
        mint::authority = config,
        mint::token_program = token_program,
        extensions::group_member_pointer::authority = config,
        extensions::group_member_pointer::member_address = mint,
        extensions::metadata_pointer::authority = config,
        extensions::metadata_pointer::metadata_address = mint,
    )]
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(init, payer = payer, space = 8 + Launch::INIT_SPACE, seeds = [b"launch", mint.key().as_ref()], bump)]
    pub launch: Box<Account<'info, Launch>>,
    #[account(init, payer = payer, space = 8 + Curve::INIT_SPACE, seeds = [b"curve", mint.key().as_ref()], bump)]
    pub curve: Box<Account<'info, Curve>>,
    #[account(
        init,
        payer = payer,
        seeds = [b"stock", mint.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = config,
        token::token_program = token_program,
    )]
    pub stock: Box<InterfaceAccount<'info, TokenAccount>>,
    pub token_program: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Claim<'info> {
    #[account(mut)]
    pub claimer: Signer<'info>,
    #[account(seeds = [b"config"], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [b"launch", mint.key().as_ref()], bump = launch.bump)]
    pub launch: Box<Account<'info, Launch>>,
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = launch.group @ PadError::WrongGroup)]
    pub group_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Program<'info, Token2022>,
}

#[derive(Accounts)]
pub struct SetGroupInfo<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(seeds = [b"config"], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(seeds = [b"group", mint.key().as_ref()], bump = group.bump, has_one = owner)]
    pub group: Box<Account<'info, Group>>,
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetNickname<'info> {
    pub creator: Signer<'info>,
    #[account(seeds = [b"config"], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [b"launch", mint.key().as_ref()], bump = launch.bump, has_one = creator)]
    pub launch: Box<Account<'info, Launch>>,
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = launch.group @ PadError::WrongGroup)]
    pub group_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Program<'info, Token2022>,
}

#[derive(Accounts)]
pub struct Expire<'info> {
    pub cranker: Signer<'info>,
    #[account(seeds = [b"config"], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [b"group", launch.group.as_ref()], bump = group.bump)]
    pub group: Box<Account<'info, Group>>,
    #[account(mut, seeds = [b"launch", mint.key().as_ref()], bump = launch.bump)]
    pub launch: Box<Account<'info, Launch>>,
    #[account(mut, seeds = [b"curve", mint.key().as_ref()], bump = curve.bump, close = group)]
    pub curve: Box<Account<'info, Curve>>,
    #[account(mut, seeds = [b"stock", mint.key().as_ref()], bump)]
    pub stock: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Program<'info, Token2022>,
}

#[derive(Accounts)]
pub struct Trade<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,
    #[account(seeds = [b"config"], bump = config.bump, has_one = treasury)]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: pinned to config.treasury
    #[account(mut)]
    pub treasury: UncheckedAccount<'info>,
    #[account(mut, seeds = [b"group", curve.group.as_ref()], bump = group.bump)]
    pub group: Box<Account<'info, Group>>,
    #[account(mut, seeds = [b"curve", mint.key().as_ref()], bump = curve.bump)]
    pub curve: Box<Account<'info, Curve>>,
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut, seeds = [b"stock", mint.key().as_ref()], bump)]
    pub stock: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        init_if_needed,
        payer = buyer,
        associated_token::mint = mint,
        associated_token::authority = buyer,
        associated_token::token_program = token_program,
    )]
    pub trader_ata: Box<InterfaceAccount<'info, TokenAccount>>,
    pub token_program: Program<'info, Token2022>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct WithdrawCreator<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    #[account(seeds = [b"launch", launch.mint.as_ref()], bump = launch.bump, has_one = creator)]
    pub launch: Box<Account<'info, Launch>>,
    #[account(mut, seeds = [b"curve", launch.mint.as_ref()], bump = curve.bump)]
    pub curve: Box<Account<'info, Curve>>,
}
