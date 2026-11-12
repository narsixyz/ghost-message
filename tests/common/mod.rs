//! Shared LiteSVM harness: loads `../target/deploy/ghost_message.so` and the real session-keys
//! program (`fixtures/session_keys.so`, dumped from devnet), sends v1 transactions, and builds
//! GhostMessage and `gpl_session` instructions.
#![allow(dead_code)]

use litesvm::LiteSVM;
use solana_address::Address;
use solana_clock::Clock;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::{VersionedMessage, v1};
use solana_signer::Signer;
use solana_transaction::{InstructionError, TransactionError, versioned::VersionedTransaction};

pub const PROGRAM_SO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/deploy/ghost_message.so");
/// Test-only recorder loaded at the permission, Magic and Privacy (SPP) program addresses (see
/// `tests/cpi-recorder`).
pub const RECORDER_SO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/deploy/cpi_recorder.so");
/// MagicBlock delegation program dumped from devnet.
pub const DELEGATION_SO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/delegation_program.so");
/// `gpl_session` dumped from devnet: `solana program dump -u devnet KeyspM2ss... session_keys.so`.
pub const SESSION_SO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/session_keys.so");
pub const SYSTEM_PROGRAM: Address = Address::new_from_array([0; 32]);

// Unset v1 limits are ZERO, not runtime defaults: always set the CU and loaded-data limits.
// Session-program instructions load the ~510 KB `gpl_session` binary.
pub const CONFIG: v1::TransactionConfig = v1::TransactionConfig::empty()
    .with_compute_unit_limit(200_000)
    .with_loaded_accounts_data_size_limit(2 * 1024 * 1024)
    .with_priority_fee(1_000);

// MagicBlock.
pub const DELEGATION_PROGRAM: Address = Address::from_str_const("DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh");
pub const PERMISSION_PROGRAM: Address = Address::from_str_const("ACLseoPoyC3cBqoUtkbjZ4aDrkurZW86v19pXz2XQnp1");
pub const MAGIC_PROGRAM: Address = Address::from_str_const("Magic11111111111111111111111111111111111111");
pub const MAGIC_CONTEXT: Address = Address::from_str_const("MagicContext1111111111111111111111111111111");
pub const EPHEMERAL_VAULT: Address = Address::from_str_const("MagicVau1t999999999999999999999999999999999");
pub const TEE_VALIDATOR: Address = Address::from_str_const("MTEWGuqxUpYZGFJQcp8tLN7x5v9BSeoFHYWQQ3n3xzo");
// Helius Privacy Rings.
pub const SPP_PROGRAM: Address = Address::from_str_const("ring1GaLCuJnkUMVkX2JeZLTTX17wKsV4fZmPVzXANW");
pub const SPP_TRANSACT_TAG: u8 = 16;
pub const UNDELEGATE_CALLBACK: [u8; 8] = [196, 28, 41, 206, 48, 37, 51, 167];
/// TX_LOGS | TX_BALANCES | TX_MESSAGE | ACCOUNT_SIGNATURES
pub const MEMBER_FLAGS: u8 = 0b1_1110;

pub const T0: i64 = 1_800_000_000;
pub const TTL: i64 = 3_600;

// Room layout (see program/src/state.rs).
pub const ROOM_LEN: usize = 4256;
pub const OFF_STATUS: usize = 2;
pub const OFF_CREATOR: usize = 8;
pub const OFF_GUEST: usize = 40;
pub const OFF_CREATOR_BOX: usize = 72;
pub const OFF_GUEST_BOX: usize = 104;
pub const OFF_ROOM_ID: usize = 136;
pub const OFF_EXPIRES: usize = 144;
pub const OFF_NEXT_SEQ: usize = 152;
pub const OFF_MESSAGES: usize = 160;
pub const SLOT_LEN: usize = 256;
pub const SLOTS: usize = 16;

pub const CREATED: u8 = 0;
pub const OPEN: u8 = 1;
pub const JOINED: u8 = 2;
pub const CLOSED: u8 = 3;

// Custom error codes (program/src/error.rs).
pub const UNAUTHORIZED: u32 = 0;
pub const INVALID_ROOM_STATUS: u32 = 1;
pub const ROOM_EXPIRED: u32 = 2;
pub const INVALID_EXPIRY: u32 = 3;
pub const INVALID_GUEST: u32 = 4;
pub const INVALID_BOX_KEY: u32 = 5;
pub const INVALID_MESSAGE_KIND: u32 = 6;
pub const INVALID_MESSAGE_LENGTH: u32 = 7;
pub const INVALID_SESSION_TOKEN: u32 = 9;
pub const SESSION_EXPIRED: u32 = 10;

// MagicBlock session keys (`gpl_session`, Anchor).
pub const SESSION_PROGRAM: Address = Address::from_str_const("KeyspM2ssCJbqUhQ4k7sveSiY4WjnYsrXkC8oDbwde5");
/// Instruction discriminators: `sha256("global:<name>")[..8]`.
pub const IX_CREATE_SESSION: [u8; 8] = [242, 193, 143, 179, 150, 25, 122, 227];
pub const IX_CREATE_SESSION_V2: [u8; 8] = [223, 233, 108, 7, 65, 194, 235, 38];
pub const IX_REVOKE_SESSION: [u8; 8] = [86, 92, 198, 120, 144, 2, 7, 194];
pub const IX_REVOKE_SESSION_V2: [u8; 8] = [211, 59, 125, 188, 43, 155, 8, 102];
/// Account discriminators: `sha256("account:<Name>")[..8]`.
pub const ACC_SESSION_TOKEN: [u8; 8] = [233, 4, 115, 14, 46, 21, 1, 15];
pub const ACC_SESSION_TOKEN_V2: [u8; 8] = [178, 3, 85, 254, 13, 116, 128, 41];
/// `gpl_session` custom error `InvalidAuthority` (Anchor errors start at 6000).
pub const SESSION_ERR_INVALID_AUTHORITY: u32 = 6003;
/// Lamports `create_session` moves to the session signer with `top_up = true`.
pub const SESSION_TOP_UP: u64 = 10_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    V1,
    V2,
}

impl Version {
    pub fn seed(self) -> &'static [u8] {
        match self {
            Version::V1 => b"session_token",
            Version::V2 => b"session_token_v2",
        }
    }

    pub fn len(self) -> usize {
        match self {
            Version::V1 => 112,
            Version::V2 => 144,
        }
    }

    pub fn discriminator(self) -> [u8; 8] {
        match self {
            Version::V1 => ACC_SESSION_TOKEN,
            Version::V2 => ACC_SESSION_TOKEN_V2,
        }
    }

    pub fn valid_until_offset(self) -> usize {
        self.len() - 8
    }
}

/// A session created through the real `gpl_session` program.
pub struct Session {
    pub key: Keypair,
    pub token: Address,
    pub authority: Address,
    pub version: Version,
}

pub const CREATOR_BOX: [u8; 32] = [0xC1; 32];
pub const GUEST_BOX: [u8; 32] = [0x61; 32];

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

pub struct Ctx {
    pub svm: LiteSVM,
    pub program_id: Address,
    pub creator: Keypair,
    pub guest: Keypair,
    pub stranger: Keypair,
}

pub fn setup() -> Ctx {
    let so = std::fs::read(PROGRAM_SO).unwrap_or_else(|e| {
        panic!("{PROGRAM_SO}: {e}. Run `cargo build-sbf --tools-version v1.58 --arch v3` first")
    });
    // e_flags (offset 0x30) holds the SBPF version. `cargo build-sbf` 4.1.x defaults to v0.
    assert_eq!(so[0x30], 3, "expected an SBPF v3 build, got v{}", so[0x30]);
    let mut svm = LiteSVM::new();
    let program_id = Address::new_unique();
    svm.add_program(program_id, &so).unwrap();
    let session_so = std::fs::read(SESSION_SO).unwrap_or_else(|e| panic!("{SESSION_SO}: {e}"));
    svm.add_program(SESSION_PROGRAM, &session_so).unwrap();
    let dlp = std::fs::read(DELEGATION_SO).unwrap_or_else(|e| panic!("{DELEGATION_SO}: {e}"));
    svm.add_program(DELEGATION_PROGRAM, &dlp).unwrap();
    let recorder = std::fs::read(RECORDER_SO).unwrap_or_else(|e| {
        panic!("{RECORDER_SO}: {e}. Run `cargo build-sbf --tools-version v1.58 --arch v3` first")
    });
    svm.add_program(PERMISSION_PROGRAM, &recorder).unwrap();
    svm.add_program(MAGIC_PROGRAM, &recorder).unwrap();
    svm.add_program(SPP_PROGRAM, &recorder).unwrap();
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

pub fn warp(svm: &mut LiteSVM, unix_timestamp: i64) {
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp = unix_timestamp;
    svm.set_sysvar(&clock);
}

/// Sends one v1 transaction. The first signer pays the fee.
pub fn send(svm: &mut LiteSVM, ix: Instruction, signers: &[&Keypair]) -> Result<(), TransactionError> {
    send_cu(svm, ix, signers).map(|_| ())
}

/// Like `send`, returning the compute units consumed.
pub fn send_cu(svm: &mut LiteSVM, ix: Instruction, signers: &[&Keypair]) -> Result<u64, TransactionError> {
    send_logs(svm, ix, signers).map(|(cu, _)| cu)
}

/// Like `send`, returning the compute units consumed and the transaction logs.
pub fn send_logs(
    svm: &mut LiteSVM,
    ix: Instruction,
    signers: &[&Keypair],
) -> Result<(u64, Vec<String>), TransactionError> {
    let message = v1::Message::try_compile_with_config(
        &signers[0].pubkey(),
        &[ix],
        svm.latest_blockhash(),
        CONFIG,
    )
    .unwrap();
    let tx = VersionedTransaction::try_new(VersionedMessage::V1(message), signers).unwrap();
    let result = svm
        .send_transaction(tx)
        .map(|meta| (meta.compute_units_consumed, meta.logs))
        .map_err(|e| e.err);
    svm.expire_blockhash();
    result
}

/// One CPI the recorder received.
#[derive(Debug)]
pub struct Recorded {
    pub program: Address,
    pub data: Vec<u8>,
    /// `(address, is_signer, is_writable)` in instruction order.
    pub accounts: Vec<(Address, bool, bool)>,
}

/// Extracts the recorder's `Program data:` entries from transaction logs, attributing each to
/// the program that was invoked at that point.
pub fn recorded(logs: &[String]) -> Vec<Recorded> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut stack: Vec<Address> = Vec::new();
    let mut out = Vec::new();
    for line in logs {
        if let Some(rest) = line.strip_prefix("Program ") {
            let mut words = rest.split_whitespace();
            let first = words.next().unwrap_or_default();
            match (first, words.next()) {
                ("data:", _) => {
                    let parts: Vec<Vec<u8>> = rest["data:".len()..]
                        .split_whitespace()
                        .map(|p| b64.decode(p).unwrap())
                        .collect();
                    let accounts = parts[1]
                        .as_chunks::<34>()
                        .0
                        .iter()
                        .map(|c| {
                            (
                                Address::new_from_array(c[..32].try_into().unwrap()),
                                c[32] == 1,
                                c[33] == 1,
                            )
                        })
                        .collect();
                    out.push(Recorded {
                        program: *stack.last().unwrap(),
                        data: parts[0].clone(),
                        accounts,
                    });
                }
                (id, Some("invoke")) => stack.push(id.parse().unwrap()),
                (_, Some("success")) | (_, Some("failed:")) => {
                    stack.pop();
                }
                _ => {}
            }
        }
    }
    out
}

pub fn custom(code: u32) -> TransactionError {
    TransactionError::InstructionError(0, InstructionError::Custom(code))
}

pub fn ix_err(err: InstructionError) -> TransactionError {
    TransactionError::InstructionError(0, err)
}

impl Ctx {
    pub fn room_address(&self, creator: &Address, room_id: u64) -> Address {
        Address::find_program_address(
            &[b"room", creator.as_ref(), &room_id.to_le_bytes()],
            &self.program_id,
        )
        .0
    }

    pub fn room(&self, room: &Address) -> Vec<u8> {
        self.svm.get_account(room).unwrap().data
    }

    pub fn create_ix(&self, creator: &Address, room: &Address, room_id: u64) -> Instruction {
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

    pub fn room_ix(&self, signer: &Address, room: &Address, data: Vec<u8>) -> Instruction {
        self.room_ix_via(signer, room, None, data)
    }

    /// `[signer, room]`, plus the session token as account 2 when given.
    pub fn room_ix_via(
        &self,
        signer: &Address,
        room: &Address,
        token: Option<&Address>,
        data: Vec<u8>,
    ) -> Instruction {
        let mut accounts = vec![
            AccountMeta::new_readonly(*signer, true),
            AccountMeta::new(*room, false),
        ];
        if let Some(token) = token {
            accounts.push(AccountMeta::new_readonly(*token, false));
        }
        Instruction::new_with_bytes(self.program_id, &data, accounts)
    }

    pub fn permission_address(room: &Address) -> Address {
        Address::find_program_address(&[b"permission:", room.as_ref()], &PERMISSION_PROGRAM).0
    }

    /// `[signer, room, permission, vault, magic_program, permission_program]` + optional token.
    pub fn permission_ix(
        &self,
        signer: &Address,
        room: &Address,
        token: Option<&Address>,
        data: Vec<u8>,
    ) -> Instruction {
        let mut accounts = vec![
            AccountMeta::new_readonly(*signer, true),
            AccountMeta::new(*room, false),
            AccountMeta::new(Self::permission_address(room), false),
            AccountMeta::new(EPHEMERAL_VAULT, false),
            AccountMeta::new_readonly(MAGIC_PROGRAM, false),
            AccountMeta::new_readonly(PERMISSION_PROGRAM, false),
        ];
        if let Some(token) = token {
            accounts.push(AccountMeta::new_readonly(*token, false));
        }
        Instruction::new_with_bytes(self.program_id, &data, accounts)
    }

    pub fn open_data(guest: &Address, box_pk: [u8; 32], expires_at: i64) -> Vec<u8> {
        let mut data = vec![2];
        data.extend_from_slice(guest.as_ref());
        data.extend_from_slice(&box_pk);
        data.extend_from_slice(&expires_at.to_le_bytes());
        data
    }

    pub fn create(&mut self, room_id: u64) -> Address {
        let room = self.room_address(&self.creator.pubkey(), room_id);
        let ix = self.create_ix(&self.creator.pubkey(), &room, room_id);
        send(&mut self.svm, ix, &[&self.creator]).unwrap();
        room
    }

    pub fn open_as(
        &mut self,
        signer: &Keypair,
        room: &Address,
        guest: &Address,
        box_pk: [u8; 32],
        expires_at: i64,
    ) -> Result<(), TransactionError> {
        self.open_via(signer, room, None, guest, box_pk, expires_at)
    }

    pub fn open_via(
        &mut self,
        signer: &Keypair,
        room: &Address,
        token: Option<&Address>,
        guest: &Address,
        box_pk: [u8; 32],
        expires_at: i64,
    ) -> Result<(), TransactionError> {
        let ix = self.permission_ix(
            &signer.pubkey(),
            room,
            token,
            Self::open_data(guest, box_pk, expires_at),
        );
        send(&mut self.svm, ix, &[signer])
    }

    pub fn open(&mut self, room: &Address) {
        let (creator, guest) = (self.creator.insecure_clone(), self.guest.pubkey());
        self.open_as(&creator, room, &guest, CREATOR_BOX, T0 + TTL)
            .unwrap();
    }

    pub fn join_as(
        &mut self,
        signer: &Keypair,
        room: &Address,
        box_pk: [u8; 32],
    ) -> Result<(), TransactionError> {
        self.join_via(signer, room, None, box_pk)
    }

    pub fn join_via(
        &mut self,
        signer: &Keypair,
        room: &Address,
        token: Option<&Address>,
        box_pk: [u8; 32],
    ) -> Result<(), TransactionError> {
        let mut data = vec![3];
        data.extend_from_slice(&box_pk);
        let ix = self.room_ix_via(&signer.pubkey(), room, token, data);
        send(&mut self.svm, ix, &[signer])
    }

    pub fn join(&mut self, room: &Address) {
        let guest = self.guest.insecure_clone();
        self.join_as(&guest, room, GUEST_BOX).unwrap();
    }

    /// Created, opened, and joined.
    pub fn joined_room(&mut self) -> Address {
        let room = self.create(1);
        self.open(&room);
        self.join(&room);
        room
    }

    pub fn send_as(
        &mut self,
        signer: &Keypair,
        room: &Address,
        kind: u8,
        nonce: [u8; 24],
        ciphertext: &[u8],
    ) -> Result<(), TransactionError> {
        self.send_via(signer, room, None, kind, nonce, ciphertext)
            .map(|_| ())
    }

    /// Returns the compute units consumed.
    pub fn send_via(
        &mut self,
        signer: &Keypair,
        room: &Address,
        token: Option<&Address>,
        kind: u8,
        nonce: [u8; 24],
        ciphertext: &[u8],
    ) -> Result<u64, TransactionError> {
        let mut data = vec![4, kind];
        data.extend_from_slice(&nonce);
        data.extend_from_slice(ciphertext);
        let ix = self.room_ix_via(&signer.pubkey(), room, token, data);
        send_cu(&mut self.svm, ix, &[signer])
    }

    pub fn close_as(&mut self, signer: &Keypair, room: &Address) -> Result<(), TransactionError> {
        self.close_via(signer, room, None)
    }

    pub fn close_via(
        &mut self,
        signer: &Keypair,
        room: &Address,
        token: Option<&Address>,
    ) -> Result<(), TransactionError> {
        let ix = self.permission_ix(&signer.pubkey(), room, token, vec![5]);
        send(&mut self.svm, ix, &[signer])
    }

    // -----------------------------------------------------------------------------------------
    // gpl_session (the real program)
    // -----------------------------------------------------------------------------------------

    pub fn token_address(
        version: Version,
        target_program: &Address,
        session_signer: &Address,
        authority: &Address,
    ) -> Address {
        Address::find_program_address(
            &[
                version.seed(),
                target_program.as_ref(),
                session_signer.as_ref(),
                authority.as_ref(),
            ],
            &SESSION_PROGRAM,
        )
        .0
    }

    /// `create_session` / `create_session_v2` with `top_up = true` (0.01 SOL to the session key,
    /// so it can pay its own fees). The authority pays rent; both the authority and the session
    /// key sign. `target_program` must be executable.
    pub fn create_session_ix(
        version: Version,
        authority: &Address,
        session_signer: &Address,
        target_program: &Address,
        valid_until: Option<i64>,
    ) -> Instruction {
        let token = Self::token_address(version, target_program, session_signer, authority);
        let (disc, accounts) = match version {
            Version::V1 => (
                IX_CREATE_SESSION,
                vec![
                    AccountMeta::new(token, false),
                    AccountMeta::new(*session_signer, true),
                    AccountMeta::new(*authority, true),
                    AccountMeta::new_readonly(*target_program, false),
                    AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
                ],
            ),
            Version::V2 => (
                IX_CREATE_SESSION_V2,
                vec![
                    AccountMeta::new(token, false),
                    AccountMeta::new(*session_signer, true),
                    // fee_payer = authority
                    AccountMeta::new(*authority, true),
                    AccountMeta::new_readonly(*authority, true),
                    AccountMeta::new_readonly(*target_program, false),
                    AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
                ],
            ),
        };
        // Borsh: top_up Option<bool>, valid_until Option<i64>, lamports Option<u64>.
        let mut data = disc.to_vec();
        data.extend_from_slice(&[1, 1]);
        match valid_until {
            Some(v) => {
                data.push(1);
                data.extend_from_slice(&v.to_le_bytes());
            }
            None => data.push(0),
        }
        data.push(0);
        Instruction::new_with_bytes(SESSION_PROGRAM, &data, accounts)
    }

    /// Creates a session from `authority` to a fresh key, targeting `target_program`.
    pub fn session_for(
        &mut self,
        version: Version,
        authority: &Keypair,
        target_program: &Address,
        valid_until: Option<i64>,
    ) -> Session {
        let key = Keypair::new();
        let ix = Self::create_session_ix(
            version,
            &authority.pubkey(),
            &key.pubkey(),
            target_program,
            valid_until,
        );
        send(&mut self.svm, ix, &[authority, &key]).unwrap();
        Session {
            token: Self::token_address(version, target_program, &key.pubkey(), &authority.pubkey()),
            key,
            authority: authority.pubkey(),
            version,
        }
    }

    /// A session for this program, valid for the default hour.
    pub fn session(&mut self, version: Version, authority: &Keypair) -> Session {
        let program_id = self.program_id;
        self.session_for(version, authority, &program_id, None)
    }

    /// `revoke_session` (V1, anyone may send it) or `revoke_session_v2` (the authority must sign
    /// while the session is active). The first signer pays the fee.
    pub fn revoke_session(
        &mut self,
        session: &Session,
        signers: &[&Keypair],
        authority_signs: bool,
    ) -> Result<(), TransactionError> {
        let ix = match session.version {
            Version::V1 => Instruction::new_with_bytes(
                SESSION_PROGRAM,
                &IX_REVOKE_SESSION,
                vec![
                    AccountMeta::new(session.token, false),
                    AccountMeta::new(session.authority, false),
                    AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
                ],
            ),
            Version::V2 => Instruction::new_with_bytes(
                SESSION_PROGRAM,
                &IX_REVOKE_SESSION_V2,
                vec![
                    AccountMeta::new(session.token, false),
                    // fee_payer = authority (writable, receives the rent)
                    AccountMeta::new(session.authority, authority_signs),
                    AccountMeta::new_readonly(session.authority, authority_signs),
                    AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
                ],
            ),
        };
        send(&mut self.svm, ix, signers)
    }
}

pub struct Slot {
    pub seq: u64,
    pub sender: u8,
    pub kind: u8,
    pub len: u16,
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

pub fn slot(data: &[u8], index: usize) -> Slot {
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

pub fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

pub fn bytes32(data: &[u8], offset: usize) -> &[u8] {
    &data[offset..offset + 32]
}
