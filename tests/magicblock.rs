//! MagicBlock CPIs.
//!
//! - `DelegateRoom` runs against the **real** delegation program (`fixtures/delegation_program.so`,
//!   dumped from devnet).
//! - The permission program and the Magic program only exist inside a MagicBlock ER. Here a
//!   recorder (`tests/cpi-recorder`) sits at their addresses and logs every CPI it
//!   receives, so these tests check the exact bytes, account order and signer/writable flags our
//!   program sends. Whether the real programs accept them is checked by the e2e on `mb-stack`.
//! - `ProcessUndelegation` needs a buffer signed by the delegation program, which only the real
//!   undelegation flow produces. Its guards are tested here; the happy path is in the e2e.

mod common;

use common::*;
use solana_account::Account;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::InstructionError;

fn buffer_address(ctx: &Ctx, room: &Address) -> Address {
    Address::find_program_address(&[b"buffer", room.as_ref()], &ctx.program_id).0
}

fn record_address(room: &Address) -> Address {
    Address::find_program_address(&[b"delegation", room.as_ref()], &DELEGATION_PROGRAM).0
}

fn metadata_address(room: &Address) -> Address {
    Address::find_program_address(&[b"delegation-metadata", room.as_ref()], &DELEGATION_PROGRAM).0
}

fn delegate_ix(ctx: &Ctx, signer: &Address, room: &Address) -> Instruction {
    Instruction::new_with_bytes(
        ctx.program_id,
        &[1],
        vec![
            AccountMeta::new(*signer, true),
            AccountMeta::new(*room, false),
            AccountMeta::new_readonly(ctx.program_id, false),
            AccountMeta::new(buffer_address(ctx, room), false),
            AccountMeta::new(record_address(room), false),
            AccountMeta::new(metadata_address(room), false),
            AccountMeta::new_readonly(DELEGATION_PROGRAM, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ],
    )
}

fn undelegate_ix(ctx: &Ctx, payer: &Address, room: &Address) -> Instruction {
    Instruction::new_with_bytes(
        ctx.program_id,
        &[6],
        vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(*room, false),
            AccountMeta::new(MAGIC_CONTEXT, false),
            AccountMeta::new_readonly(MAGIC_PROGRAM, false),
        ],
    )
}

fn reclaim_ix(ctx: &Ctx, creator: &Address, room: &Address) -> Instruction {
    Instruction::new_with_bytes(
        ctx.program_id,
        &[7],
        vec![AccountMeta::new(*creator, false), AccountMeta::new(*room, false)],
    )
}

fn open_logs(ctx: &mut Ctx, room: &Address) -> Vec<String> {
    let creator = ctx.creator.insecure_clone();
    let ix = ctx.permission_ix(
        &creator.pubkey(),
        room,
        None,
        Ctx::open_data(&ctx.guest.pubkey(), CREATOR_BOX, T0 + TTL),
    );
    send_logs(&mut ctx.svm, ix, &[&creator]).unwrap().1
}

fn close_logs(ctx: &mut Ctx, signer: &Keypair, room: &Address) -> Vec<String> {
    let ix = ctx.permission_ix(&signer.pubkey(), room, None, vec![5]);
    send_logs(&mut ctx.svm, ix, &[signer]).unwrap().1
}

// ---------------------------------------------------------------------------------------------
// DelegateRoom — real delegation program
// ---------------------------------------------------------------------------------------------

#[test]
fn delegate_room_hands_the_room_to_the_delegation_program() {
    let mut ctx = setup();
    let room = ctx.create(7);
    let before = ctx.svm.get_account(&room).unwrap();
    let creator = ctx.creator.insecure_clone();
    let creator_before = ctx.svm.get_balance(&creator.pubkey()).unwrap();

    let ix = delegate_ix(&ctx, &creator.pubkey(), &room);
    send(&mut ctx.svm, ix, &[&creator]).unwrap();

    let after = ctx.svm.get_account(&room).unwrap();
    assert_eq!(after.owner, DELEGATION_PROGRAM);
    assert_eq!(after.data, before.data, "delegation keeps the data");
    assert_eq!(after.lamports, before.lamports, "permission prefund stays with the room");

    // The delegation record pins the TEE validator (bytes 8..40) and our program (40..72).
    let record = ctx.svm.get_account(&record_address(&room)).unwrap();
    assert_eq!(record.owner, DELEGATION_PROGRAM);
    assert_eq!(&record.data[8..40], TEE_VALIDATOR.as_ref());
    assert_eq!(&record.data[40..72], ctx.program_id.as_ref());
    assert!(ctx.svm.get_account(&metadata_address(&room)).is_some());

    // The buffer is closed and its rent is back with the creator.
    assert!(
        ctx.svm
            .get_account(&buffer_address(&ctx, &room))
            .is_none_or(|a| a.lamports == 0)
    );
    let spent = creator_before - ctx.svm.get_balance(&creator.pubkey()).unwrap();
    println!("DelegateRoom cost the creator {spent} lamports (fees + delegation PDAs)");
}

#[test]
fn delegate_room_only_by_the_creator_and_only_when_created() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let stranger = ctx.stranger.insecure_clone();
    let ix = delegate_ix(&ctx, &stranger.pubkey(), &room);
    assert_eq!(
        send(&mut ctx.svm, ix, &[&stranger]).unwrap_err(),
        custom(UNAUTHORIZED)
    );

    // A room closed on Solana (never delegated) can't be delegated.
    let creator = ctx.creator.insecure_clone();
    ctx.close_as(&creator, &room).unwrap();
    let ix = delegate_ix(&ctx, &creator.pubkey(), &room);
    assert_eq!(
        send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
        custom(INVALID_ROOM_STATUS)
    );
}

#[test]
fn delegate_room_checks_its_accounts() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let creator = ctx.creator.insecure_clone();

    let mut ix = delegate_ix(&ctx, &creator.pubkey(), &room);
    ix.accounts[3].pubkey = Address::new_unique();
    assert_eq!(
        send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
        ix_err(InstructionError::InvalidSeeds),
        "buffer"
    );
    for (index, label) in [(2, "owner program"), (6, "delegation program"), (7, "system")] {
        let mut ix = delegate_ix(&ctx, &creator.pubkey(), &room);
        ix.accounts[index].pubkey = Address::new_unique();
        assert_eq!(
            send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
            ix_err(InstructionError::IncorrectProgramId),
            "{label}"
        );
    }
    let mut ix = delegate_ix(&ctx, &creator.pubkey(), &room);
    ix.data.push(0);
    assert_eq!(
        send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
        ix_err(InstructionError::InvalidInstructionData)
    );
    // Nothing changed.
    assert_eq!(ctx.svm.get_account(&room).unwrap().owner, ctx.program_id);
}

#[test]
fn delegated_room_is_frozen_on_solana() {
    // On Solana a delegated room is owned by the delegation program, so nothing of ours can
    // touch it until it comes back: no second delegation, no open, no close, no reclaim.
    let mut ctx = setup();
    let room = ctx.create(1);
    let creator = ctx.creator.insecure_clone();
    let guest = ctx.guest.pubkey();
    let ix = delegate_ix(&ctx, &creator.pubkey(), &room);
    send(&mut ctx.svm, ix, &[&creator]).unwrap();

    let owner_err = ix_err(InstructionError::InvalidAccountOwner);
    let ix = delegate_ix(&ctx, &creator.pubkey(), &room);
    assert_eq!(send(&mut ctx.svm, ix, &[&creator]).unwrap_err(), owner_err);
    assert_eq!(
        ctx.open_as(&creator, &room, &guest, CREATOR_BOX, T0 + TTL)
            .unwrap_err(),
        owner_err
    );
    assert_eq!(ctx.close_as(&creator, &room).unwrap_err(), owner_err);
    let ix = reclaim_ix(&ctx, &creator.pubkey(), &room);
    assert_eq!(send(&mut ctx.svm, ix, &[&creator]).unwrap_err(), owner_err);
}

// ---------------------------------------------------------------------------------------------
// OpenRoom — CreateEphemeralPermission
// ---------------------------------------------------------------------------------------------

#[test]
fn open_room_creates_a_private_permission_for_both_members() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let logs = open_logs(&mut ctx, &room);
    let cpis = recorded(&logs);
    assert_eq!(cpis.len(), 1, "{logs:#?}");
    let cpi = &cpis[0];
    assert_eq!(cpi.program, PERMISSION_PROGRAM);

    // u64 6 | is_private 1 | (flags, creator) | (flags, guest) — 9 + 33·2 bytes, no count.
    let mut expected = 6u64.to_le_bytes().to_vec();
    expected.push(1);
    expected.push(MEMBER_FLAGS);
    expected.extend_from_slice(ctx.creator.pubkey().as_ref());
    expected.push(MEMBER_FLAGS);
    expected.extend_from_slice(ctx.guest.pubkey().as_ref());
    assert_eq!(cpi.data, expected);
    assert_eq!(cpi.data.len(), 9 + 33 * 2);

    // [payer = room (signer, writable), permissioned = room (signer), permission (w), vault (w),
    //  magic program]. The runtime merges duplicate accounts, so both room entries share flags.
    let keys: Vec<_> = cpi.accounts.iter().map(|a| a.0).collect();
    assert_eq!(
        keys,
        vec![room, room, Ctx::permission_address(&room), EPHEMERAL_VAULT, MAGIC_PROGRAM]
    );
    assert!(cpi.accounts[0].1 && cpi.accounts[0].2, "room signs and pays");
    assert!(cpi.accounts[1].1, "room signs as the permissioned account");
    assert!(!cpi.accounts[2].1 && cpi.accounts[2].2, "permission writable");
    assert!(!cpi.accounts[3].1 && cpi.accounts[3].2, "vault writable");
    assert!(!cpi.accounts[4].1 && !cpi.accounts[4].2, "magic program read-only");
}

#[test]
fn member_flags_grant_reads_but_not_authority() {
    assert_eq!(MEMBER_FLAGS & 1, 0, "no AUTHORITY: members can't edit the permission");
    assert_eq!(MEMBER_FLAGS, (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4));
}

#[test]
fn open_room_checks_the_permission_accounts() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let creator = ctx.creator.insecure_clone();
    let data = Ctx::open_data(&ctx.guest.pubkey(), CREATOR_BOX, T0 + TTL);

    let cases = [
        (2, Address::new_unique(), InstructionError::InvalidSeeds),
        // Another room's permission.
        (2, Ctx::permission_address(&Address::new_unique()), InstructionError::InvalidSeeds),
        (3, Address::new_unique(), InstructionError::IncorrectProgramId),
        (4, Address::new_unique(), InstructionError::IncorrectProgramId),
        (5, Address::new_unique(), InstructionError::IncorrectProgramId),
    ];
    for (index, address, err) in cases {
        let mut ix = ctx.permission_ix(&creator.pubkey(), &room, None, data.clone());
        ix.accounts[index].pubkey = address;
        assert_eq!(
            send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
            ix_err(err),
            "account {index}"
        );
    }
    let mut ix = ctx.permission_ix(&creator.pubkey(), &room, None, data);
    ix.accounts.truncate(5);
    #[allow(deprecated)]
    let missing = InstructionError::NotEnoughAccountKeys;
    assert_eq!(send(&mut ctx.svm, ix, &[&creator]).unwrap_err(), ix_err(missing));

    // Every failure rolled back: the guest was never written.
    let d = ctx.room(&room);
    assert_eq!(d[OFF_STATUS], CREATED);
    assert_eq!(bytes32(&d, OFF_GUEST), [0; 32]);
}

#[test]
fn open_room_fails_when_the_permission_cpi_fails() {
    // On Solana (no Magic program / permission program builtin) the CPI fails, so the guest can
    // never be written to a room outside the PER. Simulate by removing the recorder.
    let mut ctx = setup();
    let room = ctx.create(1);
    ctx.svm
        .set_account(
            PERMISSION_PROGRAM,
            Account {
                lamports: 1,
                data: vec![],
                owner: SYSTEM_PROGRAM,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    let creator = ctx.creator.insecure_clone();
    let guest = ctx.guest.pubkey();
    assert!(
        ctx.open_as(&creator, &room, &guest, CREATOR_BOX, T0 + TTL)
            .is_err()
    );
    let d = ctx.room(&room);
    assert_eq!(d[OFF_STATUS], CREATED);
    assert_eq!(bytes32(&d, OFF_GUEST), [0; 32]);
}

// ---------------------------------------------------------------------------------------------
// CloseRoom — CloseEphemeralPermission
// ---------------------------------------------------------------------------------------------

#[test]
fn close_room_closes_the_permission_of_an_opened_room() {
    for joined in [false, true] {
        let mut ctx = setup();
        let room = ctx.create(1);
        ctx.open(&room);
        if joined {
            ctx.join(&room);
        }
        let guest = ctx.guest.insecure_clone();
        let logs = close_logs(&mut ctx, &guest, &room);
        let cpis = recorded(&logs);
        assert_eq!(cpis.len(), 1, "{logs:#?}");
        let cpi = &cpis[0];
        assert_eq!(cpi.program, PERMISSION_PROGRAM);
        assert_eq!(cpi.data, 8u64.to_le_bytes());
        let keys: Vec<_> = cpi.accounts.iter().map(|a| a.0).collect();
        assert_eq!(
            keys,
            vec![room, room, room, Ctx::permission_address(&room), EPHEMERAL_VAULT, MAGIC_PROGRAM]
        );
        assert!(cpi.accounts[2].1, "room signs as the permissioned account");
        assert_eq!(ctx.room(&room)[OFF_STATUS], CLOSED);
    }
}

#[test]
fn close_room_of_a_created_room_sends_no_cpi() {
    // A room that was never opened has no permission. It can be closed on Solana too, and the
    // permission accounts aren't even checked.
    let mut ctx = setup();
    let room = ctx.create(1);
    let creator = ctx.creator.insecure_clone();
    let mut ix = ctx.permission_ix(&creator.pubkey(), &room, None, vec![5]);
    for meta in &mut ix.accounts[2..6] {
        meta.pubkey = Address::new_unique();
    }
    let (_, logs) = send_logs(&mut ctx.svm, ix, &[&creator]).unwrap();
    assert!(recorded(&logs).is_empty());
    assert_eq!(ctx.room(&room)[OFF_STATUS], CLOSED);
}

#[test]
fn close_room_checks_permission_accounts_when_opened() {
    let mut ctx = setup();
    let room = ctx.create(1);
    ctx.open(&room);
    let creator = ctx.creator.insecure_clone();
    let mut ix = ctx.permission_ix(&creator.pubkey(), &room, None, vec![5]);
    ix.accounts[2].pubkey = Address::new_unique();
    assert_eq!(
        send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
        ix_err(InstructionError::InvalidSeeds)
    );
    // Rolled back: still open, guest still there.
    let d = ctx.room(&room);
    assert_eq!(d[OFF_STATUS], OPEN);
    assert_eq!(bytes32(&d, OFF_GUEST), ctx.guest.pubkey().as_ref());
}

// ---------------------------------------------------------------------------------------------
// UndelegateRoom — ScheduleIntentBundle
// ---------------------------------------------------------------------------------------------

#[test]
fn undelegate_room_schedules_commit_and_undelegate() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let creator = ctx.creator.insecure_clone();
    ctx.close_as(&creator, &room).unwrap();

    // Permissionless: a stranger pays.
    let stranger = ctx.stranger.insecure_clone();
    let ix = undelegate_ix(&ctx, &stranger.pubkey(), &room);
    let (_, logs) = send_logs(&mut ctx.svm, ix, &[&stranger]).unwrap();
    let cpis = recorded(&logs);
    assert_eq!(cpis.len(), 1, "{logs:#?}");
    let cpi = &cpis[0];
    assert_eq!(cpi.program, MAGIC_PROGRAM);
    assert_eq!(
        cpi.data,
        vec![
            11, 0, 0, 0, // ScheduleIntentBundle
            0, // commit: None
            1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, // commit_and_undelegate: Standalone([2])
            0, 0, 0, 0, // UndelegateTypeArgs::Standalone
            0, 0, // finalize variants: None
            0, 0, 0, 0, 0, 0, 0, 0, // no standalone actions
        ]
    );
    // Index 2 in the CPI accounts is the room.
    let keys: Vec<_> = cpi.accounts.iter().map(|a| a.0).collect();
    assert_eq!(keys, vec![stranger.pubkey(), MAGIC_CONTEXT, room]);
    assert!(cpi.accounts[0].1 && cpi.accounts[0].2, "payer signs");
    assert!(cpi.accounts[1].2 && cpi.accounts[2].2, "context and room writable");
}

#[test]
fn undelegate_room_only_once_closed() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let stranger = ctx.stranger.insecure_clone();
    let ix = undelegate_ix(&ctx, &stranger.pubkey(), &room);
    assert_eq!(
        send(&mut ctx.svm, ix, &[&stranger]).unwrap_err(),
        custom(INVALID_ROOM_STATUS),
        "private data is still there"
    );
}

#[test]
fn undelegate_room_checks_magic_accounts() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let creator = ctx.creator.insecure_clone();
    ctx.close_as(&creator, &room).unwrap();
    for index in [2, 3] {
        let mut ix = undelegate_ix(&ctx, &creator.pubkey(), &room);
        ix.accounts[index].pubkey = Address::new_unique();
        assert_eq!(
            send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
            ix_err(InstructionError::IncorrectProgramId)
        );
    }
}

// ---------------------------------------------------------------------------------------------
// ReclaimRoom
// ---------------------------------------------------------------------------------------------

#[test]
fn reclaim_room_returns_every_lamport_to_the_creator() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let creator = ctx.creator.insecure_clone();
    ctx.close_as(&creator, &room).unwrap();
    let room_lamports = ctx.svm.get_balance(&room).unwrap();
    let creator_before = ctx.svm.get_balance(&creator.pubkey()).unwrap();

    // Anyone may send it; the lamports can only go to the creator.
    let stranger = ctx.stranger.insecure_clone();
    let ix = reclaim_ix(&ctx, &creator.pubkey(), &room);
    send(&mut ctx.svm, ix, &[&stranger]).unwrap();

    assert!(ctx.svm.get_account(&room).is_none_or(|a| a.lamports == 0));
    assert_eq!(
        ctx.svm.get_balance(&creator.pubkey()).unwrap(),
        creator_before + room_lamports
    );
}

#[test]
fn reclaim_room_rejects_open_rooms_and_wrong_recipients() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let creator = ctx.creator.insecure_clone();
    let stranger = ctx.stranger.insecure_clone();

    let ix = reclaim_ix(&ctx, &creator.pubkey(), &room);
    assert_eq!(
        send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
        custom(INVALID_ROOM_STATUS)
    );
    ctx.close_as(&creator, &room).unwrap();
    let ix = reclaim_ix(&ctx, &stranger.pubkey(), &room);
    assert_eq!(
        send(&mut ctx.svm, ix, &[&stranger]).unwrap_err(),
        custom(UNAUTHORIZED)
    );
}

// ---------------------------------------------------------------------------------------------
// ProcessUndelegation — guards
// ---------------------------------------------------------------------------------------------

fn callback_ix(ctx: &Ctx, room: Address, buffer: Address, payer: Address) -> Instruction {
    let mut data = UNDELEGATE_CALLBACK.to_vec();
    data.extend_from_slice(&3u32.to_le_bytes());
    for seed in [&b"room"[..], &[1; 32], &[0; 8]] {
        data.extend_from_slice(&(seed.len() as u32).to_le_bytes());
        data.extend_from_slice(seed);
    }
    Instruction::new_with_bytes(
        ctx.program_id,
        &data,
        vec![
            AccountMeta::new(room, false),
            AccountMeta::new_readonly(buffer, true),
            AccountMeta::new(payer, true),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ],
    )
}

#[test]
fn undelegation_callback_requires_the_delegation_programs_buffer() {
    let mut ctx = setup();
    let payer = ctx.creator.insecure_clone();
    let fake_buffer = Keypair::new();
    let room = Address::new_unique();

    // A keypair-owned buffer: wrong owner.
    let ix = callback_ix(&ctx, room, fake_buffer.pubkey(), payer.pubkey());
    assert_eq!(
        send(&mut ctx.svm, ix, &[&payer, &fake_buffer]).unwrap_err(),
        ix_err(InstructionError::InvalidAccountOwner)
    );

    // Owned by the delegation program and signed, but not the canonical PDA.
    ctx.svm
        .set_account(
            fake_buffer.pubkey(),
            Account {
                lamports: 1_000_000,
                data: vec![0; ROOM_LEN],
                owner: DELEGATION_PROGRAM,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    let ix = callback_ix(&ctx, room, fake_buffer.pubkey(), payer.pubkey());
    assert_eq!(
        send(&mut ctx.svm, ix, &[&payer, &fake_buffer]).unwrap_err(),
        ix_err(InstructionError::InvalidSeeds)
    );

    // Unsigned buffer.
    let mut ix = callback_ix(&ctx, room, fake_buffer.pubkey(), payer.pubkey());
    ix.accounts[1].is_signer = false;
    assert_eq!(
        send(&mut ctx.svm, ix, &[&payer]).unwrap_err(),
        ix_err(InstructionError::MissingRequiredSignature)
    );
}

// ---------------------------------------------------------------------------------------------
// Whole lifecycle as the ER sees it (delegation skipped: in the ER the room is program-owned)
// ---------------------------------------------------------------------------------------------

#[test]
fn full_room_lifecycle_cpis_in_order() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let (creator, guest) = (ctx.creator.insecure_clone(), ctx.guest.insecure_clone());

    let open = recorded(&open_logs(&mut ctx, &room));
    ctx.join(&room);
    ctx.send_as(&creator, &room, 0, [1; 24], &[9; 64]).unwrap();
    ctx.send_as(&guest, &room, 1, [2; 24], &[8; 64]).unwrap();
    let close = recorded(&close_logs(&mut ctx, &creator, &room));
    let ix = undelegate_ix(&ctx, &guest.pubkey(), &room);
    let (_, logs) = send_logs(&mut ctx.svm, ix, &[&guest]).unwrap();
    let undelegate = recorded(&logs);

    let programs: Vec<_> = [open, close, undelegate]
        .iter()
        .flatten()
        .map(|c| (c.program, c.data[0]))
        .collect();
    assert_eq!(
        programs,
        vec![(PERMISSION_PROGRAM, 6), (PERMISSION_PROGRAM, 8), (MAGIC_PROGRAM, 11)]
    );

    // What would be committed to Solana: only public seeds and the Closed status.
    let d = ctx.room(&room);
    assert_eq!(d[OFF_STATUS], CLOSED);
    assert!(d[OFF_GUEST..OFF_ROOM_ID].iter().all(|b| *b == 0));
    assert!(d[OFF_EXPIRES..].iter().all(|b| *b == 0));

    // Back on Solana (here: still program-owned), the creator reclaims everything.
    let ix = reclaim_ix(&ctx, &creator.pubkey(), &room);
    send(&mut ctx.svm, ix, &[&creator]).unwrap();
    assert!(ctx.svm.get_account(&room).is_none_or(|a| a.lamports == 0));
}

