# GhostMessage client

Bun client for the GhostMessage program: rooms on the MagicBlock TEE, Helius Privacy Rings
payments, and payments held by the program. The scripts drive everything on devnet or on a
local MagicBlock stack.

Needs **Bun 1.4 or newer**. Older Bun lacks WebCrypto X25519 `deriveBits`, which the Helius
prover's encrypted (HPKE) requests need; the client stops early with a clear message instead of
the SDK's bare `CLIENT_UNEXPECTED`. Scripts load `../.env` with `--env-file`.

```sh
bun install
bun run typecheck
```

## Files

| File | What |
| --- | --- |
| `src/ghost.ts` | Program client: PDAs (room, permission, session token, payment authority), instruction encoders, room decoder |
| `src/crypto.ts` | Box key from a wallet signature, NaCl `box` seal/open, `kind 1` (payment note) and `kind 2` (shielded address) formats |
| `src/net.ts` | Legacy transactions, token-gated PER connections, polling helpers |
| `src/v1.ts` | v1 transactions through `@solana/kit` (Solana only: the rollup rejects v1) |
| `src/payment.ts` | Helius Privacy Rings: register, deposit, private transfer, receipt check, balance sync |
| `src/held-payment.ts` | Held payments: PDA keys, lock, held balance, `ClaimPayment` / `RefundPayment` |
| `scripts/held-payment-devnet.ts` | `bun run held`: lock, rejected early refund, rejected stranger claim, claim, refund after the deadline |
| `scripts/ghost.ts` | `bun run ghost`: devnet CLI (`scenario`, `create`, `open`, `join`, `send`, `read`, `close`) |
| `scripts/payment-devnet.ts` | `bun run payment`: a direct private payment between two wallets |
| `scripts/e2e-local.ts` | `bun run e2e:local`: the room lifecycle on a local MagicBlock stack |

## Devnet

Needs `HELIUS_API_KEY=...` in `../.env` (gitignored) and a funded Solana CLI wallet
(`~/.config/solana/id.json`, or `FUNDER_KEYPAIR`). The scripts create fresh wallets and fund
them from it. Program: `BXofFc5nXaU6skjEbZ8sJQn9gSgsyMFg9kVvpiNVCW4V` (override with
`GHOST_PROGRAM_ID`).

```sh
bun run held                            # held payment: lock → claim, lock → refund
bun run ghost -- scenario               # room on the TEE + payment, end to end
bun run ghost -- scenario --no-pay      # MagicBlock part only (no Helius key needed)
bun run ghost -- read <room> --key <wallet.json>
bun run payment                         # direct private payment only
```

### Held payments

```ts
import * as held from "./src/held-payment";

const payment = held.heldPayment(payer, recipient, deadline);       // PDA + bump
const escrow  = held.pdaKeys(myKeys, theirShieldedAddress, payment); // same keys on both sides

await held.lock(client, payerKeys, escrow, lamports, { depositIfShort });          // payer
await held.release(client, "claim", recipientKeys, payment, escrow, recipientAddr); // recipient
await held.release(client, "refund", payerKeys, payment, escrow, payerAddr);       // payer, after deadline
```

- **Lock** is a normal private transfer to `ShieldedAddress.forPda(pda)`, with
  `pda = ["payment", payer, recipient, deadline]` under the GhostMessage program.
- **Claim / refund** spend the PDA's UTXOs: the client builds the SPP `transact` (proof from the
  Helius prover), lists the PDA as a read-only owner, and wraps it in `ClaimPayment` (8) or
  `RefundPayment` (9). The program checks the signer (and the deadline for a refund), then CPIs
  SPP and signs for the PDA.
- **PDA keys:** a PDA has no keys of its own. Its nullifier and viewing keys come from an ECDH
  of the payer's and recipient's viewing keys, expanded with HKDF with the PDA in the info tag,
  so both of them, and nobody else, can find and spend the UTXO.

## Local MagicBlock stack

```sh
# from the repo root: a build that delegates to the local ER validator
cargo build-sbf --manifest-path program/Cargo.toml --features local-er \
  --sbf-out-dir target/deploy-local --tools-version v1.58 --arch v3
mb-stack --reset --bpf-program $(solana-keygen pubkey target/deploy/ghost_message-keypair.json) \
  target/deploy-local/ghost_message.so                    # leave running

cd client && bun run e2e:local                            # second terminal
```

`mb-stack` (`@magicblock-labs/ephemeral-validator` 1.1.1) runs the base validator (:8899), the
ER (:7799) and the query filtering service (:6699), which plays the TEE endpoint. It already
loads the delegation, permission and session-keys programs.
