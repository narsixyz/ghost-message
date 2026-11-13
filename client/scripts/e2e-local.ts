// Local e2e: the whole room lifecycle on a real local MagicBlock stack, plus probes for the
// open questions in docs/research.md §5.
//
//   mb-stack: base validator :8899, ephemeral validator :7799, query filtering service :6699
//   (the QFS plays the TEE endpoint: token auth + permission filtering, no TDX attestation).
//
// Run (see client/README.md): build with `--features local-er`, start mb-stack with the
// program preloaded, then `bun run scripts/e2e-local.ts`.
import { Connection, Keypair, type PublicKey } from "@solana/web3.js";
import * as g from "../src/ghost";
import * as cr from "../src/crypto";
import { sendV1 } from "../src/v1";
import {
  TxError,
  airdrop,
  authedConnection,
  check,
  expectCustom,
  expectFailure,
  log,
  send,
  sleep,
  unix,
  view,
  waitFor,
} from "../src/net";

const BASE = process.env.BASE_URL ?? "http://127.0.0.1:8899";
const ER = process.env.ER_URL ?? "http://127.0.0.1:7799";
const QFS = process.env.QFS_URL ?? "http://127.0.0.1:6699";

const base = new Connection(BASE, "confirmed");
const er = new Connection(ER, "confirmed"); // direct ER: a test privilege, used for assertions
const findings: Record<string, string> = {};
const finding = (q: string, answer: string) => {
  findings[q] = answer;
  log(`  FINDING ${q}: ${answer}`);
};

async function roomOn(conn: Connection, room: PublicKey) {
  const acc = await conn.getAccountInfo(room);
  return acc ? g.decodeRoom(acc.data as Buffer) : null;
}

async function main() {
  const creator = Keypair.generate();
  const guest = Keypair.generate();
  const stranger = Keypair.generate();
  for (const k of [creator, guest, stranger]) await airdrop(base, k.publicKey, 5);
  log(`program ${g.programId.toBase58()}`);

  const creatorBox = cr.deriveBoxKey(cr.signWith(creator.secretKey));
  const guestBox = cr.deriveBoxKey(cr.signWith(guest.secretKey));
  check(
    Buffer.from(cr.deriveBoxKey(cr.signWith(guest.secretKey)).publicKey).equals(
      Buffer.from(guestBox.publicKey),
    ),
    "box key derivation is deterministic",
  );

  // ---- Solana: create and delegate --------------------------------------------------------
  const roomId = BigInt(unix());
  const room = g.roomPda(creator.publicKey, roomId);
  await send(base, "CreateRoom", [g.createRoom(creator.publicKey, roomId)], [creator]);
  await send(base, "DelegateRoom", [g.delegateRoom(creator.publicKey, room)], [creator]);
  check(
    (await base.getAccountInfo(room))!.owner.equals(g.DELEGATION_PROGRAM),
    "room is owned by the delegation program on Solana",
  );
  const record = await base.getAccountInfo(g.delegationRecord(room));
  log(`  delegation record validator: ${new (await import("@solana/web3.js")).PublicKey(record!.data.subarray(8, 40)).toBase58()}`);

  // ---- PER: authenticated connections --------------------------------------------------------
  const creatorConn = await authedConnection(QFS, creator);
  const guestConn = await authedConnection(QFS, guest);
  const strangerConn = await authedConnection(QFS, stranger);

  // Before OpenRoom there is no permission: is the delegated room readable by anyone?
  await waitFor("room cloned into the ER", () => roomOn(er, room), 30_000).catch(() => null);
  log(`  before OpenRoom, a stranger reads the room: ${await view(strangerConn, room)}`);

  // ---- OpenRoom: guest + permission in one instruction -----------------------------------
  const expiresAt = BigInt(unix() + 3600);
  const openSig = await send(
    creatorConn,
    "OpenRoom",
    [g.openRoom(creator.publicKey, room, guest.publicKey, creatorBox.publicKey, expiresAt)],
    [creator],
  );
  const opened = (await roomOn(er, room))!;
  check(opened.status === "Open" && opened.guest.equals(guest.publicKey), "room is Open with the guest set");
  const perm = await er.getAccountInfo(g.permissionPda(room));
  check(perm !== null && perm.owner.equals(g.PERMISSION_PROGRAM), "the permission program created the room's permission");
  log(`  permission account: ${perm!.data.length} bytes`);

  const seen = {
    stranger: await view(strangerConn, room),
    creator: await view(creatorConn, room),
    guest: await view(guestConn, room),
  };
  log(`  reads after OpenRoom: ${JSON.stringify(seen)}`);
  check(seen.stranger.startsWith("hidden"), "a stranger can't read the room");
  check(seen.creator === "visible" && seen.guest === "visible", "both members can read the room");

  // Q6: is the OpenRoom transaction (it carries the guest's address) hidden from non-members?
  const describe = async (conn: Connection) => {
    try {
      const t = await conn.getTransaction(openSig, { maxSupportedTransactionVersion: 0 });
      if (t === null) return "hidden (null)";
      const keys = t.transaction.message.staticAccountKeys.map((k) => k.toBase58());
      const ixData = t.transaction.message.compiledInstructions.map((i) => Buffer.from(i.data));
      const leaks = keys.includes(guest.publicKey.toBase58()) || ixData.some((d) => d.includes(guest.publicKey.toBuffer()));
      return `returned (${keys.length} keys, ${leaks ? "GUEST ADDRESS PRESENT" : "no guest address"})`;
    } catch (e) {
      return `hidden (error: ${(e as Error).message.slice(0, 60)})`;
    }
  };
  const strangerTx = await describe(strangerConn);
  finding("Q6 OpenRoom tx visible to a non-member", strangerTx);
  log(`  the creator asks for its own OpenRoom tx: ${await describe(creatorConn)}`);
  const sigs = await strangerConn
    .getSignaturesForAddress(room, { limit: 10 })
    .then((s: unknown[]) => `${s.length} returned`)
    .catch((e: Error) => `hidden (${e.message.slice(0, 60)})`);
  log(`  a stranger lists signatures touching the room: ${sigs}`);

  // ---- Session keys --------------------------------------------------------------------------
  // Q4: can a session key with no lamports act on the ER? (top_up = false)
  const guestSession = Keypair.generate();
  const guestToken = g.sessionTokenV2(guestSession.publicKey, guest.publicKey);
  await send(
    base,
    "create_session_v2 (guest, no top-up)",
    [g.createSessionV2(guest.publicKey, guestSession.publicKey, BigInt(unix() + 3600), false)],
    [guest, guestSession],
  );
  check((await base.getBalance(guestSession.publicKey)) === 0, "guest session key holds 0 lamports");

  // Q2: is the Solana token cloned into the ER and readable there?
  const tokenOnEr = await waitFor("session token in the ER", () => er.getAccountInfo(guestToken), 20_000).catch(() => null);
  finding("Q2a session token readable in the ER", tokenOnEr ? `yes (owner ${tokenOnEr.owner.toBase58().slice(0, 8)}…, ${tokenOnEr.data.length} bytes)` : "not by a direct read (may be cloned on first use)");

  // Q1: which connection accepts a session-key-signed write?
  const sessionConn = await authedConnection(QFS, guestSession);
  const join = g.joinRoom(guestSession.publicKey, room, guestBox.publicKey, guestToken);
  const attempts: [string, Connection][] = [
    ["QFS authenticated as the guest wallet", guestConn],
    ["QFS authenticated as the session key", sessionConn],
  ];
  let joinedVia = "";
  for (const [label, conn] of attempts) {
    try {
      await send(conn, `JoinRoom via session key (${label})`, [join], [guestSession], { skipPreflight: true });
      joinedVia = label;
      break;
    } catch (e) {
      log(`  JoinRoom signed by a 0-lamport session key via ${label}: FAILED ${(e as Error).message.split("\n")[0].slice(0, 160)}`);
    }
  }
  finding("Q4 0-lamport session key as fee payer on the ER", joinedVia ? "works" : "fails");
  finding("Q1 session-key write through the token-gated endpoint", joinedVia ? `works (${joinedVia})` : "did not work with a 0-lamport key; retried below with a funded key");

  if (!joinedVia) {
    // Fund the session key on Solana and retry: the ER clones its balance.
    await send(base, "fund guest session key", [
      (await import("@solana/web3.js")).SystemProgram.transfer({
        fromPubkey: guest.publicKey,
        toPubkey: guestSession.publicKey,
        lamports: 10_000_000,
      }),
    ], [guest]);
    await sleep(2000);
    for (const [label, conn] of attempts) {
      try {
        await send(conn, `JoinRoom via funded session key (${label})`, [join], [guestSession], { skipPreflight: true });
        joinedVia = label;
        break;
      } catch (e) {
        log(`  funded session key via ${label}: FAILED ${(e as Error).message.split("\n")[0].slice(0, 160)}`);
      }
    }
    finding("Q1 session-key write through the token-gated endpoint (funded key)", joinedVia ? `works (${joinedVia})` : "fails");
  }
  check(joinedVia !== "", "the guest joined through its session key");
  const sessionWriteConn = joinedVia.includes("session key") ? sessionConn : guestConn;
  check((await roomOn(er, room))!.status === "Joined", "room is Joined");

  // The creator also uses a session key, funded by top-up.
  const creatorSession = Keypair.generate();
  const creatorToken = g.sessionTokenV2(creatorSession.publicKey, creator.publicKey);
  await send(base, "create_session_v2 (creator, top-up)", [
    g.createSessionV2(creator.publicKey, creatorSession.publicKey, BigInt(unix() + 3600), true),
  ], [creator, creatorSession]);
  const creatorSessionConn = joinedVia.includes("session key")
    ? await authedConnection(QFS, creatorSession)
    : creatorConn;

  // ---- Messages ------------------------------------------------------------------------------
  const say = async (who: "creator" | "guest", text: string) => {
    const [key, token, conn, mine, peer] =
      who === "creator"
        ? [creatorSession, creatorToken, creatorSessionConn, creatorBox, guestBox.publicKey]
        : [guestSession, guestToken, sessionWriteConn, guestBox, creatorBox.publicKey];
    const { nonce, ciphertext } = cr.seal(cr.encodeText(text), peer, mine);
    return send(conn, `SendMessage ${who}`, [
      g.sendMessage(key.publicKey, room, g.KIND.text, nonce, ciphertext, token),
    ], [key], { skipPreflight: true });
  };
  await say("creator", "Here's the bounty for fixing that bug.");

  // Q3: v1 transactions on the ER (token-gated endpoint) and on the base validator.
  const v1Probe = async (url: string, label: string, ix: Parameters<typeof sendV1>[2], signers: Keypair[]) => {
    try {
      await sendV1(url, label, ix, signers);
      return "accepted";
    } catch (e) {
      return `rejected (${(e as Error).message.split("\n")[0].slice(0, 140)})`;
    }
  };
  {
    const { nonce, ciphertext } = cr.seal(cr.encodeText("sent as a v1 transaction"), guestBox.publicKey, creatorBox);
    const sessionUrl = (creatorSessionConn as unknown as { rpcEndpoint: string }).rpcEndpoint;
    finding("Q3 v1 tx on the ER (via the token-gated endpoint)", await v1Probe(sessionUrl, "v1 SendMessage", [
      g.sendMessage(creatorSession.publicKey, room, g.KIND.text, nonce, ciphertext, creatorToken),
    ], [creatorSession]));
    const { nonce: n2, ciphertext: c2 } = cr.seal(cr.encodeText("v1, direct ER"), guestBox.publicKey, creatorBox);
    finding("Q3 v1 tx on the ER (direct, no QFS)", await v1Probe(ER, "v1 SendMessage direct", [
      g.sendMessage(creatorSession.publicKey, room, g.KIND.text, n2, c2, creatorToken),
    ], [creatorSession]));
    finding("Q3 v1 tx on the mb-stack base validator", await v1Probe(BASE, "v1 CreateRoom", [
      g.createRoom(stranger.publicKey, 1n),
    ], [stranger]));
  }
  await say("guest", "Thanks! Sending you my shielded address next.");

  // Each member reads the room through its own authenticated connection and decrypts.
  const guestView = g.decodeRoom((await guestConn.getAccountInfo(room))!.data as Buffer);
  const texts = guestView.messages.map((m) => {
    // NaCl box uses one shared key per pair (X25519(guest_sk, creator_pk)), so the guest
    // decrypts both directions with the creator's public key.
    const plain = cr.open(m, guestView.creatorBoxPk, guestBox);
    return `${m.sender}: ${plain ? cr.decodeText(plain) : "<undecryptable>"}`;
  });
  log(`  guest decrypts: ${JSON.stringify(texts)}`);
  check(texts[0] === "creator: Here's the bounty for fixing that bug.", "guest decrypts the creator's message");
  check(texts.at(-1) === "guest: Thanks! Sending you my shielded address next.", "guest decrypts its own message");
  check(
    cr.open(guestView.messages[0], guestView.creatorBoxPk, cr.deriveBoxKey(cr.signWith(stranger.secretKey))) === null,
    "a stranger's box key can't decrypt the ciphertext",
  );

  // ---- Q2b: revoke latency -------------------------------------------------------------------
  await send(base, "revoke_session_v2 (guest)", [g.revokeSessionV2(guestSession.publicKey, guest.publicKey)], [guest]);
  const revokedAt = Date.now();
  let stillWorks = 0;
  let revokeSeen = "";
  for (let i = 0; i < 30; i++) {
    try {
      await say("guest", `after revoke #${i}`);
      stillWorks++;
    } catch (e) {
      revokeSeen = e instanceof TxError ? `custom ${e.customCode ?? JSON.stringify(e.err)}` : (e as Error).message.slice(0, 80);
      break;
    }
    await sleep(1000);
  }
  finding(
    "Q2b revoke on Solana reaching the ER",
    revokeSeen
      ? `rejected after ${((Date.now() - revokedAt) / 1000).toFixed(1)}s (${stillWorks} sends still went through; error ${revokeSeen})`
      : `NOT rejected within 30s (${stillWorks} sends went through)`,
  );

  // ---- Guards on the real ER -----------------------------------------------------------------
  await expectCustom(g.ERR.Unauthorized, send(strangerConn, "stranger SendMessage", [
    g.sendMessage(stranger.publicKey, room, 0, new Uint8Array(24), new Uint8Array(32)),
  ], [stranger], { skipPreflight: true }), "stranger can't send");
  log("  ok: a stranger's SendMessage is rejected (Unauthorized)");

  // ---- Close, undelegate, reclaim ------------------------------------------------------------
  await send(creatorSessionConn, "CloseRoom (creator session)", [
    g.closeRoom(creatorSession.publicKey, room, creatorToken),
  ], [creatorSession], { skipPreflight: true });
  const closed = (await roomOn(er, room))!;
  check(closed.status === "Closed" && closed.nextSeq === 0n && closed.guest.equals(new (await import("@solana/web3.js")).PublicKey(new Uint8Array(32))), "room is Closed and wiped");
  const permAfter = await er.getAccountInfo(g.permissionPda(room));
  check(permAfter === null || permAfter.lamports === 0 || permAfter.data.length === 0, "the permission was closed");
  log(`  after close, a stranger reads the room: ${await view(strangerConn, room)}`);

  await send(creatorConn, "UndelegateRoom", [g.undelegateRoom(creator.publicKey, room)], [creator], { skipPreflight: true });
  const back = await waitFor("room back on Solana", async () => {
    const acc = await base.getAccountInfo(room);
    return acc?.owner.equals(g.programId) ? acc : null;
  }, 90_000);
  const onSolana = g.decodeRoom(back.data as Buffer);
  check(onSolana.status === "Closed" && onSolana.messages.length === 0, "room is back on Solana, Closed, with no messages");
  check(Buffer.from(back.data.subarray(40, 136)).every((b) => b === 0) && Buffer.from(back.data.subarray(144)).every((b) => b === 0), "nothing private reached Solana");

  const before = await base.getBalance(creator.publicKey);
  await send(base, "ReclaimRoom", [g.reclaimRoom(creator.publicKey, room)], [stranger]);
  check((await base.getAccountInfo(room)) === null, "room closed on Solana");
  log(`  creator got back ${(await base.getBalance(creator.publicKey)) - before} lamports`);

  console.log("\nFINDINGS");
  for (const [q, a] of Object.entries(findings)) console.log(`- ${q}: ${a}`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
