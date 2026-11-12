use pinocchio::{AccountView, Address, ProgramResult, error::ProgramError};

use crate::{
    constants::status,
    error::GhostError,
    state::Room,
    utils::{
        accounts::{require_owned_by, require_signer},
        data::DataReader,
        magicblock::schedule_commit,
    },
};

/// # UndelegateRoom (discriminator `6`) — PER
///
/// Schedules commit-and-undelegate of a `Closed` room. Permissionless: a closed room holds only
/// its public seeds (creator, room id) and status, so anyone may send it back to Solana.
///
/// Asynchronous: success means scheduled. The delegation program later calls
/// `ProcessUndelegation` on Solana to re-create the room there.
///
/// ## Accounts
/// 0. `[signer, writable]` payer — any wallet
/// 1. `[writable]` room — `Closed`, delegated
/// 2. `[writable]` magic_context — `MagicContext1111111111111111111111111111111`
/// 3. `[]` magic_program — `Magic11111111111111111111111111111111111111`
///
/// ## Instruction data
/// None.
///
/// ## Errors
/// - `InvalidInstructionData`, `NotEnoughAccountKeys`, `MissingRequiredSignature`
/// - `InvalidAccountOwner`, `InvalidAccountData`: not a room
/// - `IncorrectProgramId`: Magic context or program is wrong
/// - `InvalidRoomStatus` (`0x1`): not `Closed`
pub fn undelegate_room(
    program_id: &Address,
    accounts: &mut [AccountView],
    data: &[u8],
) -> ProgramResult {
    DataReader::new(data).finish()?;
    let [payer, room, magic_context, magic_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(payer)?;
    require_owned_by(room, program_id)?;
    {
        let data = room.try_borrow()?;
        if Room::load(&data)?.status != status::CLOSED {
            return Err(GhostError::InvalidRoomStatus.into());
        }
    }
    schedule_commit(payer, magic_context, magic_program, room, true)
}
