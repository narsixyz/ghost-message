use pinocchio::{AccountView, Address, ProgramResult, cpi::Seed, error::ProgramError};

use crate::{
    constants::{ROOM_SEED, status},
    error::GhostError,
    state::Room,
    utils::{
        accounts::{now, require_owned_by, require_signer},
        data::DataReader,
        magicblock::{PermissionAccounts, create_ephemeral_permission, member_flags},
    },
};

/// Flags both members get on the room's permission: read the room, and see its transactions'
/// logs, balances, messages and signatures. No `AUTHORITY`: only the program (through the room
/// PDA) can change or close the permission.
pub const MEMBER_FLAGS: u8 = member_flags::TX_LOGS
    | member_flags::TX_BALANCES
    | member_flags::TX_MESSAGE
    | member_flags::ACCOUNT_SIGNATURES;

/// # OpenRoom (discriminator `2`) — PER
///
/// Sets the guest, the creator's X25519 box key and the expiry, and in the **same instruction**
/// creates the room's private `EphemeralPermission` with members `[creator, guest]`. The guest is
/// therefore never readable by non-members.
///
/// Only works on the PER: the permission CPI needs the Magic program and the ephemeral vault,
/// which don't exist on Solana. So the guest can't be written to a room on Solana.
///
/// The room PDA pays the permission rent from the lamports `CreateRoom` pre-funded.
///
/// ## Accounts
/// 0. `[signer]` creator (wallet only; session keys can't open rooms)
/// 1. `[writable]` room — status `Created`, delegated
/// 2. `[writable]` permission — `["permission:", room]` under the permission program
/// 3. `[writable]` ephemeral_vault — `MagicVau1t999999999999999999999999999999999`
/// 4. `[]` magic_program — `Magic11111111111111111111111111111111111111`
/// 5. `[]` permission_program — `ACLseoPoyC3cBqoUtkbjZ4aDrkurZW86v19pXz2XQnp1`
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
/// - `IncorrectProgramId`: vault, Magic program or permission program is wrong
/// - `InvalidSeeds`: permission is not the room's permission PDA
/// - `Unauthorized` (`0x0`): signer is not the creator
/// - `InvalidRoomStatus` (`0x1`): not `Created`
/// - `InvalidExpiry` (`0x3`), `InvalidGuest` (`0x4`), `InvalidBoxKey` (`0x5`)
pub fn open_room(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let mut reader = DataReader::new(data);
    let guest = reader.address()?;
    let creator_box_pk = reader.array::<32>()?;
    let expires_at = reader.i64()?;
    reader.finish()?;

    let [
        signer,
        room,
        permission,
        vault,
        magic_program,
        permission_program,
        ..,
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(signer)?;
    require_owned_by(room, program_id)?;

    let (creator_key, id_bytes, bump) = {
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

        r.guest = Address::new_from_array(guest.to_bytes());
        r.creator_box_pk = creator_box_pk;
        r.set_expires_at(expires_at);
        r.status = status::OPEN;
        r.seed_parts()
    };

    let creator = Address::new_from_array(creator_key);
    create_ephemeral_permission(
        room,
        &PermissionAccounts {
            permission,
            vault,
            magic_program,
            permission_program,
        },
        true,
        &[(&creator, MEMBER_FLAGS), (&guest, MEMBER_FLAGS)],
        &[
            Seed::from(ROOM_SEED),
            Seed::from(&creator_key),
            Seed::from(&id_bytes),
            Seed::from(&bump),
        ],
    )
}
