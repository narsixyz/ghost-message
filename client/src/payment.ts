// Confidential payments with Helius Privacy Rings (Default Ring) through `@heliuslabs/spr`.
//
// What it hides: asset and amount of a private transfer. What it does NOT hide: that the sender
// paid the recipient (both are public on Solana). Deposits from a public balance reveal the
// amount.
//
// Runs on Node (the SDK targets Node ≥ 24); everything is a Solana v1 transaction.
import {
  SOL_MINT,
  ShieldedAddress,
  ShieldedKeypair,
  SigningKey,
  Wallet,
  createSprClient,
  initializePoseidon,
  syncWallet,
  type Bytes32,
} from "@heliuslabs/spr";
import { LocalKeys, SignedTeePolicy, atSlot } from "@heliuslabs/spr/client";
import { DepositAsset, depositInstruction, transactInstruction } from "@heliuslabs/spr/interface";
import {
  getRegisterInstructionAsync,
  getSetMergeEncryptionInstructionAsync,
} from "@heliuslabs/spr/instructions";
import { MergeEncryptionScheme } from "@heliuslabs/spr";
import {
  AssetRegistry,
  ConfidentialTransfer,
  LocalShieldedKeys,
  ProofInputUtxo,
  decryptToBalances,
} from "@heliuslabs/spr/transaction";
import { isWalletRegistered } from "@heliuslabs/spr/wallet";
import {
  type Instruction,
  type Signature,
  type TransactionSigner,
  appendTransactionMessageInstructions,
  assertIsTransactionWithBlockhashLifetime,
  createTransactionMessage,
  getBase58Decoder,
  getBase58Encoder,
  getSignatureFromTransaction,
  pipe,
  sendTransactionWithoutConfirmingFactory,
  setTransactionMessageConfig,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
} from "@solana/kit";
import type { Keypair } from "@solana/web3.js";

export type SprClient = Awaited<ReturnType<typeof createSprClient>>;
export const SOL = SOL_MINT;

/**
 * The prover's TEE channel encrypts requests with HPKE over X25519 through WebCrypto. Bun only
 * supports X25519 `deriveBits` from 1.4; older versions fail deep in the SDK with a bare
 * `CLIENT_UNEXPECTED`, so fail early with the reason instead.
 */
function requireX25519WebCrypto() {
  const bun = (globalThis as { Bun?: { version: string } }).Bun;
  if (!bun) return;
  const [major, minor] = bun.version.split(".").map(Number);
  if (major < 1 || (major === 1 && minor < 4)) {
    throw new Error(`Bun ${bun.version} lacks WebCrypto X25519 deriveBits, which the Helius prover needs. Run \`bun upgrade\` (>= 1.4).`);
  }
}

/** Devnet Helius endpoint: Solana RPC + Photon indexer + attested TEE prover behind one key. */
export async function connectHelius(apiKey: string): Promise<SprClient> {
  requireX25519WebCrypto();
  await initializePoseidon();
  return createSprClient({
    solanaRpcUrl: `https://devnet.helius-rpc.com/?api-key=${encodeURIComponent(apiKey)}`,
    proofDataSource: "prover",
    proverTee: SignedTeePolicy.forNetwork("devnet"),
  });
}

/**
 * The wallet's private-wallet keys, derived from the same Ed25519 seed as the Solana wallet
 * (the Solana CLI keypair format: seed[32] || pubkey[32]). The signer address is the wallet.
 */
export function shieldedFromWallet(wallet: Keypair): ShieldedKeypair {
  const seed = Uint8Array.from(wallet.secretKey.subarray(0, 32)) as Bytes32;
  try {
    return ShieldedKeypair.fromKeypair(SigningKey.fromEd25519Bytes(seed));
  } finally {
    seed.fill(0);
  }
}

export const signatureToBytes = (s: string) => new Uint8Array(getBase58Encoder().encode(s));
export const signatureFromBytes = (b: Uint8Array) => getBase58Decoder().decode(b) as Signature;

export interface Landed {
  signature: Signature;
  slot: bigint;
}

/** Signs and sends a v1 transaction, then waits for the SDK's confirmation (gives the slot). */
export function sender(client: SprClient, feePayer: TransactionSigner) {
  const sendTx = sendTransactionWithoutConfirmingFactory({ rpc: client.solanaRpc });
  return async (instructions: readonly Instruction[]): Promise<Landed> => {
    const { value: lifetime } = await client.solanaRpc.getLatestBlockhash().send();
    const signed = await signTransactionMessageWithSigners(
      pipe(
        createTransactionMessage({ version: 1 }),
        (m) => setTransactionMessageFeePayerSigner(feePayer, m),
        (m) => setTransactionMessageLifetimeUsingBlockhash(lifetime, m),
        // ~140k CU for a transfer; v1 budgets zero unless set.
        (m) => setTransactionMessageConfig({ computeUnitLimit: 450_000, loadedAccountsDataSizeLimit: 64 * 1024 * 1024 }, m),
        (m) => appendTransactionMessageInstructions(instructions, m),
      ),
    );
    assertIsTransactionWithBlockhashLifetime(signed);
    await sendTx(signed, { commitment: "confirmed" });
    const signature = getSignatureFromTransaction(signed);
    const slot = await client.confirmTransaction(signature);
    return { signature, slot };
  };
}

/** Registers the private wallet (user registry: nullifier + viewing keys). Idempotent. */
export async function register(client: SprClient, keys: ShieldedKeypair): Promise<"registered" | "already"> {
  const signer = keys.toSolanaSigner();
  if (await isWalletRegistered({ rpc: client, owner: signer.address })) return "already";
  const address = keys.shieldedAddress();
  await sender(client, signer)([
    await getRegisterInstructionAsync({
      owner: signer,
      nullifierPublicKey: address.nullifierPublicKey,
      viewingPublicKey: address.viewingPublicKey.toBytes(),
    }),
    await getSetMergeEncryptionInstructionAsync({
      owner: signer,
      scheme: MergeEncryptionScheme.PoseidonCtrV1,
    }),
  ]);
  return "registered";
}

/** Moves public SOL into the wallet's private balance (this step is public: amount shown). */
export async function depositSol(client: SprClient, keys: ShieldedKeypair, lamports: bigint): Promise<Landed> {
  const signer = keys.toSolanaSigner();
  const address = keys.shieldedAddress();
  return sender(client, signer)([
    await depositInstruction({
      tree: client.tree,
      depositor: signer,
      deposits: [
        {
          asset: DepositAsset.sol(),
          viewTag: address.confidentialViewTag(),
          recipientOwnerHash: address.ownerHash(),
          amount: lamports,
        },
      ],
    }),
  ]);
}

/** Decrypts what `keys` owns in the outputs of one transaction (gated on its slot). */
export async function solReceivedIn(client: SprClient, keys: ShieldedKeypair, tx: Landed) {
  const response = await client.getShieldedTransactionsBySignature(tx.signature, atSlot(tx.slot));
  const balances = await decryptToBalances({
    keypair: keys,
    registry: new AssetRegistry(),
    transactions: response.transactions.map(({ transaction }) => transaction),
  });
  return balances.balance(SOL_MINT);
}

/** Full private balance after syncing every page from the indexer. */
export async function privateSolBalance(client: SprClient, keys: ShieldedKeypair) {
  const wallet = new Wallet({ identity: keys.shieldedAddress(), registry: new AssetRegistry() });
  await syncWallet({ wallet, keys: LocalShieldedKeys.fromKeypair(keys), client, config: { pageLimit: 50 } });
  const sol = wallet.balances().find((b) => b.mint === SOL_MINT);
  return { amount: sol?.amount ?? 0n, utxos: sol?.utxos ?? [] };
}

/**
 * Confidential SOL transfer from `from`'s private balance to `to` (a shielded address from a
 * `kind = 2` room message). Deposits first if the private balance can't cover it with one UTXO.
 * Returns the transfer transaction.
 */
export async function privateTransferSol(
  client: SprClient,
  from: ShieldedKeypair,
  to: ShieldedAddress,
  lamports: bigint,
  opts: { depositIfShort?: bigint } = {},
): Promise<{ transfer: Landed; deposit?: Landed }> {
  const signer = from.toSolanaSigner();
  let deposit: Landed | undefined;
  let utxo = (await privateSolBalance(client, from)).utxos
    .filter((u) => u.amount >= lamports)
    .sort((a, b) => (a.amount < b.amount ? -1 : 1))[0];
  if (!utxo) {
    if (!opts.depositIfShort) throw new Error("no private UTXO covers the amount");
    deposit = await depositSol(client, from, opts.depositIfShort);
    utxo = (await solReceivedIn(client, from, deposit)).utxos[0];
  }
  const transfer = new ConfidentialTransfer(from.shieldedAddress(), [ProofInputUtxo.fromKeypair(utxo, from)], signer.address);
  transfer.send(to, SOL_MINT, lamports);
  const data = await client.proveTransact(transfer.sign(from, new AssetRegistry()), LocalKeys.fromKeypair(from, client.proofService));
  const ix = await transactInstruction({
    payer: signer,
    ownerSigners: [signer],
    inputTree: client.tree,
    outputTree: client.tree,
    data,
  });
  return { transfer: await sender(client, signer)([ix]), deposit };
}

export { ShieldedAddress };
