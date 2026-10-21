//! §5 ¶2's first submessage, read as §6.2 reads it.
//!
//! One function, and it is here rather than in [`crate::conformity`] because **both**
//! halves of the crate owe §6.2 ¶2 something and the clause is symmetric. The endpoint
//! half chains it in front of `Message::tokens` to build the echo; the client half
//! chains it in front of the same iterator to say what came back
//! ([`Reply::tokens`](crate::transport::Reply::tokens)), which is what that caller
//! returns on its next request. A rule that has one implementation cannot sit behind a
//! feature only one of its two callers enables, so this module is gated by neither.
//!
//! It is re-exported from the crate root, which is the path every caller uses.

use nlipsy_protocol::message::{Format, Message, Submessage};

/// The request's first submessage as a [`Submessage`], when that submessage is a
/// `token`.
///
/// §5 ¶2 calls the leading object of a message "the first submessage", so a message
/// that *is* a token carries one for §6.2's purposes — and the reference SDK's
/// `create_token` emits exactly that message. [`Message::tokens`] cannot yield it,
/// because the first submessage is not a [`Submessage`] value, so every site that owes
/// §6.2 something — the echo, the session lookup, the count guard, a federator's
/// stripping — chains this in front of that iterator instead of calling it alone.
///
/// The `Label` travels with it. ECMA-430 does not define one for the first submessage
/// and [`Message`] carries it only because the reference SDK emits it; §6.2 ¶2 asks
/// for "the same submessage", and the copy is closer to the same with it than without.
pub fn first_submessage_token(request: &Message) -> Option<Submessage> {
    if !request.is_token() {
        return None;
    }
    let token = Submessage::new(
        Format::Token,
        request.subformat.clone(),
        request.content.clone(),
    );
    Some(match &request.label {
        Some(label) => token.with_label(label.clone()),
        None => token,
    })
}
