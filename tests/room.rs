//! Runs `../target/deploy/ghost_message.so` in LiteSVM with v1 transactions.
//! Build it first from the project root: `cargo build-sbf --tools-version v1.58 --arch v3`.
//!
//! LiteSVM has no Ephemeral Rollup. The "PER" instructions (`OpenRoom`, `JoinRoom`,
//! `SendMessage`, `CloseRoom`) run here against a program-owned room, which is how a delegated
//! account looks inside the ER.


mod common;

use common::*;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::{VersionedMessage, v1};
use solana_signer::Signer;
use solana_transaction::{InstructionError, TransactionError, versioned::VersionedTransaction};

// ---------------------------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------------------------

#[test]
fn unknown_or_unimplemented_instructions_fail() {
    let mut ctx = setup();
    let payer = ctx.creator.insecure_clone();
    // 1, 6, 7 now exist but need accounts; 8, 255 and a wrong callback prefix are unknown.
    for data in [vec![], vec![8], vec![255], vec![196, 0, 0, 0, 0, 0, 0, 0]] {
        let ix = Instruction::new_with_bytes(ctx.program_id, &data, vec![]);
        assert_eq!(
            send(&mut ctx.svm, ix, &[&payer]).unwrap_err(),
            ix_err(InstructionError::InvalidInstructionData),
            "data {data:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// CreateRoom
// ---------------------------------------------------------------------------------------------

#[test]
fn create_room_initializes_layout() {
    let mut ctx = setup();
    let room = ctx.create(42);
    let account = ctx.svm.get_account(&room).unwrap();
    assert_eq!(account.owner, ctx.program_id);
    assert_eq!(account.data.len(), ROOM_LEN);

    let (_, bump) = Address::find_program_address(
        &[b"room", ctx.creator.pubkey().as_ref(), &42u64.to_le_bytes()],
        &ctx.program_id,
    );
    let d = &account.data;
    assert_eq!(d[0], 1, "discriminator");
    assert_eq!(d[1], bump);
    assert_eq!(d[OFF_STATUS], CREATED);
    assert_eq!(bytes32(d, OFF_CREATOR), ctx.creator.pubkey().as_ref());
    assert_eq!(read_u64(d, OFF_ROOM_ID), 42);
    // Nothing but discriminator, bump, status, creator and room id is set.
    assert!(d[OFF_GUEST..OFF_ROOM_ID].iter().all(|b| *b == 0));
    assert!(d[OFF_EXPIRES..].iter().all(|b| *b == 0));

    // Rent exemption + permission rent for [creator, guest]: (35 + 3·33 + 60) · 32.
    let rent = ctx.svm.minimum_balance_for_rent_exemption(ROOM_LEN);
    assert_eq!(account.lamports, rent + (35 + 3 * 33 + 60) * 32);
}

#[test]
fn create_room_ids_are_independent() {
    let mut ctx = setup();
    let a = ctx.create(1);
    let b = ctx.create(2);
    assert_ne!(a, b);
    assert_eq!(ctx.room(&b)[OFF_STATUS], CREATED);
}

#[test]
fn create_room_twice_fails() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let ix = ctx.create_ix(&ctx.creator.pubkey(), &room, 1);
    let creator = ctx.creator.insecure_clone();
    assert_eq!(
        send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
        ix_err(InstructionError::AccountAlreadyInitialized)
    );
}

#[test]
fn create_room_rejects_wrong_pda() {
    let mut ctx = setup();
    let creator = ctx.creator.insecure_clone();
    // Address for a different id, and another creator's address.
    for room in [
        ctx.room_address(&creator.pubkey(), 2),
        ctx.room_address(&ctx.guest.pubkey(), 1),
    ] {
        let ix = ctx.create_ix(&creator.pubkey(), &room, 1);
        assert_eq!(
            send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
            ix_err(InstructionError::InvalidSeeds)
        );
    }
}

#[test]
fn create_room_works_on_prefunded_address() {
    let mut ctx = setup();
    let room = ctx.room_address(&ctx.creator.pubkey(), 1);
    ctx.svm.airdrop(&room, 1_000_000).unwrap();
    let room = ctx.create(1);
    let account = ctx.svm.get_account(&room).unwrap();
    assert_eq!(account.owner, ctx.program_id);
    let rent = ctx.svm.minimum_balance_for_rent_exemption(ROOM_LEN);
    assert_eq!(account.lamports, rent + (35 + 3 * 33 + 60) * 32);
}

#[test]
fn create_room_requires_creator_signature() {
    let mut ctx = setup();
    let room = ctx.room_address(&ctx.creator.pubkey(), 1);
    let mut ix = ctx.create_ix(&ctx.creator.pubkey(), &room, 1);
    ix.accounts[0].is_signer = false;
    let payer = ctx.stranger.insecure_clone();
    assert_eq!(
        send(&mut ctx.svm, ix, &[&payer]).unwrap_err(),
        ix_err(InstructionError::MissingRequiredSignature)
    );
}

#[test]
// The program returns `ProgramError::NotEnoughAccountKeys`, which the runtime still maps to
// this (deprecated) variant.
#[allow(deprecated)]
fn create_room_rejects_bad_data_and_accounts() {
    let mut ctx = setup();
    let creator = ctx.creator.insecure_clone();
    let room = ctx.room_address(&creator.pubkey(), 1);

    let mut ix = ctx.create_ix(&creator.pubkey(), &room, 1);
    ix.data.push(0);
    assert_eq!(
        send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
        ix_err(InstructionError::InvalidInstructionData)
    );

    let mut ix = ctx.create_ix(&creator.pubkey(), &room, 1);
    ix.accounts[2] = AccountMeta::new_readonly(Address::new_unique(), false);
    assert_eq!(
        send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
        ix_err(InstructionError::IncorrectProgramId)
    );

    let mut ix = ctx.create_ix(&creator.pubkey(), &room, 1);
    ix.accounts.pop();
    assert_eq!(
        send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
        ix_err(InstructionError::NotEnoughAccountKeys)
    );
}

// ---------------------------------------------------------------------------------------------
// OpenRoom
// ---------------------------------------------------------------------------------------------

#[test]
fn open_room_sets_guest_key_and_expiry() {
    let mut ctx = setup();
    let room = ctx.create(1);
    ctx.open(&room);
    let d = ctx.room(&room);
    assert_eq!(d[OFF_STATUS], OPEN);
    assert_eq!(bytes32(&d, OFF_GUEST), ctx.guest.pubkey().as_ref());
    assert_eq!(bytes32(&d, OFF_CREATOR_BOX), CREATOR_BOX);
    assert_eq!(bytes32(&d, OFF_GUEST_BOX), [0; 32]);
    assert_eq!(read_u64(&d, OFF_EXPIRES) as i64, T0 + TTL);
}

#[test]
fn open_room_only_by_creator() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let guest = ctx.guest.pubkey();
    for signer in [ctx.guest.insecure_clone(), ctx.stranger.insecure_clone()] {
        assert_eq!(
            ctx.open_as(&signer, &room, &guest, CREATOR_BOX, T0 + TTL)
                .unwrap_err(),
            custom(UNAUTHORIZED)
        );
    }
}

#[test]
fn open_room_only_once() {
    let mut ctx = setup();
    let room = ctx.create(1);
    ctx.open(&room);
    let (creator, other) = (ctx.creator.insecure_clone(), ctx.stranger.pubkey());
    assert_eq!(
        ctx.open_as(&creator, &room, &other, CREATOR_BOX, T0 + TTL)
            .unwrap_err(),
        custom(INVALID_ROOM_STATUS)
    );
}

#[test]
fn open_room_validates_arguments() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let creator = ctx.creator.insecure_clone();
    let guest = ctx.guest.pubkey();
    let zero = Address::new_from_array([0; 32]);

    let cases = [
        (zero, CREATOR_BOX, T0 + TTL, INVALID_GUEST),
        (creator.pubkey(), CREATOR_BOX, T0 + TTL, INVALID_GUEST),
        (guest, [0; 32], T0 + TTL, INVALID_BOX_KEY),
        (guest, CREATOR_BOX, T0, INVALID_EXPIRY),
        (guest, CREATOR_BOX, T0 - 1, INVALID_EXPIRY),
    ];
    for (g, box_pk, expires_at, code) in cases {
        assert_eq!(
            ctx.open_as(&creator, &room, &g, box_pk, expires_at)
                .unwrap_err(),
            custom(code)
        );
    }
    assert_eq!(ctx.room(&room)[OFF_STATUS], CREATED);
}

#[test]
fn open_room_rejects_foreign_account() {
    let mut ctx = setup();
    let creator = ctx.creator.insecure_clone();
    let guest = ctx.guest.pubkey();
    // A system-owned account where the room should be.
    let fake = Keypair::new().pubkey();
    ctx.svm.airdrop(&fake, 1_000_000).unwrap();
    assert_eq!(
        ctx.open_as(&creator, &fake, &guest, CREATOR_BOX, T0 + TTL)
            .unwrap_err(),
        ix_err(InstructionError::InvalidAccountOwner)
    );
}

#[test]
fn open_room_rejects_trailing_data() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let mut data = Ctx::open_data(&ctx.guest.pubkey(), CREATOR_BOX, T0 + TTL);
    data.push(0);
    let ix = ctx.permission_ix(&ctx.creator.pubkey(), &room, None, data);
    let creator = ctx.creator.insecure_clone();
    assert_eq!(
        send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
        ix_err(InstructionError::InvalidInstructionData)
    );
}

// ---------------------------------------------------------------------------------------------
// JoinRoom
// ---------------------------------------------------------------------------------------------

#[test]
fn join_room_sets_guest_key() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let d = ctx.room(&room);
    assert_eq!(d[OFF_STATUS], JOINED);
    assert_eq!(bytes32(&d, OFF_GUEST_BOX), GUEST_BOX);
}

#[test]
fn join_room_only_by_guest() {
    let mut ctx = setup();
    let room = ctx.create(1);
    ctx.open(&room);
    for signer in [ctx.creator.insecure_clone(), ctx.stranger.insecure_clone()] {
        assert_eq!(
            ctx.join_as(&signer, &room, GUEST_BOX).unwrap_err(),
            custom(UNAUTHORIZED)
        );
    }
}

#[test]
fn join_room_requires_open_status() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let guest = ctx.guest.insecure_clone();
    // Not opened yet.
    assert_eq!(
        ctx.join_as(&guest, &room, GUEST_BOX).unwrap_err(),
        custom(INVALID_ROOM_STATUS)
    );
    ctx.open(&room);
    ctx.join(&room);
    // Already joined.
    assert_eq!(
        ctx.join_as(&guest, &room, GUEST_BOX).unwrap_err(),
        custom(INVALID_ROOM_STATUS)
    );
}

#[test]
fn join_room_rejects_zero_key_and_expiry() {
    let mut ctx = setup();
    let room = ctx.create(1);
    ctx.open(&room);
    let guest = ctx.guest.insecure_clone();
    assert_eq!(
        ctx.join_as(&guest, &room, [0; 32]).unwrap_err(),
        custom(INVALID_BOX_KEY)
    );
    warp(&mut ctx.svm, T0 + TTL);
    assert_eq!(
        ctx.join_as(&guest, &room, GUEST_BOX).unwrap_err(),
        custom(ROOM_EXPIRED)
    );
}

// ---------------------------------------------------------------------------------------------
// SendMessage
// ---------------------------------------------------------------------------------------------

#[test]
fn send_message_writes_slots_for_both_members() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let (creator, guest) = (ctx.creator.insecure_clone(), ctx.guest.insecure_clone());

    ctx.send_as(&creator, &room, 0, [1; 24], &[0xAA; 40]).unwrap();
    ctx.send_as(&guest, &room, 2, [2; 24], &[0xBB; 81]).unwrap();

    let d = ctx.room(&room);
    assert_eq!(read_u64(&d, OFF_NEXT_SEQ), 2);

    let s0 = slot(&d, 0);
    assert_eq!((s0.seq, s0.sender, s0.kind, s0.len), (0, 0, 0, 40));
    assert_eq!(s0.nonce, [1; 24]);
    assert!(s0.ciphertext[..40].iter().all(|b| *b == 0xAA));
    assert!(s0.ciphertext[40..].iter().all(|b| *b == 0));

    let s1 = slot(&d, 1);
    assert_eq!((s1.seq, s1.sender, s1.kind, s1.len), (1, 1, 2, 81));
    assert_eq!(s1.nonce, [2; 24]);
    assert!(s1.ciphertext[..81].iter().all(|b| *b == 0xBB));

    for i in 2..SLOTS {
        assert!(slot(&d, i).ciphertext.iter().all(|b| *b == 0));
    }
}

#[test]
fn send_message_ring_wraps_and_clears_old_bytes() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let creator = ctx.creator.insecure_clone();

    // Fill all 16 slots with full-size messages, then overwrite slot 0 with a short one.
    for i in 0..SLOTS as u8 {
        ctx.send_as(&creator, &room, 0, [i; 24], &[i + 1; 220]).unwrap();
    }
    ctx.send_as(&creator, &room, 1, [0xEE; 24], &[0xEE; 16]).unwrap();

    let d = ctx.room(&room);
    assert_eq!(read_u64(&d, OFF_NEXT_SEQ), 17);

    let s0 = slot(&d, 0);
    assert_eq!((s0.seq, s0.kind, s0.len), (16, 1, 16));
    assert_eq!(s0.nonce, [0xEE; 24]);
    assert!(s0.ciphertext[..16].iter().all(|b| *b == 0xEE));
    assert!(
        s0.ciphertext[16..].iter().all(|b| *b == 0),
        "the overwritten message must not survive in the padding"
    );

    // Slots 1..16 still hold seq 1..16.
    for i in 1..SLOTS {
        let s = slot(&d, i);
        assert_eq!(s.seq, i as u64);
        assert!(s.ciphertext.iter().all(|b| *b == i as u8 + 1));
    }
}

#[test]
fn send_message_length_bounds() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let creator = ctx.creator.insecure_clone();
    ctx.send_as(&creator, &room, 0, [0; 24], &[1; 16]).unwrap();
    ctx.send_as(&creator, &room, 0, [0; 24], &[1; 220]).unwrap();
    for len in [0, 15, 221] {
        assert_eq!(
            ctx.send_as(&creator, &room, 0, [0; 24], &vec![1; len])
                .unwrap_err(),
            custom(INVALID_MESSAGE_LENGTH),
            "len {len}"
        );
    }
}

#[test]
fn send_message_rejects_unknown_kind() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let creator = ctx.creator.insecure_clone();
    for kind in [3, 255] {
        assert_eq!(
            ctx.send_as(&creator, &room, kind, [0; 24], &[1; 32])
                .unwrap_err(),
            custom(INVALID_MESSAGE_KIND)
        );
    }
}

#[test]
fn send_message_rejects_short_header() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    // kind + 23-byte nonce: the nonce is incomplete.
    let mut data = vec![4, 0];
    data.extend_from_slice(&[0; 23]);
    let ix = ctx.room_ix(&ctx.creator.pubkey(), &room, data);
    let creator = ctx.creator.insecure_clone();
    assert_eq!(
        send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
        ix_err(InstructionError::InvalidInstructionData)
    );
}

#[test]
fn send_message_only_by_members() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let stranger = ctx.stranger.insecure_clone();
    assert_eq!(
        ctx.send_as(&stranger, &room, 0, [0; 24], &[1; 32])
            .unwrap_err(),
        custom(UNAUTHORIZED)
    );
}

#[test]
fn send_message_requires_joined_status() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let creator = ctx.creator.insecure_clone();
    assert_eq!(
        ctx.send_as(&creator, &room, 0, [0; 24], &[1; 32]).unwrap_err(),
        custom(INVALID_ROOM_STATUS)
    );
    ctx.open(&room);
    assert_eq!(
        ctx.send_as(&creator, &room, 0, [0; 24], &[1; 32]).unwrap_err(),
        custom(INVALID_ROOM_STATUS)
    );
}

#[test]
fn send_message_stops_at_expiry() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let guest = ctx.guest.insecure_clone();
    warp(&mut ctx.svm, T0 + TTL - 1);
    ctx.send_as(&guest, &room, 0, [0; 24], &[1; 32]).unwrap();
    warp(&mut ctx.svm, T0 + TTL);
    assert_eq!(
        ctx.send_as(&guest, &room, 0, [0; 24], &[1; 32]).unwrap_err(),
        custom(ROOM_EXPIRED)
    );
}

// ---------------------------------------------------------------------------------------------
// CloseRoom
// ---------------------------------------------------------------------------------------------

fn assert_wiped(ctx: &Ctx, room: &Address) {
    let d = ctx.room(room);
    assert_eq!(d.len(), ROOM_LEN);
    assert_eq!(d[0], 1);
    assert_eq!(d[OFF_STATUS], CLOSED);
    assert_eq!(bytes32(&d, OFF_CREATOR), ctx.creator.pubkey().as_ref());
    assert_eq!(read_u64(&d, OFF_ROOM_ID), 1, "room id is a public seed and stays");
    assert!(
        d[OFF_GUEST..OFF_ROOM_ID].iter().all(|b| *b == 0) && d[OFF_EXPIRES..].iter().all(|b| *b == 0),
        "guest, box keys, expiry, seq and messages must be zero"
    );
}

#[test]
fn close_room_by_creator_wipes_everything() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let creator = ctx.creator.insecure_clone();
    for i in 0..20u8 {
        ctx.send_as(&creator, &room, 0, [i; 24], &[i; 100]).unwrap();
    }
    ctx.close_as(&creator, &room).unwrap();
    assert_wiped(&ctx, &room);
}

#[test]
fn close_room_by_guest() {
    let mut ctx = setup();
    let room = ctx.create(1);
    ctx.open(&room);
    let guest = ctx.guest.insecure_clone();
    ctx.close_as(&guest, &room).unwrap();
    assert_wiped(&ctx, &room);
}

#[test]
fn close_room_by_creator_before_open() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let creator = ctx.creator.insecure_clone();
    ctx.close_as(&creator, &room).unwrap();
    assert_wiped(&ctx, &room);
}

#[test]
fn close_room_stranger_only_after_expiry() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let stranger = ctx.stranger.insecure_clone();
    warp(&mut ctx.svm, T0 + TTL - 1);
    assert_eq!(
        ctx.close_as(&stranger, &room).unwrap_err(),
        custom(UNAUTHORIZED)
    );
    warp(&mut ctx.svm, T0 + TTL);
    ctx.close_as(&stranger, &room).unwrap();
    assert_wiped(&ctx, &room);
}

#[test]
fn close_room_unopened_room_never_expires_for_strangers() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let stranger = ctx.stranger.insecure_clone();
    warp(&mut ctx.svm, T0 + 365 * 86_400);
    assert_eq!(
        ctx.close_as(&stranger, &room).unwrap_err(),
        custom(UNAUTHORIZED)
    );
}

#[test]
fn closed_room_is_terminal() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let (creator, guest) = (ctx.creator.insecure_clone(), ctx.guest.insecure_clone());
    ctx.close_as(&creator, &room).unwrap();

    assert_eq!(
        ctx.close_as(&guest, &room).unwrap_err(),
        // The guest was wiped, so the old guest is now a stranger, but status is checked first.
        custom(INVALID_ROOM_STATUS)
    );
    assert_eq!(
        ctx.send_as(&creator, &room, 0, [0; 24], &[1; 32]).unwrap_err(),
        custom(INVALID_ROOM_STATUS)
    );
    assert_eq!(
        ctx.join_as(&guest, &room, GUEST_BOX).unwrap_err(),
        custom(INVALID_ROOM_STATUS)
    );
    let g = guest.pubkey();
    assert_eq!(
        ctx.open_as(&creator, &room, &g, CREATOR_BOX, T0 + TTL)
            .unwrap_err(),
        custom(INVALID_ROOM_STATUS)
    );
}

#[test]
fn close_room_rejects_instruction_data() {
    let mut ctx = setup();
    let room = ctx.create(1);
    let ix = ctx.permission_ix(&ctx.creator.pubkey(), &room, None, vec![5, 0]);
    let creator = ctx.creator.insecure_clone();
    assert_eq!(
        send(&mut ctx.svm, ix, &[&creator]).unwrap_err(),
        ix_err(InstructionError::InvalidInstructionData)
    );
}

// ---------------------------------------------------------------------------------------------
// v1 transaction limits
// ---------------------------------------------------------------------------------------------

#[test]
fn v1_loaded_data_limit_is_enforced() {
    let mut ctx = setup();
    let room = ctx.room_address(&ctx.creator.pubkey(), 1);
    let ix = ctx.create_ix(&ctx.creator.pubkey(), &room, 1);
    let message = v1::Message::try_compile_with_config(
        &ctx.creator.pubkey(),
        &[ix],
        ctx.svm.latest_blockhash(),
        CONFIG.with_loaded_accounts_data_size_limit(1024),
    )
    .unwrap();
    let tx = VersionedTransaction::try_new(VersionedMessage::V1(message), &[&ctx.creator]).unwrap();
    assert_eq!(
        ctx.svm.send_transaction(tx).unwrap_err().err,
        TransactionError::MaxLoadedAccountsDataSizeExceeded
    );
}
