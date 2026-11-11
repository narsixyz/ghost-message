//! Little-endian instruction-data reader.
//!
//! Every read copies bytes into a fixed array, so it never depends on the alignment of the
//! instruction data. `finish` rejects trailing bytes, so each instruction accepts exactly one
//! encoding.

use pinocchio::{Address, error::ProgramError};

pub struct DataReader<'a> {
    data: &'a [u8],
}

impl<'a> DataReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], ProgramError> {
        let (head, rest) = self
            .data
            .split_first_chunk::<N>()
            .ok_or(ProgramError::InvalidInstructionData)?;
        self.data = rest;
        Ok(*head)
    }

    pub fn u8(&mut self) -> Result<u8, ProgramError> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u64(&mut self) -> Result<u64, ProgramError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub fn i64(&mut self) -> Result<i64, ProgramError> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    pub fn address(&mut self) -> Result<Address, ProgramError> {
        Ok(Address::new_from_array(self.array()?))
    }

    /// Everything that is left. Ends the reader.
    pub fn rest(self) -> &'a [u8] {
        self.data
    }

    /// Errors if any bytes are left.
    pub fn finish(self) -> Result<(), ProgramError> {
        if self.data.is_empty() {
            Ok(())
        } else {
            Err(ProgramError::InvalidInstructionData)
        }
    }
}
