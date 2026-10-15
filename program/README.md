# NTS-1 program source (read-only copy)

Anchor 0.32 program `EvH7PZ34bGKi7SyiUP37G4o3B36XXhPpkWCxhfiMu9AN`, copied from the
NTS-1 monorepo for review. `src/curve.rs` is the bonding-curve math the SDK mirrors;
`src/lib.rs` has the `buy` / `sell` handlers and the `Trade` accounts the adapter
builds. `launchpad.idl.json` is the Anchor IDL (also on-chain, account
C4UMQW4Fhds27aRbWzeGi9DFmw6nmGFXwcZ4WA3b2ELC).

Build: `anchor build` inside an Anchor 0.32 workspace with this crate as a member.
