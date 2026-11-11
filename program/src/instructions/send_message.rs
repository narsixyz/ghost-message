use pinocchio::{AccountView, Address, ProgramResult, error::ProgramError};

use crate::{
    constants::{MAC_LEN, MAX_CIPHERTEXT, MESSAGE_SLOTS, kind, sender, status},
    error::GhostError,
    state::Room,
    utils::{
        accounts::{now, require_owned_by, require_signer},
        data::DataReader,
    },
};

/// # SendMessage (discriminator `4`) — PER
///
/// Writes an encrypted message into the next ring slot (`next_seq % 16`), overwriting the
/// oldest message once the ring is full. The program never sees plaintext: the client encrypts
/// with NaCl `box`.
///
/// ## Accounts
/// 0. `[signer]` creator or guest (phase 2: or that member's session key)
/// 1. `[writable]` room — status `Joined`, not expired
///
/// ## Instruction data
/// | Offset | Size | Field |
/// |---|---|---|
/// | 0 | 1 | kind (`0` text, `1` payment note, `2` shielded address) |
/// | 1 | 24 | nonce |
/// | 25 | 16..=220 | ciphertext (the rest of the data) |
///
/// ## Errors
/// - `InvalidInstructionData`, `NotEnoughAccountKeys`, `MissingRequiredSignature`
/// - `InvalidAccountOwner`, `InvalidAccountData`: not a room
/// - `Unauthorized` (`0x0`): signer is not a member
/// - `InvalidRoomStatus` (`0x1`): not `Joined`
/// - `RoomExpired` (`0x2`), `InvalidMessageKind` (`0x6`), `InvalidMessageLength` (`0x7`)
pub fn send_message(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let mut reader = DataReader::new(data);
    let message_kind = reader.u8()?;
    let nonce = reader.array()?;
    let ciphertext = reader.rest();
    if !matches!(
        message_kind,
        kind::TEXT | kind::PAYMENT_NOTE | kind::SHIELDED_ADDRESS
    ) {
        return Err(GhostError::InvalidMessageKind.into());
    }
    if !(MAC_LEN..=MAX_CIPHERTEXT).contains(&ciphertext.len()) {
        return Err(GhostError::InvalidMessageLength.into());
    }

    let [signer, room, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(signer)?;
    require_owned_by(room, program_id)?;

    let mut data = room.try_borrow_mut()?;
    let r = Room::load_mut(&mut data)?;
    if r.status != status::JOINED {
        return Err(GhostError::InvalidRoomStatus.into());
    }
    let from = if signer.address() == &r.creator {
        sender::CREATOR
    } else if signer.address() == &r.guest {
        sender::GUEST
    } else {
        return Err(GhostError::Unauthorized.into());
    };
    if r.is_expired(now()?) {
        return Err(GhostError::RoomExpired.into());
    }

    let seq = r.next_seq();
    let next = seq.checked_add(1).ok_or(GhostError::MathOverflow)?;
    r.messages[(seq % MESSAGE_SLOTS as u64) as usize].write(
        seq,
        from,
        message_kind,
        &nonce,
        ciphertext,
    );
    r.set_next_seq(next);
    Ok(())
}
