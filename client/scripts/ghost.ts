// GhostMessage CLI (devnet): private rooms on MagicBlock's TEE + private payments with Helius.
//
//   bun run ghost -- scenario                 full two-wallet run (phase 5)
//   bun run ghost -- create <room_id>         create + delegate a room
//   bun run ghost -- open <room> <guest>      set the guest, create the private permission
//   bun run ghost -- join <room>              join as the guest
//   bun run ghost -- send <room> <text...>    encrypt and send a text message
//   bun run ghost -- read <room>              decrypt the room as a member
//   bun run ghost -- close <room>             wipe the room, then undelegate it
//
// Wallet: --key <path> (default ~/.config/solana/id.json). HELIUS_API_KEY in ../.env.
// Endpoints: BASE_URL (default Helius devnet, else api.devnet.solana.com),
//            TEE_URL (default https://devnet-tee.magicblock.app).
import { verifyTeeRpcIntegrity } from "@magicblock-labs/ephemeral-rollups-sdk";
import { Connection, Keypair, LAMPORTS_PER_SOL, PublicKey, SystemProgram } from "@solana/web3.js";
import { homedir } from "node:os";
import * as cr from "../src/crypto";
import * as g from "../src/ghost";
import { authedConnection, check, expectCustom, log, send, unix, view, waitFor } from "../src/net";
import * as pay from "../src/payment";

const API_KEY = process.env.HELIUS_API_KEY?.trim();
const BASE_URL =
  process.env.BASE_URL ?? (API_KEY ? `https://devnet.helius-rpc.com/?api-key=${API_KEY}` : "https://api.devnet.solana.com");
const TEE_URL = process.env.TEE_URL ?? "https://devnet-tee.magicblock.app";
const base = new Connection(BASE_URL, "confirmed");

const argv = process.argv.slice(2);
const flag = (name: string) => {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 ? argv.splice(i, 2)[1] : undefined;
};
const wallet = g.loadKey(flag("key") ?? `${homedir()}/.config/solana/id.json`);
const noPay = argv.includes("--no-pay") && argv.splice(argv.indexOf("--no-pay"), 1).length > 0;
const [command, ...args] = argv;

let attested = false;
async function tee(key: Keypair): Promise<Connection> {
  if (!attested) {
    await verifyTeeRpcIntegrity(TEE_URL); // Intel TDX quote over a fresh challenge
    attested = true;
    log(`TEE attestation verified for ${TEE_URL}`);
  }
  return authedConnection(TEE_URL, key);
}

async function readRoom(conn: Connection, room: PublicKey) {
  const acc = await conn.getAccountInfo(room);
  if (!acc) throw new Error(`can't read ${room.toBase58()} (not a member, or not on this endpoint)`);
  return g.decodeRoom(acc.data as Buffer);
}

/** Decrypts every message as `me` and renders it by kind. */
function render(room: g.Room, me: Keypair) {
  const myBox = cr.deriveBoxKey(cr.signWith(me.secretKey));
  const iAmCreator = room.creator.equals(me.publicKey);
  const peer = iAmCreator ? room.guestBoxPk : room.creatorBoxPk;
  return room.messages.map((m) => {
    const plain = cr.open(m, peer, myBox);
    let body = "<can't decrypt>";
    if (plain) {
      if (m.kind === g.KIND.text) body = cr.decodeText(plain);
      if (m.kind === g.KIND.shieldedAddress) body = `[shielded address, ${plain.length - 1} bytes]`;
      if (m.kind === g.KIND.paymentNote) {
        const n = cr.decodePaymentNote(plain);
        body = `[payment note: ${n.amount} lamports, tx ${pay.signatureFromBytes(n.signature).slice(0, 12)}… — claimed by sender]`;
      }
    }
    return `#${m.seq} ${m.sender}: ${body}`;
  });
}

async function undelegateAndReclaim(payer: Keypair, conn: Connection, room: PublicKey, creator: PublicKey) {
  await send(conn, "UndelegateRoom", [g.undelegateRoom(payer.publicKey, room)], [payer], { skipPreflight: true });
  const t = Date.now();
  await waitFor("room back on Solana", async () => {
    const acc = await base.getAccountInfo(room);
    return acc?.owner.equals(g.programId) ? acc : null;
  }, 180_000);
  log(`  room back on Solana after ${((Date.now() - t) / 1000).toFixed(1)}s`);
  await send(base, "ReclaimRoom", [g.reclaimRoom(creator, room)], [payer]);
}

// ---------------------------------------------------------------------------------------------

async function scenario() {
  if (!API_KEY && !noPay) throw new Error("set HELIUS_API_KEY in ghost-message/.env, or run with --no-pay");
  const timings: [string, number][] = [];
  const step = async <T>(label: string, f: () => Promise<T>): Promise<T> => {
    const t = Date.now();
    const v = await f();
    timings.push([label, Date.now() - t]);
    return v;
  };

  const a = Keypair.generate(); // creator, pays
  const b = Keypair.generate(); // guest, gets paid
  const eve = Keypair.generate(); // outsider with a valid TEE token
  await send(base, "fund A, B", [
    SystemProgram.transfer({ fromPubkey: wallet.publicKey, toPubkey: a.publicKey, lamports: 0.1 * LAMPORTS_PER_SOL }),
    SystemProgram.transfer({ fromPubkey: wallet.publicKey, toPubkey: b.publicKey, lamports: 0.05 * LAMPORTS_PER_SOL }),
  ], [wallet]);
  log(`program ${g.programId.toBase58()}  A ${a.publicKey.toBase58()}  B ${b.publicKey.toBase58()}`);
  const aBox = cr.deriveBoxKey(cr.signWith(a.secretKey));
  const bBox = cr.deriveBoxKey(cr.signWith(b.secretKey));

  // 1. A creates the room and delegates it to the TEE validator (Solana).
  const roomId = BigInt(unix());
  const room = g.roomPda(a.publicKey, roomId);
  await step("CreateRoom + DelegateRoom (Solana)", () =>
    send(base, "CreateRoom + DelegateRoom", [g.createRoom(a.publicKey, roomId), g.delegateRoom(a.publicKey, room)], [a]),
  );
  const rec = await base.getAccountInfo(g.delegationRecord(room));
  check(new PublicKey(rec!.data.subarray(8, 40)).equals(g.TEE_VALIDATOR), "delegated to the TEE validator");

  // 2. A opens it for B (PER): guest + private permission in one instruction.
  const aTee = await step("TEE attestation + auth (A)", () => tee(a));
  const bTee = await tee(b);
  const eveTee = await tee(eve);
  await waitFor("room on the TEE", async () => (await aTee.getAccountInfo(room)) ?? null, 30_000);
  await step("OpenRoom (PER)", () =>
    send(aTee, "OpenRoom", [g.openRoom(a.publicKey, room, b.publicKey, aBox.publicKey, BigInt(unix() + 3600))], [a], { skipPreflight: true }),
  );
  check((await view(eveTee, room)).startsWith("hidden"), "an outsider can't read the room on the TEE");
  check((await view(bTee, room)) === "visible", "B can read the room");

  // 3. Both create V2 session keys (Solana), so chatting needs no wallet approvals.
  const aSess = Keypair.generate();
  const bSess = Keypair.generate();
  const aTok = g.sessionTokenV2(aSess.publicKey, a.publicKey);
  const bTok = g.sessionTokenV2(bSess.publicKey, b.publicKey);
  await step("create_session_v2 x2 (Solana)", async () => {
    await send(base, "session A", [g.createSessionV2(a.publicKey, aSess.publicKey, BigInt(unix() + 3600), false)], [a, aSess]);
    await send(base, "session B", [g.createSessionV2(b.publicKey, bSess.publicKey, BigInt(unix() + 3600), false)], [b, bSess]);
  });
  const say = (who: "A" | "B", kind: number, plaintext: Uint8Array) => {
    const [key, tok, conn, mine, peer] = who === "A" ? [aSess, aTok, aTee, aBox, bBox.publicKey] : [bSess, bTok, bTee, bBox, aBox.publicKey];
    const { nonce, ciphertext } = cr.seal(plaintext, peer, mine);
    return send(conn, `SendMessage ${who}`, [g.sendMessage(key.publicKey, room, kind, nonce, ciphertext, tok)], [key], { skipPreflight: true });
  };

  // 4. B joins with its session key and the two chat.
  await step("JoinRoom (PER, session key)", () =>
    send(bTee, "JoinRoom", [g.joinRoom(bSess.publicKey, room, bBox.publicKey, bTok)], [bSess], { skipPreflight: true }),
  );
  await step("SendMessage text (PER, session key)", () => say("A", g.KIND.text, cr.encodeText("Here's the bounty for fixing that bug.")));

  let transfer: pay.Landed | undefined;
  if (noPay) {
    log("  --no-pay: skipping the Helius steps (5–7)");
    await say("B", g.KIND.text, cr.encodeText("Thanks!"));
  } else {
  // 5. B shares its shielded address (kind 2). B registers its Helius private wallet first.
  const client = await pay.connectHelius(API_KEY!);
  const aKeys = pay.shieldedFromWallet(a);
  const bKeys = pay.shieldedFromWallet(b);
  await step("Helius register (B)", () => pay.register(client, bKeys));
  await say("B", g.KIND.shieldedAddress, cr.encodeShieldedAddress(bKeys.shieldedAddress().toBytes()));

  // 6. A reads B's address out of the room and pays privately (Solana, Helius).
  const roomForA = await readRoom(aTee, room);
  const addrMsg = roomForA.messages.find((m) => m.kind === g.KIND.shieldedAddress && m.sender === "guest")!;
  const bAddress = pay.ShieldedAddress.fromBytes(cr.decodeShieldedAddress(cr.open(addrMsg, roomForA.guestBoxPk, aBox)!));
  const amount = 4_000_000n;
  const paid = await step("Helius deposit + private transfer (A)", () =>
    pay.privateTransferSol(client, aKeys, bAddress, amount, { depositIfShort: 10_000_000n }),
  );
  transfer = paid.transfer;
  log(`  deposit ${paid.deposit?.signature}\n  transfer ${transfer.signature}`);

  // 7. A sends the payment note (kind 1); B confirms it against its own private balance.
  const landed = transfer;
  await say("A", g.KIND.paymentNote, cr.encodePaymentNote({ signature: pay.signatureToBytes(landed.signature), mint: new PublicKey(pay.SOL), amount }));
  await say("B", g.KIND.text, cr.encodeText("Got it, thanks!"));
  const roomForB = await readRoom(bTee, room);
  const noteMsg = roomForB.messages.find((m) => m.kind === g.KIND.paymentNote)!;
  const note = cr.decodePaymentNote(cr.open(noteMsg, roomForB.creatorBoxPk, bBox)!);
  const received = await step("B confirms the payment (Photon)", () =>
    pay.solReceivedIn(client, bKeys, { signature: pay.signatureFromBytes(note.signature), slot: landed.slot }),
  );
  check(received.amount === note.amount, `B found ${received.amount} lamports in the noted transaction: payment received`);
  }
  for (const line of render(await readRoom(bTee, room), b)) log(`  B reads ${line}`);

  // 8. Outsider checks on the TEE.
  await expectCustom(g.ERR.Unauthorized, send(eveTee, "Eve SendMessage", [g.sendMessage(eve.publicKey, room, 0, new Uint8Array(24), new Uint8Array(32))], [eve], { skipPreflight: true }), "outsider send");
  log("  ok: an outsider's SendMessage is rejected");

  // 9. Close (wipes + closes the permission), undelegate, reclaim.
  await step("CloseRoom (PER, session key)", () =>
    send(aTee, "CloseRoom", [g.closeRoom(aSess.publicKey, room, aTok)], [aSess], { skipPreflight: true }),
  );
  await step("UndelegateRoom → callback → ReclaimRoom", () => undelegateAndReclaim(a, aTee, room, a.publicKey));
  check((await base.getAccountInfo(room)) === null, "room is gone from Solana; rent back with A");

  console.log("\nTIMINGS");
  for (const [label, ms] of timings) console.log(`- ${label}: ${(ms / 1000).toFixed(2)}s`);
  console.log(`\nroom      https://explorer.solana.com/address/${room.toBase58()}?cluster=devnet`);
  if (transfer) console.log(`transfer  https://explorer.solana.com/tx/${transfer.signature}?cluster=devnet`);
}

// ---------------------------------------------------------------------------------------------

async function main() {
  const room = args[0] ? new PublicKey(args[0]) : undefined;
  switch (command) {
    case "scenario":
      return scenario();
    case "create": {
      const id = BigInt(args[0] ?? unix());
      const r = g.roomPda(wallet.publicKey, id);
      await send(base, "CreateRoom + DelegateRoom", [g.createRoom(wallet.publicKey, id), g.delegateRoom(wallet.publicKey, r)], [wallet]);
      return console.log(r.toBase58());
    }
    case "open": {
      const box = cr.deriveBoxKey(cr.signWith(wallet.secretKey));
      await send(await tee(wallet), "OpenRoom", [g.openRoom(wallet.publicKey, room!, new PublicKey(args[1]), box.publicKey, BigInt(unix() + 24 * 3600))], [wallet], { skipPreflight: true });
      return console.log("opened");
    }
    case "join": {
      const box = cr.deriveBoxKey(cr.signWith(wallet.secretKey));
      await send(await tee(wallet), "JoinRoom", [g.joinRoom(wallet.publicKey, room!, box.publicKey)], [wallet], { skipPreflight: true });
      return console.log("joined");
    }
    case "send": {
      const conn = await tee(wallet);
      const r = await readRoom(conn, room!);
      const peer = r.creator.equals(wallet.publicKey) ? r.guestBoxPk : r.creatorBoxPk;
      const { nonce, ciphertext } = cr.seal(cr.encodeText(args.slice(1).join(" ")), peer, cr.deriveBoxKey(cr.signWith(wallet.secretKey)));
      await send(conn, "SendMessage", [g.sendMessage(wallet.publicKey, room!, g.KIND.text, nonce, ciphertext)], [wallet], { skipPreflight: true });
      return console.log("sent");
    }
    case "read": {
      const r = await readRoom(await tee(wallet), room!);
      console.log(`${r.status}, expires ${new Date(Number(r.expiresAt) * 1000).toISOString()}`);
      return render(r, wallet).forEach((l) => console.log(l));
    }
    case "close": {
      const conn = await tee(wallet);
      const r = await readRoom(conn, room!);
      await send(conn, "CloseRoom", [g.closeRoom(wallet.publicKey, room!)], [wallet], { skipPreflight: true });
      await undelegateAndReclaim(wallet, conn, room!, r.creator);
      return console.log("closed, undelegated, reclaimed");
    }
    default:
      console.log("commands: scenario | create [id] | open <room> <guest> | join <room> | send <room> <text> | read <room> | close <room>");
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
