use pinocchio::{AccountView, Address, ProgramResult, cpi::Seed, error::ProgramError};

use crate::{
    constants::{ROOM_SEED, status},
    error::GhostError,
    state::Room,
    utils::{
        accounts::{now, require_owned_by, require_signer},
        data::DataReader,
        magicblock::{PermissionAccounts, close_ephemeral_permission},
        session::actor,
    },
};

/// # CloseRoom (discriminator `5`) — PER (or Solana for a room that was never opened)
///
/// Zeroes the guest, both box keys, the expiry, the message count, and every message slot, so
/// nothing private reaches Solana on undelegation. The room becomes `Closed` (terminal).
///
/// If the room was opened (`Open` or `Joined`), its `EphemeralPermission` is closed in the same
/// instruction: the member list (creator + guest) must not outlive the room, and the
/// permission's rent goes back to the room. A `Created` room has no permission, so no CPI runs
/// and it can be closed on Solana too (a room that was never delegated).
///
/// Who may close:
/// - the creator, in any status except `Closed`;
/// - the guest, once the room is `Open` or `Joined`;
/// - anyone, once the room has expired.
///
/// ## Accounts
/// 0. `[signer]` closer, or a member's session key
/// 1. `[writable]` room — not `Closed`
/// 2. `[writable]` permission — `["permission:", room]` under the permission program
/// 3. `[writable]` ephemeral_vault — `MagicVau1t999999999999999999999999999999999`
/// 4. `[]` magic_program
/// 5. `[]` permission_program
/// 6. `[]` session_token — optional; required when account 0 is a session key
///
/// Accounts 2–5 are only checked and used when the room was opened.
///
/// ## Instruction data
/// None.
///
/// ## Errors
/// - `InvalidInstructionData`, `NotEnoughAccountKeys`, `MissingRequiredSignature`
/// - `InvalidAccountOwner`, `InvalidAccountData`: not a room
/// - `IncorrectProgramId`, `InvalidSeeds`: wrong permission accounts (opened rooms only)
/// - `Unauthorized` (`0x0`): signer is not a member and the room has not expired
/// - `InvalidRoomStatus` (`0x1`): already `Closed`
/// - `InvalidSessionToken` (`0x9`), `SessionExpired` (`0xa`)
pub fn close_room(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    DataReader::new(data).finish()?;

    let [
        signer,
        room,
        permission,
        vault,
        magic_program,
        permission_program,
        rest @ ..,
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(signer)?;
    require_owned_by(room, program_id)?;

    let (had_permission, (creator_key, id_bytes, bump)) = {
        let mut data = room.try_borrow_mut()?;
        let r = Room::load_mut(&mut data)?;
        if r.status == status::CLOSED {
            return Err(GhostError::InvalidRoomStatus.into());
        }
        let now = now()?;
        let actor = actor(program_id, signer, rest.first(), now)?;
        // The guest is zero while `Created`, and no token or signer resolves to the zero address.
        let is_member = actor == r.creator || actor == r.guest;
        if !is_member && !r.is_expired(now) {
            return Err(GhostError::Unauthorized.into());
        }

        let had_permission = r.status != status::CREATED;
        r.wipe_private();
        r.status = status::CLOSED;
        (had_permission, r.seed_parts())
    };

    if had_permission {
        close_ephemeral_permission(
            room,
            &PermissionAccounts {
                permission,
                vault,
                magic_program,
                permission_program,
            },
            &[
                Seed::from(ROOM_SEED),
                Seed::from(&creator_key),
                Seed::from(&id_bytes),
                Seed::from(&bump),
            ],
        )?;
    }
    Ok(())
}
