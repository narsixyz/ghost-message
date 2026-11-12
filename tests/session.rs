//! Phase 2: session keys.
//!
//! Happy paths and lifecycle (create, expire, revoke) use the **real** `gpl_session` program
//! (`fixtures/session_keys.so`, dumped from devnet), so the token bytes our program reads are the
//! bytes MagicBlock's program actually writes. Hand-made accounts are used only for forgeries the
//! real program can never produce (wrong owner, wrong address, wrong discriminator or size).

mod common;

use common::*;
use solana_account::Account;
use solana_address::Address;
use solana_keypair::Keypair;
use solana_signer::Signer;

const BOTH: [Version; 2] = [Version::V1, Version::V2];

fn token_data(ctx: &Ctx, token: &Address) -> Vec<u8> {
    ctx.svm.get_account(token).unwrap().data
}

/// Writes `data` as an account owned by `owner` at `address`.
fn plant(ctx: &mut Ctx, address: Address, owner: Address, data: Vec<u8>) {
    let lamports = ctx.svm.minimum_balance_for_rent_exemption(data.len());
    ctx.svm
        .set_account(
            address,
            Account {
                lamports,
                data,
                owner,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
}

// ---------------------------------------------------------------------------------------------
// The real gpl_session program writes what our program expects
// ---------------------------------------------------------------------------------------------

#[test]
fn real_token_layout_matches_what_the_program_reads() {
    for version in BOTH {
        let mut ctx = setup();
        let guest = ctx.guest.insecure_clone();
        let s = ctx.session_for(version, &guest, &ctx.program_id.clone(), Some(T0 + 600));

        let account = ctx.svm.get_account(&s.token).unwrap();
        assert_eq!(account.owner, SESSION_PROGRAM);
        let d = account.data;
        assert_eq!(d.len(), version.len(), "{version:?}");
        assert_eq!(d[..8], version.discriminator());
        assert_eq!(&d[8..40], guest.pubkey().as_ref(), "authority");
        assert_eq!(&d[40..72], ctx.program_id.as_ref(), "target_program");
        assert_eq!(&d[72..104], s.key.pubkey().as_ref(), "session_signer");
        if version == Version::V2 {
            assert_eq!(&d[104..136], guest.pubkey().as_ref(), "fee_payer");
        }
        let off = version.valid_until_offset();
        assert_eq!(read_u64(&d, off) as i64, T0 + 600, "valid_until");

        // top_up moved 0.01 SOL to the session key, so it can pay its own fees.
        assert_eq!(ctx.svm.get_balance(&s.key.pubkey()), Some(SESSION_TOP_UP));
    }
}

#[test]
fn real_program_defaults_to_one_hour_and_caps_at_seven_days() {
    for version in BOTH {
        let mut ctx = setup();
        let guest = ctx.guest.insecure_clone();
        let s = ctx.session(version, &guest);
        let d = token_data(&ctx, &s.token);
        assert_eq!(read_u64(&d, version.valid_until_offset()) as i64, T0 + 3_600);

        let key = Keypair::new();
        let ix = Ctx::create_session_ix(
            version,
            &guest.pubkey(),
            &key.pubkey(),
            &ctx.program_id,
            Some(T0 + 7 * 86_400 + 1),
        );
        // gpl_session `ValidityTooLong` = Anchor custom error 6000.
        assert_eq!(
            send(&mut ctx.svm, ix, &[&guest, &key]).unwrap_err(),
            custom(6000)
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Happy paths: a session key acts for its wallet
// ---------------------------------------------------------------------------------------------

#[test]
fn session_keys_join_send_and_close_for_both_members() {
    for version in BOTH {
        let mut ctx = setup();
        let room = ctx.create(1);
        ctx.open(&room);
        let (creator, guest) = (ctx.creator.insecure_clone(), ctx.guest.insecure_clone());
        let cs = ctx.session(version, &creator);
        let gs = ctx.session(version, &guest);

        // The session key is the only signer and pays the fee from its top-up.
        ctx.join_via(&gs.key, &room, Some(&gs.token), GUEST_BOX)
            .unwrap();
        let d = ctx.room(&room);
        assert_eq!(d[OFF_STATUS], JOINED);
        assert_eq!(bytes32(&d, OFF_GUEST_BOX), GUEST_BOX);

        ctx.send_via(&gs.key, &room, Some(&gs.token), 0, [1; 24], &[0xAA; 32])
            .unwrap();
        ctx.send_via(&cs.key, &room, Some(&cs.token), 1, [2; 24], &[0xBB; 64])
            .unwrap();
        // The wallets themselves still work without a token.
        ctx.send_as(&guest, &room, 2, [3; 24], &[0xCC; 80]).unwrap();

        let d = ctx.room(&room);
        assert_eq!(read_u64(&d, OFF_NEXT_SEQ), 3);
        // `sender` records the member, not the session key.
        assert_eq!((slot(&d, 0).sender, slot(&d, 0).kind), (1, 0), "{version:?}");
        assert_eq!((slot(&d, 1).sender, slot(&d, 1).kind), (0, 1));
        assert_eq!((slot(&d, 2).sender, slot(&d, 2).kind), (1, 2));

        ctx.close_via(&cs.key, &room, Some(&cs.token)).unwrap();
        let d = ctx.room(&room);
        assert_eq!(d[OFF_STATUS], CLOSED);
        assert!(d[OFF_GUEST..OFF_ROOM_ID].iter().all(|b| *b == 0));
        assert!(d[OFF_EXPIRES..].iter().all(|b| *b == 0));
    }
}

#[test]
fn guest_session_can_close_an_open_room() {
    let mut ctx = setup();
    let room = ctx.create(1);
    ctx.open(&room);
    let guest = ctx.guest.insecure_clone();
    let gs = ctx.session(Version::V2, &guest);
    ctx.close_via(&gs.key, &room, Some(&gs.token)).unwrap();
    assert_eq!(ctx.room(&room)[OFF_STATUS], CLOSED);
}

#[test]
fn session_message_cost() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let creator = ctx.creator.insecure_clone();
    let v1 = ctx.session(Version::V1, &creator);
    let v2 = ctx.session(Version::V2, &creator);
    let direct = ctx
        .send_via(&creator, &room, None, 0, [0; 24], &[1; 220])
        .unwrap();
    let with_v1 = ctx
        .send_via(&v1.key, &room, Some(&v1.token), 0, [0; 24], &[1; 220])
        .unwrap();
    let with_v2 = ctx
        .send_via(&v2.key, &room, Some(&v2.token), 0, [0; 24], &[1; 220])
        .unwrap();
    println!("SendMessage CU: direct {direct}, V1 session {with_v1}, V2 session {with_v2}");
    // The token check is one `find_program_address` plus a few compares. Its cost depends on
    // how many bumps are tried (~1.5k CU each, from 255 down), which depends on the random
    // session key, so the bound is loose. ER transactions cost no fee either way.
    assert!(direct < 5_000, "direct {direct}");
    assert!(with_v1 < 60_000 && with_v2 < 60_000, "{with_v1} / {with_v2}");
}

// ---------------------------------------------------------------------------------------------
// Authorization: a session key is never more than its wallet
// ---------------------------------------------------------------------------------------------

#[test]
fn session_key_without_token_is_a_stranger() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let creator = ctx.creator.insecure_clone();
    let cs = ctx.session(Version::V1, &creator);
    assert_eq!(
        ctx.send_via(&cs.key, &room, None, 0, [0; 24], &[1; 32])
            .unwrap_err(),
        custom(UNAUTHORIZED)
    );
    assert_eq!(
        ctx.close_via(&cs.key, &room, None).unwrap_err(),
        custom(UNAUTHORIZED)
    );
}

#[test]
fn stranger_with_a_valid_session_is_still_a_stranger() {
    for version in BOTH {
        let mut ctx = setup();
        let room = ctx.joined_room();
        let stranger = ctx.stranger.insecure_clone();
        let ss = ctx.session(version, &stranger);
        assert_eq!(
            ctx.send_via(&ss.key, &room, Some(&ss.token), 0, [0; 24], &[1; 32])
                .unwrap_err(),
            custom(UNAUTHORIZED)
        );
        assert_eq!(
            ctx.close_via(&ss.key, &room, Some(&ss.token)).unwrap_err(),
            custom(UNAUTHORIZED)
        );
    }
}

#[test]
fn creator_session_cannot_join_as_guest() {
    let mut ctx = setup();
    let room = ctx.create(1);
    ctx.open(&room);
    let creator = ctx.creator.insecure_clone();
    let cs = ctx.session(Version::V1, &creator);
    assert_eq!(
        ctx.join_via(&cs.key, &room, Some(&cs.token), GUEST_BOX)
            .unwrap_err(),
        custom(UNAUTHORIZED)
    );
}

#[test]
fn session_key_cannot_open_a_room() {
    // OpenRoom ignores session tokens: only the creator's wallet can choose the guest.
    let mut ctx = setup();
    let room = ctx.create(1);
    let creator = ctx.creator.insecure_clone();
    let cs = ctx.session(Version::V1, &creator);
    let guest = ctx.guest.pubkey();
    assert_eq!(
        ctx.open_via(&cs.key, &room, Some(&cs.token), &guest, CREATOR_BOX, T0 + TTL)
            .unwrap_err(),
        custom(UNAUTHORIZED)
    );
}

#[test]
fn someone_elses_token_is_rejected() {
    // The guest's session key presents the creator's token: the token names another signer.
    for version in BOTH {
        let mut ctx = setup();
        let room = ctx.joined_room();
        let (creator, guest) = (ctx.creator.insecure_clone(), ctx.guest.insecure_clone());
        let cs = ctx.session(version, &creator);
        let gs = ctx.session(version, &guest);
        assert_eq!(
            ctx.send_via(&gs.key, &room, Some(&cs.token), 0, [0; 24], &[1; 32])
                .unwrap_err(),
            custom(INVALID_SESSION_TOKEN)
        );
        // The wallet itself can't present its session's token either: the token names the
        // session key as signer, not the wallet.
        assert_eq!(
            ctx.send_via(&creator, &room, Some(&cs.token), 0, [0; 24], &[1; 32])
                .unwrap_err(),
            custom(INVALID_SESSION_TOKEN)
        );
    }
}

#[test]
fn token_for_another_program_is_rejected() {
    // A real session the creator made for a different program (here: the session program
    // itself, which is executable). Same wallet, same key, wrong target.
    for version in BOTH {
        let mut ctx = setup();
        let room = ctx.joined_room();
        let creator = ctx.creator.insecure_clone();
        let other = ctx.session_for(version, &creator, &SESSION_PROGRAM, None);
        assert_eq!(
            ctx.send_via(&other.key, &room, Some(&other.token), 0, [0; 24], &[1; 32])
                .unwrap_err(),
            custom(INVALID_SESSION_TOKEN)
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Lifecycle: expiry and revocation
// ---------------------------------------------------------------------------------------------

#[test]
fn session_expires_at_valid_until() {
    for version in BOTH {
        let mut ctx = setup();
        let room = ctx.joined_room();
        let guest = ctx.guest.insecure_clone();
        let program_id = ctx.program_id;
        let gs = ctx.session_for(version, &guest, &program_id, Some(T0 + 600));

        warp(&mut ctx.svm, T0 + 599);
        ctx.send_via(&gs.key, &room, Some(&gs.token), 0, [0; 24], &[1; 32])
            .unwrap();
        warp(&mut ctx.svm, T0 + 600);
        assert_eq!(
            ctx.send_via(&gs.key, &room, Some(&gs.token), 0, [0; 24], &[1; 32])
                .unwrap_err(),
            custom(SESSION_EXPIRED)
        );
        assert_eq!(
            ctx.close_via(&gs.key, &room, Some(&gs.token)).unwrap_err(),
            custom(SESSION_EXPIRED)
        );
        // The wallet itself is unaffected.
        ctx.send_as(&guest, &room, 0, [0; 24], &[1; 32]).unwrap();
    }
}

#[test]
fn expired_session_can_not_close_even_an_expired_room() {
    // "Anyone may close after expiry" still requires a valid token if one is passed: an invalid
    // token is an error, never a silent fallback.
    let mut ctx = setup();
    let room = ctx.joined_room();
    let guest = ctx.guest.insecure_clone();
    let program_id = ctx.program_id;
    let gs = ctx.session_for(Version::V1, &guest, &program_id, Some(T0 + 600));
    warp(&mut ctx.svm, T0 + TTL);
    assert_eq!(
        ctx.close_via(&gs.key, &room, Some(&gs.token)).unwrap_err(),
        custom(SESSION_EXPIRED)
    );
    // Without the token the key is a stranger, and strangers may close an expired room.
    ctx.close_via(&gs.key, &room, None).unwrap();
}

#[test]
fn v1_anyone_can_revoke_and_the_token_stops_working() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let (guest, stranger) = (ctx.guest.insecure_clone(), ctx.stranger.insecure_clone());
    let gs = ctx.session(Version::V1, &guest);
    ctx.send_via(&gs.key, &room, Some(&gs.token), 0, [0; 24], &[1; 32])
        .unwrap();

    // V1 lets anyone revoke (rent goes back to the authority). This is a griefing vector.
    let before = ctx.svm.get_balance(&guest.pubkey()).unwrap();
    ctx.revoke_session(&gs, &[&stranger], false).unwrap();
    assert!(ctx.svm.get_account(&gs.token).is_none_or(|a| a.lamports == 0));
    assert!(ctx.svm.get_balance(&guest.pubkey()).unwrap() > before);

    assert_eq!(
        ctx.send_via(&gs.key, &room, Some(&gs.token), 0, [0; 24], &[1; 32])
            .unwrap_err(),
        custom(INVALID_SESSION_TOKEN)
    );
}

#[test]
fn v2_only_the_authority_can_revoke_an_active_session() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let (guest, stranger) = (ctx.guest.insecure_clone(), ctx.stranger.insecure_clone());
    let gs = ctx.session(Version::V2, &guest);

    assert_eq!(
        ctx.revoke_session(&gs, &[&stranger], false).unwrap_err(),
        custom(SESSION_ERR_INVALID_AUTHORITY)
    );
    ctx.send_via(&gs.key, &room, Some(&gs.token), 0, [0; 24], &[1; 32])
        .unwrap();

    ctx.revoke_session(&gs, &[&guest], true).unwrap();
    assert_eq!(
        ctx.send_via(&gs.key, &room, Some(&gs.token), 0, [0; 24], &[1; 32])
            .unwrap_err(),
        custom(INVALID_SESSION_TOKEN)
    );
}

#[test]
fn v2_anyone_can_clean_up_an_expired_session() {
    let mut ctx = setup();
    let (guest, stranger) = (ctx.guest.insecure_clone(), ctx.stranger.insecure_clone());
    let gs = ctx.session(Version::V2, &guest);
    warp(&mut ctx.svm, T0 + 3_600);
    ctx.revoke_session(&gs, &[&stranger], false).unwrap();
    assert!(ctx.svm.get_account(&gs.token).is_none_or(|a| a.lamports == 0));
}

// ---------------------------------------------------------------------------------------------
// Forgeries the real program can't produce
// ---------------------------------------------------------------------------------------------

#[test]
fn real_token_bytes_under_another_owner_are_rejected() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let guest = ctx.guest.insecure_clone();
    let gs = ctx.session(Version::V1, &guest);
    let data = token_data(&ctx, &gs.token);

    // Same bytes, owned by the system program, by our program, and by a random program.
    for owner in [SYSTEM_PROGRAM, ctx.program_id, Address::new_unique()] {
        let fake = Address::new_unique();
        plant(&mut ctx, fake, owner, data.clone());
        assert_eq!(
            ctx.send_via(&gs.key, &room, Some(&fake), 0, [0; 24], &[1; 32])
                .unwrap_err(),
            custom(INVALID_SESSION_TOKEN),
            "owner {owner}"
        );
    }
}

#[test]
fn token_at_the_wrong_address_is_rejected() {
    // Correct owner, discriminator and fields, but not at the PDA of those fields.
    for version in BOTH {
        let mut ctx = setup();
        let room = ctx.joined_room();
        let guest = ctx.guest.insecure_clone();
        let gs = ctx.session(version, &guest);
        let data = token_data(&ctx, &gs.token);
        let fake = Address::new_unique();
        plant(&mut ctx, fake, SESSION_PROGRAM, data);
        assert_eq!(
            ctx.send_via(&gs.key, &room, Some(&fake), 0, [0; 24], &[1; 32])
                .unwrap_err(),
            custom(INVALID_SESSION_TOKEN)
        );
    }
}

#[test]
fn v1_bytes_at_the_v2_address_are_rejected() {
    // The seed prefix must match the version the bytes claim to be.
    let mut ctx = setup();
    let room = ctx.joined_room();
    let guest = ctx.guest.insecure_clone();
    let gs = ctx.session(Version::V1, &guest);
    let data = token_data(&ctx, &gs.token);
    let v2_address =
        Ctx::token_address(Version::V2, &ctx.program_id, &gs.key.pubkey(), &guest.pubkey());
    plant(&mut ctx, v2_address, SESSION_PROGRAM, data);
    assert_eq!(
        ctx.send_via(&gs.key, &room, Some(&v2_address), 0, [0; 24], &[1; 32])
            .unwrap_err(),
        custom(INVALID_SESSION_TOKEN)
    );
}

#[test]
fn tampered_discriminator_or_size_is_rejected() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let guest = ctx.guest.insecure_clone();
    let gs = ctx.session(Version::V1, &guest);
    let original = token_data(&ctx, &gs.token);

    let mut wrong_disc = original.clone();
    wrong_disc[..8].copy_from_slice(&ACC_SESSION_TOKEN_V2);
    let mut too_long = original.clone();
    too_long.push(0);
    let too_short = original[..111].to_vec();

    for data in [wrong_disc, too_long, too_short, vec![]] {
        plant(&mut ctx, gs.token, SESSION_PROGRAM, data);
        assert_eq!(
            ctx.send_via(&gs.key, &room, Some(&gs.token), 0, [0; 24], &[1; 32])
                .unwrap_err(),
            custom(INVALID_SESSION_TOKEN)
        );
    }

    // Restoring the real bytes makes it work again: the rejections were about the bytes.
    plant(&mut ctx, gs.token, SESSION_PROGRAM, original);
    ctx.send_via(&gs.key, &room, Some(&gs.token), 0, [0; 24], &[1; 32])
        .unwrap();
}

#[test]
fn room_or_signer_passed_as_token_is_rejected() {
    let mut ctx = setup();
    let room = ctx.joined_room();
    let creator = ctx.creator.insecure_clone();
    for token in [room, creator.pubkey()] {
        assert_eq!(
            ctx.send_via(&creator, &room, Some(&token), 0, [0; 24], &[1; 32])
                .unwrap_err(),
            // Neither is owned by the session program; the owner check fails before any borrow.
            custom(INVALID_SESSION_TOKEN)
        );
    }
}

#[test]
fn missing_token_account_is_rejected() {
    // A token address that doesn't exist is system-owned with no data.
    let mut ctx = setup();
    let room = ctx.joined_room();
    let creator = ctx.creator.insecure_clone();
    let nothing = Address::new_unique();
    assert_eq!(
        ctx.send_via(&creator, &room, Some(&nothing), 0, [0; 24], &[1; 32])
            .unwrap_err(),
        custom(INVALID_SESSION_TOKEN)
    );
}
