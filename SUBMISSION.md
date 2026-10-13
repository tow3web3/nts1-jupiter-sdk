# Jupiter integration: what to submit

Form: https://support.jup.ag/requests/new/amm-integrators
Discord: #developer-support on discord.gg/jup (mention the native-SOL point below)

## Text to paste

**Project:** NTS-1, the Numbered Token Standard (https://nts1.fun). Every coin is a
Token-2022 mint that is also member #N of a capped collection, with its own permanent
constant-product bonding curve from the first second. Program
`5ZmEp1vQj45R3itxEo6hgH8aX4E8UNCkRsTw13gjchco` on mainnet-beta, Anchor 0.32.

**SDK:** https://github.com/tow3web3/nts1-jupiter-sdk (Apache-2.0, forkable).
Implements `jupiter-amm-interface::Amm` for the `Curve` account: deserialisation,
exact-in quotes for buy (SOL -> coin) and sell (coin -> SOL), 1 % fee on the SOL
side, swap account metas in on-chain order, `Swap::Placeholder { data: [0 | 1] }`
for the direction. No network calls.

**Parity:** `tests/parity.rs` loads a committed mainnet snapshot of a live curve and
the program binary into LiteSVM, runs `Amm::quote`, executes the native `buy` then
`sell`, and asserts the on-chain token and lamport deltas equal the quotes exactly.
We did not use `jupiter-amm-test-kit` because the program moves native SOL
(lamports from and to the signer, like Pump / Moonshot / Boop), which the kit lists
as a limitation. We would need a "wrapped buy / wrapped sell" style variant on the
Jupiter side, as for those launchpads.

**Markets:** one curve per coin, discoverable with `getProgramAccounts` on the
program, `Curve` discriminator `[191,180,249,66,180,71,51,182]`, data length 135.
Reserve mints `[WSOL, coin]`. New markets appear at mint time.

**Security audit:** [link to the audit report]

**Traction:** [coins minted, volume, holders, links to the site and the $NTS1 token]

**Team:** [names, links]

## Before sending

- [ ] Publish the SDK as its own public repository (copy of `jupiter/` from the
      monorepo, with `tests/fixtures/` committed) and put the link above.
- [ ] Audit of the program (the form asks for it; the curves hold SOL).
- [ ] Traction numbers from https://nts1.fun/api/root.
- [ ] Open a Discord thread first: ask how they want to handle the native-SOL swap
      (dedicated variant), it unblocks the review.
