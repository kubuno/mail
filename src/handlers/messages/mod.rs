//! HTTP handlers for individual messages, split by responsibility rather than
//! piled into one file: reading, sending, attachments, flags, address
//! suggestions, OpenPGP, and the calendar-invitation RSVP. Everything the
//! router and the other handlers import keeps its historical path
//! `crate::handlers::messages::<name>` through the re-exports below.

mod attachments;
mod flags;
mod invite;
mod pgp;
mod read;
mod send;
mod suggest;

pub use attachments::download_attachment;
pub use flags::{delete_message, mark_read, star_message};
pub use invite::invite_reply;
pub use pgp::decode_pgp_in_place;
pub use read::get_message;
pub use send::send_message;
pub use suggest::suggest_addresses;

// These are consumed only from within this crate (other handlers, workers), so
// they re-export at crate visibility — `pub use` of a `pub(crate)` item is
// rejected as widening its visibility.
pub(crate) use pgp::sender_autocrypt_key;
pub(crate) use read::strip_storage_paths;
pub(crate) use send::send_message_inner;
