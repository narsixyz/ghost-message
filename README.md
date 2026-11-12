# ghost-message

Private messages (MagicBlock PER + session keys) and confidential payments (Helius Privacy Rings)
on Solana. Pinocchio 0.12, built as SBPF v3, tested in LiteSVM with v1 transactions.

Status: the room program is on devnet at `BXofFc5nXaU6skjEbZ8sJQn9gSgsyMFg9kVvpiNVCW4V`
and one full scenario runs end to end: a private room on MagicBlock's TEE (session keys, NaCl-box
encrypted messages) carrying a confidential Helius Privacy Rings payment.
Design, findings and roadmap: [docs/research.md](docs/research.md).

```sh
cargo build-sbf --tools-version v1.58 --arch v3      # program + test-only cpi-recorder
xxd -s 0x30 -l 4 target/deploy/ghost_message.so   # 0300 0000
cd tests && cargo test
```

## Layout

```text
program/              the on-chain program (Pinocchio, no_std)
tests/                LiteSVM tests: own Cargo workspace and lockfile
  room.rs             phase 1: rooms, messages, close
  session.rs          phase 2: session keys (real gpl_session program)
  magicblock.rs       phase 3: delegation, permissions, undelegation
  privacy.rs          ClaimPayment / RefundPayment: SPP transact CPI signed by the payment PDA
  common/mod.rs       shared harness
  cpi-recorder/       test-only on-chain program standing in for MagicBlock's ER programs and SPP
  fixtures/           programs dumped from devnet (delegation, session keys)
client/               TypeScript client, mb-stack e2e, devnet payment and CLI (see client/README.md)
docs/                 research.md
```
