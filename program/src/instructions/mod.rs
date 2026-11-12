//! Instruction processors. Each file documents its accounts, data, and errors.

mod close_room;
mod create_room;
mod delegate_room;
mod join_room;
mod open_room;
mod process_undelegation;
mod reclaim_room;
mod send_message;
mod undelegate_room;

pub use {
    close_room::close_room, create_room::create_room, delegate_room::delegate_room,
    join_room::join_room, open_room::{MEMBER_FLAGS, open_room},
    process_undelegation::process_undelegation, reclaim_room::reclaim_room,
    send_message::send_message, undelegate_room::undelegate_room,
};
