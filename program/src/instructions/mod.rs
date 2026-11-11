//! Instruction processors. Each file documents its accounts, data, and errors.

mod close_room;
mod create_room;
mod join_room;
mod open_room;
mod send_message;

pub use {
    close_room::close_room, create_room::create_room, join_room::join_room, open_room::open_room,
    send_message::send_message,
};
