# nts1-jupiter-sdk

Jupiter AMM adapter for NTS-1 bonding curves. One `Curve` account = one market
between SOL and one NTS-1 coin (a Token-2022 mint with 9 decimals).

- Program: `5ZmEp1vQj45R3itxEo6hgH8aX4E8UNCkRsTw13gjchco` (mainnet-beta and devnet, same id)
- Source: `../programs/launchpad` (Anchor 0.32), audited copy of the math in `src/curve.rs`
- Interface: `jupiter-amm-interface` (`Amm` trait), label `NTS-1`

## What the adapter does

| Trait method | Behaviour |
|---|---|
| `from_keyed_account` | parses a `Curve` account (discriminator checked), derives the `config`, `group` and `stock` PDAs |
| `get_reserve_mints` | `[WSOL, coin mint]` |
| `get_accounts_to_update` | `[curve, config, stock]`: curve state, treasury address, unsold supply |
| `quote` (ExactIn only) | buy: fee = 1 % of SOL in, tokens = constant product on the net; sell: SOL = constant product, fee = 1 % of the SOL out. Buys are capped by the stock. |
| `get_swap_and_account_metas` | the 11 accounts of the program's `Trade` context in on-chain order, `Swap::Placeholder { data: [0 buy / 1 sell] }` |
| `program_dependencies` | Token-2022, Associated Token |

No network calls anywhere.

## Native SOL

The program takes lamports from the signer on `buy` and pays lamports to the
signer on `sell`; it does not use a wrapped-SOL token account. This is the same
shape as Pump, Moonshot and Boop, which Jupiter routes through dedicated
"wrapped buy / wrapped sell" variants. The `Placeholder` data carries the only
decision bit (direction); the instruction data is built in `encode::encode_swap`.

Because the swap moves native SOL, the `jupiter-amm-test-kit` parity harness
cannot drive it yet (documented limitation: SOL wrapping). `tests/parity.rs`
does the equivalent with LiteSVM: load a mainnet snapshot, quote, execute the
native instruction, assert exact deltas (tokens received on buy, lamports
received on sell, fee split adding up).

## Run

```
cargo test
```

Fixtures live in `tests/fixtures/<coin mint>/` (`accounts.json` + `launchpad.so`).
Refresh one with:

```
RPC=<mainnet rpc> node scripts/snapshot.mjs <coin mint>
```

## Markets

Each coin is its own market. Discover curves with `getProgramAccounts` on the
program filtered by the `Curve` discriminator `[191,180,249,66,180,71,51,182]`
(data length 135). New coins appear at mint time, about one per click on
https://nts1.fun.
