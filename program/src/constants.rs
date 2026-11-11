//! Seeds, limits, and MagicBlock rent constants.

/// Room PDA seed: `["room", creator, room_id u64 LE]`.
pub const ROOM_SEED: &[u8] = b"room";

/// Number of message slots in a room's ring buffer.
pub const MESSAGE_SLOTS: usize = 16;
/// Largest ciphertext a slot holds: 204 bytes of plaintext + the 16-byte NaCl `box` MAC.
pub const MAX_CIPHERTEXT: usize = 220;
/// NaCl `box` MAC length. A ciphertext is never shorter than this.
pub const MAC_LEN: usize = 16;
/// NaCl `box` nonce length.
pub const NONCE_LEN: usize = 24;

/// Room status values.
pub mod status {
    /// Created on Solana; no guest yet.
    pub const CREATED: u8 = 0;
    /// Guest and creator box key set; waiting for the guest to join.
    pub const OPEN: u8 = 1;
    /// Both box keys set; messages allowed until `expires_at`.
    pub const JOINED: u8 = 2;
    /// Private data zeroed. Terminal.
    pub const CLOSED: u8 = 3;
}

/// Message kinds.
pub mod kind {
    pub const TEXT: u8 = 0;
    pub const PAYMENT_NOTE: u8 = 1;
    pub const SHIELDED_ADDRESS: u8 = 2;
}

/// Who wrote a message slot.
pub mod sender {
    pub const CREATOR: u8 = 0;
    pub const GUEST: u8 = 1;
}

/// MagicBlock ephemeral rent per byte (lamports) and per-account overhead (bytes). Used to
/// pre-fund the room for the `EphemeralPermission` it pays for on the PER (phase 3).
pub const EPHEMERAL_RENT_PER_BYTE: u64 = 32;
pub const EPHEMERAL_ACCOUNT_OVERHEAD: u64 = 60;

/// Size of an `EphemeralPermission` with `members` extra members (the permission program adds
/// one default member): `35 + (1 + members) * 33`.
pub const fn ephemeral_permission_size(members: u64) -> u64 {
    35 + (1 + members) * 33
}

/// Lamports the permission program charges the permissioned PDA on the ER.
pub const fn ephemeral_permission_rent(members: u64) -> u64 {
    (ephemeral_permission_size(members) + EPHEMERAL_ACCOUNT_OVERHEAD) * EPHEMERAL_RENT_PER_BYTE
}

/// Extra lamports `CreateRoom` adds on top of rent exemption: permission rent for
/// `[creator, guest]`.
pub const ROOM_PERMISSION_PREFUND: u64 = ephemeral_permission_rent(2);
