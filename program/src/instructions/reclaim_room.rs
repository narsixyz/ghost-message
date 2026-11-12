use pinocchio::{AccountView, Address, ProgramResult, error::ProgramError};

use crate::{
    constants::status,
    error::GhostError,
    state::Room,
    utils::{
        accounts::{close_account, require_owned_by},
        data::DataReader,
    },
};

/// # ReclaimRoom (discriminator `7`) — Solana
///
/// Closes a `Closed` room that is back on Solana (owned by this program again, after
/// `UndelegateRoom` + `ProcessUndelegation`, or a room that was never delegated) and returns
/// all its lamports to the creator. Permissionless: the lamports can only go to the creator.
///
/// A delegated room is owned by the delegation program on Solana, so this fails there with
/// `InvalidAccountOwner` until undelegation completes.
///
/// ## Accounts
/// 0. `[writable]` creator — `Room::creator`; receives the lamports
/// 1. `[writable]` room — `Closed`
///
/// ## Instruction data
/// None.
///
/// ## Errors
/// - `InvalidInstructionData`, `NotEnoughAccountKeys`
/// - `InvalidAccountOwner`, `InvalidAccountData`: not a room, or still delegated
/// - `Unauthorized` (`0x0`): account 0 is not the creator
/// - `InvalidRoomStatus` (`0x1`): not `Closed`
pub fn reclaim_room(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    DataReader::new(data).finish()?;
    let [creator, room, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_owned_by(room, program_id)?;
    {
        let data = room.try_borrow()?;
        let r = Room::load(&data)?;
        if creator.address() != &r.creator {
            return Err(GhostError::Unauthorized.into());
        }
        if r.status != status::CLOSED {
            return Err(GhostError::InvalidRoomStatus.into());
        }
    }
    close_account(room, creator)
}
