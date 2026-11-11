//! Program error codes, returned as `ProgramError::Custom(code)`. Append only; never renumber.

use pinocchio::error::ProgramError;

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhostError {
    /// The signer is not allowed to do this (not the creator, not the guest, not a member).
    Unauthorized = 0,
    /// The room is not in a status that allows this instruction.
    InvalidRoomStatus = 1,
    /// The room has expired (`now >= expires_at`).
    RoomExpired = 2,
    /// `expires_at` is not in the future.
    InvalidExpiry = 3,
    /// The guest is the zero address or the creator.
    InvalidGuest = 4,
    /// A box public key is all zeros.
    InvalidBoxKey = 5,
    /// Message kind is not text, payment note, or shielded address.
    InvalidMessageKind = 6,
    /// Ciphertext is shorter than the MAC or longer than a slot.
    InvalidMessageLength = 7,
    /// Arithmetic overflowed.
    MathOverflow = 8,
}

impl From<GhostError> for ProgramError {
    fn from(e: GhostError) -> Self {
        ProgramError::Custom(e as u32)
    }
}
