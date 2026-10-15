// Snapshot the accounts of one NTS-1 curve from mainnet into a fixture the parity
// test can load offline:  node scripts/snapshot.mjs <coin mint>
// Writes tests/fixtures/<coin mint>/accounts.json (base64 data) and copies the
// program binary next to it when ../target/deploy/launchpad.so exists.
import { readFileSync, writeFileSync, mkdirSync, existsSync, copyFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Connection, PublicKey } from "@solana/web3.js";

const here = dirname(fileURLToPath(import.meta.url));
const PROGRAM = new PublicKey("Ak5FkaLsCjR9MADLshY5wh7Nj1kcJNnCtf4iXX8FiZBH");
const mint = new PublicKey(process.argv[2] ?? (() => { throw new Error("usage: snapshot.mjs <coin mint>"); })());
const rpc = process.env.RPC ?? "https://api.mainnet-beta.solana.com";
const conn = new Connection(rpc, "confirmed");

const pda = (seeds) => PublicKey.findProgramAddressSync(seeds, PROGRAM)[0];
const config = pda([Buffer.from("config")]);
const curve = pda([Buffer.from("curve"), mint.toBuffer()]);
const stock = pda([Buffer.from("stock"), mint.toBuffer()]);

const curveInfo = await conn.getAccountInfo(curve);
if (!curveInfo) throw new Error("curve not found");
const groupMint = new PublicKey(curveInfo.data.subarray(40, 72));
const group = pda([Buffer.from("group"), groupMint.toBuffer()]);
const cfgInfo = await conn.getAccountInfo(config);
const treasury = new PublicKey(cfgInfo.data.subarray(72, 104));

const keys = [curve, config, stock, group, mint, treasury];
const infos = await conn.getMultipleAccountsInfo(keys);
const accounts = keys.map((k, i) => {
  const a = infos[i];
  if (!a) throw new Error("missing " + k.toBase58());
  return { pubkey: k.toBase58(), lamports: a.lamports, owner: a.owner.toBase58(), executable: a.executable, rent_epoch: 0, data: Buffer.from(a.data).toString("base64") };
});
const slot = await conn.getSlot();
const clock = await conn.getAccountInfo(new PublicKey("SysvarC1ock11111111111111111111111111111111"));
const unix = Number(clock.data.readBigInt64LE(32));

const dir = join(here, "..", "tests", "fixtures", mint.toBase58());
mkdirSync(dir, { recursive: true });
writeFileSync(join(dir, "accounts.json"), JSON.stringify({ program: PROGRAM.toBase58(), mint: mint.toBase58(), curve: curve.toBase58(), config: config.toBase58(), stock: stock.toBase58(), group: group.toBase58(), treasury: treasury.toBase58(), slot, unix_timestamp: unix, accounts }, null, 1));
const so = join(here, "..", "..", "target", "deploy", "launchpad.so");
if (existsSync(so)) copyFileSync(so, join(dir, "launchpad.so"));
console.log("fixture", dir, accounts.length, "accounts, slot", slot, existsSync(so) ? "+ program" : "(no program .so found)");
