// Phase 4: a confidential payment between two wallets on devnet with Helius Privacy Rings,
// carried by the two GhostMessage message formats it needs:
//   kind = 2  B → A  B's shielded address
//   kind = 1  A → B  payment note {tx signature, mint, amount}
// Both are NaCl-box encrypted exactly as they are in a room (phase 5 sends them through one).
//
//   HELIUS_API_KEY in ../.env; funded from the Solana CLI wallet (~/.config/solana/id.json).
//   npm run payment
import { Connection, Keypair, LAMPORTS_PER_SOL, PublicKey, SystemProgram } from "@solana/web3.js";
import { homedir } from "node:os";
import * as cr from "../src/crypto";
import { loadKey } from "../src/ghost";
import { check, log, send } from "../src/net";
import * as pay from "../src/payment";

const DEPOSIT = 10_000_000n; // 0.01 SOL into A's private balance
const AMOUNT = 4_000_000n; // 0.004 SOL paid privately to B

async function main() {
  const apiKey = process.env.HELIUS_API_KEY?.trim();
  if (!apiKey) throw new Error("set HELIUS_API_KEY in ghost-message/.env");
  const client = await pay.connectHelius(apiKey);
  const rpc = new Connection(`https://devnet.helius-rpc.com/?api-key=${apiKey}`, "confirmed");

  // Two fresh wallets, funded from the CLI wallet.
  const funder = loadKey(process.env.FUNDER_KEYPAIR ?? `${homedir()}/.config/solana/id.json`);
  const a = Keypair.generate();
  const b = Keypair.generate();
  await send(rpc, "fund A and B", [
    SystemProgram.transfer({ fromPubkey: funder.publicKey, toPubkey: a.publicKey, lamports: 0.05 * LAMPORTS_PER_SOL }),
    SystemProgram.transfer({ fromPubkey: funder.publicKey, toPubkey: b.publicKey, lamports: 0.02 * LAMPORTS_PER_SOL }),
  ], [funder]);
  log(`A ${a.publicKey.toBase58()}  B ${b.publicKey.toBase58()}`);

  const aKeys = pay.shieldedFromWallet(a);
  const bKeys = pay.shieldedFromWallet(b);
  check(aKeys.toSolanaSigner().address === a.publicKey.toBase58(), "private wallet signer = Solana wallet (same seed)");
  const aBox = cr.deriveBoxKey(cr.signWith(a.secretKey));
  const bBox = cr.deriveBoxKey(cr.signWith(b.secretKey));

  // 1. B registers its private wallet (once).
  log(`B register: ${await pay.register(client, bKeys)}`);

  // 2. B → A: shielded address as a kind = 2 message.
  const addressMsg = cr.seal(cr.encodeShieldedAddress(bKeys.shieldedAddress().toBytes()), aBox.publicKey, bBox);
  log(`  kind=2 plaintext ${bKeys.shieldedAddress().toBytes().length + 1} bytes, ciphertext ${addressMsg.ciphertext.length} bytes`);
  const opened = cr.open(addressMsg, bBox.publicKey, aBox);
  check(opened !== null, "A decrypts B's shielded address");
  const bAddress = pay.ShieldedAddress.fromBytes(cr.decodeShieldedAddress(opened!));

  // 3. A deposits and pays B privately.
  const t0 = Date.now();
  const { transfer, deposit } = await pay.privateTransferSol(client, aKeys, bAddress, AMOUNT, { depositIfShort: DEPOSIT });
  log(`deposit ${deposit?.signature}`);
  log(`private transfer ${transfer.signature} (${((Date.now() - t0) / 1000).toFixed(1)}s incl. deposit + proof)`);

  // 4. A → B: payment note as a kind = 1 message.
  const note = cr.encodePaymentNote({
    signature: pay.signatureToBytes(transfer.signature),
    mint: new PublicKey(pay.SOL),
    amount: AMOUNT,
  });
  const noteMsg = cr.seal(note, bBox.publicKey, aBox);
  log(`  kind=1 plaintext ${note.length} bytes, ciphertext ${noteMsg.ciphertext.length} bytes`);

  // 5. B decrypts the note — "claimed by sender" — and confirms against its own private balance.
  const claimed = cr.decodePaymentNote(cr.open(noteMsg, aBox.publicKey, bBox)!);
  log(`B sees a claim: ${claimed.amount} lamports of ${claimed.mint.toBase58()} in ${pay.signatureFromBytes(claimed.signature)}`);
  const received = await pay.solReceivedIn(client, bKeys, transfer);
  check(received.amount === claimed.amount, `B finds a ${received.amount}-lamport UTXO in that transaction: claim confirmed`);
  const bBalance = await pay.privateSolBalance(client, bKeys);
  check(bBalance.amount === AMOUNT, `B's synced private balance is ${bBalance.amount}`);
  const aBalance = await pay.privateSolBalance(client, aKeys);
  log(`A's private balance after paying: ${aBalance.amount} (change)`);

  // 6. What anyone sees of the transfer on Solana.
  const tx = await rpc.getTransaction(transfer.signature, { maxSupportedTransactionVersion: 1 } as never).catch(() => null);
  if (tx) {
    const keys = tx.transaction.message.staticAccountKeys.map((k: PublicKey) => k.toBase58());
    const pre = tx.meta?.preBalances ?? [];
    const post = tx.meta?.postBalances ?? [];
    log(`public view of the transfer: ${keys.length} account keys; A in keys: ${keys.includes(a.publicKey.toBase58())}; B in keys: ${keys.includes(b.publicKey.toBase58())}`);
    log(`  lamport changes: ${keys.map((k: string, i: number) => `${k.slice(0, 6)}… ${(post[i] ?? 0) - (pre[i] ?? 0)}`).filter((s: string) => !s.endsWith(" 0")).join(", ")}`);
  } else {
    log("public view: web3.js 1.x can't decode a v1 transaction; inspect it in an explorer");
  }
  console.log(`\nexplorer: https://explorer.solana.com/tx/${transfer.signature}?cluster=devnet`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
