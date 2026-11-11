use pinocchio::{AccountView, Address, ProgramResult, error::ProgramError};

use crate::{
    constants::status,
    error::GhostError,
    state::Room,
    utils::{
        accounts::{now, require_owned_by, require_signer},
        data::DataReader,
    },
};

/// # CloseRoom (discriminator `5`) — PER
///
/// Zeroes the guest, both box keys, the expiry, the message count, and every message slot, so
/// nothing private reaches Solana on undelegation. The room becomes `Closed` (terminal).
///
/// Who may close:
/// - the creator, in any status except `Closed`;
/// - the guest, once the room is `Open` or `Joined`;
/// - anyone, once the room has expired.
///
/// ## Accounts
/// 0. `[signer]` closer (phase 2: or a member's session key)
/// 1. `[writable]` room — not `Closed`
///
/// ## Instruction data
/// None.
///
/// ## Errors
/// - `InvalidInstructionData`, `NotEnoughAccountKeys`, `MissingRequiredSignature`
/// - `InvalidAccountOwner`, `InvalidAccountData`: not a room
/// - `Unauthorized` (`0x0`): signer is not a member and the room has not expired
/// - `InvalidRoomStatus` (`0x1`): already `Closed`
pub fn close_room(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    DataReader::new(data).finish()?;

    let [signer, room, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(signer)?;
    require_owned_by(room, program_id)?;

    let mut data = room.try_borrow_mut()?;
    let r = Room::load_mut(&mut data)?;
    if r.status == status::CLOSED {
        return Err(GhostError::InvalidRoomStatus.into());
    }
    // The guest is zero while `Created`, and the zero address can't sign.
    let is_member = signer.address() == &r.creator || signer.address() == &r.guest;
    if !is_member && !r.is_expired(now()?) {
        return Err(GhostError::Unauthorized.into());
    }

    r.wipe_private();
    r.status = status::CLOSED;
    Ok(())
}
