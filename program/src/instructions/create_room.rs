use pinocchio::{
    AccountView, Address, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::{
    constants::{ROOM_PERMISSION_PREFUND, ROOM_SEED, status},
    state::Room,
    utils::{
        accounts::{create_pda_account, require_program, require_signer},
        data::DataReader,
    },
};

/// # CreateRoom (discriminator `0`) — Solana
///
/// Creates an empty room in `Created` status. No guest is stored: Solana only learns that the
/// creator made a room. The room gets extra lamports to pay its `EphemeralPermission` rent on
/// the PER.
///
/// ## Accounts
/// 0. `[signer, writable]` creator — pays rent
/// 1. `[writable]` room — PDA `["room", creator, room_id u64 LE]`, must not exist
/// 2. `[]` system_program
///
/// ## Instruction data
/// | Offset | Size | Field |
/// |---|---|---|
/// | 0 | 8 | room_id (`u64`) |
///
/// ## Errors
/// - `InvalidInstructionData`: data is not exactly 8 bytes
/// - `NotEnoughAccountKeys`, `MissingRequiredSignature`, `IncorrectProgramId`
/// - `AccountAlreadyInitialized`: room already exists
/// - `InvalidSeeds`: room is not the canonical PDA
pub fn create_room(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let mut reader = DataReader::new(data);
    let room_id = reader.u64()?;
    reader.finish()?;

    let [creator, room, system_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(creator)?;
    require_program(system_program, &pinocchio_system::ID)?;
    if room.owned_by(program_id) {
        return Err(ProgramError::AccountAlreadyInitialized);
    }

    let id_bytes = room_id.to_le_bytes();
    let (room_address, bump) = Address::find_program_address(
        &[ROOM_SEED, creator.address().as_ref(), &id_bytes],
        program_id,
    );
    if room.address() != &room_address {
        return Err(ProgramError::InvalidSeeds);
    }

    let bump_seed = [bump];
    let seeds = [
        Seed::from(ROOM_SEED),
        Seed::from(creator.address().as_ref()),
        Seed::from(&id_bytes),
        Seed::from(&bump_seed),
    ];
    create_pda_account(
        creator,
        room,
        Room::LEN,
        ROOM_PERMISSION_PREFUND,
        program_id,
        &Signer::from(&seeds),
    )?;

    let mut data = room.try_borrow_mut()?;
    let r = Room::init(&mut data)?;
    r.bump = bump;
    r.status = status::CREATED;
    r.creator = Address::new_from_array(creator.address().to_bytes());
    Ok(())
}
