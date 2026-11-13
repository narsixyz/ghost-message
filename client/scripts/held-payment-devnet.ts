// Held payments on devnet: private funds owned by a GhostMessage PDA, released by the program
// through a CPI into the Helius Privacy Program (SPP).
//
//   1. A locks 0.004 SOL for B            (private transfer to the payment PDA)
//   2. A can't refund before the deadline (program: PaymentNotRefundable)
//   3. Eve can't claim, even holding the PDA's keys (program: wrong PDA for her signature)
//   4. B claims                           (GhostMessage ClaimPayment → SPP transact, PDA-signed)
//   5. A locks 0.003 SOL with a short deadline, waits it out, refunds (RefundPayment)
//
//   HELIUS_API_KEY in ../.env; funded from the Solana CLI wallet (~/.config/solana/id.json);
//   the program (with ClaimPayment/RefundPayment) deployed at GHOST_PROGRAM_ID or the
//   target/deploy keypair's address.
//   bun run held
import { Connection, Keypair, LAMPORTS_PER_SOL, SystemProgram } from "@solana/web3.js";
import { homedir } from "node:os";
import { loadKey, programId } from "../src/ghost";
import * as held from "../src/held-payment";
import { check, log, send, sleep } from "../src/net";
import * as pay from "../src/payment";

const DEPOSIT = 12_000_000n; // A's deposit: covers both held payments
const AMOUNT = 4_000_000n; // held for B, claimed
const REFUNDED = 3_000_000n; // held for B, refunded to A
const SHORT_DEADLINE_S = 25;

/** Runs `p`, expecting the program to reject it with one of `codes` (custom error numbers). */
async function rejects(p: Promise<unknown>, codes: number[], what: string) {
  try {
    await p;
  } catch (e) {
    const text = errorText(e);
    const hit = codes.find((c) => text.includes(`custom program error: 0x${c.toString(16)}`) || text.includes(`"Custom":${c}`));
    if (hit !== undefined) return log(`  ok: ${what} (custom error ${hit})`);
    throw new Error(`${what}: expected custom error ${codes.join(" or ")}, got:\n${text.slice(0, 1500)}`);
  }
  throw new Error(`${what}: expected a failure, but it succeeded`);
}

function errorText(e: unknown): string {
  const seen = new Set<unknown>();
  const parts: string[] = [];
  const walk = (x: unknown) => {
    if (!x || typeof x !== "object" || seen.has(x)) return;
    seen.add(x);
    const o = x as { message?: string; context?: unknown; cause?: unknown; logs?: string[] };
    if (o.message) parts.push(o.message);
    try {
      parts.push(JSON.stringify(o.context, (_, v) => (typeof v === "bigint" ? Number(v) : v)));
    } catch {}
    if (o.logs) parts.push(o.logs.join("\n"));
    walk(o.cause);
  };
  walk(e);
  return parts.join("\n");
}

/** Raw transaction bytes as anyone fetches them (v1-capable, base64). */
async function rawTx(url: string, signature: string): Promise<Uint8Array> {
  for (let i = 0; i < 20; i++) {
    const r = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "getTransaction",
        params: [signature, { encoding: "base64", maxSupportedTransactionVersion: 1, commitment: "confirmed" }],
      }),
    }).then((x) => x.json());
    if (r.result) return Uint8Array.from(Buffer.from(r.result.transaction[0], "base64"));
    await sleep(500);
  }
  throw new Error(`getTransaction ${signature}: not found`);
}

const contains = (hay: Uint8Array, needle: Uint8Array) =>
  Buffer.from(hay.buffer, hay.byteOffset, hay.length).indexOf(Buffer.from(needle)) >= 0;

async function chainTime(rpc: Connection): Promise<bigint> {
  return BigInt((await rpc.getBlockTime(await rpc.getSlot("confirmed"))) ?? Math.floor(Date.now() / 1000));
}

async function main() {
  const apiKey = process.env.HELIUS_API_KEY?.trim();
  if (!apiKey) throw new Error("set HELIUS_API_KEY in ghost-message/.env");
  const url = `https://devnet.helius-rpc.com/?api-key=${encodeURIComponent(apiKey)}`;
  const client = await pay.connectHelius(apiKey);
  const rpc = new Connection(url, "confirmed");
  log(`program ${programId.toBase58()}`);
  const programInfo = await rpc.getAccountInfo(programId);
  if (!programInfo?.executable) throw new Error(`program ${programId.toBase58()} is not deployed on devnet`);

  const funder = loadKey(process.env.FUNDER_KEYPAIR ?? `${homedir()}/.config/solana/id.json`);
  const [a, b, eve] = [Keypair.generate(), Keypair.generate(), Keypair.generate()];
  await send(rpc, "fund A, B and Eve", [
    SystemProgram.transfer({ fromPubkey: funder.publicKey, toPubkey: a.publicKey, lamports: 0.06 * LAMPORTS_PER_SOL }),
    SystemProgram.transfer({ fromPubkey: funder.publicKey, toPubkey: b.publicKey, lamports: 0.03 * LAMPORTS_PER_SOL }),
    SystemProgram.transfer({ fromPubkey: funder.publicKey, toPubkey: eve.publicKey, lamports: 0.02 * LAMPORTS_PER_SOL }),
  ], [funder]);
  log(`A ${a.publicKey.toBase58()}  B ${b.publicKey.toBase58()}  Eve ${eve.publicKey.toBase58()}`);
  const [aKeys, bKeys, eveKeys] = [a, b, eve].map(pay.shieldedFromWallet);
  log(`B register: ${await pay.register(client, bKeys)}`);

  // 1. A locks AMOUNT for B. The PDA's keys come from A's and B's viewing keys; both sides agree.
  const p1 = held.heldPayment(a.publicKey, b.publicKey, (await chainTime(rpc)) + 3600n);
  const p1ForA = held.pdaKeys(aKeys, bKeys.shieldedAddress(), p1);
  const p1ForB = held.pdaKeys(bKeys, aKeys.shieldedAddress(), p1);
  check(
    Buffer.from(p1ForA.address.toBytes()).equals(Buffer.from(p1ForB.address.toBytes())),
    "A and B derive the same PDA shielded address (ECDH)",
  );
  log(`payment PDA ${p1.pda.toBase58()} (deadline ${p1.deadline})`);
  let t = Date.now();
  const { transfer: lock1, deposit } = await held.lock(client, aKeys, p1ForA, AMOUNT, { depositIfShort: DEPOSIT });
  log(`deposit ${deposit?.signature}`);
  log(`lock ${lock1.signature} (${((Date.now() - t) / 1000).toFixed(1)}s incl. deposit + proof)`);
  const lockBytes = await rawTx(url, lock1.signature);
  check(contains(lockBytes, held.addressBytes(p1.pda)), "the lock names the payment PDA (its view tag)");
  check(!contains(lockBytes, held.addressBytes(b.publicKey)), "B's address is NOT in the lock transaction");
  const seen = await held.heldBalance(client, p1ForB);
  check(seen.amount === AMOUNT, `B sees ${seen.amount} lamports held for it`);

  // 2. A can't take it back before the deadline.
  await rejects(
    held.release(client, "refund", aKeys, p1, p1ForA, aKeys.shieldedAddress()),
    [11],
    "A's refund before the deadline is rejected (PaymentNotRefundable)",
  );

  // 3. Eve can't claim, even with the PDA's keys: the program derives the PDA from the signer.
  await rejects(
    held.release(client, "claim", eveKeys, p1, p1ForB, eveKeys.shieldedAddress()),
    [12, 14],
    "Eve's claim is rejected (no PDA signature for her)",
  );

  // 4. B claims into its own private balance.
  t = Date.now();
  const claim = await held.release(client, "claim", bKeys, p1, p1ForB, bKeys.shieldedAddress());
  log(`claim ${claim.signature} (${((Date.now() - t) / 1000).toFixed(1)}s incl. proof)`);
  const got = await pay.solReceivedIn(client, bKeys, claim);
  check(got.amount === AMOUNT, `B received ${got.amount} lamports in the claim`);
  check((await held.heldBalance(client, p1ForB)).amount === 0n, "the PDA holds nothing after the claim");
  const claimBytes = await rawTx(url, claim.signature);
  check(contains(claimBytes, held.addressBytes(programId)), "the claim went through GhostMessage");
  log(`  note: A's address is in the claim (ClaimPayment's payer field): ${contains(claimBytes, held.addressBytes(a.publicKey))}`);

  // 5. A locks REFUNDED with a short deadline, waits, refunds.
  const p2 = held.heldPayment(a.publicKey, b.publicKey, (await chainTime(rpc)) + BigInt(SHORT_DEADLINE_S));
  const p2ForA = held.pdaKeys(aKeys, bKeys.shieldedAddress(), p2);
  const lock2 = await held.lock(client, aKeys, p2ForA, REFUNDED);
  log(`lock #2 ${lock2.transfer.signature} (deadline ${p2.deadline})`);
  while ((await chainTime(rpc)) <= p2.deadline) await sleep(2000);
  log("deadline passed on-chain");
  const refund = await held.release(client, "refund", aKeys, p2, p2ForA, aKeys.shieldedAddress());
  log(`refund ${refund.signature}`);
  const back = await pay.solReceivedIn(client, aKeys, refund);
  check(back.amount === REFUNDED, `A got ${back.amount} lamports back`);

  console.log("\nexplorer:");
  for (const [what, sig] of [["lock", lock1.signature], ["claim", claim.signature], ["refund", refund.signature]])
    console.log(`  ${what.padEnd(6)} https://explorer.solana.com/tx/${sig}?cluster=devnet`);
}

main().catch((e) => {
  console.error(errorText(e) || e);
  process.exit(1);
});
