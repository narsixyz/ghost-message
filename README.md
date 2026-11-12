# ghost-message

Private messages (MagicBlock PER + session keys) and confidential payments (Helius Privacy Rings)
on Solana. Pinocchio 0.12, built as SBPF v3, tested in LiteSVM with v1 transactions.

Status: phases 1–2 done, phase 3 in progress. `program/` has the full room lifecycle (create,
delegate, open with a private permission, join, send, close, undelegate, reclaim) and MagicBlock
session keys. The e2e on a local MagicBlock stack is next.
Design, findings and roadmap: [docs/research.md](docs/research.md).
Per-phase notes: [docs/phases/](docs/phases/README.md).

```sh
cargo build-sbf --tools-version v1.58 --arch v3      # program + test-only cpi-recorder
xxd -s 0x30 -l 4 target/deploy/ghost_message.so   # 0300 0000
cd tests && cargo test
```
