//! GhostMessage program: temporary two-person chat rooms with client-side encrypted messages.
//! Design: `docs/research.md`.
//!
//! Phases 1–2: room logic, plus session keys for `JoinRoom`, `SendMessage` and `CloseRoom`
//! (`utils::session`). No MagicBlock CPIs yet.
//!
//! | Discriminator | Instruction | Runs on | Status |
//! |---|---|---|---|
//! | 0 | `CreateRoom(room_id)` | Solana | implemented |
//! | 1 | `DelegateRoom` | Solana | phase 3 |
//! | 2 | `OpenRoom(guest, creator_box_pk, expires_at)` | PER | implemented, no permission CPI yet |
//! | 3 | `JoinRoom(guest_box_pk)` | PER | implemented |
//! | 4 | `SendMessage(kind, nonce, ciphertext)` | PER | implemented |
//! | 5 | `CloseRoom` | PER | implemented |
//! | 6 | `UndelegateRoom` | PER | phase 3 |
//! | 7 | `ReclaimRoom` | Solana | phase 3 |
#![no_std]

pub mod constants;
pub mod error;
pub mod instructions;
pub mod state;
pub mod utils;

use pinocchio::{
    AccountView, Address, ProgramResult, error::ProgramError, no_allocator, nostd_panic_handler,
    program_entrypoint,
};

program_entrypoint!(process_instruction);
no_allocator!();
nostd_panic_handler!();

pub fn process_instruction(
    program_id: &Address,
    accounts: &mut [AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let (discriminator, data) = instruction_data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;
    match *discriminator {
        0 => instructions::create_room(program_id, accounts, data),
        2 => instructions::open_room(program_id, accounts, data),
        3 => instructions::join_room(program_id, accounts, data),
        4 => instructions::send_message(program_id, accounts, data),
        5 => instructions::close_room(program_id, accounts, data),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}
