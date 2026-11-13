// Held payments: private funds owned by a GhostMessage PDA and released only by the program.
//
//   lock    payer → ShieldedAddress.forPda(payment PDA)        plain Privacy Rings transfer
//   claim   GhostMessage ClaimPayment  → CPI SPP transact       recipient signs, any time
//   refund  GhostMessage RefundPayment → CPI SPP transact       payer signs, after the deadline
//
// The PDA is `["payment", payer, recipient, deadline]` (program/src/instructions/payment.rs). SPP
// only spends a UTXO whose owner signs, and only GhostMessage can sign for that PDA
// (`invoke_signed`), so the program decides who can move the funds.
//
// A PDA has no keys of its own. Its nullifier and viewing keys follow the Zolana spec's
// two-holder PDA derivation: both holders compute the same ECDH secret from their own viewing
// key and the other's viewing public key, then expand it with the PDA address in the info tag.
// So the payer and the recipient can each find and spend the UTXO, and nobody else can.
import {
  NullifierKey,
  SOL_MINT,
  ShieldedAddress,
  ShieldedKeypair,
  ViewingKey,
  Wallet,
  syncWallet,
  type Bytes31,
  type Bytes32,
} from "@heliuslabs/spr";
import { LocalKeys } from "@heliuslabs/spr/client";
import { transactInstruction } from "@heliuslabs/spr/interface";
import {
  AssetRegistry,
  ConfidentialTransfer,
  LocalShieldedKeys,
  ProofInputUtxo,
  encryptConfidentialTransfer,
} from "@heliuslabs/spr/transaction";
import { AccountRole, type Address, type Instruction, address, getAddressEncoder } from "@solana/kit";
import { PublicKey } from "@solana/web3.js";
import { hkdfSync } from "node:crypto";
import { paymentAuthority, paymentHeader, programId } from "./ghost";
import { type Landed, type SprClient, privateTransferSol, sender } from "./payment";

/** SPP `transact` tag on the deployed devnet program (`@heliuslabs/spr` 0.9.0-alpha). */
const SPP_TRANSACT_TAG = 16;

const PDA_ROOT_SALT = "TSPP/pda_root/x25519/v1";
const PDA_NULLIFIER_INFO = "TSPP/pda_nf/v1";
const PDA_VIEWING_INFO = "TSPP/pda_view/v1";

/** One held payment: who, until when, and the PDA that owns the funds. */
export interface HeldPayment {
  payer: PublicKey;
  recipient: PublicKey;
  deadline: bigint;
  pda: PublicKey;
  bump: number;
}

export function heldPayment(payer: PublicKey, recipient: PublicKey, deadline: bigint): HeldPayment {
  const [pda, bump] = paymentAuthority(payer, recipient, deadline);
  return { payer, recipient, deadline, pda, bump };
}

/** The PDA's private-wallet keys, as one of its two holders sees them. */
export interface PdaKeys {
  address: ShieldedAddress;
  nullifierKey: NullifierKey;
  viewingKey: ViewingKey;
  keys: LocalShieldedKeys;
}

/**
 * Derives the payment PDA's keys from `me` and the other holder's shielded address. The payer
 * and the recipient get the same keys: ECDH(a_sk, B_pk) = ECDH(b_sk, A_pk).
 */
export function pdaKeys(me: ShieldedKeypair, other: ShieldedAddress, payment: HeldPayment): PdaKeys {
  const shared = me.ecdh(other.viewingPublicKey);
  const pda = payment.pda.toBytes();
  const expand = (info: string, length: number) =>
    new Uint8Array(hkdfSync("sha256", shared, PDA_ROOT_SALT, Buffer.concat([Buffer.from(info), pda]), length));
  const nullifierSecret = expand(PDA_NULLIFIER_INFO, 31);
  const viewingSecret = expand(PDA_VIEWING_INFO, 32);
  // X25519 clamping.
  viewingSecret[0] &= 248;
  viewingSecret[31] = (viewingSecret[31] & 127) | 64;

  const nullifierKey = NullifierKey.fromSecret(nullifierSecret as Bytes31);
  const viewingKey = ViewingKey.fromBytes(viewingSecret as Bytes32);
  nullifierSecret.fill(0);
  viewingSecret.fill(0);
  const shieldedAddress = ShieldedAddress.forPda({
    pda: payment.pda.toBase58() as Address,
    nullifierPublicKey: nullifierKey.publicKey(),
    viewingPublicKey: viewingKey.publicKey(),
  });
  return {
    address: shieldedAddress,
    nullifierKey,
    viewingKey,
    keys: LocalShieldedKeys.fromKeys({ address: shieldedAddress, viewingKeys: [viewingKey], nullifierKey }),
  };
}

/** Payer: moves `lamports` from its private balance into the PDA (a normal private transfer). */
export function lock(
  client: SprClient,
  payer: ShieldedKeypair,
  escrow: PdaKeys,
  lamports: bigint,
  opts: { depositIfShort?: bigint } = {},
) {
  return privateTransferSol(client, payer, escrow.address, lamports, opts);
}

/** What the PDA holds, from either holder's view. */
export async function heldBalance(client: SprClient, escrow: PdaKeys) {
  const wallet = new Wallet({ identity: escrow.address, registry: new AssetRegistry() });
  await syncWallet({ wallet, keys: escrow.keys, client, config: { pageLimit: 50 } });
  const sol = wallet.balances().find((b) => b.mint === SOL_MINT);
  return { amount: sol?.amount ?? 0n, utxos: sol?.utxos ?? [] };
}

/**
 * Releases everything the PDA holds to `to` through GhostMessage: `claim` (recipient signs)
 * or `refund` (payer signs, after the deadline). `signer` authorizes the instruction and pays
 * the fees. Returns the landed transaction.
 */
export async function release(
  client: SprClient,
  action: "claim" | "refund",
  signer: ShieldedKeypair,
  payment: HeldPayment,
  escrow: PdaKeys,
  to: ShieldedAddress,
): Promise<Landed & { amount: bigint }> {
  const { amount, utxos } = await heldBalance(client, escrow);
  if (utxos.length === 0) throw new Error(`the payment PDA ${payment.pda.toBase58()} holds nothing`);
  const solanaSigner = signer.toSolanaSigner();
  const pda = address(payment.pda.toBase58());

  // The private transaction: spend the PDA's UTXOs, everything to `to`.
  const transfer = new ConfidentialTransfer(
    escrow.address,
    utxos.map((u) => ProofInputUtxo.fromNullifierKey(u, escrow.nullifierKey)),
    solanaSigner.address,
  );
  transfer.send(to, SOL_MINT, amount);
  const prepared = transfer.prepare();
  const [txKey] = await escrow.keys.transactionKeys([
    { viewingPublicKey: escrow.viewingKey.publicKey(), firstNullifier: prepared.firstNullifier },
  ]);
  const sealed = encryptConfidentialTransfer(txKey, { outputs: prepared.proofOutputs(), assets: new AssetRegistry() });
  const proofInputs = prepared.finalize({ txViewingPublicKey: sealed.txViewingPublicKey, salt: sealed.salt, payload: sealed.payload });
  txKey.destroy();

  const proofKeys = LocalKeys.fromKeys(
    { address: escrow.address, viewingKeys: [escrow.viewingKey], nullifierKey: escrow.nullifierKey },
    client.proofService,
  );
  const data = await client.proveTransact(proofInputs, proofKeys);
  proofKeys.destroy();

  // The SPP instruction as SPP lays it out, with the PDA as an owner signer...
  const spp = await transactInstruction({
    payer: solanaSigner,
    ownerSigners: [pda],
    inputTree: client.tree,
    outputTree: client.tree,
    data,
  });
  const sppData = spp.data!;
  if (sppData[0] !== SPP_TRANSACT_TAG) throw new Error(`unexpected SPP transact tag ${sppData[0]}`);

  // ...wrapped in ClaimPayment / RefundPayment: the PDA goes in read-only and unsigned, and the
  // program makes it a signer in its CPI.
  const counterparty = action === "claim" ? payment.payer : payment.recipient;
  const header = paymentHeader(action === "claim" ? 8 : 9, counterparty, payment.deadline, payment.bump);
  const ghostIx: Instruction = {
    programAddress: address(programId.toBase58()),
    data: new Uint8Array([...header, ...sppData.subarray(1)]),
    accounts: [
      { address: solanaSigner.address, role: AccountRole.READONLY_SIGNER, signer: solanaSigner } as never,
      ...spp.accounts!.map((a) => (a.address === pda ? { address: pda, role: AccountRole.READONLY } : a)),
    ],
  };
  const landed = await sender(client, solanaSigner)([ghostIx]);
  return { ...landed, amount };
}

/** Bytes of a Solana address, for matching in raw transaction data. */
export const addressBytes = (a: PublicKey) => new Uint8Array(getAddressEncoder().encode(address(a.toBase58())));
