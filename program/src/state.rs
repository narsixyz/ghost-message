//! `Room` account. PDA `["room", creator, room_id u64 LE]`, 4,248 bytes, fixed size (a
//! delegated account can't be resized safely).
//!
//! All fields have alignment 1 (numbers are little-endian byte arrays), so casting account data
//! to `&Room` is sound at any address.
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 1 | discriminator (`1`) |
//! | 1 | 1 | bump |
//! | 2 | 1 | status (`constants::status`) |
//! | 3 | 5 | padding |
//! | 8 | 32 | creator |
//! | 40 | 32 | guest (zero until `OpenRoom`) |
//! | 72 | 32 | creator X25519 box public key |
//! | 104 | 32 | guest X25519 box public key (zero until `JoinRoom`) |
//! | 136 | 8 | expires_at (`i64`, unix; zero until `OpenRoom`) |
//! | 144 | 8 | next_seq (`u64`) |
//! | 152 | 16 × 256 | messages (ring buffer, slot = `seq % 16`) |
//!
//! Message slot (256 bytes):
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 8 | seq (`u64`) |
//! | 8 | 1 | sender (`0` creator, `1` guest) |
//! | 9 | 1 | kind (`0` text, `1` payment note, `2` shielded address) |
//! | 10 | 2 | len (`u16`, ciphertext bytes) |
//! | 12 | 24 | nonce |
//! | 36 | 220 | ciphertext (zero-padded) |

use pinocchio::{Address, error::ProgramError};

use crate::constants::{MAX_CIPHERTEXT, MESSAGE_SLOTS, NONCE_LEN};

#[repr(C)]
pub struct Message {
    seq: [u8; 8],
    pub sender: u8,
    pub kind: u8,
    len: [u8; 2],
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: [u8; MAX_CIPHERTEXT],
}

const _: () = assert!(size_of::<Message>() == 256 && align_of::<Message>() == 1);

impl Message {
    pub fn seq(&self) -> u64 {
        u64::from_le_bytes(self.seq)
    }

    pub fn ciphertext_len(&self) -> u16 {
        u16::from_le_bytes(self.len)
    }

    /// Overwrites the slot. Bytes after the ciphertext are zeroed so nothing from the
    /// overwritten message survives.
    pub fn write(&mut self, seq: u64, sender: u8, kind: u8, nonce: &[u8; NONCE_LEN], ct: &[u8]) {
        self.seq = seq.to_le_bytes();
        self.sender = sender;
        self.kind = kind;
        self.len = (ct.len() as u16).to_le_bytes();
        self.nonce = *nonce;
        let (used, rest) = self.ciphertext.split_at_mut(ct.len());
        used.copy_from_slice(ct);
        rest.fill(0);
    }
}

#[repr(C)]
pub struct Room {
    pub discriminator: u8,
    pub bump: u8,
    pub status: u8,
    _padding: [u8; 5],
    pub creator: Address,
    pub guest: Address,
    pub creator_box_pk: [u8; 32],
    pub guest_box_pk: [u8; 32],
    expires_at: [u8; 8],
    next_seq: [u8; 8],
    pub messages: [Message; MESSAGE_SLOTS],
}

const _: () = assert!(size_of::<Room>() == 4248 && align_of::<Room>() == 1);

impl Room {
    pub const DISCRIMINATOR: u8 = 1;
    pub const LEN: usize = size_of::<Self>();

    pub fn expires_at(&self) -> i64 {
        i64::from_le_bytes(self.expires_at)
    }

    pub fn set_expires_at(&mut self, value: i64) {
        self.expires_at = value.to_le_bytes();
    }

    pub fn next_seq(&self) -> u64 {
        u64::from_le_bytes(self.next_seq)
    }

    pub fn set_next_seq(&mut self, value: u64) {
        self.next_seq = value.to_le_bytes();
    }

    /// Expired once `OpenRoom` has set an expiry and it has passed. A room that was never
    /// opened (`expires_at == 0`) does not expire.
    pub fn is_expired(&self, now: i64) -> bool {
        let expires_at = self.expires_at();
        expires_at != 0 && now >= expires_at
    }

    /// Mutable view of an existing room. The caller checks the owner first.
    pub fn load_mut(data: &mut [u8]) -> Result<&mut Self, ProgramError> {
        if data.len() != Self::LEN || data[0] != Self::DISCRIMINATOR {
            return Err(ProgramError::InvalidAccountData);
        }
        // SAFETY: length checked; alignment 1 (asserted); every bit pattern is valid.
        Ok(unsafe { &mut *(data.as_mut_ptr() as *mut Self) })
    }

    /// For a freshly created (zeroed) account: checks the length and writes the discriminator.
    pub fn init(data: &mut [u8]) -> Result<&mut Self, ProgramError> {
        if data.len() != Self::LEN {
            return Err(ProgramError::InvalidAccountData);
        }
        data[0] = Self::DISCRIMINATOR;
        // SAFETY: length checked; alignment 1 (asserted); every bit pattern is valid.
        Ok(unsafe { &mut *(data.as_mut_ptr() as *mut Self) })
    }

    /// Zeroes everything a non-member must never see after undelegation: guest, box keys,
    /// expiry, message count, and every message slot.
    pub fn wipe_private(&mut self) {
        self.guest = Address::new_from_array([0; 32]);
        self.creator_box_pk = [0; 32];
        self.guest_box_pk = [0; 32];
        self.expires_at = [0; 8];
        self.next_seq = [0; 8];
        for m in self.messages.iter_mut() {
            m.seq = [0; 8];
            m.sender = 0;
            m.kind = 0;
            m.len = [0; 2];
            m.nonce = [0; NONCE_LEN];
            m.ciphertext = [0; MAX_CIPHERTEXT];
        }
    }
}
