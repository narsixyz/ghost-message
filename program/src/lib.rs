//! GhostMessage program: temporary two-person chat rooms with client-side encrypted messages.
//! Design: `docs/research.md`.
//!
//! Session keys (`utils::session`) work for `JoinRoom`, `SendMessage` and `CloseRoom`. MagicBlock
//! CPIs (`utils::magicblock`) are hand-encoded.
//!
//! | Discriminator | Instruction | Runs on |
//! |---|---|---|
//! | 0 | `CreateRoom(room_id)` | Solana |
//! | 1 | `DelegateRoom` | Solana |
//! | 2 | `OpenRoom(guest, creator_box_pk, expires_at)` + private permission | PER |
//! | 3 | `JoinRoom(guest_box_pk)` | PER |
//! | 4 | `SendMessage(kind, nonce, ciphertext)` | PER |
//! | 5 | `CloseRoom` + close permission | PER (Solana if never opened) |
//! | 6 | `UndelegateRoom` | PER |
//! | 7 | `ReclaimRoom` | Solana |
//! | `[196, 28, 41, 206, 48, 37, 51, 167]` | `ProcessUndelegation` (delegation program callback) | Solana |
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
        1 => instructions::delegate_room(program_id, accounts, data),
        2 => instructions::open_room(program_id, accounts, data),
        3 => instructions::join_room(program_id, accounts, data),
        4 => instructions::send_message(program_id, accounts, data),
        5 => instructions::close_room(program_id, accounts, data),
        6 => instructions::undelegate_room(program_id, accounts, data),
        7 => instructions::reclaim_room(program_id, accounts, data),
        _ => {
            let rest = instruction_data
                .strip_prefix(&utils::magicblock::UNDELEGATE_CALLBACK_DISCRIMINATOR)
                .ok_or(ProgramError::InvalidInstructionData)?;
            instructions::process_undelegation(program_id, accounts, rest)
        }
    }
}
