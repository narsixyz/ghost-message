// v1 transactions through @solana/kit (web3.js 1.x can only build legacy and v0).
import {
  AccountRole,
  type Instruction,
  type KeyPairSigner,
  address,
  appendTransactionMessageInstructions,
  createKeyPairSignerFromBytes,
  createSolanaRpc,
  createTransactionMessage,
  getBase64EncodedWireTransaction,
  getSignatureFromTransaction,
  pipe,
  setTransactionMessageConfig,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
} from "@solana/kit";
import type { Keypair, TransactionInstruction } from "@solana/web3.js";
import { TxError, sleep } from "./net";

export const kitSigner = (k: Keypair): Promise<KeyPairSigner> =>
  createKeyPairSignerFromBytes(k.secretKey);

/** Converts a web3.js instruction, attaching kit signers to the signer accounts. */
export function toKitInstruction(ix: TransactionInstruction, signers: KeyPairSigner[]): Instruction {
  return {
    programAddress: address(ix.programId.toBase58()),
    data: new Uint8Array(ix.data),
    accounts: ix.keys.map((k) => {
      const role = k.isSigner
        ? k.isWritable
          ? AccountRole.WRITABLE_SIGNER
          : AccountRole.READONLY_SIGNER
        : k.isWritable
          ? AccountRole.WRITABLE
          : AccountRole.READONLY;
      const signer = signers.find((s) => s.address === k.pubkey.toBase58());
      return signer ? { address: signer.address, role, signer } : { address: address(k.pubkey.toBase58()), role };
    }),
  } as Instruction;
}

/**
 * Sends one **v1** transaction (first signer pays). v1 has no defaults: the compute-unit and
 * loaded-accounts-data limits must be set or the transaction gets zero.
 */
export async function sendV1(
  url: string,
  label: string,
  ixs: TransactionInstruction[],
  signers: Keypair[],
  config = { computeUnitLimit: 200_000, loadedAccountsDataSizeLimit: 1024 * 1024 },
): Promise<string> {
  const rpc = createSolanaRpc(url);
  const kitSigners = await Promise.all(signers.map(kitSigner));
  const { value: blockhash } = await rpc.getLatestBlockhash().send();
  const message = pipe(
    createTransactionMessage({ version: 1 }),
    (m) => setTransactionMessageFeePayerSigner(kitSigners[0], m),
    (m) => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
    (m) => setTransactionMessageConfig(config, m),
    (m) => appendTransactionMessageInstructions(ixs.map((i) => toKitInstruction(i, kitSigners)), m),
  );
  const tx = await signTransactionMessageWithSigners(message);
  const sig = getSignatureFromTransaction(tx);
  try {
    await rpc
      .sendTransaction(getBase64EncodedWireTransaction(tx), { encoding: "base64", skipPreflight: true })
      .send();
  } catch (e) {
    throw new TxError(label, (e as Error).message, []);
  }
  for (let i = 0; i < 300; i++) {
    const st = (await rpc.getSignatureStatuses([sig]).send()).value[0];
    if (st?.err) throw new TxError(label, st.err, []);
    if (st?.confirmationStatus === "confirmed" || st?.confirmationStatus === "finalized") return sig;
    await sleep(100);
  }
  throw new TxError(label, "confirmation timeout", []);
}
