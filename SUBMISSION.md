# Jupiter integration: the submission, step by step

Done: SDK public at https://github.com/tow3web3/nts1-jupiter-sdk (adapter, parity suite,
program source), audit brief at `docs/AUDIT.md`, traction script `node scripts/traction.mjs`.

## Step 1: Discord thread (5 minutes, before the form)

1. Join https://discord.gg/jup, open the channel `#developer-support`.
2. Post this:

```
Hi, we're NTS-1 (https://nts1.fun), a Token-2022 standard where every coin is member #N of a capped collection and has its own permanent bonding curve from the first second. Program EvH7PZ34bGKi7SyiUP37G4o3B36XXhPpkWCxhfiMu9AN on mainnet-beta.

We implemented the Amm trait: https://github.com/tow3web3/nts1-jupiter-sdk (Apache-2.0, forkable). It ships a LiteSVM parity suite on a committed mainnet snapshot: quote, then the native buy and sell, on-chain deltas equal the quotes exactly.

One question before we submit the AMM integrators form: our swaps move native SOL (lamports from and to the signer, the same shape as Pump and Moonshot), so jupiter-amm-test-kit cannot drive them (SOL wrapping is a documented limitation). How do you want to handle the swap variant on your side for that? Happy to adapt the SDK to whatever shape you prefer.
```

3. Note the answer; if they ask for changes, forward them to me.

## Step 2: the audit (the real gate)

Pick one, send them `docs/AUDIT.md` and the public repo. Indicative only, ask for quotes:

| Auditor | Notes |
|---|---|
| Sec3 | Solana-focused; X-Ray automated scan is cheap and fast, manual audit on top |
| OtterSec | Solana specialists, premium, longer queue |
| Zellic | strong on Solana, premium |
| Neodyme | Solana security research team |
| Halborn | large firm, wider scope |

Ask for: scope = `programs/launchpad` (~1,500 lines), fixed-price quote, start date,
report format they publish. Jupiter wants the report link.

## Step 3: traction (keep minting, keep trading)

Run `node scripts/traction.mjs` the day you submit and paste the output. The more
coins traded on nts1.fun the better: Jupiter's market listing rule routes a market only
if a $500 round trip moves the price less than 30 % (or a $1000 buy vs a $500 buy has
under 20 % impact). A coin with a few SOL of reserve passes, an empty one does not.

## Step 4: the form

https://support.jup.ag/requests/new/amm-integrators

- **Project:** NTS-1, the Numbered Token Standard, https://nts1.fun
- **Program ID:** EvH7PZ34bGKi7SyiUP37G4o3B36XXhPpkWCxhfiMu9AN (mainnet-beta)
- **SDK repository:** https://github.com/tow3web3/nts1-jupiter-sdk (Apache-2.0, forkable; parity suite in `tests/parity.rs`, fixtures committed)
- **Description:** paste this:

```
NTS-1 is a token standard on Token-2022: every coin is a fungible mint that is also member #N of a capped collection (TokenGroupMember / TokenGroup extensions) and trades on its own permanent constant-product bonding curve from the first second. One Curve account per coin is the market between SOL and the coin. Swaps are exact-in buy (SOL -> coin) and sell (coin -> SOL) with a 1 % fee on the SOL side. The SDK implements the Amm trait with no network calls (curve, config and stock accounts delivered through update) and emits the 11 swap accounts in on-chain order with Swap::Placeholder { data: [0 buy | 1 sell] }. Markets are discoverable with getProgramAccounts on the program (Curve discriminator [191,180,249,66,180,71,51,182], data length 135). Native SOL note: the program moves lamports from and to the signer (same shape as Pump and Moonshot), which is why parity is proven with our LiteSVM harness instead of the test kit; see the Discord thread <link>.
```

- **Security audit:** link to the report (Step 2)
- **Traction:** output of `scripts/traction.mjs` + the $NTS1 token on pump.fun (CA 65XDoCF6mLGpp7nK8CnaVZV7TLAPeYjkva2K3LYr44vi)
- **Team:** your name / handle, links

## Step 5: review

Jupiter may ask for SDK changes and will add a swap variant for the native-SOL shape
on their side. Send me whatever they write; the code side is mine.

## Step 6: after listing

Each coin is its own market. Jupiter rechecks liquidity every 30 minutes and only routes
the markets that pass the rule in Step 3. Aggregators and terminals (Axiom, DexScreener,
Photon) then pick the coins up from Jupiter routing.
