//! Records every instruction it receives and succeeds.
//!
//! Emits one `sol_log_data` entry (`Program data: <base64> <base64>`):
//! 1. the instruction data, verbatim;
//! 2. per account, 34 bytes: `address[32] | is_signer u8 | is_writable u8`.
#![no_std]

use pinocchio::{
    AccountView, Address, ProgramResult, no_allocator, nostd_panic_handler, program_entrypoint,
};
use solana_program_log::log_data;

program_entrypoint!(process_instruction);
no_allocator!();
nostd_panic_handler!();

const MAX_ACCOUNTS: usize = 8;

pub fn process_instruction(
    _program_id: &Address,
    accounts: &mut [AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let mut blob = [0u8; 34 * MAX_ACCOUNTS];
    let n = accounts.len().min(MAX_ACCOUNTS);
    for (chunk, account) in blob.as_chunks_mut::<34>().0.iter_mut().zip(accounts.iter()) {
        chunk[..32].copy_from_slice(account.address().as_ref());
        chunk[32] = account.is_signer() as u8;
        chunk[33] = account.is_writable() as u8;
    }
    log_data(&[instruction_data, &blob[..34 * n]]);
    Ok(())
}
