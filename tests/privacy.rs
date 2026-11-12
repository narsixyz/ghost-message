//! `ClaimPayment` (8) and `RefundPayment` (9): the program releases private (Helius Privacy
//! Rings) funds owned by its payment-authority PDA by CPI-ing SPP `transact`, signed with
//! `invoke_signed`.
//!
//! LiteSVM has no SPP with a tree, prover and indexer, so the CPI recorder stands at the SPP
//! address. The tests assert exactly what the program sends (tag, data, accounts, flags, the PDA
//! signature) and every authorization rule. Whether SPP then accepts the spend is the devnet e2e's
//! job.

mod common;

use common::*;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::InstructionError;

const DEADLINE: i64 = T0 + 600;

// Error codes (program/src/error.rs).
const PAYMENT_NOT_REFUNDABLE: u32 = 11;
const INVALID_PAYMENT_AUTHORITY: u32 = 12;
const INVALID_PRIVACY_PROGRAM: u32 = 13;
const MISSING_PAYMENT_AUTHORITY: u32 = 14;

fn payment_pda(ctx: &Ctx, payer: &Address, recipient: &Address, deadline: i64) -> (Address, u8) {
    Address::find_program_address(
        &[b"payment", payer.as_ref(), recipient.as_ref(), &deadline.to_le_bytes()],
        &ctx.program_id,
    )
}

/// Stand-in SPP `transact` data. The program never parses it, only forwards it.
fn transact_data() -> Vec<u8> {
    (0..614u32).map(|i| (i * 7 + 3) as u8).collect()
}

/// A 1-in transfer's SPP account list, as the SDK lays it out: `payer, output_tree, SPP,
/// system_program, input_tree, nullifier_pda, owner signer (the PDA)`.
struct SppAccounts {
    metas: Vec<AccountMeta>,
}

impl SppAccounts {
    fn new(fee_payer: &Address, owner: &Address) -> Self {
        let tree = Address::new_unique();
        Self {
            metas: vec![
                AccountMeta::new(*fee_payer, true),
                AccountMeta::new(tree, false),
                AccountMeta::new_readonly(SPP_PROGRAM, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
                AccountMeta::new(tree, false),
                AccountMeta::new(Address::new_unique(), false),
                AccountMeta::new_readonly(*owner, false),
            ],
        }
    }
}

#[allow(clippy::too_many_arguments)] // mirrors the instruction's fields one to one
fn payment_ix(
    ctx: &Ctx,
    discriminator: u8,
    signer: &Address,
    counterparty: &Address,
    deadline: i64,
    bump: u8,
    spp: &[AccountMeta],
    transact: &[u8],
) -> Instruction {
    let mut data = vec![discriminator];
    data.extend_from_slice(counterparty.as_ref());
    data.extend_from_slice(&deadline.to_le_bytes());
    data.push(bump);
    data.extend_from_slice(transact);
    let mut accounts = vec![AccountMeta::new_readonly(*signer, true)];
    accounts.extend_from_slice(spp);
    Instruction::new_with_bytes(ctx.program_id, &data, accounts)
}

/// `ClaimPayment` signed by `recipient`, who also pays the fee.
fn claim(ctx: &mut Ctx, recipient: &Keypair, payer: &Address, deadline: i64) -> Result<(u64, Vec<Recorded>), solana_transaction::TransactionError> {
    let (pda, bump) = payment_pda(ctx, payer, &recipient.pubkey(), deadline);
    let spp = SppAccounts::new(&recipient.pubkey(), &pda);
    let ix = payment_ix(ctx, 8, &recipient.pubkey(), payer, deadline, bump, &spp.metas, &transact_data());
    send_logs(&mut ctx.svm, ix, &[recipient]).map(|(cu, logs)| (cu, recorded(&logs)))
}

/// `RefundPayment` signed by `payer`, who also pays the fee.
fn refund(ctx: &mut Ctx, payer: &Keypair, recipient: &Address, deadline: i64) -> Result<Vec<Recorded>, solana_transaction::TransactionError> {
    let (pda, bump) = payment_pda(ctx, &payer.pubkey(), recipient, deadline);
    let spp = SppAccounts::new(&payer.pubkey(), &pda);
    let ix = payment_ix(ctx, 9, &payer.pubkey(), recipient, deadline, bump, &spp.metas, &transact_data());
    send_logs(&mut ctx.svm, ix, &[payer]).map(|(_, logs)| recorded(&logs))
}

/// The one CPI went to SPP with `[16] ++ transact`, the forwarded accounts in order, the same
/// flags, and the PDA (only the PDA) promoted to signer.
fn assert_signed_transact(calls: &[Recorded], spp: &[AccountMeta], pda: &Address) {
    assert_eq!(calls.len(), 1, "exactly one CPI");
    let call = &calls[0];
    assert_eq!(call.program, SPP_PROGRAM);
    let mut expected = vec![SPP_TRANSACT_TAG];
    expected.extend_from_slice(&transact_data());
    assert_eq!(call.data, expected, "tag 16 + the client's transact bytes, unchanged");
    assert_eq!(call.accounts.len(), spp.len());
    for (got, meta) in call.accounts.iter().zip(spp) {
        let (address, is_signer, is_writable) = *got;
        assert_eq!(address, meta.pubkey);
        assert_eq!(is_writable, meta.is_writable, "writability kept for {address}");
        let want_signer = meta.is_signer || address == *pda;
        assert_eq!(is_signer, want_signer, "signer flag for {address}");
    }
}

// --- ClaimPayment ----------------------------------------------------------------------------

#[test]
fn recipient_claims_and_the_program_signs_for_its_pda() {
    let mut ctx = setup();
    let (payer, recipient) = (ctx.creator.pubkey(), ctx.guest.insecure_clone());
    let (pda, bump) = payment_pda(&ctx, &payer, &recipient.pubkey(), DEADLINE);
    let spp = SppAccounts::new(&recipient.pubkey(), &pda);
    let ix = payment_ix(&ctx, 8, &recipient.pubkey(), &payer, DEADLINE, bump, &spp.metas, &transact_data());
    let (cu, logs) = send_logs(&mut ctx.svm, ix, &[&recipient]).unwrap();
    assert_signed_transact(&recorded(&logs), &spp.metas, &pda);
    println!("ClaimPayment (614-byte transact, 7 SPP accounts): {cu} CU");
}

#[test]
fn recipient_can_still_claim_after_the_deadline() {
    let mut ctx = setup();
    warp(&mut ctx.svm, DEADLINE + 1);
    let (payer, recipient) = (ctx.creator.pubkey(), ctx.guest.insecure_clone());
    let (_, calls) = claim(&mut ctx, &recipient, &payer, DEADLINE).unwrap();
    assert_eq!(calls.len(), 1);
}

#[test]
fn someone_else_cannot_claim_the_recipients_payment() {
    let mut ctx = setup();
    let payer = ctx.creator.pubkey();
    let recipient = ctx.guest.pubkey();
    let stranger = ctx.stranger.insecure_clone();
    // The stranger forwards the real PDA (the one holding the funds) and the real bump, but the
    // program derives the PDA from the *signer*, so it never matches.
    let (pda, bump) = payment_pda(&ctx, &payer, &recipient, DEADLINE);
    let spp = SppAccounts::new(&stranger.pubkey(), &pda);
    let ix = payment_ix(&ctx, 8, &stranger.pubkey(), &payer, DEADLINE, bump, &spp.metas, &transact_data());
    let err = send(&mut ctx.svm, ix, &[&stranger]).unwrap_err();
    assert!(
        err == custom(MISSING_PAYMENT_AUTHORITY) || err == custom(INVALID_PAYMENT_AUTHORITY),
        "got {err:?}"
    );
}

#[test]
fn the_payer_cannot_claim_its_own_payment() {
    let mut ctx = setup();
    let payer = ctx.creator.insecure_clone();
    let recipient = ctx.guest.pubkey();
    let (pda, bump) = payment_pda(&ctx, &payer.pubkey(), &recipient, DEADLINE);
    let spp = SppAccounts::new(&payer.pubkey(), &pda);
    // Claim "as recipient" while signing as the payer: counterparty field = payer.
    let ix = payment_ix(&ctx, 8, &payer.pubkey(), &payer.pubkey(), DEADLINE, bump, &spp.metas, &transact_data());
    let err = send(&mut ctx.svm, ix, &[&payer]).unwrap_err();
    assert!(
        err == custom(MISSING_PAYMENT_AUTHORITY) || err == custom(INVALID_PAYMENT_AUTHORITY),
        "got {err:?}"
    );
}

#[test]
fn claim_needs_the_recipients_signature() {
    let mut ctx = setup();
    let payer = ctx.creator.pubkey();
    let recipient = ctx.guest.pubkey();
    let fee_payer = ctx.stranger.insecure_clone();
    let (pda, bump) = payment_pda(&ctx, &payer, &recipient, DEADLINE);
    let spp = SppAccounts::new(&fee_payer.pubkey(), &pda);
    let mut ix = payment_ix(&ctx, 8, &recipient, &payer, DEADLINE, bump, &spp.metas, &transact_data());
    ix.accounts[0].is_signer = false;
    let err = send(&mut ctx.svm, ix, &[&fee_payer]).unwrap_err();
    assert_eq!(err, ix_err(InstructionError::MissingRequiredSignature));
}

#[test]
fn a_relayer_can_pay_the_fee_while_the_recipient_authorizes() {
    let mut ctx = setup();
    let payer = ctx.creator.pubkey();
    let recipient = ctx.guest.insecure_clone();
    let relayer = ctx.stranger.insecure_clone();
    let (pda, bump) = payment_pda(&ctx, &payer, &recipient.pubkey(), DEADLINE);
    let spp = SppAccounts::new(&relayer.pubkey(), &pda);
    let ix = payment_ix(&ctx, 8, &recipient.pubkey(), &payer, DEADLINE, bump, &spp.metas, &transact_data());
    let (_, logs) = send_logs(&mut ctx.svm, ix, &[&relayer, &recipient]).unwrap();
    assert_signed_transact(&recorded(&logs), &spp.metas, &pda);
}

// --- RefundPayment ---------------------------------------------------------------------------

#[test]
fn payer_cannot_refund_before_or_at_the_deadline() {
    let mut ctx = setup();
    let (payer, recipient) = (ctx.creator.insecure_clone(), ctx.guest.pubkey());
    for now in [T0, DEADLINE - 1, DEADLINE] {
        warp(&mut ctx.svm, now);
        assert_eq!(refund(&mut ctx, &payer, &recipient, DEADLINE).unwrap_err(), custom(PAYMENT_NOT_REFUNDABLE), "now = {now}");
    }
}

#[test]
fn payer_refunds_after_the_deadline() {
    let mut ctx = setup();
    warp(&mut ctx.svm, DEADLINE + 1);
    let (payer, recipient) = (ctx.creator.insecure_clone(), ctx.guest.pubkey());
    let (pda, bump) = payment_pda(&ctx, &payer.pubkey(), &recipient, DEADLINE);
    let spp = SppAccounts::new(&payer.pubkey(), &pda);
    let ix = payment_ix(&ctx, 9, &payer.pubkey(), &recipient, DEADLINE, bump, &spp.metas, &transact_data());
    let (_, logs) = send_logs(&mut ctx.svm, ix, &[&payer]).unwrap();
    assert_signed_transact(&recorded(&logs), &spp.metas, &pda);
}

#[test]
fn the_recipient_cannot_use_refund() {
    let mut ctx = setup();
    warp(&mut ctx.svm, DEADLINE + 1);
    let payer = ctx.creator.pubkey();
    let recipient = ctx.guest.insecure_clone();
    let (pda, bump) = payment_pda(&ctx, &payer, &recipient.pubkey(), DEADLINE);
    let spp = SppAccounts::new(&recipient.pubkey(), &pda);
    // Recipient signs RefundPayment naming itself as the recipient: the program derives
    // ["payment", recipient, recipient, deadline], not the funded PDA.
    let ix = payment_ix(&ctx, 9, &recipient.pubkey(), &recipient.pubkey(), DEADLINE, bump, &spp.metas, &transact_data());
    let err = send(&mut ctx.svm, ix, &[&recipient]).unwrap_err();
    assert!(
        err == custom(MISSING_PAYMENT_AUTHORITY) || err == custom(INVALID_PAYMENT_AUTHORITY),
        "got {err:?}"
    );
}

#[test]
fn a_past_deadline_cannot_unlock_a_future_payment() {
    // The funds sit under the PDA for DEADLINE. The payer claims an already-passed deadline to
    // get past the time check: that names a different PDA, so the program refuses to sign for the
    // funded one.
    let mut ctx = setup();
    warp(&mut ctx.svm, T0 + 10);
    let (payer, recipient) = (ctx.creator.insecure_clone(), ctx.guest.pubkey());
    let (funded, _) = payment_pda(&ctx, &payer.pubkey(), &recipient, DEADLINE);
    let past = T0;
    let (_, past_bump) = payment_pda(&ctx, &payer.pubkey(), &recipient, past);
    let spp = SppAccounts::new(&payer.pubkey(), &funded);
    let ix = payment_ix(&ctx, 9, &payer.pubkey(), &recipient, past, past_bump, &spp.metas, &transact_data());
    assert_eq!(send(&mut ctx.svm, ix, &[&payer]).unwrap_err(), custom(MISSING_PAYMENT_AUTHORITY));
}

// --- The CPI's guard rails -------------------------------------------------------------------

#[test]
fn the_cpi_must_target_the_privacy_program() {
    let mut ctx = setup();
    let (payer, recipient) = (ctx.creator.pubkey(), ctx.guest.insecure_clone());
    let (pda, bump) = payment_pda(&ctx, &payer, &recipient.pubkey(), DEADLINE);
    // The recorder is also at the Magic program address, so only our check stops this.
    for fake in [MAGIC_PROGRAM, PERMISSION_PROGRAM, SYSTEM_PROGRAM] {
        let mut spp = SppAccounts::new(&recipient.pubkey(), &pda);
        spp.metas[2] = AccountMeta::new_readonly(fake, false);
        let ix = payment_ix(&ctx, 8, &recipient.pubkey(), &payer, DEADLINE, bump, &spp.metas, &transact_data());
        assert_eq!(send(&mut ctx.svm, ix, &[&recipient]).unwrap_err(), custom(INVALID_PRIVACY_PROGRAM), "{fake}");
    }
}

#[test]
fn the_pda_must_be_among_the_forwarded_accounts() {
    let mut ctx = setup();
    let (payer, recipient) = (ctx.creator.pubkey(), ctx.guest.insecure_clone());
    let (_, bump) = payment_pda(&ctx, &payer, &recipient.pubkey(), DEADLINE);
    let spp = SppAccounts::new(&recipient.pubkey(), &Address::new_unique());
    let ix = payment_ix(&ctx, 8, &recipient.pubkey(), &payer, DEADLINE, bump, &spp.metas, &transact_data());
    assert_eq!(send(&mut ctx.svm, ix, &[&recipient]).unwrap_err(), custom(MISSING_PAYMENT_AUTHORITY));
}

#[test]
fn a_bump_that_derives_no_program_address_fails() {
    let mut ctx = setup();
    let (payer, recipient) = (ctx.creator.pubkey(), ctx.guest.insecure_clone());
    let (pda, _) = payment_pda(&ctx, &payer, &recipient.pubkey(), DEADLINE);
    let seeds_for = |bump: u8| {
        Address::create_program_address(
            &[b"payment", payer.as_ref(), recipient.pubkey().as_ref(), &DEADLINE.to_le_bytes(), &[bump]],
            &ctx.program_id,
        )
    };
    let on_curve = (0..=255u8).rev().find(|b| seeds_for(*b).is_err()).expect("some bump is on the curve");
    let spp = SppAccounts::new(&recipient.pubkey(), &pda);
    let ix = payment_ix(&ctx, 8, &recipient.pubkey(), &payer, DEADLINE, on_curve, &spp.metas, &transact_data());
    assert_eq!(send(&mut ctx.svm, ix, &[&recipient]).unwrap_err(), custom(INVALID_PAYMENT_AUTHORITY));
}

#[test]
fn a_non_canonical_bump_names_a_pda_that_holds_nothing() {
    // A valid but non-canonical bump derives another PDA of the same (payer, recipient,
    // deadline). The SDK funds the canonical one, so forwarding that one fails.
    let mut ctx = setup();
    let (payer, recipient) = (ctx.creator.pubkey(), ctx.guest.insecure_clone());
    let (pda, canonical) = payment_pda(&ctx, &payer, &recipient.pubkey(), DEADLINE);
    let other = (0..canonical)
        .rev()
        .find(|b| {
            Address::create_program_address(
                &[b"payment", payer.as_ref(), recipient.pubkey().as_ref(), &DEADLINE.to_le_bytes(), &[*b]],
                &ctx.program_id,
            )
            .is_ok()
        })
        .expect("another off-curve bump");
    let spp = SppAccounts::new(&recipient.pubkey(), &pda);
    let ix = payment_ix(&ctx, 8, &recipient.pubkey(), &payer, DEADLINE, other, &spp.metas, &transact_data());
    assert_eq!(send(&mut ctx.svm, ix, &[&recipient]).unwrap_err(), custom(MISSING_PAYMENT_AUTHORITY));
}

#[test]
fn malformed_instruction_data_is_rejected() {
    let mut ctx = setup();
    let (payer, recipient) = (ctx.creator.pubkey(), ctx.guest.insecure_clone());
    let (pda, bump) = payment_pda(&ctx, &payer, &recipient.pubkey(), DEADLINE);
    let spp = SppAccounts::new(&recipient.pubkey(), &pda);
    let full = payment_ix(&ctx, 8, &recipient.pubkey(), &payer, DEADLINE, bump, &spp.metas, &transact_data());
    // No transact bytes, and every truncation of the 41-byte header.
    for len in [42 - 1, 41, 33, 1] {
        let mut ix = full.clone();
        ix.data.truncate(len);
        assert_eq!(
            send(&mut ctx.svm, ix, &[&recipient]).unwrap_err(),
            ix_err(InstructionError::InvalidInstructionData),
            "data length {len}"
        );
    }
}

#[test]
#[allow(deprecated)] // `InstructionError::NotEnoughAccountKeys` is what the program's error maps to.
fn too_few_or_too_many_spp_accounts() {
    let mut ctx = setup();
    let (payer, recipient) = (ctx.creator.pubkey(), ctx.guest.insecure_clone());
    let (pda, bump) = payment_pda(&ctx, &payer, &recipient.pubkey(), DEADLINE);

    // Only the signer: no SPP account at index 2.
    let ix = payment_ix(&ctx, 8, &recipient.pubkey(), &payer, DEADLINE, bump, &[], &transact_data());
    assert_eq!(send(&mut ctx.svm, ix, &[&recipient]).unwrap_err(), ix_err(InstructionError::NotEnoughAccountKeys));

    // 25 SPP accounts: one more than the program forwards.
    let mut spp = SppAccounts::new(&recipient.pubkey(), &pda);
    while spp.metas.len() < 25 {
        spp.metas.push(AccountMeta::new_readonly(Address::new_unique(), false));
    }
    let ix = payment_ix(&ctx, 8, &recipient.pubkey(), &payer, DEADLINE, bump, &spp.metas, &transact_data());
    assert_eq!(send(&mut ctx.svm, ix, &[&recipient]).unwrap_err(), ix_err(InstructionError::InvalidArgument));
}

#[test]
fn the_largest_transact_that_fits_a_transaction_is_forwarded() {
    // v1 transactions cap at 1,232 bytes, so the transact data can't get near the program's
    // 1,231-byte buffer. Check a ~900-byte one goes through intact.
    let mut ctx = setup();
    let (payer, recipient) = (ctx.creator.pubkey(), ctx.guest.insecure_clone());
    let (pda, bump) = payment_pda(&ctx, &payer, &recipient.pubkey(), DEADLINE);
    let spp = SppAccounts::new(&recipient.pubkey(), &pda);
    let big: Vec<u8> = (0..900u32).map(|i| i as u8).collect();
    let ix = payment_ix(&ctx, 8, &recipient.pubkey(), &payer, DEADLINE, bump, &spp.metas, &big);
    let (_, logs) = send_logs(&mut ctx.svm, ix, &[&recipient]).unwrap();
    let calls = recorded(&logs);
    assert_eq!(calls[0].data[0], SPP_TRANSACT_TAG);
    assert_eq!(&calls[0].data[1..], &big[..]);
}
