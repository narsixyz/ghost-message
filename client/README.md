# GhostMessage client

TypeScript client for the GhostMessage program, plus the local MagicBlock e2e, the devnet payment run and the devnet scenario.

| File | What |
| --- | --- |
| `src/ghost.ts` | Program client: PDAs, instruction encoders, room decoder, session-token helpers |
| `src/crypto.ts` | Box-key derivation from a wallet signature, NaCl `box` seal/open, `kind 1/2` payload formats |
| `src/net.ts` | Legacy transaction sending, token-gated PER connections, polling helpers |
| `src/v1.ts` | v1 transactions through `@solana/kit` (Solana only: the ER rejects v1) |
| `src/payment.ts` | Helius Privacy Rings: register, deposit, private transfer, receipt check, balance sync |
| `scripts/e2e-local.ts` | Phase 3: full lifecycle on a local MagicBlock stack + open-question probes |
| `scripts/payment-devnet.ts` | Phase 4: confidential payment between two wallets on devnet |
| `scripts/ghost.ts` | Phase 5: devnet CLI (`scenario`, `create`, `open`, `join`, `send`, `read`, `close`) |

```sh
bun install
```

## Phase 3: local MagicBlock stack

```sh
# from the repo root: a build that delegates to the local ER validator
cargo build-sbf --manifest-path program/Cargo.toml --features local-er \
  --sbf-out-dir target/deploy-local --tools-version v1.58 --arch v3
mb-stack --reset --bpf-program $(solana-keygen pubkey target/deploy/ghost_message-keypair.json) \
  target/deploy-local/ghost_message.so                    # leave running

cd client && bun run scripts/e2e-local.ts                 # second terminal
```

`mb-stack` (`@magicblock-labs/ephemeral-validator` 1.1.1) runs the base validator (:8899), the ER
(:7799) and the query filtering service (:6699), which plays the TEE endpoint. It already
loads the delegation, permission and session-keys programs.

## Phases 4–5: devnet

Needs `HELIUS_API_KEY=...` in `../.env` (gitignored) and a funded Solana CLI wallet. The program
is deployed on devnet at `BXofFc5nXaU6skjEbZ8sJQn9gSgsyMFg9kVvpiNVCW4V`.

```sh
bun run payment                         # phase 4: Helius payment only
bun run ghost -- scenario               # phase 5: room on the TEE + payment, end to end
bun run ghost -- scenario --no-pay      # MagicBlock half only (no Helius key needed)
bun run ghost -- read <room> --key <wallet.json>
```

Everything runs on Bun **1.4 or newer** (`bun install`, `bun run <script>`); scripts load `../.env` with `--env-file`. Older Bun lacks WebCrypto X25519 `deriveBits`, which the Helius prover's encrypted (HPKE) requests need.
