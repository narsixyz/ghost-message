# GhostMessage: Research & Design

Private messages. Confidential payments. Solana.

As of 2026-11-13. Phases 1–5 are implemented and run on devnet: the program is deployed at `BXofFc5nXaU6skjEbZ8sJQn9gSgsyMFg9kVvpiNVCW4V`, and one full scenario (private room on the MagicBlock TEE + a Helius private payment) passes end to end.

## 1. The idea

Two people talk in a temporary chat room. Either of them can attach a private token payment to a message.

1. User A creates a room and invites User B.
2. Both create a session key, so they can chat without approving every message in their wallet.
3. A sends "Here's the bounty for fixing that bug."
4. A sends B a confidential payment.
5. B reads the message and finds the payment in its private balance.

Messaging and payments use two **separate** privacy systems:

| Part | Technology | What it hides |
| --- | --- | --- |
| Room state and messages | MagicBlock Private Ephemeral Rollup (PER) | Room contents and room transactions, from anyone who isn't a member |
| Frequent messages without wallet pop-ups | MagicBlock session keys (`session-keys` program) | Nothing. This is a UX feature, not a privacy one |
| Message confidentiality, including from the operator | Client-side encryption (NaCl `box`) | Message text, even from the TEE operator and after undelegation |
| Payment | Helius Privacy Rings (Default Ring) | Asset and amount. **Not** sender or recipient |
| Rooms, membership, authorization | Our Pinocchio program | Nothing on its own. It enforces who can do what |

**Ground rules for v1:**
- The message and the payment are two separate transactions on two different execution layers. We don't claim atomic delivery.
- Privacy Rings does not make the message private, and the PER does not make the payment private.
- TEE execution alone is not end-to-end encryption. Messages are encrypted on the client.

## 2. Architecture

```text
 User A (wallet + session key)                 User B (wallet + session key)
        │                                               │
        │  base layer (Solana): create room, delegate, session tokens
        ▼                                               ▼
 ┌──────────────────────────────────────────────────────────────────┐
 │ GhostMessage program (Pinocchio 0.12, SBPF v3)                    │
 │  Room PDA ["room", creator, room_id]                              │
 └──────────────────────────────────────────────────────────────────┘
        │ delegate to the TEE validator
        ▼
 ┌──────────────────────────────────────────────────────────────────┐
 │ MagicBlock PER (TEE ER + permission program)                      │
 │  OpenRoom · JoinRoom · SendMessage · CloseRoom · UndelegateRoom   │
 │  Room readable only by creator + guest (EphemeralPermission)      │
 └──────────────────────────────────────────────────────────────────┘

 Payment, separate tx on Solana:
 A's shielded balance ──SPP transact──► B's shielded balance   (Helius Privacy Rings)
 A then sends an encrypted "payment note" message in the room pointing at that tx.
```

## 3. What we learned

### 3.1 Prior art on this machine: `cyphers-program-v1`

`~/Workspace/companies/cyphers/cyphers-magicblock/cyphers-program-v1` is a Pinocchio 0.12 + SBPF v3 + v1 program that already runs on MagicBlock PER. It ran on a real local MagicBlock stack. GhostMessage should reuse its patterns, not rediscover them.

- **MagicBlock CPIs are hand-encoded** (`program/src/utils/magicblock.rs`). `ephemeral-rollups-pinocchio` 0.17.3 needs Pinocchio 0.11 with `alloc`, which can't share a build with Pinocchio 0.12 and `no_allocator!`. The file covers:
  - `delegate_pda`: buffer PDA → zero → `Assign` to the delegation program → `delegate` CPI. Then `close_buffer` after the last CPI.
  - `schedule_commit(..., undelegate)`: Magic program `ScheduleIntentBundle` (variant 11), commit or commit-and-undelegate.
  - `create_ephemeral_permission`: permission program discriminator 6, `is_private`, plus members. ER only.
  - The undelegation callback discriminator `[196, 28, 41, 206, 48, 37, 51, 167]`.
- **Testing split:**
  - LiteSVM has no ER. "Inside the PER" instructions are tested against program-owned accounts, which is how a delegated account looks inside the ER.
  - `delegate` runs against the real delegation program, dumped to `tests/fixtures/delegation_program.so`.
  - `e2e/` (TypeScript, bun) runs everything on `mb-stack`, including who-can-read-what probes through the query filtering service.
- **Lessons:**
  - A permission is created **after** delegation, on the ER. Until then the delegated account is readable.
  - Undelegation writes the account data back to Solana, so private data must be zeroed first.
  - Token accounts stay public unless they're made private, and making them private made the endpoint answer `403` to transactions that touched them.

### 3.2 MagicBlock PER

**Addresses:**

| Item | Value |
| --- | --- |
| Delegation program | `DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh` |
| Magic program / context | `Magic11111111111111111111111111111111111111` / `MagicContext1111111111111111111111111111111` |
| Permission program | `ACLseoPoyC3cBqoUtkbjZ4aDrkurZW86v19pXz2XQnp1` |
| TEE validator (devnet and mainnet) | `MTEWGuqxUpYZGFJQcp8tLN7x5v9BSeoFHYWQQ3n3xzo` |
| Devnet TEE RPC | `https://devnet-tee.magicblock.app?token=<authToken>` |
| Local ER validator identity | `mAGicPQYBMvcYveUZA5F5UNNwyHvfYh5xkLS2Fr1mev` |

**Lifecycle:**

1. The account is created on Solana.
2. It's delegated to the TEE validator.
3. An `EphemeralPermission` is created on the ER. The PDA signs and pays its rent, so the PDA must be pre-funded.
4. Transactions run in the ER.
5. It's committed, or committed and undelegated.
6. The delegation program calls back into our program to re-create the account on Solana.

**Permissions:**
- `is_private` plus a member list. Flags: `AUTHORITY`, `TX_LOGS`, `TX_BALANCES`, `TX_MESSAGE`, `ACCOUNT_SIGNATURES`.
- "Today a permission implies read access." There is no separate write permission yet, so write authorization is our program's job.
- Updating or closing a permission is a CPI too. MagicBlock warns: "always include the authority so the data PDA can't lock itself out."

**Reading private state:**
- The client calls `verifyTeeRpcIntegrity` (Intel TDX attestation), then `getAuthToken(rpc, wallet.publicKey, signFn)`. That's one wallet signature, which yields a token.
- The endpoint filters reads by the token's public key against the member list.

**Fees:**
- "Normal ER transactions cost 0 in the current release."
- Commits 1–10 are free without a fee payer.
- Undelegation charges a 0.003 SOL session fee, plus 0.001 SOL per commit after the first.

**Local stack:**
- Install with `bun add -g @magicblock-labs/ephemeral-validator@latest` (1.1.1). `mb-stack --reset` then runs three services:
  - Base layer: `mb-test-validator` on 8899.
  - ER: `ephemeral-validator` on 7799.
  - TEE ingress emulator: the query filtering service (QFS) on 6699.
- The QFS handles token auth and permission filtering, but not TDX attestation.

### 3.3 Session keys

Source: `magicblock-labs/gum-program-library`, `programs/gpl_session`. Crate `session-keys` 3.1.1 (Anchor). Program `KeyspM2ssCJbqUhQ4k7sveSiY4WjnYsrXkC8oDbwde5`.

- **Pieces:** an ephemeral keypair the client keeps, plus a **session token** PDA on Solana:
  ```text
  seeds = ["session_token", target_program, session_signer, authority]   (under KeyspM...)
  data  = disc[8] = sha256("account:SessionToken")[..8]
          authority[32] | target_program[32] | session_signer[32] | valid_until: i64   (112 bytes)
  ```
  V2 (`"session_token_v2"`) adds `fee_payer[32]` before `valid_until` (144 bytes).
- **Creation:** the wallet signs `create_session`. `valid_until` defaults to now + 1 h and can be at most 7 days. With `top_up` (default 0.01 SOL), the session signer also gets lamports.
- **Revocation:**
  - V1: anyone can revoke, and the rent goes back to the authority.
  - V2: only the authority can revoke while the session is still active.
- **Validation in the target program**, which is what the `session_auth_or` macro does:
  1. Token owner = `KeyspM...`.
  2. Discriminator matches.
  3. The PDA re-derives from `(target_program = our program, session_signer = the tx signer, authority)`.
  4. `now < valid_until`.

  If no token is passed, it falls back to "the signer must be the authority."
- **Pinocchio:** there is no Pinocchio crate. We read the 112 bytes ourselves. That's easy and needs no CPI.
- **Inside the ER:**
  - The token is a Solana account that isn't delegated, so the ER clones it read-only. cyphers' `Config` relies on the same behavior.
  - A revoke on Solana only reaches the ER once the clone re-syncs. That timing is unverified.

### 3.4 Helius Privacy Rings

The full background is in the private-escrow research. What matters here:

- **Hidden and public:**
  - The Default Ring hides **asset and amount**.
  - **Sender and recipient are public.** On Solana anyone can see that A paid B.
  - Deposits and withdrawals to public balances show the amount.
- **Before receiving**, the recipient registers a private wallet. Its shielded address is `owner_hash + viewing_pk`, 65 bytes, and the sender needs it.
- **Operations:** register, deposit, sync balance, private transfer, withdraw, read history. The TypeScript SDK is `@heliuslabs/spr` 0.9.0-alpha. `@heliuslabs/zolana` 0.5.0-alpha is the lower-level SDK.
- **Cost and placement:** a transfer costs about 140k CU and is about 1,279 bytes as a v1 transaction. It runs on Solana, **not** in the ER. The SPP program, Merkle tree, Photon indexer and prover all live with the base chain.

## 4. Design

### 4.1 Room account

One PDA per room: `["room", creator, room_id: u64]`. It's a fixed size, because a delegated account can't be resized safely. Messages live in a **ring buffer** inside the room, so no accounts are created inside the ER.

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 1 | discriminator |
| 1 | 1 | bump |
| 2 | 1 | status: `Created` = 0, `Open` = 1, `Joined` = 2, `Closed` = 3 |
| 3 | 5 | padding |
| 8 | 32 | creator |
| 40 | 32 | guest (zero until `OpenRoom`) |
| 72 | 32 | creator X25519 box public key |
| 104 | 32 | guest X25519 box public key (zero until `JoinRoom`) |
| 136 | 8 | `room_id` (u64, PDA seed so the room can sign its CPIs) |
| 144 | 8 | `expires_at` (i64, unix) |
| 152 | 8 | `next_seq` (u64) |
| 160 | 16 × 256 | messages: `seq u64, sender u8 (0 creator / 1 guest), kind u8, len u16, nonce[24], ciphertext[220]` |

The total is 4,256 bytes. `kind` is 0 for text, 1 for a payment note and 2 for a shielded address. A ciphertext of 220 bytes holds 204 bytes of plaintext plus a 16-byte MAC. The 17th message overwrites the oldest. There's no history indexing, and that's intentional.

### 4.2 Instructions

| Disc | Instruction | Runs on | Signer | What it does |
| --- | --- | --- | --- | --- |
| 0 | `CreateRoom(room_id)` | Solana | creator | Creates the room. It is pre-funded so it can pay the permission rent. **No guest is stored here.** Solana only learns "creator made a room" |
| 1 | `DelegateRoom` | Solana | creator | Delegates the room to the TEE validator |
| 2 | `OpenRoom(guest, creator_box_pk, expires_at)` | PER | creator | Writes the guest and the creator's key, and CPIs `CreateEphemeralPermission(private, [creator, guest])` **in the same instruction**, so the guest is never visible |
| 3 | `JoinRoom(guest_box_pk)` | PER | guest or guest's session key | Status becomes `Joined` |
| 4 | `SendMessage(kind, nonce, ciphertext)` | PER | a member or that member's session key | Writes the next ring slot. Requires `Joined` and `now < expires_at` |
| 5 | `CloseRoom` | PER | the creator (any status), the guest (once `Open`), or anyone after `expires_at` | Zeroes the guest, keys, `expires_at`, `next_seq` and messages. Status becomes `Closed`. A room that was never opened has `expires_at = 0` and never expires |
| 6 | `UndelegateRoom` | PER | anyone, once `Closed` | Commit and undelegate |
| 7 | `ReclaimRoom` | Solana | anyone | Closes the room and returns the rent to the creator |
| 196 | `ProcessUndelegation` | Solana | delegation program | Callback that re-creates the room on Solana |

**Authorization helper (`actor`)** for instructions 3–5:
- Accounts are `[signer, room]`, optionally followed by `session_token`.
- With a token: check it as in section 3.3 with `target_program = our program id` and `session_signer = signer`. Then `actor = token.authority`. V1 and V2 tokens are both accepted. A token that is passed but invalid is an error (`InvalidSessionToken` / `SessionExpired`), never a fallback to the signer.
- Clients should create **V2** sessions: anyone can revoke a V1 token at any time, only the authority can revoke an active V2 token.
- Without a token: `actor = signer`.
- Then `actor` must be the creator or the guest, as the instruction requires.

A session key can only send, join and close. It can't create, delegate or move funds.

### 4.3 Message encryption

- **Box keys:** each user derives an X25519 box key from a wallet signature over a fixed string (`"GhostMessage box key v1"` → sha512 → `nacl.box.keyPair.fromSecretKey`). Any device can re-create it, and the wallet signs only once. It is never reused for anything else, including the Helius viewing key.
- **Sending:** the message is `nacl.box(plaintext, nonce, peer_box_pk, my_box_sk)`, written to the room. Only the two members can decrypt it, not the TEE operator. Ciphertext that reaches Solana after undelegation stays unreadable. We still zero it first.
- **No new protocol:** this is plain NaCl `box`, with no ratchet and no forward secrecy. That's acceptable for a temporary room and worth saying in the article.

### 4.4 Payment flow (separate from the room)

1. **Register (B, once):** B registers a Helius private wallet.
2. **Share the address:** B sends its shielded address to A as a `kind = 2` message. The PER keeps the address exchange private.
3. **Pay:** A deposits into its shielded balance if needed, then makes a private transfer to B's shielded address. This happens on Solana, with `@heliuslabs/spr`, and gives a transaction signature.
4. **Payment note:** A sends a `kind = 1` message with the encrypted `{tx signature, asset, amount}`.
5. **Confirm:** B syncs its private balance and matches the new UTXO to the note. Until then the UI shows the payment as *claimed by sender*, not *received*.

The two steps can fail independently: the payment can land without the note, or the note can arrive without the payment. The UI must show both states.

### 4.5 What each observer sees

| Observer | Sees | Doesn't see |
| --- | --- | --- |
| Anyone on Solana | creator made a room (`CreateRoom`, `DelegateRoom`), session token creation, the final empty room after undelegation, **Helius transfer A → B** (A signs; B's address is in the instruction data as the view tag — measured in phase 4) | guest, messages, amount, asset |
| Anyone on the PER endpoint who isn't a member | that the room account exists | room data, room transactions (if `TX_MESSAGE`/`TX_LOGS` are withheld) |
| Members (creator, guest) | everything in the room | – |
| TEE operator (in theory, if the TEE is broken) | room metadata, who sends when | message text (client-side encryption) |

**The biggest privacy gap:** the Default Ring's public sender and recipient link A and B on Solana. That undoes the hiding of room membership for any room with a payment. This must be written up, not hidden. Options later: an anonymous Custom Ring, or paying through an intermediary note.

## 5. Open questions (to settle in a spike before building)

**All answered in phase 3** (mb-stack) and confirmed on devnet in phase 5. In short: session keys write through the member's authenticated connection without being permission members (1); tokens are readable in the ER and a revoke is seen immediately (2); the ER rejects v1 transactions (3); session keys need no lamports on the ER (4); the permission format is `9 + 33·n` (5); the `OpenRoom` transaction is stripped for non-members (6).

1. **Session key through the TEE endpoint.**
   - The read token belongs to a wallet, and permission members are wallets.
   - Will the endpoint accept a transaction **signed by a session key** that writes a private room, over a connection authenticated as the member wallet?
   - If not, the session key must be a member too, which means `UpdateEphemeralPermission` (encoding unknown) or members `[A, B, A_session, B_session]` set at `OpenRoom`.
2. **Session token in the ER.**
   - Is a Solana token account that isn't delegated cloned and readable inside the PER?
   - How quickly does a revoke on Solana reach the ER?
3. **v1 transactions on the ER.** Does `ephemeral-validator` 1.1.1 or the TEE endpoint accept v1 (`VersionedMessage::V1`) transactions? If not, ER transactions use v0 and only Solana transactions use v1.
4. **Fee payer in the ER.** ER transactions cost 0. Does the session signer still need to exist with lamports in the ER to act as fee payer?
5. **Permission with two members.** ~~Confirm the wire format~~ **Answered:** `u64 6 | is_private | (flags, pubkey)*`, no count, `9 + 33·n` bytes (`ephemeral-rollups-pinocchio` 0.17.3).
6. **Transaction visibility.** Is the `OpenRoom` transaction message (it carries the guest's key) hidden from people who aren't members? Probe it with the QFS, the way cyphers' `PRIVATE=1` run did.
7. **Two local stacks.** MagicBlock needs `mb-stack` (its own base validator). Helius needs surfpool + Photon + prover (`zolana dev artifacts`). They don't share a base chain locally, so the joined flow can only be tested on **devnet**. Both live on Solana devnet.
8. **Session program locally.** `mb-test-validator` must load `KeyspM...`: dump it from devnet like the delegation program.

## 6. Roadmap

| Phase | Goal | Done when |
| --- | --- | --- |
| 0 | Scaffold (**done**) | Pinocchio 0.12, SBPF v3, LiteSVM v1 tests pass |
| 1 | Room logic without MagicBlock (**done**) | `CreateRoom`, `OpenRoom` (no CPI yet), `JoinRoom`, `SendMessage`, `CloseRoom` with direct signers; LiteSVM tests for every auth and status error, ring wrap-around, expiry |
| 2 | Session keys (**done**) | `actor` helper; LiteSVM tests with a hand-made token account (wrong owner, wrong PDA, expired, wrong target program, other user's token) |
| 3 | MagicBlock (**done**) | Port `magicblock.rs` from cyphers; delegate test with `delegation_program.so`; then a TypeScript e2e on `mb-stack`: two wallets, session keys, private reads through the QFS, a non-member gets nothing. Answers questions 1–6 |
| 4 | Helius payment (**done**) | Register B, deposit, private transfer A→B, B syncs; `kind = 1`/`2` messages; on devnet |
| 5 | Devnet end-to-end (**done**) | Devnet TEE (`verifyTeeRpcIntegrity` for real) + Helius devnet, one full scenario. A minimal CLI client, not a complex frontend |
| 6 | Article | "GhostMessage: building private messaging and payments on Solana with MagicBlock and Helius Privacy Rings", from the research log |

**Out of scope:** group chats, file uploads, history indexing, a custom E2EE protocol, custom ZK circuits, a complex frontend, notifications.

## 7. Research log template

One entry per session in `docs/research-log/`:

```markdown
# Research Log — <date>
## Topic
## What I understood
## How it works
## What I tested
## Problems I encountered
## Questions remaining
## Article material
```

## Sources

- MagicBlock docs:
  - [PER quickstart](https://docs.magicblock.gg/pages/private-ephemeral-rollups-pers/how-to-guide/quickstart.md)
  - [Access control](https://docs.magicblock.gg/pages/private-ephemeral-rollups-pers/how-to-guide/access-control.md)
  - [Authorization](https://docs.magicblock.gg/pages/private-ephemeral-rollups-pers/introduction/authorization.md)
  - [Local development](https://docs.magicblock.gg/pages/private-ephemeral-rollups-pers/how-to-guide/local-development.md)
  - [Fees](https://docs.magicblock.gg/pages/ephemeral-rollups-ers/introduction/fees-and-commit-economics.md)
- Session keys:
  - [How they work](https://docs.magicblock.gg/pages/tools/session-keys/how-do-session-keys-work.md)
  - [Integrating](https://docs.magicblock.gg/pages/tools/session-keys/integrating-sessions-in-your-program.md)
  - [`gpl_session` source](https://github.com/magicblock-labs/gum-program-library/tree/main/programs/gpl_session)
- [MagicBlock starter kits](https://github.com/magicblock-labs/starter-kits)
- Helius:
  - [Privacy Rings docs](https://www.helius.dev/docs/privacy)
  - [privacy-ring-examples](https://github.com/helius-labs/privacy-ring-examples)
  - npm `@heliuslabs/spr`, `@heliuslabs/zolana`
- Local: `cyphers-program-v1` (README, `findings.md`, `program/src/utils/magicblock.rs`, `e2e/README.md`) and `private-escrow-pinocchio/docs/research.md`
