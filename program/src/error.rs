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
    /// The session token is not a valid `gpl_session` token from a member to this signer for
    /// this program (wrong owner, size, discriminator, target program, signer, or address).
    InvalidSessionToken = 9,
    /// The session token has expired (`now >= valid_until`).
    SessionExpired = 10,
    /// `RefundPayment` before the deadline (`now <= deadline`).
    PaymentNotRefundable = 11,
    /// The payment-authority seeds and bump derive no program address.
    InvalidPaymentAuthority = 12,
    /// The `transact` account list does not target the Solana Privacy Program.
    InvalidPrivacyProgram = 13,
    /// The payment-authority PDA is not among the `transact` accounts.
    MissingPaymentAuthority = 14,
}

impl From<GhostError> for ProgramError {
    fn from(e: GhostError) -> Self {
        ProgramError::Custom(e as u32)
    }
}
