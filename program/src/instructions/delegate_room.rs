use pinocchio::{AccountView, Address, ProgramResult, cpi::Seed, error::ProgramError};

use crate::{
    constants::{ROOM_SEED, status},
    error::GhostError,
    state::Room,
    utils::{
        accounts::{require_owned_by, require_signer},
        data::DataReader,
        magicblock::{DelegateAccounts, ROOM_VALIDATOR, close_buffer, delegate_pda},
    },
};

/// # DelegateRoom (discriminator `1`) — Solana
///
/// Delegates a `Created` room to the TEE validator (`magicblock::ROOM_VALIDATOR`). Everything
/// after this (`OpenRoom` … `UndelegateRoom`) runs on the PER.
///
/// The validator is a compile-time constant, never a client value: a room on a non-TEE
/// validator would expose the guest and the message metadata.
///
/// Only `Created` rooms can be delegated: they hold nothing private yet (creator and room id
/// are public PDA seeds).
///
/// ## Accounts
/// 0. `[signer, writable]` creator — pays the buffer and delegation PDAs
/// 1. `[writable]` room
/// 2. `[]` owner_program — this program's executable account
/// 3. `[writable]` buffer — PDA `["buffer", room]` under this program
/// 4. `[writable]` delegation_record — `["delegation", room]` under the delegation program
/// 5. `[writable]` delegation_metadata — `["delegation-metadata", room]` under the delegation
///    program
/// 6. `[]` delegation_program — `DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh`
/// 7. `[]` system_program
///
/// ## Instruction data
/// None.
///
/// ## Errors
/// - `InvalidInstructionData`, `NotEnoughAccountKeys`, `MissingRequiredSignature`
/// - `InvalidAccountOwner`, `InvalidAccountData`: not a room (or already delegated)
/// - `IncorrectProgramId`: owner, delegation, or system program is wrong
/// - `InvalidSeeds`: buffer is not the canonical PDA
/// - `Unauthorized` (`0x0`): signer is not the creator
/// - `InvalidRoomStatus` (`0x1`): not `Created`
pub fn delegate_room(
    program_id: &Address,
    accounts: &mut [AccountView],
    data: &[u8],
) -> ProgramResult {
    DataReader::new(data).finish()?;
    let [
        creator,
        room,
        owner_program,
        buffer,
        delegation_record,
        delegation_metadata,
        delegation_program,
        system_program,
        ..,
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(creator)?;
    require_owned_by(room, program_id)?;

    let (creator_key, id_bytes, bump) = {
        let data = room.try_borrow()?;
        let r = Room::load(&data)?;
        if creator.address() != &r.creator {
            return Err(GhostError::Unauthorized.into());
        }
        if r.status != status::CREATED {
            return Err(GhostError::InvalidRoomStatus.into());
        }
        r.seed_parts()
    };

    delegate_pda(
        DelegateAccounts {
            payer: &mut *creator,
            pda: &mut *room,
            owner_program: &*owner_program,
            buffer: &mut *buffer,
            delegation_record: &*delegation_record,
            delegation_metadata: &*delegation_metadata,
            delegation_program: &*delegation_program,
            system_program: &*system_program,
        },
        program_id,
        &[ROOM_SEED, &creator_key, &id_bytes],
        &[
            Seed::from(ROOM_SEED),
            Seed::from(&creator_key),
            Seed::from(&id_bytes),
            Seed::from(&bump),
        ],
        &ROOM_VALIDATOR,
    )?;
    close_buffer(creator, buffer)
}
