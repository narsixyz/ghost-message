//! Runs `../target/deploy/ghost_message.so` in LiteSVM with v1 transactions.
//! Build it first from the project root: `cargo build-sbf --tools-version v1.58 --arch v3`.
//!
//! LiteSVM has no Ephemeral Rollup. The "PER" instructions (`OpenRoom`, `JoinRoom`,
//! `SendMessage`, `CloseRoom`) run here against a program-owned room, which is how a delegated
//! account looks inside the ER.

use litesvm::LiteSVM;
use solana_address::Address;
use solana_clock::Clock;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::{VersionedMessage, v1};
use solana_signer::Signer;
use solana_transaction::{InstructionError, TransactionError, versioned::VersionedTransaction};

const PROGRAM_SO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/deploy/ghost_message.so");
const SYSTEM_PROGRAM: Address = Address::new_from_array([0; 32]);

// Unset v1 limits are ZERO, not runtime defaults: always set the CU and loaded-data limits.
const CONFIG: v1::TransactionConfig = v1::TransactionConfig::empty()
    .with_compute_unit_limit(200_000)
    .with_loaded_accounts_data_size_limit(256 * 1024)
    .with_priority_fee(1_000);

const T0: i64 = 1_800_000_000;
const TTL: i64 = 3_600;

// Room layout (see program/src/state.rs).
const ROOM_LEN: usize = 4248;
const OFF_STATUS: usize = 2;
const OFF_CREATOR: usize = 8;
const OFF_GUEST: usize = 40;
const OFF_CREATOR_BOX: usize = 72;
const OFF_GUEST_BOX: usize = 104;
const OFF_EXPIRES: usize = 136;
const OFF_NEXT_SEQ: usize = 144;
const OFF_MESSAGES: usize = 152;
const SLOT_LEN: usize = 256;
const SLOTS: usize = 16;

const CREATED: u8 = 0;
const OPEN: u8 = 1;
const JOINED: u8 = 2;
const CLOSED: u8 = 3;

// Custom error codes (program/src/error.rs).
const UNAUTHORIZED: u32 = 0;
const INVALID_ROOM_STATUS: u32 = 1;
const ROOM_EXPIRED: u32 = 2;
const INVALID_EXPIRY: u32 = 3;
const INVALID_GUEST: u32 = 4;
const INVALID_BOX_KEY: u32 = 5;
const INVALID_MESSAGE_KIND: u32 = 6;
const INVALID_MESSAGE_LENGTH: u32 = 7;

const CREATOR_BOX: [u8; 32] = [0xC1; 32];
const GUEST_BOX: [u8; 32] = [0x61; 32];

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

struct Ctx {
    svm: LiteSVM,
    program_id: Address,
    creator: Keypair,
    guest: Keypair,
    stranger: Keypair,
}

fn setup() -> Ctx {
    let so = std::fs::read(PROGRAM_SO).unwrap_or_else(|e| {
        panic!("{PROGRAM_SO}: {e}. Run `cargo build-sbf --tools-version v1.58 --arch v3` first")
    });
    // e_flags (offset 0x30) holds the SBPF version. `cargo build-sbf` 4.1.x defaults to v0.
    assert_eq!(so[0x30], 3, "expected an SBPF v3 build, got v{}", so[0x30]);
    let mut svm = LiteSVM::new();
    let program_id = Address::new_unique();
    svm.add_program(program_id, &so).unwrap();
    let (creator, guest, stranger) = (Keypair::new(), Keypair::new(), Keypair::new());
    for key in [&creator, &guest, &stranger] {
        svm.airdrop(&key.pubkey(), 10_000_000_000).unwrap();
    }
    warp(&mut svm, T0);
    Ctx {
        svm,
        program_id,
        creator,
        guest,
        stranger,
    }
}

fn warp(svm: &mut LiteSVM, unix_timestamp: i64) {
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp = unix_timestamp;
    svm.set_sysvar(&clock);
}

/// Sends one v1 transaction. The first signer pays the fee.
fn send(svm: &mut LiteSVM, ix: Instruction, signers: &[&Keypair]) -> Result<(), TransactionError> {
    let message = v1::Message::try_compile_with_config(
        &signers[0].pubkey(),
        &[ix],
        svm.latest_blockhash(),
        CONFIG,
    )
    .unwrap();
    let tx = VersionedTransaction::try_new(VersionedMessage::V1(message), signers).unwrap();
    let result = svm.send_transaction(tx).map(|_| ()).map_err(|e| e.err);
    svm.expire_blockhash();
    result
}

fn custom(code: u32) -> TransactionError {
    TransactionError::InstructionError(0, InstructionError::Custom(code))
}

fn ix_err(err: InstructionError) -> TransactionError {
    TransactionError::InstructionError(0, err)
}

impl Ctx {
    fn room_address(&self, creator: &Address, room_id: u64) -> Address {
        Address::find_program_address(
            &[b"room", creator.as_ref(), &room_id.to_le_bytes()],
            &self.program_id,
        )
        .0
    }

    fn room(&self, room: &Address) -> Vec<u8> {
        self.svm.get_account(room).unwrap().data
    }

    fn create_ix(&self, creator: &Address, room: &Address, room_id: u64) -> Instruction {
        let mut data = vec![0];
        data.extend_from_slice(&room_id.to_le_bytes());
        Instruction::new_with_bytes(
            self.program_id,
            &data,
            vec![
                AccountMeta::new(*creator, true),
                AccountMeta::new(*room, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
            ],
        )
    }

    fn room_ix(&self, signer: &Address, room: &Address, data: Vec<u8>) -> Instruction {
        Instruction::new_with_bytes(
            self.program_id,
            &data,
            vec![
                AccountMeta::new_readonly(*signer, true),
                AccountMeta::new(*room, false),
            ],
        )
    }

    fn create(&mut self, room_id: u64) -> Address {
        let room = self.room_address(&self.creator.pubkey(), room_id);
        let ix = self.create_ix(&self.creator.pubkey(), &room, room_id);
        send(&mut self.svm, ix, &[&self.creator]).unwrap();
        room
    }

    fn open_as(
        &mut self,
        signer: &Keypair,
        room: &Address,
        guest: &Address,
        box_pk: [u8; 32],
        expires_at: i64,
    ) -> Result<(), TransactionError> {
        let mut data = vec![2];
        data.extend_from_slice(guest.as_ref());
        data.extend_from_slice(&box_pk);
        data.extend_from_slice(&expires_at.to_le_bytes());
        let ix = self.room_ix(&signer.pubkey(), room, data);
        send(&mut self.svm, ix, &[signer])
    }

    fn open(&mut self, room: &Address) {
        let (creator, guest) = (self.creator.insecure_clone(), self.guest.pubkey());
        self.open_as(&creator, room, &guest, CREATOR_BOX, T0 + TTL)
            .unwrap();
    }

    fn join_as(
        &mut self,
        signer: &Keypair,
        room: &Address,
        box_pk: [u8; 32],
    ) -> Result<(), TransactionError> {
        let mut data = vec![3];
        data.extend_from_slice(&box_pk);
        let ix = self.room_ix(&signer.pubkey(), room, data);
        send(&mut self.svm, ix, &[signer])
    }

    fn join(&mut self, room: &Address) {
        let guest = self.guest.insecure_clone();
        self.join_as(&guest, room, GUEST_BOX).unwrap();
    }

    /// Created, opened, and joined.
    fn joined_room(&mut self) -> Address {
        let room = self.create(1);
        self.open(&room);
        self.join(&room);
        room
    }

    fn send_as(
        &mut self,
        signer: &Keypair,
        room: &Address,
        kind: u8,
        nonce: [u8; 24],
        ciphertext: &[u8],
    ) -> Result<(), TransactionError> {
        let mut data = vec![4, kind];
        data.extend_from_slice(&nonce);
        data.extend_from_slice(ciphertext);
        let ix = self.room_ix(&signer.pubkey(), room, data);
        send(&mut self.svm, ix, &[signer])
    }

    fn close_as(&mut self, signer: &Keypair, room: &Address) -> Result<(), TransactionError> {
        let ix = self.room_ix(&signer.pubkey(), room, vec![5]);
        send(&mut self.svm, ix, &[signer])
    }
}

struct Slot {
    seq: u64,
    sender: u8,
    kind: u8,
    len: u16,
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
}

fn slot(data: &[u8], index: usize) -> Slot {
    let s = &data[OFF_MESSAGES + index * SLOT_LEN..][..SLOT_LEN];
    Slot {
        seq: u64::from_le_bytes(s[0..8].try_into().unwrap()),
        sender: s[8],
        kind: s[9],
        len: u16::from_le_bytes(s[10..12].try_into().unwrap()),
        nonce: s[12..36].try_into().unwrap(),
        ciphertext: s[36..].to_vec(),
    }
}

fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

fn bytes32(data: &[u8], offset: usize) -> &[u8] {
    &data[offset..offset + 32]
}

// ---------------------------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------------------------

#[test]
fn unknown_or_unimplemented_instructions_fail() {
    let mut ctx = setup();
    let payer = ctx.creator.insecure_clone();
    for data in [vec![], vec![1], vec![6], vec![7], vec![8], vec![255]] {
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
    // Nothing but discriminator, bump, status and creator is set.
    assert!(d[OFF_GUEST..].iter().all(|b| *b == 0));

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
    let mut data = vec![2];
    data.extend_from_slice(ctx.guest.pubkey().as_ref());
    data.extend_from_slice(&CREATOR_BOX);
    data.extend_from_slice(&(T0 + TTL).to_le_bytes());
    data.push(0);
    let ix = ctx.room_ix(&ctx.creator.pubkey(), &room, data);
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
    assert!(
        d[OFF_GUEST..].iter().all(|b| *b == 0),
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
    let ix = ctx.room_ix(&ctx.creator.pubkey(), &room, vec![5, 0]);
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
