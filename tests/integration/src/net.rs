//! One concept: how anything here finds anything else.
//!
//! Every endpoint in this process — servers, devices, people's laptops —
//! is bound here, and every one of them is dialled by its bare
//! [`iroh::EndpointId`]. No addresses are passed between the pieces of
//! this suite, because none are passed between the pieces of the
//! product: registration is an id in a text field.
//!
//! # The address book, and why it is not a test-only code path
//!
//! A deployed endpoint publishes itself to n0's DNS and is dialled by id
//! from anywhere. Nothing published here reaches anything, because a
//! suite has no internet and would not want to wait for one — so every
//! endpoint carries a [`files::AddressBook`], seeded as each one binds.
//!
//! The distinction that matters: the book is a property of the
//! *endpoint*, resolved beneath `connect`, so nothing above it knows
//! there is a book at all. [`files::IrohRemotes`] is the code a
//! deployment runs, dialling the id it was given, and it takes the same
//! path here.
//!
//! That is a change from the arrangement this suite started with, where
//! the harness had its own `RemoteFiles` implementation consulting a
//! `HashMap`. It passed, and it proved rather less than it looked:
//! federation over a dialler that exists nowhere else is federation
//! nobody can deploy — and there was, in fact, no other implementation
//! in the repository.
//!
//! # One book per process
//!
//! `cargo nextest` runs each test in its own process, so a process holds
//! exactly one scenario and a process-wide book holds exactly that
//! scenario's endpoints. Run under `cargo test`, several scenarios share
//! it; that is still correct, because ids do not collide.

use files::AddressBook;

/// The addresses every endpoint in this process can resolve.
///
/// Shared: [`AddressBook`] is a handle to one table, so an endpoint
/// bound before another one existed still resolves it.
pub fn book() -> AddressBook {
    static BOOK: std::sync::OnceLock<AddressBook> = std::sync::OnceLock::new();
    BOOK.get_or_init(AddressBook::new).clone()
}

/// Bind an endpoint on `key` and publish it to the book.
///
/// Publishing is the half that makes bare-id dialling work here — it
/// stands in for the pkarr record a deployed endpoint writes as it
/// binds, and happens at the same moment for the same reason.
pub async fn bind(key: iroh::SecretKey) -> iroh::Endpoint {
    let endpoint = files::bind_endpoint(key, Some(book()))
        .await
        .expect("bind an endpoint");
    publish(&endpoint);
    endpoint
}

/// Re-publish an endpoint that has just rebound.
///
/// A restart keeps the id and gets fresh addresses, so the book has to
/// be told — the same update a deployed endpoint makes by republishing
/// its pkarr record. Without it, the id survives the restart and
/// resolves to where the old process was.
pub fn publish(endpoint: &iroh::Endpoint) {
    book().add_endpoint_info(endpoint.addr());
}
