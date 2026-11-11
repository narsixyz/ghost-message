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

/// # OpenRoom (discriminator `2`) — PER
///
/// Sets the guest, the creator's X25519 box key, and the expiry. Phase 3 adds the
/// `CreateEphemeralPermission(private, [creator, guest])` CPI to this same instruction, so the
/// guest is never visible to non-members.
///
/// ## Accounts
/// 0. `[signer]` creator
/// 1. `[writable]` room — status `Created`
///
/// ## Instruction data
/// | Offset | Size | Field |
/// |---|---|---|
/// | 0 | 32 | guest |
/// | 32 | 32 | creator_box_pk |
/// | 64 | 8 | expires_at (`i64`, > now) |
///
/// ## Errors
/// - `InvalidInstructionData`, `NotEnoughAccountKeys`, `MissingRequiredSignature`
/// - `InvalidAccountOwner`, `InvalidAccountData`: not a room
/// - `Unauthorized` (`0x0`): signer is not the creator
/// - `InvalidRoomStatus` (`0x1`): not `Created`
/// - `InvalidExpiry` (`0x3`), `InvalidGuest` (`0x4`), `InvalidBoxKey` (`0x5`)
pub fn open_room(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let mut reader = DataReader::new(data);
    let guest = reader.address()?;
    let creator_box_pk = reader.array::<32>()?;
    let expires_at = reader.i64()?;
    reader.finish()?;

    let [signer, room, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(signer)?;
    require_owned_by(room, program_id)?;

    let mut data = room.try_borrow_mut()?;
    let r = Room::load_mut(&mut data)?;
    if signer.address() != &r.creator {
        return Err(GhostError::Unauthorized.into());
    }
    if r.status != status::CREATED {
        return Err(GhostError::InvalidRoomStatus.into());
    }
    if guest.as_ref() == [0; 32] || guest == r.creator {
        return Err(GhostError::InvalidGuest.into());
    }
    if creator_box_pk == [0; 32] {
        return Err(GhostError::InvalidBoxKey.into());
    }
    if expires_at <= now()? {
        return Err(GhostError::InvalidExpiry.into());
    }

    r.guest = guest;
    r.creator_box_pk = creator_box_pk;
    r.set_expires_at(expires_at);
    r.status = status::OPEN;
    Ok(())
}
