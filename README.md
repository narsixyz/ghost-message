# GhostMessage

Private two-person chat rooms and private payments on Solana.

- **Rooms** live on a MagicBlock Private Ephemeral Rollup (PER) inside an Intel TDX TEE. Only the
  two members can read a room, its messages or its transactions.
- **Messages** are encrypted in the client with NaCl `box`, so the TEE operator only sees
  ciphertext. Session keys sign them, so chatting needs no wallet approval per message.
- **Payments** use Helius Privacy Rings, which hide the amount and the asset. A payment can be
  sent directly, or **held by the program**: the funds sit in a private balance owned by a
  GhostMessage PDA, and only the program can release them, to the recipient (`ClaimPayment`) or
  back to the payer after a deadline (`RefundPayment`).

The on-chain program is pure [Pinocchio](https://github.com/anza-xyz/pinocchio) 0.12 (`no_std`,
no allocator, no Anchor), built as SBPF v3. Every external call (MagicBlock delegation,
permissions, commits, session tokens, the Helius Privacy Program) is hand-encoded.

**Devnet:** `BXofFc5nXaU6skjEbZ8sJQn9gSgsyMFg9kVvpiNVCW4V` (39,728 bytes, TEE validator
`MTEWGuqx…`). Design notes and research: [docs/research.md](docs/research.md).

## How it works

```text
            client (Bun)                      MagicBlock TEE rollup                 Solana
 ┌────────────────────────────┐     ┌─────────────────────────────────┐   ┌──────────────────────────────┐
 │ wallet                     │     │ Room account (4,256 B)          │   │ GhostMessage program         │
 │ box key ← sign(msg)        │ PER │  permission: [creator, guest]   │   │  CreateRoom / DelegateRoom   │
 │ session key (no popups)    ├────►│  16 encrypted message slots     │◄─►│  ReclaimRoom                 │
 │ NaCl box encrypt/decrypt   │     │ OpenRoom, JoinRoom, SendMessage │   │  ClaimPayment / RefundPayment│
 │                            │     │ CloseRoom, UndelegateRoom       │   │      │ CPI, invoke_signed     │
 │ Helius private wallet      │     └─────────────────────────────────┘   │      ▼                       │
 │ (@heliuslabs/spr)          ├──────────────────────────────────────────►│ Helius Privacy Program (SPP) │
 └────────────────────────────┘                                           └──────────────────────────────┘
```

### Rooms

1. `CreateRoom` + `DelegateRoom` (Solana): the creator makes the room PDA and delegates it to the
   TEE validator, which is a compile-time constant.
2. `OpenRoom` (PER): sets the guest and, **in the same instruction**, creates the private
   permission with exactly two members. A delegated account with no permission is public, so
   the guest is never written without it. The permission CPI only exists on the rollup, so
   `OpenRoom` can't run on Solana.
3. `JoinRoom`, `SendMessage` (PER): signed by a session key (`gpl_session` V2 token, read as raw
   bytes) or the wallet. Messages go into a 16-slot ring buffer.
4. `CloseRoom` (PER) wipes every private byte (guest, keys, messages, message count) and closes
   the permission; `UndelegateRoom` sends the room back to Solana; `ReclaimRoom` returns the rent
   to the creator.

### Payments

The receiver shares their shielded address in the room (an encrypted `kind 2` message). The payer
pays with the Helius SDK and sends an encrypted `kind 1` payment note (transaction signature,
mint, amount). The receiver treats the note as a claim until they decrypt the matching output
with their own keys.

### Held payments (the program controls private funds)

| Step | Who | What happens |
| --- | --- | --- |
| Lock | payer | A normal Privacy Rings transfer to `ShieldedAddress.forPda(pda)`, where `pda = ["payment", payer, recipient, deadline]` |
| Claim | recipient | `ClaimPayment`: the program checks the recipient signed, then CPIs SPP `transact` and signs for the PDA with `invoke_signed` |
| Refund | payer | `RefundPayment`: same, but only the payer and only after the deadline |

SPP spends a UTXO only when its owner signs, and only GhostMessage can sign for that PDA, so the
program decides who moves the funds. The client builds the private transaction and its proof;
the program forwards it unchanged (SPP `transact`, tag 16) after its own checks. The PDA's
nullifier and viewing keys come from an ECDH of the payer's and recipient's viewing keys, so
both of them, and nobody else, can find and spend the locked UTXO.

On devnet, a claim shows GhostMessage at the top level calling the Privacy Program as an inner
instruction (143k CU in total, ~3.8k of it GhostMessage's own).

## Instructions

| # | Instruction | Runs on | Signer |
| --- | --- | --- | --- |
| 0 | `CreateRoom(room_id)` | Solana | creator |
| 1 | `DelegateRoom` | Solana | creator |
| 2 | `OpenRoom(guest, creator_box_pk, expires_at)` + private permission | PER | creator (wallet only) |
| 3 | `JoinRoom(guest_box_pk)` | PER | guest or its session key |
| 4 | `SendMessage(kind, nonce, ciphertext)` | PER | member or its session key |
| 5 | `CloseRoom` + close permission | PER (Solana if never opened) | member, or anyone after expiry |
| 6 | `UndelegateRoom` | PER | anyone, `Closed` rooms only |
| 7 | `ReclaimRoom` | Solana | anyone; rent goes to the creator |
| 8 | `ClaimPayment(payer, deadline, bump, transact)` | Solana | recipient |
| 9 | `RefundPayment(recipient, deadline, bump, transact)` | Solana | payer, after the deadline |
| `[196, 28, 41, 206, 48, 37, 51, 167]` | `ProcessUndelegation` | Solana | delegation program callback |

Each processor in `program/src/instructions/` documents its accounts, data layout and errors.
Error codes are in `program/src/error.rs` (`0`–`14`).

## What is private

| | Hidden from the public? |
| --- | --- |
| That a room exists, and its creator | No (`CreateRoom` is on Solana) |
| The guest, the messages, how many and when | Yes (rollup + permission; wiped before the room returns) |
| Message text from the TEE operator | Yes (encrypted in the client) |
| A private transfer's amount and asset | Yes (Privacy Rings) |
| A deposit's amount | No |
| Who paid whom (direct payment) | No, the Default Ring shows sender and recipient |
| Who a held payment is for, at lock time | Yes, the lock names the PDA, not the recipient |
| Who claimed it | No, the claim is signed by the recipient and names the payer |

## Build and test

Rust (pinned in `rust-toolchain.toml`), Solana CLI with platform tools v1.58, and Bun 1.4 or newer.

```sh
cargo build-sbf --tools-version v1.58 --arch v3      # program + test-only cpi-recorder
xxd -s 0x30 -l 4 target/deploy/ghost_message.so   # 0300 0000 (SBPF v3)
cd tests && cargo test                             # 91 LiteSVM tests
cargo test -p ghost-message                        # 4 unit tests (from the repo root)
```

The tests run against the real delegation and session-keys programs (dumped from devnet). The
MagicBlock ER programs and the Helius Privacy Program are replaced by `cpi-recorder`, which logs
every CPI so the tests can check the exact bytes, accounts and signer flags the program sends.

## Run it

```sh
cd client && bun install
bun run held                       # held payment on devnet: lock, claim, refund
bun run ghost -- scenario          # room on the TEE + payment, end to end
```

Devnet runs need `HELIUS_API_KEY=...` in `.env` at the repo root (gitignored) and a funded
Solana CLI wallet. See [client/README.md](client/README.md) for every script.

## Layout

```text
program/              the on-chain program (Pinocchio, no_std)
  src/instructions/   one file per instruction (payment.rs: ClaimPayment, RefundPayment)
  src/utils/          accounts, data reader, session tokens, MagicBlock CPIs, privacy.rs (SPP CPI)
tests/                LiteSVM tests: own Cargo workspace and lockfile
  room.rs             rooms, messages, close
  session.rs          session keys (real gpl_session program)
  magicblock.rs       delegation, permissions, undelegation
  privacy.rs          ClaimPayment / RefundPayment: SPP transact CPI signed by the payment PDA
  common/mod.rs       shared harness
  cpi-recorder/       test-only program standing in for MagicBlock's ER programs and SPP
  fixtures/           programs dumped from devnet (delegation, session keys)
client/               Bun client: program client, Helius payments, held payments, e2e scripts
docs/                 research.md
```

## Limits

- Devnet only. Mainnet needs the mainnet session program, Helius mainnet endpoints and an audit.
- No forward secrecy: NaCl `box` with long-lived keys.
- With the Default Ring, payer and payee are public on a direct payment, and a claim links them.
  Anonymous Custom Rings (not live yet) would hide both.
- A message and a payment are separate transactions on separate layers, so a payment note is a
  claim until the receiver confirms it.
