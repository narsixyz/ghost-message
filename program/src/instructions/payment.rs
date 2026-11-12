use pinocchio::{
    AccountView, Address, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::{
    constants::PAYMENT_SEED,
    error::GhostError,
    utils::{
        accounts::{now, require_signer},
        data::DataReader,
        privacy::spp_transact_signed,
    },
};

/// The payment-authority PDA `["payment", payer, recipient, deadline i64 LE, bump]`.
///
/// It owns the private funds of one payer → recipient payment (several payments with the same
/// three values share it). The payer locks funds with an ordinary Privacy Rings transfer to
/// `ShieldedAddress.forPda(pda, …)`; after that only this program can spend them, because SPP
/// requires the owner's signature and only this program can sign for the PDA.
///
/// The client passes the bump (canonical, from `find_program_address`), so the derivation is
/// one `create_program_address` whatever the bump.
pub struct PaymentAuthority {
    pub address: Address,
    deadline: [u8; 8],
    bump: [u8; 1],
}

impl PaymentAuthority {
    /// ## Errors
    /// - `InvalidPaymentAuthority` (`0xc`): the seeds and bump derive no program address
    pub fn new(
        program_id: &Address,
        payer: &Address,
        recipient: &Address,
        deadline: i64,
        bump: u8,
    ) -> Result<Self, ProgramError> {
        let deadline = deadline.to_le_bytes();
        let bump = [bump];
        let address = Address::create_program_address(
            &[PAYMENT_SEED, payer.as_ref(), recipient.as_ref(), &deadline, &bump],
            program_id,
        )
        .map_err(|_| GhostError::InvalidPaymentAuthority)?;
        Ok(Self { address, deadline, bump })
    }

    /// Forwards SPP `transact`, signed by this PDA.
    fn transact(
        &self,
        payer: &Address,
        recipient: &Address,
        spp_accounts: &[AccountView],
        transact_data: &[u8],
    ) -> ProgramResult {
        let seeds = [
            Seed::from(PAYMENT_SEED),
            Seed::from(payer.as_ref()),
            Seed::from(recipient.as_ref()),
            Seed::from(&self.deadline),
            Seed::from(&self.bump),
        ];
        spp_transact_signed(spp_accounts, transact_data, &self.address, &Signer::from(&seeds))
    }
}

/// # ClaimPayment (discriminator `8`) — Solana
///
/// The recipient spends the private funds held by the payment-authority PDA
/// `["payment", payer, recipient, deadline]`. The program checks that the recipient signed,
/// then CPIs SPP `transact` and signs for the PDA. Allowed at any time until the funds are
/// gone, including after the deadline if the payer hasn't refunded yet.
///
/// The program does not read the private transaction: the recipient chooses the outputs (to
/// their own private balance, normally), and SPP checks the proof, the nullifiers and that the
/// spent UTXOs belong to the signing owners.
///
/// ## Accounts
/// 0. `[signer]` recipient
/// 1. onwards: the SPP `transact` accounts, in SPP's order, with the payment-authority PDA in the
///    owner-signer run (read-only, not signer; the program makes it a signer in the CPI)
///
/// ## Instruction data
/// | Offset | Size | Field |
/// |---|---|---|
/// | 0 | 32 | payer |
/// | 32 | 8 | deadline (`i64`, unix seconds) |
/// | 40 | 1 | bump of the payment-authority PDA |
/// | 41 | .. | SPP `transact` data, without its tag, forwarded unchanged |
///
/// ## Errors
/// - `InvalidInstructionData`, `NotEnoughAccountKeys`, `MissingRequiredSignature`
/// - `InvalidPaymentAuthority` (`0xc`), `InvalidPrivacyProgram` (`0xd`),
///   `MissingPaymentAuthority` (`0xe`)
/// - any SPP error
pub fn claim_payment(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let (payer, deadline, bump, transact_data) = read_args(data)?;
    let [recipient, spp_accounts @ ..] = &accounts[..] else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(recipient)?;
    let recipient = recipient.address();

    let authority = PaymentAuthority::new(program_id, &payer, recipient, deadline, bump)?;
    authority.transact(&payer, recipient, spp_accounts, transact_data)
}

/// # RefundPayment (discriminator `9`) — Solana
///
/// After the deadline, the payer takes back whatever the payment-authority PDA
/// `["payment", payer, recipient, deadline]` still holds. Same CPI as `ClaimPayment`; only the
/// signer and the time check differ.
///
/// ## Accounts
/// 0. `[signer]` payer
/// 1. onwards: the SPP `transact` accounts (see `ClaimPayment`)
///
/// ## Instruction data
/// | Offset | Size | Field |
/// |---|---|---|
/// | 0 | 32 | recipient |
/// | 32 | 8 | deadline (`i64`, unix seconds) |
/// | 40 | 1 | bump of the payment-authority PDA |
/// | 41 | .. | SPP `transact` data, without its tag, forwarded unchanged |
///
/// ## Errors
/// - as `ClaimPayment`, plus `PaymentNotRefundable` (`0xb`): `now <= deadline`
pub fn refund_payment(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let (recipient, deadline, bump, transact_data) = read_args(data)?;
    let [payer, spp_accounts @ ..] = &accounts[..] else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(payer)?;
    if now()? <= deadline {
        return Err(GhostError::PaymentNotRefundable.into());
    }
    let payer = payer.address();

    let authority = PaymentAuthority::new(program_id, payer, &recipient, deadline, bump)?;
    authority.transact(payer, &recipient, spp_accounts, transact_data)
}

/// `counterparty[32] | deadline i64 | bump u8 | transact data (non-empty)`.
fn read_args(data: &[u8]) -> Result<(Address, i64, u8, &[u8]), ProgramError> {
    let mut reader = DataReader::new(data);
    let counterparty = reader.address()?;
    let deadline = reader.i64()?;
    let bump = reader.u8()?;
    let transact_data = reader.rest();
    if transact_data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    Ok((counterparty, deadline, bump, transact_data))
}
