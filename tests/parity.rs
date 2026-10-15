//! Quote parity against the real program: load a mainnet snapshot of a curve
//! into LiteSVM, run `Nts1Amm::quote`, execute the native `buy` then `sell`,
//! and assert the on-chain deltas equal the quotes exactly.
//!
//! The Jupiter test kit cannot drive this program yet (it moves native SOL,
//! not wrapped SOL), so this harness does the same job with the same shape:
//! snapshot -> quote -> native instruction -> compare.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use base64::Engine;
use jupiter_amm_interface::{Amm, AmmContext, KeyedAccount, QuoteParams, SwapMode, SwapParams};
use litesvm::LiteSVM;
use nts1_jupiter_sdk::{encode::encode_swap_with_min, token_amount, Nts1Amm, ASSOCIATED_TOKEN, PROGRAM_ID, TOKEN_2022, WSOL};
use serde::Deserialize;
use solana_account::Account;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;

#[derive(Deserialize)]
struct FixtureAccount {
    pubkey: String,
    lamports: u64,
    owner: String,
    executable: bool,
    data: String,
}

#[derive(Deserialize)]
struct Fixture {
    mint: String,
    curve: String,
    stock: String,
    unix_timestamp: i64,
    accounts: Vec<FixtureAccount>,
}

fn fixture_dir(mint: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(mint)
}

fn load(mint: &str) -> (Fixture, HashMap<Pubkey, Box<Account>>) {
    let dir = fixture_dir(mint);
    let f: Fixture = serde_json::from_str(&fs::read_to_string(dir.join("accounts.json")).expect("fixture accounts.json")).unwrap();
    let mut map = HashMap::new();
    for a in &f.accounts {
        let data = base64::engine::general_purpose::STANDARD.decode(&a.data).unwrap();
        map.insert(
            a.pubkey.parse::<Pubkey>().unwrap(),
            Box::new(Account { lamports: a.lamports, data, owner: a.owner.parse().unwrap(), executable: a.executable, rent_epoch: 0 }),
        );
    }
    (f, map)
}

fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[owner.as_ref(), TOKEN_2022.as_ref(), mint.as_ref()], &ASSOCIATED_TOKEN).0
}

fn amm_from(svm: &LiteSVM, curve: &Pubkey) -> Nts1Amm {
    let acc = svm.get_account(curve).expect("curve in svm");
    let ka = KeyedAccount { key: *curve, account: acc, params: None };
    let mut amm = Nts1Amm::from_keyed_account(&ka, &AmmContext::default()).unwrap();
    let mut provider: HashMap<Pubkey, Box<Account>> = HashMap::new();
    for k in amm.get_accounts_to_update() {
        if let Some(a) = svm.get_account(&k) {
            provider.insert(k, Box::new(a));
        }
    }
    amm.update(&provider).unwrap();
    amm
}

fn swap_ix(amm: &Nts1Amm, user: &Pubkey, source_mint: Pubkey, destination_mint: Pubkey, in_amount: u64) -> Instruction {
    let jup = Pubkey::new_unique();
    let src = if source_mint == WSOL { Pubkey::new_unique() } else { ata(user, &amm.mint()) };
    let dst = if destination_mint == WSOL { Pubkey::new_unique() } else { ata(user, &amm.mint()) };
    let params = SwapParams {
        swap_mode: SwapMode::ExactIn,
        in_amount,
        out_amount: 0,
        source_mint,
        destination_mint,
        source_token_account: src,
        destination_token_account: dst,
        token_transfer_authority: *user,
        user: *user,
        payer: *user,
        quote_mint_to_referrer: None,
        jupiter_program_id: &jup,
        missing_dynamic_accounts_as_default: false,
    };
    let s = amm.get_swap_and_account_metas(&params).unwrap();
    Instruction { program_id: PROGRAM_ID, accounts: s.account_metas, data: encode_swap_with_min(&s.swap, in_amount, 0) }
}

const MINT: &str = "G3nAYGwxkQpUdn6Jdnd6u3PFRebu5BvkqqkwGMYttkTL";

#[test]
fn buy_then_sell_match_quotes_exactly() {
    let (f, accounts) = load(MINT);
    let mint: Pubkey = f.mint.parse().unwrap();
    let curve: Pubkey = f.curve.parse().unwrap();
    let stock: Pubkey = f.stock.parse().unwrap();

    let mut svm = LiteSVM::new();
    svm.add_program_from_file(PROGRAM_ID, fixture_dir(MINT).join("launchpad.so")).expect("program .so");
    for (k, a) in &accounts {
        svm.set_account(*k, (**a).clone()).unwrap();
    }
    let mut clock = svm.get_sysvar::<solana_clock::Clock>();
    clock.unix_timestamp = f.unix_timestamp;
    svm.set_sysvar(&clock);

    let user = Keypair::new();
    svm.airdrop(&user.pubkey(), 20_000_000_000).unwrap();

    // ---- buy 1 SOL
    let amm = amm_from(&svm, &curve);
    let sol_in = 1_000_000_000u64;
    let q = amm.quote(&QuoteParams { amount: sol_in, input_mint: WSOL, output_mint: mint, swap_mode: SwapMode::ExactIn, fee_mode: Default::default() }).unwrap();
    let stock_before = token_amount(&svm.get_account(&stock).unwrap().data).unwrap();
    let curve_lamports_before = svm.get_account(&curve).unwrap().lamports;
    let treasury_before = svm.get_account(&amm.treasury()).unwrap().lamports;
    let ix = swap_ix(&amm, &user.pubkey(), WSOL, mint, sol_in);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&user.pubkey()), &[&user], svm.latest_blockhash());
    let res = svm.send_transaction(tx);
    assert!(res.is_ok(), "buy failed: {:?}", res.err().map(|e| e.meta.logs));
    let user_ata = ata(&user.pubkey(), &mint);
    let got = token_amount(&svm.get_account(&user_ata).unwrap().data).unwrap();
    assert_eq!(got, q.out_amount, "buy: tokens received != quote");
    let stock_after = token_amount(&svm.get_account(&stock).unwrap().data).unwrap();
    assert_eq!(stock_before - stock_after, q.out_amount);
    let curve_delta = svm.get_account(&curve).unwrap().lamports - curve_lamports_before;
    let treasury_delta = svm.get_account(&amm.treasury()).unwrap().lamports - treasury_before;
    // root collection: curve gets net + creator share, treasury gets platform + owner; together the whole sol_in
    assert_eq!(curve_delta + treasury_delta, sol_in, "lamports split must add up to sol_in");
    assert_eq!(treasury_delta + curve_delta - (sol_in - q.fee_amount), q.fee_amount);

    // ---- sell everything back, quoting from the post-buy state
    let amm2 = amm_from(&svm, &curve);
    let q2 = amm2.quote(&QuoteParams { amount: got, input_mint: mint, output_mint: WSOL, swap_mode: SwapMode::ExactIn, fee_mode: Default::default() }).unwrap();
    let user_before = svm.get_account(&user.pubkey()).unwrap().lamports;
    let ix2 = swap_ix(&amm2, &user.pubkey(), mint, WSOL, got);
    let tx2 = Transaction::new_signed_with_payer(&[ix2], Some(&user.pubkey()), &[&user], svm.latest_blockhash());
    let res2 = svm.send_transaction(tx2);
    assert!(res2.is_ok(), "sell failed: {:?}", res2.err().map(|e| e.meta.logs));
    let user_after = svm.get_account(&user.pubkey()).unwrap().lamports;
    let fee = 5_000; // one signature
    assert_eq!(user_after + fee - user_before, q2.out_amount, "sell: lamports received != quote");
    assert_eq!(token_amount(&svm.get_account(&user_ata).unwrap().data).unwrap(), 0);
    assert!(q2.out_amount < sol_in);
}

#[test]
fn quote_rejects_exact_out_and_foreign_mints() {
    let (f, accounts) = load(MINT);
    let curve: Pubkey = f.curve.parse().unwrap();
    let ka = KeyedAccount { key: curve, account: (*accounts[&curve]).clone(), params: None };
    let mut amm = Nts1Amm::from_keyed_account(&ka, &AmmContext::default()).unwrap();
    amm.update(&accounts).unwrap();
    let mint: Pubkey = f.mint.parse().unwrap();
    assert!(amm.quote(&QuoteParams { amount: 1, input_mint: WSOL, output_mint: mint, swap_mode: SwapMode::ExactOut, fee_mode: Default::default() }).is_err());
    assert!(amm.quote(&QuoteParams { amount: 1, input_mint: Pubkey::new_unique(), output_mint: mint, swap_mode: SwapMode::ExactIn, fee_mode: Default::default() }).is_err());
    assert_eq!(amm.get_reserve_mints(), vec![WSOL, mint]);
}
