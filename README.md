# ghost-message

Private messages (MagicBlock PER + session keys) and confidential payments (Helius Privacy Rings)
on Solana. Pinocchio 0.12, built as SBPF v3, tested in LiteSVM with v1 transactions.

Status: phases 1–2 done. `program/` has the room logic (`CreateRoom`, `OpenRoom`, `JoinRoom`,
`SendMessage`, `CloseRoom`) and MagicBlock session keys (V1 and V2 tokens, read as raw bytes).
No MagicBlock CPIs yet.
Design, findings and roadmap: [docs/research.md](docs/research.md).
Per-phase notes: [docs/phases/](docs/phases/README.md).

```sh
cargo build-sbf --tools-version v1.58 --arch v3
xxd -s 0x30 -l 4 target/deploy/ghost_message.so   # 0300 0000
cd tests && cargo test
```
