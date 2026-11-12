//! Helius Privacy Rings: the Solana Privacy Program (SPP) `transact` CPI, hand-encoded.
//!
//! No Zolana crates: they need pinocchio 0.11 + `alloc`. The CPI is simple enough to write by
//! hand, because this program never builds or reads the private transaction. The client builds
//! the SPP `transact` instruction data (proof, nullifiers, output UTXO hashes, ciphertexts) and
//! its account list; the program checks who may authorize it, then forwards both unchanged and
//! signs for its own PDA.
//!
//! Why a PDA can own private funds: SPP derives a UTXO owner's proof input "from the verified
//! signer account — an Ed25519 signer or a PDA signing via `invoke_signed`" (Zolana spec,
//! *Owner Hash*). A UTXO whose owner hash is built from this program's PDA can therefore only be
//! spent in a `transact` that this program signs.
//!
//! ## `transact` accounts (SPP layout, forwarded verbatim)
//! `payer (s, w) | output_tree (w) | SPP | system_program | input_trees… (w) |
//!  nullifier_pdas… (w) | owner_signers… (s) | public-leg groups…`
//!
//! The PDA appears in the owner-signer run as a read-only non-signer; the CPI marks it a signer.

use pinocchio::{
    AccountView, Address, ProgramResult,
    cpi::{Signer, invoke_signed_with_bounds},
    error::ProgramError,
    instruction::{InstructionAccount, InstructionView},
};

use crate::error::GhostError;

/// `ring1GaLCuJnkUMVkX2JeZLTTX17wKsV4fZmPVzXANW`, the Solana Privacy Program (devnet).
pub const SPP_PROGRAM_ID: Address = Address::new_from_array([
    12, 188, 181, 120, 174, 97, 37, 129, 50, 238, 175, 106, 193, 33, 192, 164, 43, 136, 162, 232,
    79, 177, 30, 126, 45, 91, 160, 175, 209, 39, 49, 99,
]);

/// SPP `transact` instruction tag. `@heliuslabs/spr` 0.9.0-alpha `InstructionTag.transact`,
/// and the first byte of a real devnet private transfer
/// (`XaE4qBHq…6Jkc`). The Zolana spec at `ca4cf4f` says 12; the deployed program uses 16.
pub const SPP_TRANSACT_TAG: u8 = 16;

/// Index of the SPP program account in the `transact` account list.
const SPP_PROGRAM_INDEX: usize = 2;

/// Most accounts a forwarded `transact` may have. A 2-in transfer with a SOL leg is ~11.
pub const MAX_TRANSACT_ACCOUNTS: usize = 24;

/// Largest `transact` instruction data the program forwards (tag included). A v1 transaction is
/// at most 1,232 bytes, so the data always fits.
pub const MAX_TRANSACT_DATA: usize = 1_232;

/// Invokes SPP `transact` with the client-built `transact_data` (without the tag) and
/// `accounts`, signing for `pda` with `pda_seeds`.
///
/// Checks that account 2 is the SPP program and that `pda` is among the accounts. Everything
/// else (layout, proof, nullifiers, the owner signers matching the spent inputs) is SPP's to
/// check.
///
/// ## Errors
/// - `InvalidPrivacyProgram` (`0xd`): account 2 is not the SPP
/// - `MissingPaymentAuthority` (`0xe`): `pda` is not in `accounts`
/// - `NotEnoughAccountKeys`: fewer than 3 accounts
/// - `InvalidInstructionData`: no data, or more than `MAX_TRANSACT_DATA` bytes
/// - `InvalidArgument`: more than `MAX_TRANSACT_ACCOUNTS` accounts
pub fn spp_transact_signed(
    accounts: &[AccountView],
    transact_data: &[u8],
    pda: &Address,
    pda_seeds: &Signer,
) -> ProgramResult {
    let spp = accounts
        .get(SPP_PROGRAM_INDEX)
        .ok_or(ProgramError::NotEnoughAccountKeys)?;
    if spp.address() != &SPP_PROGRAM_ID {
        return Err(GhostError::InvalidPrivacyProgram.into());
    }
    if !accounts.iter().any(|a| a.address() == pda) {
        return Err(GhostError::MissingPaymentAuthority.into());
    }
    if accounts.len() > MAX_TRANSACT_ACCOUNTS {
        return Err(ProgramError::InvalidArgument);
    }
    if transact_data.is_empty() || transact_data.len() + 1 > MAX_TRANSACT_DATA {
        return Err(ProgramError::InvalidInstructionData);
    }

    let mut data = [0u8; MAX_TRANSACT_DATA];
    data[0] = SPP_TRANSACT_TAG;
    data[1..=transact_data.len()].copy_from_slice(transact_data);

    // Same flags as received, plus signer on the PDA.
    let metas: [InstructionAccount; MAX_TRANSACT_ACCOUNTS] = core::array::from_fn(|i| match accounts.get(i) {
        Some(account) => {
            let address = account.address();
            InstructionAccount::new(address, account.is_writable(), account.is_signer() || address == pda)
        }
        None => InstructionAccount::readonly(pda), // unused tail
    });

    invoke_signed_with_bounds::<MAX_TRANSACT_ACCOUNTS, _>(
        &InstructionView {
            program_id: &SPP_PROGRAM_ID,
            accounts: &metas[..accounts.len()],
            data: &data[..=transact_data.len()],
        },
        accounts,
        core::slice::from_ref(pda_seeds),
    )
}
