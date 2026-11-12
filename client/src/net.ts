// Transaction sending and MagicBlock endpoint helpers.
import { getAuthToken } from "@magicblock-labs/ephemeral-rollups-sdk";
import {
  Connection,
  type Keypair,
  LAMPORTS_PER_SOL,
  type PublicKey,
  Transaction,
  type TransactionInstruction,
} from "@solana/web3.js";
import nacl from "tweetnacl";

export const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
export const unix = () => Math.floor(Date.now() / 1000);
export const log = (msg: string) =>
  console.log(`[${new Date().toISOString().slice(11, 19)}] ${msg}`);

export class TxError extends Error {
  constructor(
    public label: string,
    public err: unknown,
    public logs: string[],
  ) {
    super(`${label}: ${JSON.stringify(err)}\n${logs.join("\n")}`);
  }
  get customCode(): number | undefined {
    const m = JSON.stringify(this.err).match(/"Custom":(\d+)/);
    return m ? Number(m[1]) : undefined;
  }
}

/** Sends one legacy transaction (first signer pays) and waits for `confirmed`. */
export async function send(
  conn: Connection,
  label: string,
  ixs: TransactionInstruction[],
  signers: Keypair[],
  opts: { skipPreflight?: boolean; feePayer?: Keypair } = {},
): Promise<string> {
  const tx = new Transaction().add(...ixs);
  const payer = opts.feePayer ?? signers[0];
  tx.feePayer = payer.publicKey;
  tx.recentBlockhash = (await conn.getLatestBlockhash("confirmed")).blockhash;
  tx.sign(...(opts.feePayer ? [opts.feePayer, ...signers] : signers));
  let sig: string;
  try {
    sig = await conn.sendRawTransaction(tx.serialize(), {
      skipPreflight: opts.skipPreflight ?? false,
    });
  } catch (e) {
    const err = e as { message?: string; logs?: string[]; transactionLogs?: string[] };
    throw new TxError(label, err.message ?? String(e), err.logs ?? err.transactionLogs ?? []);
  }
  for (let i = 0; i < 300; i++) {
    const st = (await conn.getSignatureStatuses([sig])).value[0];
    if (st?.err) {
      const t = await conn
        .getTransaction(sig, { maxSupportedTransactionVersion: 0, commitment: "confirmed" })
        .catch(() => null);
      throw new TxError(label, st.err, t?.meta?.logMessages ?? []);
    }
    if (st?.confirmationStatus === "confirmed" || st?.confirmationStatus === "finalized") {
      return sig;
    }
    await sleep(100);
  }
  throw new TxError(label, "confirmation timeout", []);
}

/** Resolves to the error if `p` fails, throws if it succeeds. */
export async function expectFailure(p: Promise<unknown>, what: string): Promise<TxError | Error> {
  try {
    await p;
  } catch (e) {
    return e as Error;
  }
  throw new Error(`${what}: expected a failure, but it succeeded`);
}

export async function expectCustom(code: number, p: Promise<unknown>, what: string) {
  const e = await expectFailure(p, what);
  if (e instanceof TxError && e.customCode === code) return;
  throw new Error(`${what}: expected custom error ${code}, got ${e.message.slice(0, 300)}`);
}

export function check(cond: boolean, msg: string) {
  if (!cond) throw new Error(`CHECK FAILED: ${msg}`);
  log(`  ok: ${msg}`);
}

export async function airdrop(conn: Connection, to: PublicKey, sol: number) {
  const sig = await conn.requestAirdrop(to, sol * LAMPORTS_PER_SOL);
  await conn.confirmTransaction(sig, "confirmed");
}

/**
 * A connection to a token-gated PER endpoint (the TEE endpoint, or the local query filtering
 * service) authenticated as `key`: one wallet signature yields a token, and the endpoint filters
 * reads by that key against each account's permission members.
 */
export async function authedConnection(endpoint: string, key: Keypair): Promise<Connection> {
  const { token } = await getAuthToken(endpoint, key.publicKey, async (m: Uint8Array) =>
    nacl.sign.detached(m, key.secretKey),
  );
  const sep = endpoint.includes("?") ? "&" : "?";
  return new Connection(`${endpoint}${sep}token=${token}`, "confirmed");
}

/** "visible", "hidden", or "hidden (error ...)": what `conn` can read of `account`. */
export async function view(conn: Connection, account: PublicKey): Promise<string> {
  try {
    const acc = await conn.getAccountInfo(account);
    return acc === null || acc.data.length === 0 ? "hidden" : "visible";
  } catch (e) {
    return `hidden (error: ${(e as Error).message.slice(0, 80)})`;
  }
}

export async function waitFor<T>(
  what: string,
  poll: () => Promise<T | null | undefined>,
  timeoutMs = 60_000,
): Promise<T> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const v = await poll();
    if (v !== null && v !== undefined) return v;
    await sleep(500);
  }
  throw new Error(`timed out after ${timeoutMs / 1000}s waiting for ${what}`);
}
