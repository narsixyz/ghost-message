//! MagicBlock CPIs, encoded by hand for Pinocchio 0.12 without `alloc`.
//!
//! Ported from `cyphers-program-v1/program/src/utils/magicblock.rs`. The wire formats match
//! `ephemeral-rollups-pinocchio` 0.17.3, which can't be a dependency (it needs Pinocchio 0.11
//! with `alloc`).
//!
//! | CPI | Program | Data | Runs on |
//! |---|---|---|---|
//! | `delegate` | Delegation (`DELeGG...`) | `u64 0` + `DelegateAccountArgs` | Solana |
//! | `ScheduleIntentBundle` | Magic (`Magic111...`) | `u32 11` + bincode `MagicIntentBundleArgs` | ER |
//! | `CreateEphemeralPermission` | Permission (`ACLseo...`) | `u64 6` + `is_private u8` + `(flags u8, pubkey)*` | ER |
//! | `CloseEphemeralPermission` | Permission (`ACLseo...`) | `u64 8` | ER |

use pinocchio::{
    AccountView, Address, ProgramResult,
    cpi::{Seed, Signer, invoke_signed},
    error::ProgramError,
    instruction::{InstructionAccount, InstructionView},
};
use pinocchio_system::instructions::Assign;

use crate::utils::accounts::{create_pda_account, require_program};

/// Delegation program, `DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh`.
pub const DELEGATION_PROGRAM_ID: Address = Address::new_from_array([
    181, 183, 0, 225, 242, 87, 58, 192, 204, 6, 34, 1, 52, 74, 207, 151, 184, 53, 6, 235, 140, 229,
    25, 152, 204, 98, 126, 24, 147, 128, 167, 62,
]);
/// Magic program (ER builtin), `Magic11111111111111111111111111111111111111`.
pub const MAGIC_PROGRAM_ID: Address = Address::new_from_array([
    5, 69, 180, 36, 176, 218, 112, 149, 236, 185, 214, 222, 195, 119, 215, 40, 145, 182, 231, 142,
    146, 234, 18, 214, 223, 187, 58, 64, 0, 0, 0, 0,
]);
/// Magic context account, `MagicContext1111111111111111111111111111111`.
pub const MAGIC_CONTEXT_ID: Address = Address::new_from_array([
    5, 69, 180, 36, 196, 165, 40, 191, 95, 180, 3, 47, 68, 82, 130, 142, 187, 56, 171, 193, 210,
    220, 151, 247, 63, 139, 148, 84, 128, 0, 0, 0,
]);
/// Ephemeral rent vault, `MagicVau1t999999999999999999999999999999999`.
pub const EPHEMERAL_VAULT_ID: Address = Address::new_from_array([
    5, 69, 180, 36, 224, 197, 24, 97, 240, 41, 76, 112, 66, 34, 84, 78, 202, 127, 133, 79, 194,
    135, 136, 166, 123, 118, 113, 80, 62, 224, 143, 184,
]);
/// Permission program (PER access control), `ACLseoPoyC3cBqoUtkbjZ4aDrkurZW86v19pXz2XQnp1`.
pub const PERMISSION_PROGRAM_ID: Address = Address::new_from_array([
    136, 161, 10, 196, 33, 152, 1, 214, 246, 106, 29, 60, 6, 152, 192, 102, 169, 175, 212, 217,
    180, 252, 231, 71, 151, 141, 209, 5, 168, 212, 103, 82,
]);

/// The validator rooms are delegated to. Never a client value: a room delegated to a non-TEE
/// validator would expose the guest's address and the message metadata.
///
/// Default: the MagicBlock TEE validator (devnet and mainnet),
/// `MTEWGuqxUpYZGFJQcp8tLN7x5v9BSeoFHYWQQ3n3xzo`.
#[cfg(not(feature = "local-er"))]
pub const ROOM_VALIDATOR: Address = Address::new_from_array([
    5, 61, 71, 26, 133, 158, 115, 46, 104, 11, 201, 88, 248, 65, 7, 43, 143, 63, 188, 25, 115, 155,
    230, 151, 196, 198, 129, 18, 111, 140, 31, 116,
]);
/// `local-er` feature: the local `ephemeral-validator` identity started by `mb-stack`,
/// `mAGicPQYBMvcYveUZA5F5UNNwyHvfYh5xkLS2Fr1mev`. For the e2e tests only.
#[cfg(feature = "local-er")]
pub const ROOM_VALIDATOR: Address = Address::new_from_array([
    11, 80, 9, 208, 183, 189, 108, 200, 89, 77, 168, 76, 233, 197, 132, 22, 21, 186, 202, 240, 105,
    168, 157, 64, 233, 249, 100, 104, 210, 41, 83, 87,
]);

/// Delegation buffer seed, derived under the owner program.
pub const BUFFER_SEED: &[u8] = b"buffer";
/// Undelegation buffer seed, derived under the delegation program.
pub const UNDELEGATE_BUFFER_SEED: &[u8] = b"undelegate-buffer";
/// Permission PDA seed, derived under the permission program: `["permission:", account]`.
pub const PERMISSION_SEED: &[u8] = b"permission:";
/// First 8 bytes of the delegation program's undelegation callback into the owner program
/// (Anchor `global:process_undelegation`).
pub const UNDELEGATE_CALLBACK_DISCRIMINATOR: [u8; 8] = [196, 28, 41, 206, 48, 37, 51, 167];

/// `MemberFlags` bits of the permission program.
pub mod member_flags {
    pub const AUTHORITY: u8 = 1 << 0;
    pub const TX_LOGS: u8 = 1 << 1;
    pub const TX_BALANCES: u8 = 1 << 2;
    pub const TX_MESSAGE: u8 = 1 << 3;
    pub const ACCOUNT_SIGNATURES: u8 = 1 << 4;
}

/// Delegation program `delegate` discriminator (u64 LE).
const DELEGATE_DISCRIMINATOR: u64 = 0;
/// Magic program `ScheduleIntentBundle` variant index (bincode u32 LE).
const SCHEDULE_INTENT_BUNDLE: u32 = 11;
/// Permission program `CreateEphemeralPermission` discriminator (u64 LE).
const CREATE_EPHEMERAL_PERMISSION: u64 = 6;
/// Permission program `CloseEphemeralPermission` discriminator (u64 LE).
const CLOSE_EPHEMERAL_PERMISSION: u64 = 8;
/// Never auto-commit; the program commits explicitly.
const COMMIT_FREQUENCY_MS: u32 = u32::MAX;
/// Max seeds (without bump) of any PDA this program delegates: `["room", creator, room_id]`.
const MAX_DELEGATED_SEEDS: usize = 3;
/// Max members of an ephemeral permission this program creates.
pub const MAX_PERMISSION_MEMBERS: usize = 4;

/// Accounts the delegation program needs to delegate one PDA.
pub struct DelegateAccounts<'a> {
    /// `[signer, writable]` pays the buffer and delegation PDAs; gets the buffer rent back.
    pub payer: &'a mut AccountView,
    /// `[writable]` PDA owned by this program; owned by the delegation program afterwards.
    pub pda: &'a mut AccountView,
    /// `[]` this program's executable account.
    pub owner_program: &'a AccountView,
    /// `[writable]` `["buffer", pda]` under this program; created and closed here.
    pub buffer: &'a mut AccountView,
    /// `[writable]` `["delegation", pda]` under the delegation program.
    pub delegation_record: &'a AccountView,
    /// `[writable]` `["delegation-metadata", pda]` under the delegation program.
    pub delegation_metadata: &'a AccountView,
    /// `[]` delegation program.
    pub delegation_program: &'a AccountView,
    /// `[]` System Program.
    pub system_program: &'a AccountView,
}

/// Delegates `accounts.pda` to `validator` (Solana).
///
/// `seeds` are the PDA seeds without the bump; `signer_seeds` are the same seeds plus the bump.
///
/// 1. Create `["buffer", pda]` and copy the PDA's data into it.
/// 2. Zero the PDA, hand it to the System Program, then `Assign` it to the delegation program
///    (signed by the PDA).
/// 3. CPI `delegate`; the delegation program restores the data from the buffer.
///
/// The caller must then `close_buffer` **after its last CPI**: a direct lamport move between two
/// accounts is only synced into a later CPI for the accounts that CPI receives, so closing the
/// buffer before another CPI fails with `UnbalancedInstruction`.
pub fn delegate_pda(
    accounts: DelegateAccounts,
    program_id: &Address,
    seeds: &[&[u8]],
    signer_seeds: &[Seed],
    validator: &Address,
) -> ProgramResult {
    let DelegateAccounts {
        payer,
        pda,
        owner_program,
        buffer,
        delegation_record,
        delegation_metadata,
        delegation_program,
        system_program,
    } = accounts;
    require_program(delegation_program, &DELEGATION_PROGRAM_ID)?;
    require_program(system_program, &pinocchio_system::ID)?;
    require_program(owner_program, program_id)?;
    if !pda.owned_by(program_id) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    let (expected_buffer, buffer_bump) =
        Address::find_program_address(&[BUFFER_SEED, pda.address().as_ref()], program_id);
    if buffer.address() != &expected_buffer {
        return Err(ProgramError::InvalidSeeds);
    }
    let buffer_bump = [buffer_bump];
    let buffer_seeds = [
        Seed::from(BUFFER_SEED),
        Seed::from(pda.address().as_ref()),
        Seed::from(&buffer_bump),
    ];
    let data_len = pda.data_len();
    create_pda_account(
        payer,
        buffer,
        data_len,
        0,
        program_id,
        &Signer::from(&buffer_seeds),
    )?;
    {
        let source = pda.try_borrow()?;
        buffer.try_borrow_mut()?.copy_from_slice(&source);
    }
    pda.try_borrow_mut()?.fill(0);

    let pda_signer = [Signer::from(signer_seeds)];
    pda.assign(&pinocchio_system::ID);
    Assign {
        account: pda,
        owner: &DELEGATION_PROGRAM_ID,
    }
    .invoke_signed(&pda_signer)?;

    let mut data = [0u8; 128];
    let len = encode_delegate_args(&mut data, seeds, validator)?;
    invoke_signed(
        &InstructionView {
            program_id: &DELEGATION_PROGRAM_ID,
            accounts: &[
                InstructionAccount::writable_signer(payer.address()),
                InstructionAccount::writable_signer(pda.address()),
                InstructionAccount::readonly(owner_program.address()),
                InstructionAccount::writable(buffer.address()),
                InstructionAccount::writable(delegation_record.address()),
                InstructionAccount::writable(delegation_metadata.address()),
                InstructionAccount::readonly(&pinocchio_system::ID),
            ],
            data: &data[..len],
        },
        &[
            &*payer,
            &*pda,
            owner_program,
            &*buffer,
            delegation_record,
            delegation_metadata,
            system_program,
        ],
        &pda_signer,
    )
}

/// Closes a delegation buffer: its lamports go back to the payer, then the runtime sees a
/// zero-length, System-owned account. Call only after the instruction's last CPI.
pub fn close_buffer(payer: &mut AccountView, buffer: &mut AccountView) -> ProgramResult {
    let credited = payer
        .lamports()
        .checked_add(buffer.lamports())
        .ok_or(ProgramError::ArithmeticOverflow)?;
    payer.set_lamports(credited);
    buffer.set_lamports(0);
    buffer.close()
}

/// Writes `u64 0 | commit_frequency_ms u32 | seeds Vec<Vec<u8>> | Option<validator>`.
pub fn encode_delegate_args(
    out: &mut [u8],
    seeds: &[&[u8]],
    validator: &Address,
) -> Result<usize, ProgramError> {
    if seeds.len() > MAX_DELEGATED_SEEDS {
        return Err(ProgramError::InvalidArgument);
    }
    let mut w = Writer::new(out);
    w.put(&DELEGATE_DISCRIMINATOR.to_le_bytes())?;
    w.put(&COMMIT_FREQUENCY_MS.to_le_bytes())?;
    w.put(&(seeds.len() as u32).to_le_bytes())?;
    for seed in seeds {
        w.put(&(seed.len() as u32).to_le_bytes())?;
        w.put(seed)?;
    }
    w.put(&[1])?;
    w.put(validator.as_ref())?;
    Ok(w.len())
}

/// Schedules commit-and-undelegate (or a plain commit) of one account through the Magic program
/// (ER). CPI accounts: `[payer (signer), magic_context, account]`. The commit lands on Solana
/// asynchronously: success here means scheduled, not done.
pub fn schedule_commit(
    payer: &AccountView,
    magic_context: &AccountView,
    magic_program: &AccountView,
    account: &AccountView,
    undelegate: bool,
) -> ProgramResult {
    require_program(magic_program, &MAGIC_PROGRAM_ID)?;
    require_program(magic_context, &MAGIC_CONTEXT_ID)?;
    let mut data = [0u8; 64];
    let len = encode_intent_bundle(&mut data, &[2], undelegate)?;
    invoke_signed(
        &InstructionView {
            program_id: &MAGIC_PROGRAM_ID,
            accounts: &[
                InstructionAccount::new(payer.address(), payer.is_writable(), true),
                InstructionAccount::writable(magic_context.address()),
                InstructionAccount::new(account.address(), account.is_writable(), false),
            ],
            data: &data[..len],
        },
        &[payer, magic_context, account],
        &[],
    )
}

/// Bincode-legacy `MagicBlockInstruction::ScheduleIntentBundle(MagicIntentBundleArgs)`:
///
/// ```text
/// u32 11                                   variant ScheduleIntentBundle
/// Option<CommitTypeArgs>                   commit
/// Option<CommitAndUndelegateArgs>          commit_and_undelegate
/// Option<()> None, Option<()> None         commit_finalize, commit_finalize_and_undelegate
/// u64 0                                    standalone_actions (empty Vec)
///
/// CommitTypeArgs::Standalone   = u32 0 | u64 len | u8 account index * len
/// CommitAndUndelegateArgs      = CommitTypeArgs | UndelegateTypeArgs::Standalone (u32 0)
/// ```
pub fn encode_intent_bundle(
    out: &mut [u8],
    indices: &[u8],
    undelegate: bool,
) -> Result<usize, ProgramError> {
    let mut w = Writer::new(out);
    w.put(&SCHEDULE_INTENT_BUNDLE.to_le_bytes())?;
    if undelegate {
        w.put(&[0])?; // commit: None
        w.put(&[1])?; // commit_and_undelegate: Some
        put_standalone_commit(&mut w, indices)?;
        w.put(&0u32.to_le_bytes())?; // UndelegateTypeArgs::Standalone
    } else {
        w.put(&[1])?; // commit: Some
        put_standalone_commit(&mut w, indices)?;
        w.put(&[0])?; // commit_and_undelegate: None
    }
    w.put(&[0, 0])?; // commit_finalize, commit_finalize_and_undelegate: None
    w.put(&0u64.to_le_bytes())?; // standalone_actions: empty
    Ok(w.len())
}

fn put_standalone_commit(w: &mut Writer, indices: &[u8]) -> ProgramResult {
    w.put(&0u32.to_le_bytes())?; // CommitTypeArgs::Standalone
    w.put(&(indices.len() as u64).to_le_bytes())?;
    w.put(indices)
}

/// Accounts of the permission program's ER-local instructions, besides the permissioned PDA.
pub struct PermissionAccounts<'a> {
    /// `[writable]` `["permission:", pda]` under the permission program.
    pub permission: &'a AccountView,
    /// `[writable]` ephemeral rent vault.
    pub vault: &'a AccountView,
    /// `[]` Magic program.
    pub magic_program: &'a AccountView,
    /// `[]` permission program.
    pub permission_program: &'a AccountView,
}

impl PermissionAccounts<'_> {
    /// Checks the fixed program and vault addresses and that `permission` is the permission PDA
    /// of `pda`.
    pub fn check(&self, pda: &AccountView) -> ProgramResult {
        require_program(self.permission_program, &PERMISSION_PROGRAM_ID)?;
        require_program(self.magic_program, &MAGIC_PROGRAM_ID)?;
        require_program(self.vault, &EPHEMERAL_VAULT_ID)?;
        let (expected, _) = Address::find_program_address(
            &[PERMISSION_SEED, pda.address().as_ref()],
            &PERMISSION_PROGRAM_ID,
        );
        if self.permission.address() != &expected {
            return Err(ProgramError::InvalidSeeds);
        }
        Ok(())
    }
}

/// Encodes `CreateEphemeralPermission`: `u64 6 | is_private u8 | (flags u8 | pubkey[32])*`.
/// There is no member count: the permission program reads members until the data ends, so the
/// length is `9 + 33·n`.
pub fn encode_create_permission(
    out: &mut [u8],
    is_private: bool,
    members: &[(&Address, u8)],
) -> Result<usize, ProgramError> {
    if members.len() > MAX_PERMISSION_MEMBERS {
        return Err(ProgramError::InvalidArgument);
    }
    let mut w = Writer::new(out);
    w.put(&CREATE_EPHEMERAL_PERMISSION.to_le_bytes())?;
    w.put(&[is_private as u8])?;
    for (address, flags) in members {
        w.put(&[*flags])?;
        w.put(address.as_ref())?;
    }
    Ok(w.len())
}

/// Creates the ER-local `EphemeralPermission` of `pda` (ER only). The PDA pays the permission
/// rent from its own lamports and signs as both payer and permissioned account.
pub fn create_ephemeral_permission(
    pda: &AccountView,
    accounts: &PermissionAccounts,
    is_private: bool,
    members: &[(&Address, u8)],
    signer_seeds: &[Seed],
) -> ProgramResult {
    accounts.check(pda)?;
    let mut data = [0u8; 9 + 33 * MAX_PERMISSION_MEMBERS];
    let len = encode_create_permission(&mut data, is_private, members)?;
    invoke_signed(
        &InstructionView {
            program_id: &PERMISSION_PROGRAM_ID,
            accounts: &[
                InstructionAccount::writable_signer(pda.address()),
                InstructionAccount::readonly_signer(pda.address()),
                InstructionAccount::writable(accounts.permission.address()),
                InstructionAccount::writable(accounts.vault.address()),
                InstructionAccount::readonly(accounts.magic_program.address()),
            ],
            data: &data[..len],
        },
        &[
            pda,
            pda,
            accounts.permission,
            accounts.vault,
            accounts.magic_program,
        ],
        &[Signer::from(signer_seeds)],
    )
}

/// Closes the ER-local `EphemeralPermission` of `pda` (ER only). The PDA signs as the
/// permissioned account (no separate authority signs) and, as payer, gets the rent back.
///
/// CPI accounts follow `ephemeral-rollups-pinocchio`'s `CloseEphemeralPermission` with
/// `authority_is_signer = false`: `[payer, authority, permissioned_account, permission, vault,
/// magic_program]`. The PDA fills the payer, authority and permissioned-account slots.
pub fn close_ephemeral_permission(
    pda: &AccountView,
    accounts: &PermissionAccounts,
    signer_seeds: &[Seed],
) -> ProgramResult {
    accounts.check(pda)?;
    invoke_signed(
        &InstructionView {
            program_id: &PERMISSION_PROGRAM_ID,
            accounts: &[
                InstructionAccount::writable_signer(pda.address()),
                InstructionAccount::readonly(pda.address()),
                InstructionAccount::readonly_signer(pda.address()),
                InstructionAccount::writable(accounts.permission.address()),
                InstructionAccount::writable(accounts.vault.address()),
                InstructionAccount::readonly(accounts.magic_program.address()),
            ],
            data: &CLOSE_EPHEMERAL_PERMISSION.to_le_bytes(),
        },
        &[
            pda,
            pda,
            pda,
            accounts.permission,
            accounts.vault,
            accounts.magic_program,
        ],
        &[Signer::from(signer_seeds)],
    )
}

/// Parses the undelegation callback's borsh `Vec<Vec<u8>>` seeds into `out`, returning the
/// number of seeds (`1..=out.len()`). Rejects trailing bytes.
pub fn parse_seeds<'a, const N: usize>(
    mut data: &'a [u8],
    out: &mut [&'a [u8]; N],
) -> Result<usize, ProgramError> {
    fn read_u32(data: &mut &[u8]) -> Result<usize, ProgramError> {
        let (head, rest) = data
            .split_first_chunk::<4>()
            .ok_or(ProgramError::InvalidInstructionData)?;
        *data = rest;
        Ok(u32::from_le_bytes(*head) as usize)
    }

    let count = read_u32(&mut data)?;
    if count == 0 || count > N {
        return Err(ProgramError::InvalidInstructionData);
    }
    for slot in out.iter_mut().take(count) {
        let len = read_u32(&mut data)?;
        if len > data.len() {
            return Err(ProgramError::InvalidInstructionData);
        }
        let (seed, rest) = data.split_at(len);
        *slot = seed;
        data = rest;
    }
    if !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    Ok(count)
}

/// Bounded byte writer for CPI payloads.
struct Writer<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl<'a> Writer<'a> {
    fn new(out: &'a mut [u8]) -> Self {
        Self { out, len: 0 }
    }

    fn put(&mut self, bytes: &[u8]) -> ProgramResult {
        let end = self.len + bytes.len();
        self.out
            .get_mut(self.len..end)
            .ok_or(ProgramError::InvalidArgument)?
            .copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }

    fn len(&self) -> usize {
        self.len
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::vec::Vec;

    use super::*;

    #[test]
    fn intent_bundle_commit_and_undelegate_layout() {
        let mut out = [0u8; 64];
        let len = encode_intent_bundle(&mut out, &[2], true).unwrap();
        assert_eq!(
            &out[..len],
            &[
                11, 0, 0, 0, // ScheduleIntentBundle
                0, // commit: None
                1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, // Some(Standalone([2]))
                0, 0, 0, 0, // UndelegateTypeArgs::Standalone
                0, 0, // finalize variants: None
                0, 0, 0, 0, 0, 0, 0, 0, // no standalone actions
            ]
        );
    }

    #[test]
    fn delegate_args_layout() {
        let mut out = [0u8; 128];
        let validator = Address::new_from_array([7; 32]);
        let len =
            encode_delegate_args(&mut out, &[b"room", &[9; 32], &[1; 8]], &validator).unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(&0u64.to_le_bytes());
        expected.extend_from_slice(&u32::MAX.to_le_bytes());
        expected.extend_from_slice(&3u32.to_le_bytes());
        expected.extend_from_slice(&4u32.to_le_bytes());
        expected.extend_from_slice(b"room");
        expected.extend_from_slice(&32u32.to_le_bytes());
        expected.extend_from_slice(&[9; 32]);
        expected.extend_from_slice(&8u32.to_le_bytes());
        expected.extend_from_slice(&[1; 8]);
        expected.push(1);
        expected.extend_from_slice(&[7; 32]);
        assert_eq!(&out[..len], expected.as_slice());
    }

    #[test]
    fn create_permission_two_members_is_9_plus_33n() {
        let (a, b) = (Address::new_from_array([1; 32]), Address::new_from_array([2; 32]));
        let mut out = [0u8; 9 + 33 * MAX_PERMISSION_MEMBERS];
        let len = encode_create_permission(&mut out, true, &[(&a, 0x0e), (&b, 0x0e)]).unwrap();
        assert_eq!(len, 9 + 33 * 2);
        assert_eq!(&out[..9], &[6, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(out[9], 0x0e);
        assert_eq!(&out[10..42], &[1; 32]);
        assert_eq!(out[42], 0x0e);
        assert_eq!(&out[43..75], &[2; 32]);
    }

    #[test]
    fn parse_seeds_round_trip_and_rejections() {
        let mut data = Vec::new();
        data.extend_from_slice(&3u32.to_le_bytes());
        for seed in [&b"room"[..], &[9; 32], &[1; 8]] {
            data.extend_from_slice(&(seed.len() as u32).to_le_bytes());
            data.extend_from_slice(seed);
        }
        let mut out: [&[u8]; 3] = [&[]; 3];
        assert_eq!(parse_seeds(&data, &mut out).unwrap(), 3);
        assert_eq!(out[0], b"room");

        let mut trailing = data.clone();
        trailing.push(0);
        let zero = 0u32.to_le_bytes();
        let too_many = 4u32.to_le_bytes();
        for bad in [&trailing[..], &zero, &too_many, &data[..data.len() - 1]] {
            let mut out: [&[u8]; 3] = [&[]; 3];
            assert!(parse_seeds(bad, &mut out).is_err());
        }
    }
}
