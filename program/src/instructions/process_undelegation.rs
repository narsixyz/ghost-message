use pinocchio::{
    AccountView, Address, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
    sysvars::{Sysvar, rent::Rent},
};
use pinocchio_system::instructions::CreateAccount;

use crate::{
    constants::ROOM_SEED,
    utils::{
        accounts::require_program,
        magicblock::{DELEGATION_PROGRAM_ID, UNDELEGATE_BUFFER_SEED, parse_seeds},
    },
};

/// # ProcessUndelegation (discriminator `[196, 28, 41, 206, 48, 37, 51, 167]`) — Solana
///
/// Callback the delegation program invokes when a committed room is undelegated. It re-creates
/// the room under this program with the committed data.
///
/// 1. Only the delegation program can call this meaningfully: `buffer` must sign and be owned by
///    it, and must be the canonical `["undelegate-buffer", room]` PDA.
/// 2. The seeds come from the delegation record: exactly `["room", creator, room_id]`. The room
///    is re-created with `CreateAccount` signed by those seeds, so it can only land at the
///    matching PDA.
/// 3. The buffer's bytes are copied verbatim (`CloseRoom` zeroed everything private first).
///
/// ## Accounts
/// 0. `[writable]` room — the account being returned
/// 1. `[signer]` buffer — `["undelegate-buffer", room]` under the delegation program
/// 2. `[signer, writable]` payer — pays the re-created room's rent
/// 3. `[]` system_program
///
/// ## Instruction data (after the 8-byte discriminator)
/// Borsh `Vec<Vec<u8>>` seeds: `u32 count`, then per seed `u32 len` + bytes.
///
/// ## Errors
/// - `InvalidInstructionData`: malformed seeds, wrong count, or not a room seed
/// - `NotEnoughAccountKeys`, `MissingRequiredSignature` (buffer), `IncorrectProgramId`
/// - `InvalidAccountOwner`: buffer is not owned by the delegation program
/// - `InvalidSeeds`: buffer is not the canonical undelegation buffer
pub fn process_undelegation(
    program_id: &Address,
    accounts: &mut [AccountView],
    data: &[u8],
) -> ProgramResult {
    let [room, buffer, payer, system_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_program(system_program, &pinocchio_system::ID)?;
    if !buffer.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if !buffer.owned_by(&DELEGATION_PROGRAM_ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }
    let (expected_buffer, _) = Address::find_program_address(
        &[UNDELEGATE_BUFFER_SEED, room.address().as_ref()],
        &DELEGATION_PROGRAM_ID,
    );
    if buffer.address() != &expected_buffer {
        return Err(ProgramError::InvalidSeeds);
    }

    let mut seeds: [&[u8]; 3] = [&[]; 3];
    if parse_seeds(data, &mut seeds)? != 3 || seeds[0] != ROOM_SEED {
        return Err(ProgramError::InvalidInstructionData);
    }
    let (_, bump) = Address::find_program_address(&seeds, program_id);
    let bump_seed = [bump];
    let signer_seeds = [
        Seed::from(seeds[0]),
        Seed::from(seeds[1]),
        Seed::from(seeds[2]),
        Seed::from(&bump_seed),
    ];

    let space = buffer.data_len();
    CreateAccount {
        from: payer,
        to: room,
        lamports: Rent::get()?.try_minimum_balance(space)?,
        space: space as u64,
        owner: program_id,
    }
    .invoke_signed(&[Signer::from(&signer_seeds)])?;

    room.try_borrow_mut()?
        .copy_from_slice(&buffer.try_borrow()?);
    Ok(())
}
