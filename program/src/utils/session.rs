//! MagicBlock session keys (`gpl_session`), read as raw bytes. No `session-keys` crate: it is
//! Anchor.
//!
//! A session token is a PDA owned by the session program
//! (`KeyspM2ssCJbqUhQ4k7sveSiY4WjnYsrXkC8oDbwde5`). Both versions start with an Anchor account
//! discriminator (`sha256("account:<Name>")[..8]`) and store the PDA's own seeds.
//!
//! | Version | Seeds | Size | Layout |
//! |---|---|---|---|
//! | V1 | `["session_token", target_program, session_signer, authority]` | 112 | `disc[8] \| authority[32] \| target_program[32] \| session_signer[32] \| valid_until i64` |
//! | V2 | `["session_token_v2", target_program, session_signer, authority]` | 144 | `disc[8] \| authority[32] \| target_program[32] \| session_signer[32] \| fee_payer[32] \| valid_until i64` |
//!
//! The session program creates a token only when both the authority (wallet) and the session
//! signer sign, and it writes exactly the keys it derived the PDA from. So a token that is owned
//! by the session program, sits at the PDA of its own fields, targets this program, names this
//! signer, and has not expired proves that `authority` delegated to `signer`.

use pinocchio::{AccountView, Address, error::ProgramError};

use crate::error::GhostError;

/// `KeyspM2ssCJbqUhQ4k7sveSiY4WjnYsrXkC8oDbwde5`
pub const SESSION_PROGRAM_ID: Address = Address::new_from_array([
    4, 199, 102, 62, 122, 205, 171, 45, 147, 4, 60, 163, 78, 136, 108, 183, 60, 169, 145, 142,
    174, 255, 137, 190, 131, 177, 160, 119, 240, 149, 183, 134,
]);

/// `sha256("account:SessionToken")[..8]`
pub const SESSION_TOKEN_DISCRIMINATOR: [u8; 8] = [233, 4, 115, 14, 46, 21, 1, 15];
/// `sha256("account:SessionTokenV2")[..8]`
pub const SESSION_TOKEN_V2_DISCRIMINATOR: [u8; 8] = [178, 3, 85, 254, 13, 116, 128, 41];

pub const SESSION_TOKEN_SEED: &[u8] = b"session_token";
pub const SESSION_TOKEN_V2_SEED: &[u8] = b"session_token_v2";

pub const SESSION_TOKEN_LEN: usize = 112;
pub const SESSION_TOKEN_V2_LEN: usize = 144;

/// Resolves who is acting.
///
/// - No token: the signer acts for itself.
/// - Token: it must be a valid, unexpired session token from `authority` to this `signer` for
///   this program. The authority (the wallet) is the actor.
///
/// A token that is passed but invalid is an error. There is no silent fallback to the signer.
/// The caller has already checked that `signer` signed.
///
/// ## Errors
/// - `InvalidSessionToken` (`0x9`): wrong owner, size, discriminator, target program, session
///   signer, or address
/// - `SessionExpired` (`0xa`): `now >= valid_until`
pub fn actor(
    program_id: &Address,
    signer: &AccountView,
    token: Option<&AccountView>,
    now: i64,
) -> Result<Address, ProgramError> {
    let Some(token) = token else {
        return Ok(Address::new_from_array(signer.address().to_bytes()));
    };
    if !token.owned_by(&SESSION_PROGRAM_ID) {
        return Err(GhostError::InvalidSessionToken.into());
    }
    let data = token.try_borrow()?;
    let (seed, valid_until_offset) = match data.len() {
        SESSION_TOKEN_LEN if data[..8] == SESSION_TOKEN_DISCRIMINATOR => (SESSION_TOKEN_SEED, 104),
        SESSION_TOKEN_V2_LEN if data[..8] == SESSION_TOKEN_V2_DISCRIMINATOR => {
            (SESSION_TOKEN_V2_SEED, 136)
        }
        _ => return Err(GhostError::InvalidSessionToken.into()),
    };
    let authority = &data[8..40];
    let target_program = &data[40..72];
    let session_signer = &data[72..104];
    if target_program != program_id.as_ref() || session_signer != signer.address().as_ref() {
        return Err(GhostError::InvalidSessionToken.into());
    }
    let (expected, _) = Address::find_program_address(
        &[seed, target_program, session_signer, authority],
        &SESSION_PROGRAM_ID,
    );
    if token.address() != &expected {
        return Err(GhostError::InvalidSessionToken.into());
    }
    let mut valid_until = [0u8; 8];
    valid_until.copy_from_slice(&data[valid_until_offset..valid_until_offset + 8]);
    if now >= i64::from_le_bytes(valid_until) {
        return Err(GhostError::SessionExpired.into());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(authority);
    Ok(Address::new_from_array(out))
}
