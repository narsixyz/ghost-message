//! Account checks, PDA creation, and the clock.

use pinocchio::{
    AccountView, Address, ProgramResult,
    cpi::Signer,
    error::ProgramError,
    sysvars::{Sysvar, clock::Clock, rent::Rent},
};
use pinocchio_system::instructions::{Allocate, Assign, CreateAccount, Transfer};

pub fn require_signer(account: &AccountView) -> ProgramResult {
    if account.is_signer() {
        Ok(())
    } else {
        Err(ProgramError::MissingRequiredSignature)
    }
}

pub fn require_program(account: &AccountView, id: &Address) -> ProgramResult {
    if account.address() == id {
        Ok(())
    } else {
        Err(ProgramError::IncorrectProgramId)
    }
}

pub fn require_owned_by(account: &AccountView, owner: &Address) -> ProgramResult {
    if account.owned_by(owner) {
        Ok(())
    } else {
        Err(ProgramError::InvalidAccountOwner)
    }
}

/// Current unix timestamp from the Clock sysvar (no account needed).
pub fn now() -> Result<i64, ProgramError> {
    Ok(Clock::get()?.unix_timestamp)
}

/// Creates a program-owned PDA with `space` bytes and `rent-exempt + extra_lamports` lamports.
///
/// A plain `CreateAccount` fails if anyone sent lamports to the address beforehand, which would
/// let a griefer block creation. A prefunded address is topped up, allocated, and assigned
/// instead. `signer` holds the PDA's seeds (including the canonical bump).
pub fn create_pda_account(
    payer: &AccountView,
    account: &AccountView,
    space: usize,
    extra_lamports: u64,
    owner: &Address,
    signer: &Signer,
) -> ProgramResult {
    let lamports = Rent::get()?
        .try_minimum_balance(space)?
        .checked_add(extra_lamports)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    let signers = core::slice::from_ref(signer);

    if account.lamports() == 0 {
        return CreateAccount {
            from: payer,
            to: account,
            lamports,
            space: space as u64,
            owner,
        }
        .invoke_signed(signers);
    }

    let missing = lamports.saturating_sub(account.lamports());
    if missing > 0 {
        Transfer {
            from: payer,
            to: account,
            lamports: missing,
        }
        .invoke()?;
    }
    Allocate {
        account,
        space: space as u64,
    }
    .invoke_signed(signers)?;
    Assign { account, owner }.invoke_signed(signers)
}

/// Moves all lamports from a program-owned `account` to `recipient` and closes it.
pub fn close_account(account: &mut AccountView, recipient: &mut AccountView) -> ProgramResult {
    let lamports = recipient
        .lamports()
        .checked_add(account.lamports())
        .ok_or(ProgramError::ArithmeticOverflow)?;
    recipient.set_lamports(lamports);
    account.set_lamports(0);
    account.close()
}
