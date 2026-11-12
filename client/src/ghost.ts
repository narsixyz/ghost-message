// GhostMessage program client: addresses, instruction encoders and the room decoder.
// Layouts follow the doc comment on each processor in program/src/instructions/.
import {
  type AccountMeta,
  Keypair,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
} from "@solana/web3.js";
import { readFileSync } from "node:fs";

export const DELEGATION_PROGRAM = new PublicKey("DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh");
export const PERMISSION_PROGRAM = new PublicKey("ACLseoPoyC3cBqoUtkbjZ4aDrkurZW86v19pXz2XQnp1");
export const MAGIC_PROGRAM = new PublicKey("Magic11111111111111111111111111111111111111");
export const MAGIC_CONTEXT = new PublicKey("MagicContext1111111111111111111111111111111");
export const MAGIC_VAULT = new PublicKey("MagicVau1t999999999999999999999999999999999");
export const SESSION_PROGRAM = new PublicKey("KeyspM2ssCJbqUhQ4k7sveSiY4WjnYsrXkC8oDbwde5");
export const TEE_VALIDATOR = new PublicKey("MTEWGuqxUpYZGFJQcp8tLN7x5v9BSeoFHYWQQ3n3xzo");

export function loadKey(path: string): Keypair {
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(path, "utf8"))));
}

/** Program id: `GHOST_PROGRAM_ID`, else the keypair `cargo build-sbf` wrote. */
export const programId = process.env.GHOST_PROGRAM_ID
  ? new PublicKey(process.env.GHOST_PROGRAM_ID)
  : loadKey(new URL("../../target/deploy/ghost_message-keypair.json", import.meta.url).pathname)
      .publicKey;

// Encoding ------------------------------------------------------------------------------------

const u64 = (v: bigint) => {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(v);
  return b;
};
const i64 = (v: bigint) => {
  const b = Buffer.alloc(8);
  b.writeBigInt64LE(v);
  return b;
};

// Addresses -----------------------------------------------------------------------------------

const pda = (seeds: Buffer[], program = programId) =>
  PublicKey.findProgramAddressSync(seeds, program)[0];

export const roomPda = (creator: PublicKey, id: bigint) =>
  pda([Buffer.from("room"), creator.toBuffer(), u64(id)]);
export const bufferPda = (account: PublicKey) => pda([Buffer.from("buffer"), account.toBuffer()]);
export const delegationRecord = (account: PublicKey) =>
  pda([Buffer.from("delegation"), account.toBuffer()], DELEGATION_PROGRAM);
export const delegationMetadata = (account: PublicKey) =>
  pda([Buffer.from("delegation-metadata"), account.toBuffer()], DELEGATION_PROGRAM);
export const permissionPda = (account: PublicKey) =>
  pda([Buffer.from("permission:"), account.toBuffer()], PERMISSION_PROGRAM);
export const sessionTokenV2 = (signer: PublicKey, authority: PublicKey) =>
  pda(
    [Buffer.from("session_token_v2"), programId.toBuffer(), signer.toBuffer(), authority.toBuffer()],
    SESSION_PROGRAM,
  );

const meta = (pubkey: PublicKey, isSigner: boolean, isWritable: boolean): AccountMeta => ({
  pubkey,
  isSigner,
  isWritable,
});
const w = (k: PublicKey) => meta(k, false, true);
const ws = (k: PublicKey) => meta(k, true, true);
const r = (k: PublicKey) => meta(k, false, false);
const rs = (k: PublicKey) => meta(k, true, false);
const ix = (data: Buffer, keys: AccountMeta[]) =>
  new TransactionInstruction({ programId, data, keys });

const permissionAccounts = (room: PublicKey) => [
  w(permissionPda(room)),
  w(MAGIC_VAULT),
  r(MAGIC_PROGRAM),
  r(PERMISSION_PROGRAM),
];
const token = (t?: PublicKey) => (t ? [r(t)] : []);

// Instructions --------------------------------------------------------------------------------

export const createRoom = (creator: PublicKey, id: bigint) =>
  ix(Buffer.concat([Buffer.from([0]), u64(id)]), [
    ws(creator),
    w(roomPda(creator, id)),
    r(SystemProgram.programId),
  ]);

export const delegateRoom = (creator: PublicKey, room: PublicKey) =>
  ix(Buffer.from([1]), [
    ws(creator),
    w(room),
    r(programId),
    w(bufferPda(room)),
    w(delegationRecord(room)),
    w(delegationMetadata(room)),
    r(DELEGATION_PROGRAM),
    r(SystemProgram.programId),
  ]);

export const openRoom = (
  creator: PublicKey,
  room: PublicKey,
  guest: PublicKey,
  creatorBoxPk: Uint8Array,
  expiresAt: bigint,
) =>
  ix(
    Buffer.concat([Buffer.from([2]), guest.toBuffer(), Buffer.from(creatorBoxPk), i64(expiresAt)]),
    [rs(creator), w(room), ...permissionAccounts(room)],
  );

/** `signer` is the guest, or the guest's session key with `sessionToken`. */
export const joinRoom = (
  signer: PublicKey,
  room: PublicKey,
  guestBoxPk: Uint8Array,
  sessionToken?: PublicKey,
) =>
  ix(Buffer.concat([Buffer.from([3]), Buffer.from(guestBoxPk)]), [
    rs(signer),
    w(room),
    ...token(sessionToken),
  ]);

export const sendMessage = (
  signer: PublicKey,
  room: PublicKey,
  kind: number,
  nonce: Uint8Array,
  ciphertext: Uint8Array,
  sessionToken?: PublicKey,
) =>
  ix(Buffer.concat([Buffer.from([4, kind]), Buffer.from(nonce), Buffer.from(ciphertext)]), [
    rs(signer),
    w(room),
    ...token(sessionToken),
  ]);

export const closeRoom = (signer: PublicKey, room: PublicKey, sessionToken?: PublicKey) =>
  ix(Buffer.from([5]), [rs(signer), w(room), ...permissionAccounts(room), ...token(sessionToken)]);

export const undelegateRoom = (payer: PublicKey, room: PublicKey) =>
  ix(Buffer.from([6]), [ws(payer), w(room), w(MAGIC_CONTEXT), r(MAGIC_PROGRAM)]);

export const reclaimRoom = (creator: PublicKey, room: PublicKey) =>
  ix(Buffer.from([7]), [w(creator), w(room)]);

/**
 * `gpl_session` `create_session_v2`: the wallet (`authority`) delegates to `sessionSigner` for
 * this program. Both sign. `topUp` moves 0.01 SOL to the session key on Solana.
 */
export function createSessionV2(
  authority: PublicKey,
  sessionSigner: PublicKey,
  validUntil: bigint,
  topUp: boolean,
): TransactionInstruction {
  const data = Buffer.concat([
    Buffer.from([223, 233, 108, 7, 65, 194, 235, 38]), // sha256("global:create_session_v2")[..8]
    Buffer.from([1, topUp ? 1 : 0]), // top_up: Some(bool)
    Buffer.from([1]),
    i64(validUntil), // valid_until: Some(i64)
    Buffer.from([0]), // lamports: None
  ]);
  return new TransactionInstruction({
    programId: SESSION_PROGRAM,
    data,
    keys: [
      w(sessionTokenV2(sessionSigner, authority)),
      ws(sessionSigner),
      ws(authority), // fee payer
      rs(authority),
      r(programId),
      r(SystemProgram.programId),
    ],
  });
}

/** `gpl_session` `revoke_session_v2`: the authority signs while the session is active. */
export const revokeSessionV2 = (sessionSigner: PublicKey, authority: PublicKey) =>
  new TransactionInstruction({
    programId: SESSION_PROGRAM,
    data: Buffer.from([211, 59, 125, 188, 43, 155, 8, 102]),
    keys: [
      w(sessionTokenV2(sessionSigner, authority)),
      ws(authority),
      rs(authority),
      r(SystemProgram.programId),
    ],
  });

// Room decoding -------------------------------------------------------------------------------

export const ROOM_LEN = 4256;
export const STATUS = ["Created", "Open", "Joined", "Closed"] as const;
export const KIND = { text: 0, paymentNote: 1, shieldedAddress: 2 } as const;

export interface Message {
  seq: bigint;
  sender: "creator" | "guest";
  kind: number;
  nonce: Uint8Array;
  ciphertext: Uint8Array;
}

export interface Room {
  status: (typeof STATUS)[number];
  creator: PublicKey;
  guest: PublicKey;
  creatorBoxPk: Uint8Array;
  guestBoxPk: Uint8Array;
  roomId: bigint;
  expiresAt: bigint;
  nextSeq: bigint;
  /** Oldest first. */
  messages: Message[];
}

export function decodeRoom(d: Buffer): Room {
  if (d.length !== ROOM_LEN || d[0] !== 1) throw new Error("not a GhostMessage room");
  const nextSeq = d.readBigUInt64LE(152);
  const messages: Message[] = [];
  const first = nextSeq > 16n ? nextSeq - 16n : 0n;
  for (let seq = first; seq < nextSeq; seq++) {
    const o = 160 + Number(seq % 16n) * 256;
    const len = d.readUInt16LE(o + 10);
    messages.push({
      seq: d.readBigUInt64LE(o),
      sender: d[o + 8] === 0 ? "creator" : "guest",
      kind: d[o + 9],
      nonce: new Uint8Array(d.subarray(o + 12, o + 36)),
      ciphertext: new Uint8Array(d.subarray(o + 36, o + 36 + len)),
    });
  }
  return {
    status: STATUS[d[2]],
    creator: new PublicKey(d.subarray(8, 40)),
    guest: new PublicKey(d.subarray(40, 72)),
    creatorBoxPk: new Uint8Array(d.subarray(72, 104)),
    guestBoxPk: new Uint8Array(d.subarray(104, 136)),
    roomId: d.readBigUInt64LE(136),
    expiresAt: d.readBigInt64LE(144),
    nextSeq,
    messages,
  };
}

/** Custom error codes (program/src/error.rs). */
export const ERR = {
  Unauthorized: 0,
  InvalidRoomStatus: 1,
  RoomExpired: 2,
  InvalidSessionToken: 9,
  SessionExpired: 10,
} as const;
