# NTS-1 program: audit brief

For the auditor. Scope, invariants, threat model, how to build and test.

## What it is

Anchor 0.32 program `EvH7PZ34bGKi7SyiUP37G4o3B36XXhPpkWCxhfiMu9AN` (mainnet-beta,
upgrade authority `HY8pKY8156hWXjL2z8T15siHQzW7VdUmyochRNb3sw6N`). ~1,500 lines of
Rust in `programs/launchpad/src/` (`lib.rs` handlers and accounts, `state.rs`,
`curve.rs` math, `traits.rs` seed to traits, `errors.rs`). Source is public in
https://github.com/tow3web3/nts1-jupiter-sdk/tree/master/program.

Every coin is a Token-2022 mint with MetadataPointer + TokenMetadata and
GroupMemberPointer + TokenGroupMember. Collections are mints with GroupPointer +
TokenGroup (`max_size` = the cap). The program's `Config` PDA is mint and update
authority of everything. Each coin has a `Curve` PDA holding SOL as native lamports
and a `Stock` token account holding the unsold supply (1,000,000,000 × 10^9 units).

## Instructions

| Instruction | Signer | Moves value |
|---|---|---|
| `init_config` | admin | creates Config + root group |
| `update_config` | admin | fee split, curve params, claim window, payer, treasury (future launches only) |
| `create_group` | creator | new collection (member of the root), pays rent + launch budget |
| `set_group_info` | owner | description + url, only while `size == 0` |
| `fund_group` | anyone | SOL into a group budget |
| `set_group` | owner | open/close launches, transfer ownership |
| `launch` | payer (backend) or any wallet | mint + member number + metadata + supply into the stock; when the payer is `config.payer` the group budget reimburses the rent in the same tx |
| `claim` | claimer | secret check (sha256), writes creator + claimed_at |
| `set_nickname` | creator | once |
| `expire` | anyone | after the claim window, untraded: closes curve + stock, rent back to the budget |
| `buy` | trader | SOL in → tokens out, 1 % fee split platform / owner / creator |
| `sell` | trader | tokens in → SOL out, 1 % fee on the SOL |
| `withdraw_owner_fees` / `withdraw_creator_fees` | owner / creator | collect accrued shares |

## Invariants to check

1. **Curve solvency.** `curve.reserve` lamports are always present on the Curve PDA
   (plus `creator_fees` kept there), and `reserve` only moves through `buy` and `sell`.
   No instruction lets the admin or anyone else move reserve lamports.
2. **Constant product never shrinks.** `x = virtual_sol + reserve`, `y = virtual_tokens − sold`;
   every rounding in `curve.rs` favours the pool (`div_ceil`). A buy then a sell of the
   same tokens never returns more SOL than put in.
3. **Stock accounting.** `stock.amount == supply − sold` at all times; `buy` refuses more
   than the stock; `expire` only when `trades == 0`.
4. **Numbering.** `member_number` is assigned by Token-2022 `initialize_member`; the cap
   is `max_size` enforced by Token-2022 (`SizeExceedsMaxSize`). The program never
   writes a number itself; `Launch.number` is a copy for indexing.
5. **Budget reimbursement.** In `launch`, the group budget reimburses exactly the rent
   spent by `config.payer` and nothing more; a wallet launch pays its own rent and is
   not reimbursed.
6. **Fees add up.** For every trade, platform + owner + creator shares = `fee_of(amount)`;
   root collection: owner share goes to the treasury.
7. **Claim secrecy.** Only `sha256(secret)` is on-chain; `claim` compares hashes; a
   claimed launch cannot be claimed again or expired.
8. **Authorities.** Mint/update authority of every mint and group is the Config PDA.
   `update_config` cannot change existing curves (parameters are copied at launch).
9. **Metadata fields** written by the program are the only writer (`standard`, `number`,
   `max`, `seed`, `traits`, `special`, `prompt`, `creator`, `claimed_at`, `nickname`).

## Threat model

- Attacker drains a curve: via re-entrancy, lamport arithmetic, or a fake `treasury`
  / `group` account (both are constrained: `has_one = treasury` on Config, group PDA
  seeds from `curve.group`).
- Fake accounts in `Trade`: mint not matching curve, stock PDA, Token-2022 vs SPL
  Token program substitution, ATA ownership.
- Griefing: minting with a huge budget drain; `expire` on a traded coin; `set_group_info`
  after members exist.
- Integer overflow in `u128` curve math; `fee_of` and `split` rounding dust.
- Reimbursement over-payment in `launch` (rent computation vs actual lamports spent).
- Anything the admin can do to user funds (expected: nothing).

## Build and test

```
MSYS_NO_PATHCONV=1 wsl -e bash scripts/build.sh build   # anchor build (agave 4.1.2)
MSYS_NO_PATHCONV=1 wsl -e bash scripts/build.sh test    # 13 mocha integration tests on a local validator
cargo test -p launchpad                                 # curve unit tests
cd jupiter && cargo test                                # Jupiter adapter + LiteSVM parity on a mainnet snapshot
```

Tests live in `tests/launchpad.ts` (launch, cap refusal at 11/10, claim, nickname,
expire, buy/sell slippage, fee withdrawals, policies, prompts) and `jupiter/tests/parity.rs`.

## Deployed state (2026-10-14)

- Config PDA `2dYT2e5rbovMAhoyF8veo7v64EggFbyi1wyczruzat9X`, admin = treasury
  `6oVAsXnwoEQHJyuYNEAPuHQGgDHrH6ACDvh2ZU2qMMrU`, payer `DUeaMd4ivvHoiGgSrVsYKhnquZzKECoZ83QnGKGjQ9Vz`.
- Root collection CRYPTID `9SaqMfBE5vsVb2C8hD1x3AnpAGLQWWedNjKFYFnp7PE9`, cap 1,000.
- Curve params: 30 SOL virtual, 1.073 B virtual tokens, 1 B supply, 1 % fee
  (0.40 / 0.30 / 0.30), claim window 7 days.
