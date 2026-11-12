use pinocchio::{AccountView, Address, ProgramResult, error::ProgramError};

use crate::{
    constants::status,
    error::GhostError,
    state::Room,
    utils::{
        accounts::{now, require_owned_by, require_signer},
        data::DataReader,
        session::actor,
    },
};

/// # JoinRoom (discriminator `3`) — PER
///
/// The guest publishes its X25519 box key. The room becomes `Joined`.
///
/// ## Accounts
/// 0. `[signer]` guest, or the guest's session key
/// 1. `[writable]` room — status `Open`
/// 2. `[]` session_token — optional; required when account 0 is a session key
///
/// ## Instruction data
/// | Offset | Size | Field |
/// |---|---|---|
/// | 0 | 32 | guest_box_pk |
///
/// ## Errors
/// - `InvalidInstructionData`, `NotEnoughAccountKeys`, `MissingRequiredSignature`
/// - `InvalidAccountOwner`, `InvalidAccountData`: not a room
/// - `Unauthorized` (`0x0`): signer is not the guest
/// - `InvalidRoomStatus` (`0x1`): not `Open`
/// - `RoomExpired` (`0x2`), `InvalidBoxKey` (`0x5`)
/// - `InvalidSessionToken` (`0x9`), `SessionExpired` (`0xa`)
pub fn join_room(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let mut reader = DataReader::new(data);
    let guest_box_pk = reader.array::<32>()?;
    reader.finish()?;

    let [signer, room, rest @ ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(signer)?;
    require_owned_by(room, program_id)?;

    let mut data = room.try_borrow_mut()?;
    let r = Room::load_mut(&mut data)?;
    if r.status != status::OPEN {
        return Err(GhostError::InvalidRoomStatus.into());
    }
    let now = now()?;
    if actor(program_id, signer, rest.first(), now)? != r.guest {
        return Err(GhostError::Unauthorized.into());
    }
    if r.is_expired(now) {
        return Err(GhostError::RoomExpired.into());
    }
    if guest_box_pk == [0; 32] {
        return Err(GhostError::InvalidBoxKey.into());
    }

    r.guest_box_pk = guest_box_pk;
    r.status = status::JOINED;
    Ok(())
}
