// Client-side message encryption (NaCl `box`) and the plaintext formats of each message kind.
//
// The program never sees plaintext. Only the two members can decrypt: not the TEE operator, and
// not anyone reading the room after it is back on Solana.
import { PublicKey } from "@solana/web3.js";
import nacl from "tweetnacl";

/** Fixed string the wallet signs to derive its X25519 box key. */
export const BOX_KEY_MESSAGE = "GhostMessage box key v1";
export const MAX_PLAINTEXT = 204; // 220-byte slot − 16-byte MAC

/**
 * Derives the wallet's X25519 box key from one Ed25519 signature over `BOX_KEY_MESSAGE`:
 * `sha512(signature)[..32]` → `nacl.box.keyPair.fromSecretKey`. Ed25519 signatures are
 * deterministic, so any device holding the wallet re-creates the same key. Used for nothing
 * else (in particular not as the Helius viewing key).
 */
export function deriveBoxKey(sign: (message: Uint8Array) => Uint8Array): nacl.BoxKeyPair {
  const signature = sign(new TextEncoder().encode(BOX_KEY_MESSAGE));
  const seed = nacl.hash(signature).subarray(0, 32);
  return nacl.box.keyPair.fromSecretKey(seed);
}

export const signWith = (secretKey: Uint8Array) => (m: Uint8Array) =>
  nacl.sign.detached(m, secretKey);

export interface Sealed {
  nonce: Uint8Array;
  ciphertext: Uint8Array;
}

export function seal(plaintext: Uint8Array, peerBoxPk: Uint8Array, mine: nacl.BoxKeyPair): Sealed {
  if (plaintext.length > MAX_PLAINTEXT) {
    throw new Error(`plaintext is ${plaintext.length} bytes, max ${MAX_PLAINTEXT}`);
  }
  const nonce = nacl.randomBytes(nacl.box.nonceLength);
  return { nonce, ciphertext: nacl.box(plaintext, nonce, peerBoxPk, mine.secretKey) };
}

/** Returns null if the ciphertext doesn't authenticate. */
export function open(
  { nonce, ciphertext }: Sealed,
  peerBoxPk: Uint8Array,
  mine: nacl.BoxKeyPair,
): Uint8Array | null {
  return nacl.box.open(ciphertext, nonce, peerBoxPk, mine.secretKey);
}

// Plaintext formats -----------------------------------------------------------------------------

export const encodeText = (text: string) => new TextEncoder().encode(text);
export const decodeText = (bytes: Uint8Array) => new TextDecoder().decode(bytes);

/**
 * `kind = 1` payment note: `version u8 = 1 | signature[64] | mint[32] | amount u64 LE` (105
 * bytes). Points at the Helius private transfer. It is the sender's *claim*; the recipient
 * confirms by finding the matching UTXO in its own private balance.
 */
export interface PaymentNote {
  signature: Uint8Array;
  mint: PublicKey;
  amount: bigint;
}

export function encodePaymentNote(note: PaymentNote): Uint8Array {
  if (note.signature.length !== 64) throw new Error("signature must be 64 bytes");
  const out = Buffer.alloc(105);
  out[0] = 1;
  out.set(note.signature, 1);
  out.set(note.mint.toBuffer(), 65);
  out.writeBigUInt64LE(note.amount, 97);
  return out;
}

export function decodePaymentNote(bytes: Uint8Array): PaymentNote {
  const b = Buffer.from(bytes);
  if (b.length !== 105 || b[0] !== 1) throw new Error("not a v1 payment note");
  return {
    signature: new Uint8Array(b.subarray(1, 65)),
    mint: new PublicKey(b.subarray(65, 97)),
    amount: b.readBigUInt64LE(97),
  };
}

/**
 * `kind = 2` shielded address: `version u8 = 1 | ShieldedAddress.toBytes()` (the Helius SDK's
 * `signing || nullifier || viewing` form), which the payer needs to send a private transfer.
 */
export function encodeShieldedAddress(addressBytes: Uint8Array): Uint8Array {
  const out = new Uint8Array(1 + addressBytes.length);
  out[0] = 1;
  out.set(addressBytes, 1);
  return out;
}

export function decodeShieldedAddress(bytes: Uint8Array): Uint8Array {
  if (bytes[0] !== 1) throw new Error("not a v1 shielded address message");
  return bytes.subarray(1);
}
